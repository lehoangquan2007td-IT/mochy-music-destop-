//! Always-on look-ahead peak limiter — port of Android `PeakLimiter.kt`
//! (spec §4.5): lookahead 5 ms, hold 25 ms (one 40 Hz cycle), exponential
//! release 150 ms, ceiling 1.0, settling 1e-6. The minimum gain over a
//! sliding window is kept in a monotonic queue and the held gains are
//! averaged over the window so each dip becomes a smooth ramp that lands
//! before the peak leaves the delay. Resting state = pure delay.

const LOOKAHEAD_SECONDS: f64 = 0.005;
const HOLD_SECONDS: f64 = 0.025;
const RELEASE_SECONDS: f64 = 0.15;
const CEILING: f64 = 1.0;
const SETTLED: f64 = 1e-6;

pub struct PeakLimiter {
    /// Delay (latency) in frames.
    pub lookahead_frames: usize,
    pub hold_frames: usize,
    /// Per-frame release step `1 - exp(-1/(0.15·sr))`.
    pub release_coeff: f64,
    pub ceiling: f64,
    sample_rate: u32,
    channels: usize,
    window: usize,
    delay: Vec<f32>,
    delay_pos: usize,
    // monotonic queue (rising front to back) of the lowest gain in the last window
    min_gain: Vec<f64>,
    min_frame: Vec<u64>,
    min_head: usize,
    min_size: usize,
    // held gains averaged over the window
    held: Vec<f64>,
    held_pos: usize,
    held_sum: f64,
    held_ones: usize,
    envelope: f64,
    hold_left: usize,
    frame: u64,
}

impl PeakLimiter {
    /// Stereo limiter at `sample_rate`.
    pub fn new(sample_rate: u32) -> Self {
        Self::with_channels(sample_rate, 2)
    }

    pub fn with_channels(sample_rate: u32, channels: usize) -> Self {
        let sr = sample_rate.max(1) as f64;
        let latency = ((sr * LOOKAHEAD_SECONDS) as usize).max(1);
        let window = latency + 1;
        let channels = channels.max(1);
        let mut l = PeakLimiter {
            lookahead_frames: latency,
            hold_frames: (sr * HOLD_SECONDS) as usize,
            release_coeff: 1.0 - (-1.0 / (RELEASE_SECONDS * sr)).exp(),
            ceiling: CEILING,
            sample_rate,
            channels,
            window,
            delay: vec![0.0; latency * channels],
            delay_pos: 0,
            min_gain: vec![0.0; window],
            min_frame: vec![0; window],
            min_head: 0,
            min_size: 0,
            held: vec![1.0; window],
            held_pos: 0,
            held_sum: window as f64,
            held_ones: window,
            envelope: 1.0,
            hold_left: 0,
            frame: 0,
        };
        l.reset();
        l
    }

    pub fn latency_frames(&self) -> usize {
        self.lookahead_frames
    }

    pub fn reset(&mut self) {
        self.delay.fill(0.0);
        self.delay_pos = 0;
        self.min_head = 0;
        self.min_size = 0;
        self.held.fill(1.0);
        self.held_pos = 0;
        self.held_sum = self.window as f64;
        self.held_ones = self.window;
        self.envelope = 1.0;
        self.hold_left = 0;
        self.frame = 0;
    }

    /// Nothing is being turned down: all the limiter does right now is delay.
    pub fn is_resting(&self) -> bool {
        self.envelope == 1.0 && self.held_ones == self.window
    }

    /// Limit interleaved frames in place; output is `lookahead_frames` late.
    /// Reconfigures (allocates) only if `channels` differs from construction.
    pub fn process(&mut self, samples: &mut [f32], channels: usize) {
        if channels == 0 {
            return;
        }
        if channels != self.channels {
            *self = Self::with_channels(self.sample_rate, channels);
        }
        let frames = samples.len() / channels;
        let samples = &mut samples[..frames * channels];
        let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs())) as f64;
        if self.is_resting() && peak <= self.ceiling {
            self.delay_only(samples);
            return;
        }
        for f in 0..frames {
            let i = f * channels;
            let mut p = 0.0f64;
            for c in 0..channels {
                p = p.max(samples[i + c].abs() as f64);
            }
            let needed = if p > self.ceiling { self.ceiling / p } else { 1.0 };
            if needed < 1.0 {
                self.hold_left = self.hold_frames;
            } else if self.hold_left > 0 {
                self.hold_left -= 1;
            }
            // drop at once, hold, then recover slowly; the window turns the drop into a ramp
            let recovered = self.envelope + (1.0 - self.envelope) * self.release_coeff;
            let resting = if self.hold_left > 0 {
                self.envelope
            } else if recovered > 1.0 - SETTLED {
                1.0
            } else {
                recovered
            };
            self.envelope = needed.min(resting);
            let lowest = self.lowest_in_window(self.envelope);
            let gain = self.smooth(lowest);
            for c in 0..channels {
                let d = self.delay_pos + c;
                let late = self.delay[d];
                self.delay[d] = samples[i + c];
                samples[i + c] = (late as f64 * gain) as f32;
            }
            self.delay_pos += channels;
            if self.delay_pos == self.delay.len() {
                self.delay_pos = 0;
            }
            self.frame += 1;
        }
    }

    fn delay_only(&mut self, samples: &mut [f32]) {
        let ch = self.channels;
        for frame in samples.chunks_exact_mut(ch) {
            for (c, s) in frame.iter_mut().enumerate() {
                let d = self.delay_pos + c;
                std::mem::swap(&mut self.delay[d], s);
            }
            self.delay_pos += ch;
            if self.delay_pos == self.delay.len() {
                self.delay_pos = 0;
            }
        }
        self.frame += (samples.len() / ch) as u64;
        // every gain in the window was 1, so the queue has nothing worth keeping
        self.min_size = 0;
    }

    /// The lowest of the last `window` envelopes; `value` is the newest.
    fn lowest_in_window(&mut self, value: f64) -> f64 {
        let w = self.window;
        while self.min_size > 0 && self.min_frame[self.min_head] + (w as u64) <= self.frame {
            self.min_head = if self.min_head + 1 == w { 0 } else { self.min_head + 1 };
            self.min_size -= 1;
        }
        while self.min_size > 0 {
            let mut back = self.min_head + self.min_size - 1;
            if back >= w {
                back -= w;
            }
            if self.min_gain[back] < value {
                break;
            }
            self.min_size -= 1;
        }
        let mut slot = self.min_head + self.min_size;
        if slot >= w {
            slot -= w;
        }
        self.min_gain[slot] = value;
        self.min_frame[slot] = self.frame;
        self.min_size += 1;
        self.min_gain[self.min_head]
    }

    /// Average of the last `window` held gains: each is at or below what the
    /// oldest frame in the delay needs, so no peak gets through.
    fn smooth(&mut self, gain: f64) -> f64 {
        let oldest = self.held[self.held_pos];
        if oldest == 1.0 {
            self.held_ones -= 1;
        }
        if gain == 1.0 {
            self.held_ones += 1;
        }
        self.held_sum += gain - oldest;
        self.held[self.held_pos] = gain;
        self.held_pos = if self.held_pos + 1 == self.window { 0 } else { self.held_pos + 1 };
        if self.held_ones == self.window {
            self.held_sum = self.window as f64;
            return 1.0;
        }
        self.held_sum / self.window as f64
    }
}
