//! Loudness — port of Android `LoudnessAudioProcessor.kt` (spec §4.6):
//! a stereo-linked compressor lifts quiet parts (threshold −10 − 24·c dB,
//! slope 1 − 1/(1+3c), makeup −threshold·slope·0.5, attack 8 ms, release
//! 180 ms, gain per 32-frame block, ramped), then the boost (0–12 dB), then
//! a peak limiter under 0.944 (−0.5 dBFS, release 80 ms) and a tanh soft
//! knee above 0.95. Enable/disable ramps over 0.15 s. Last in the FX chain
//! so it also catches the reverb's peaks.

const BLOCK: usize = 32;
const TOGGLE_SECONDS: f32 = 0.15;
const CEILING: f32 = 0.944;
const KNEE: f32 = 0.95;

pub struct LoudnessProcessor {
    pub sample_rate: u32,
    pub channels: usize,
    /// Boost in dB (clamped 0–12 when applied).
    pub boost_db: f32,
    /// Compression 0–1.
    pub compression: f32,
    pub enabled: bool,
    amount: f32,
    envelope: f32,
    gain: f32,
    limiter_gain: f32,
    attack: f32,
    release: f32,
    limiter_release: f32,
    reduction_db: f32,
}

#[inline]
pub(crate) fn soft_knee(sample: f32, knee: f32) -> f32 {
    let a = sample.abs();
    if a <= knee {
        sample
    } else {
        (knee + (1.0 - knee) * ((a - knee) / (1.0 - knee)).tanh()).copysign(sample)
    }
}

impl LoudnessProcessor {
    pub fn new(sample_rate: u32, channels: usize, boost_db: f32, compression: f32) -> Self {
        let sr = sample_rate.max(1) as f32;
        let mut p = LoudnessProcessor {
            sample_rate,
            channels,
            boost_db,
            compression,
            enabled: true,
            amount: 1.0,
            envelope: 0.0,
            gain: 1.0,
            limiter_gain: 1.0,
            attack: 1.0 - (-1.0 / (0.008 * sr)).exp(),
            release: 1.0 - (-1.0 / (0.18 * sr)).exp(),
            limiter_release: 1.0 - (-1.0 / (0.08 * sr)).exp(),
            reduction_db: 0.0,
        };
        p.reset();
        p
    }

    /// Flush state (seek / new track): start where the settings are, no fade-in.
    pub fn reset(&mut self) {
        self.amount = if self.enabled { 1.0 } else { 0.0 };
        self.envelope = 0.0;
        self.limiter_gain = 1.0;
        self.gain = 10f32.powf(self.boost_db.clamp(0.0, 12.0) / 20.0);
        self.reduction_db = 0.0;
    }

    /// Compressor + limiter reduction for the meter (dB, ≤ 0).
    pub fn reduction_db(&self) -> f32 {
        self.reduction_db
    }

    /// True when disabled and fully faded out (pure pass-through).
    pub fn is_bypassed(&self) -> bool {
        !self.enabled && self.amount == 0.0
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        let ch = self.channels;
        if ch == 0 || ch > 2 {
            return;
        }
        let target = if self.enabled { 1.0 } else { 0.0 };
        if target == 0.0 && self.amount == 0.0 {
            self.reduction_db = 0.0;
            return;
        }
        let c = self.compression.clamp(0.0, 1.0);
        let threshold = -10.0 - 24.0 * c;
        let slope = 1.0 - 1.0 / (1.0 + 3.0 * c);
        // makeup brings back half of what a full-scale signal would lose
        let makeup = if c > 0.0 { -threshold * slope * 0.5 } else { 0.0 };
        let boost = self.boost_db.clamp(0.0, 12.0);
        let toggle_step = 1.0 / (self.sample_rate.max(1) as f32 * TOGGLE_SECONDS);

        let frames = samples.len() / ch;
        let mut deepest = 1.0f32;
        let mut most_reduction = 0.0f32;
        let mut f = 0;
        while f < frames {
            let block = BLOCK.min(frames - f);
            let level_db = 20.0 * self.envelope.max(1e-6).log10();
            let reduction = if c > 0.0 && level_db > threshold { (threshold - level_db) * slope } else { 0.0 };
            most_reduction = most_reduction.min(reduction);
            let target_gain = 10f32.powf((boost + makeup + reduction) / 20.0);
            let gain_step = (target_gain - self.gain) / block as f32;
            for b in 0..block {
                let i = (f + b) * ch;
                let in_l = samples[i];
                let in_r = if ch == 2 { samples[i + 1] } else { in_l };
                let peak_in = in_l.abs().max(in_r.abs());
                let coeff = if peak_in > self.envelope { self.attack } else { self.release };
                self.envelope += (peak_in - self.envelope) * coeff;
                self.gain += gain_step;
                self.amount = if target > self.amount {
                    (self.amount + toggle_step).min(target)
                } else {
                    (self.amount - toggle_step).max(target)
                };
                let mut l = in_l * self.gain;
                let mut r = in_r * self.gain;
                let peak_out = l.abs().max(r.abs()) * self.limiter_gain;
                self.limiter_gain = if peak_out > CEILING {
                    self.limiter_gain * CEILING / peak_out
                } else {
                    self.limiter_gain + (1.0 - self.limiter_gain) * self.limiter_release
                };
                deepest = deepest.min(self.limiter_gain);
                l *= self.limiter_gain;
                r *= self.limiter_gain;
                samples[i] = soft_knee(in_l + (l - in_l) * self.amount, KNEE);
                if ch == 2 {
                    samples[i + 1] = soft_knee(in_r + (r - in_r) * self.amount, KNEE);
                }
            }
            f += block;
        }
        self.reduction_db = (most_reduction + 20.0 * deepest.max(1e-6).log10()) * self.amount;
    }
}
