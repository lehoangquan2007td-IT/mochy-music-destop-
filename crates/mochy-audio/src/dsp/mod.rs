//! Pure-Rust DSP. All constants come from spec §4.5/§4.6 and the Android
//! processors they port. Everything here is allocation-free once
//! constructed (except the rate-changing stages, which run on the decode
//! thread), so it can run inside the real-time output callback.

mod biquad;
pub mod fade;
pub(crate) mod chain;
mod gain;
mod limiter;
mod loudness;
mod resampler;
mod reverb;
pub(crate) mod spatial;
mod stretch;

pub use biquad::{Biquad, EqDesign, EqProcessor};
pub use chain::{ChainParams, LoudnessParams, OutputChain};
pub use fade::Fader;
pub use gain::{parse_replay_gain_number, replay_gain_factor, volume_curve_gain};
pub use limiter::PeakLimiter;
pub use loudness::LoudnessProcessor;
pub use resampler::SincResampler;
pub use reverb::{Reverb, ReverbParams};
pub use spatial::{Spatial, SpatialParams};
pub use stretch::TimeStretch;
