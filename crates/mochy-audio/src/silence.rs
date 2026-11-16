//! Skip silence: media3 `SilenceSkippingAudioProcessor(2_000_000, 20_000,
//! 256)` as Android's `MusicService` builds it — any stretch where every
//! sample of every channel stays under 256/32768 for longer than 2 s is cut
//! down to 20 ms of padding on each side; shorter pauses play untouched.
//! Runs on the decode thread on source-rate stereo, before the rate stages.
//!
//! Also reports when a long silence is in progress, for Android's "instant"
//! mode (`SilenceDetectorAudioProcessor` + `performInstantSilenceSkip`:
//! 15 s seek hops while it stays silent).

/// media3 `minimumSilenceDurationUs`.
pub const MIN_SILENCE_SECS: f64 = 2.0;
/// media3 `paddingSilenceUs`.
pub const PADDING_SECS: f64 = 0.020;
/// media3 `silenceThresholdLevel` (16-bit) as f32.
pub const THRESHOLD: f32 = 256.0 / 32768.0;
/// Instant mode: seek this far per hop (`INSTANT_SILENCE_SKIP_STEP_MS`).
pub const INSTANT_STEP_SECS: f64 = 15.0;
/// Instant mode gives up after this many hops in one silence.
pub const INSTANT_MAX_HOPS: u32 = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Noisy,
    /// Silent, but not for long enough yet: frames are held back.
    MaybeSilent,
    /// Long silence: frames are dropped (the last `pad` kept in a ring).
    Silent,
}

pub struct SilenceSkipper {
    enabled: bool,
    min_frames: usize,
    pad_frames: usize,
    state: State,
    /// Held-back frames while `MaybeSilent` (interleaved stereo).
    hold: Vec<f32>,
    /// Rolling last `pad_frames` of a long silence.
    tail: Vec<f32>,
    tail_pos: usize,
    tail_len: usize,
    skipped: u64,
}

impl SilenceSkipper {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(1) as f64;
        let pad = (sr * PADDING_SECS).round().max(1.0) as usize;
        SilenceSkipper {
            enabled: false,
            min_frames: (sr * MIN_SILENCE_SECS).round() as usize,
            pad_frames: pad,
            state: State::Noisy,
            hold: Vec::new(),
            tail: vec![0.0; pad * 2],
            tail_pos: 0,
            tail_len: 0,
            skipped: 0,
        }
    }

    pub fn set_enabled(&mut self, on: bool) {
        if self.enabled != on {
            self.enabled = on;
            self.state = State::Noisy;
            self.hold.clear();
            self.tail_len = 0;
            self.tail_pos = 0;
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Seek / new track: forget the current silence.
    pub fn reset(&mut self) {
        self.state = State::Noisy;
        self.hold.clear();
        self.tail_len = 0;
        self.tail_pos = 0;
        self.skipped = 0;
    }

    /// A silence longer than [`MIN_SILENCE_SECS`] is being skipped.
    pub fn in_long_silence(&self) -> bool {
        self.enabled && self.state == State::Silent
    }

    /// Source frames dropped since the last call.
    pub fn take_skipped(&mut self) -> u64 {
        std::mem::take(&mut self.skipped)
    }

    #[inline]
    fn is_silent(f: &[f32]) -> bool {
        f[0].abs() < THRESHOLD && f[1].abs() < THRESHOLD
    }

    fn push_tail(&mut self, f: &[f32]) {
        let i = self.tail_pos * 2;
        self.tail[i] = f[0];
        self.tail[i + 1] = f[1];
        self.tail_pos = (self.tail_pos + 1) % self.pad_frames;
        self.tail_len = (self.tail_len + 1).min(self.pad_frames);
    }

    fn drain_tail(&mut self, out: &mut Vec<f32>) {
        let start = (self.tail_pos + self.pad_frames - self.tail_len) % self.pad_frames;
        for k in 0..self.tail_len {
            let i = ((start + k) % self.pad_frames) * 2;
            out.push(self.tail[i]);
            out.push(self.tail[i + 1]);
        }
        self.tail_len = 0;
        self.tail_pos = 0;
    }

    /// Filter interleaved stereo `input` into `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if !self.enabled {
            out.extend_from_slice(input);
            return;
        }
        for f in input.chunks_exact(2) {
            let silent = Self::is_silent(f);
            match self.state {
                State::Noisy => {
                    if silent {
                        self.state = State::MaybeSilent;
                        self.hold.clear();
                        self.hold.extend_from_slice(f);
                    } else {
                        out.extend_from_slice(f);
                    }
                }
                State::MaybeSilent => {
                    if silent {
                        self.hold.extend_from_slice(f);
                        if self.hold.len() / 2 >= self.min_frames {
                            // long enough: keep the first padding, drop the rest
                            out.extend_from_slice(&self.hold[..self.pad_frames * 2]);
                            let dropped = self.hold.len() / 2 - self.pad_frames;
                            self.skipped += dropped as u64;
                            // the newest frames seed the trailing padding
                            self.tail_len = 0;
                            self.tail_pos = 0;
                            let keep_from = self.hold.len() / 2 - self.pad_frames.min(dropped);
                            let held = std::mem::take(&mut self.hold);
                            for g in held[keep_from * 2..].chunks_exact(2) {
                                self.push_tail(g);
                            }
                            // those tail frames were counted as dropped; they come back on exit
                            self.skipped -= self.tail_len as u64;
                            self.hold = held;
                            self.hold.clear();
                            self.state = State::Silent;
                        }
                    } else {
                        out.append(&mut self.hold);
                        out.extend_from_slice(f);
                        self.state = State::Noisy;
                    }
                }
                State::Silent => {
                    if silent {
                        if self.tail_len == self.pad_frames {
                            self.skipped += 1;
                        }
                        self.push_tail(f);
                    } else {
                        self.drain_tail(out);
                        out.extend_from_slice(f);
                        self.state = State::Noisy;
                    }
                }
            }
        }
    }

    /// End of stream: a short held silence still plays; a long one ends on
    /// its padding.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        match self.state {
            State::Noisy => {}
            State::MaybeSilent => out.append(&mut self.hold),
            State::Silent => self.drain_tail(out),
        }
        self.state = State::Noisy;
    }
}
