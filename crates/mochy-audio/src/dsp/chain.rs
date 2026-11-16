//! The fixed-rate FX chain run in the output callback (spec §4.6 order):
//! EQ cascade (crossfaded designs) → PeakLimiter (always on) → Reverb →
//! Spatial → Loudness → ReplayGain·VolumeCurve gain (ramped). Stereo
//! interleaved f32, allocation-free after construction, every parameter
//! live-updatable without clicks.

use super::biquad::{EqDesign, EqProcessor};
use super::limiter::PeakLimiter;
use super::loudness::LoudnessProcessor;
use super::reverb::{Reverb, ReverbParams};
use super::spatial::{Spatial, SpatialParams};

/// Loudness processor parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessParams {
    pub enabled: bool,
    pub boost_db: f32,
    pub compression: f32,
}

impl Default for LoudnessParams {
    fn default() -> Self {
        LoudnessParams { enabled: false, boost_db: 4.0, compression: 0.3 }
    }
}

/// Everything the chain needs; `Copy` so it can cross into the callback.
#[derive(Debug, Clone, Copy)]
pub struct ChainParams {
    pub eq: Option<EqDesign>,
    pub reverb: ReverbParams,
    pub spatial: SpatialParams,
    pub loudness: LoudnessParams,
    /// Final linear gain (ReplayGain factor × volume curve).
    pub gain: f32,
}

impl Default for ChainParams {
    fn default() -> Self {
        ChainParams {
            eq: None,
            reverb: ReverbParams::default(),
            spatial: SpatialParams::default(),
            loudness: LoudnessParams::default(),
            gain: 1.0,
        }
    }
}

/// Live parameter update for the callback (all `Copy`, nothing to free).
/// Deliberately unboxed: boxing would allocate/free on the audio thread.
#[derive(Debug, Clone, Copy)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum ParamMsg {
    Eq(Option<EqDesign>),
    Reverb(ReverbParams),
    Spatial(SpatialParams),
    Loudness(LoudnessParams),
    Gain(f32),
    /// Transport fade: ramp to `target` over `frames` (smoothstep); `wait`
    /// holds the ramp until real audio flows. Handled by the callback.
    Fade { target: f32, frames: u32, wait: bool },
    /// Transport fade level, at once.
    FadeSet(f32),
    /// Switch the final gain once the device has taken ring frame `at`
    /// (a gapless / crossfaded track change). Cleared by a plain `Gain`.
    GainAt { at: u64, gain: f32 },
}

const GAIN_RAMP_SECONDS: f32 = 0.02;

pub struct OutputChain {
    sample_rate: u32,
    eq: EqProcessor,
    limiter: PeakLimiter,
    reverb: Reverb,
    spatial: Spatial,
    loudness: LoudnessProcessor,
    gain: f32,
    gain_target: f32,
    gain_step: f32,
}

impl OutputChain {
    pub fn new(sample_rate: u32, p: &ChainParams) -> Self {
        let mut loudness = LoudnessProcessor::new(sample_rate, 2, p.loudness.boost_db, p.loudness.compression);
        loudness.enabled = p.loudness.enabled;
        loudness.reset();
        OutputChain {
            sample_rate,
            eq: EqProcessor::new(sample_rate, p.eq),
            limiter: PeakLimiter::with_channels(sample_rate, 2),
            reverb: Reverb::new(sample_rate, p.reverb),
            spatial: Spatial::new(sample_rate, p.spatial),
            loudness,
            gain: p.gain,
            gain_target: p.gain,
            gain_step: 0.0,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn set_eq(&mut self, design: Option<EqDesign>) {
        self.eq.set_design(design);
    }

    pub fn set_reverb(&mut self, p: ReverbParams) {
        self.reverb.set_params(p);
    }

    pub fn set_spatial(&mut self, p: SpatialParams) {
        self.spatial.set_params(p);
    }

    pub fn set_loudness(&mut self, p: LoudnessParams) {
        self.loudness.enabled = p.enabled;
        self.loudness.boost_db = p.boost_db;
        self.loudness.compression = p.compression;
    }

    pub fn set_gain(&mut self, gain: f32) {
        let gain = if gain.is_finite() { gain.max(0.0) } else { 0.0 };
        self.gain_target = gain;
        let frames = (self.sample_rate as f32 * GAIN_RAMP_SECONDS).max(1.0);
        self.gain_step = (gain - self.gain).abs() / frames;
    }

    pub(crate) fn apply(&mut self, msg: ParamMsg) {
        match msg {
            ParamMsg::Eq(d) => self.set_eq(d),
            ParamMsg::Reverb(p) => self.set_reverb(p),
            ParamMsg::Spatial(p) => self.set_spatial(p),
            ParamMsg::Loudness(p) => self.set_loudness(p),
            ParamMsg::Gain(g) => self.set_gain(g),
            ParamMsg::Fade { .. } | ParamMsg::FadeSet(_) | ParamMsg::GainAt { .. } => {}
        }
    }

    /// Frames of delay the chain adds (the limiter's lookahead).
    pub fn latency_frames(&self) -> usize {
        self.limiter.latency_frames()
    }

    /// Loudness meter (dB ≤ 0).
    pub fn loudness_reduction_db(&self) -> f32 {
        self.loudness.reduction_db()
    }

    /// Seek / new track: clear filter memories and tails.
    pub fn reset(&mut self) {
        self.eq.reset();
        self.limiter.reset();
        self.reverb.reset();
        self.spatial.reset();
        self.loudness.reset();
    }

    /// Process stereo interleaved samples in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        self.eq.process(buf);
        self.limiter.process(buf, 2);
        self.reverb.process(buf);
        self.spatial.process(buf);
        self.loudness.process(buf);
        if self.gain == self.gain_target {
            let g = self.gain;
            if g != 1.0 {
                buf.iter_mut().for_each(|s| *s *= g);
            }
        } else {
            for frame in buf.as_chunks_mut::<2>().0 {
                self.gain = if self.gain < self.gain_target {
                    (self.gain + self.gain_step).min(self.gain_target)
                } else {
                    (self.gain - self.gain_step).max(self.gain_target)
                };
                frame[0] *= self.gain;
                frame[1] *= self.gain;
            }
        }
        for s in buf.iter_mut() {
            if !s.is_finite() {
                *s = 0.0;
            }
            *s = s.clamp(-1.0, 1.0);
        }
    }
}
