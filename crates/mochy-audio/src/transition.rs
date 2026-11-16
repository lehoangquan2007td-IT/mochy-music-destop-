//! Track transitions (port of Android `MusicService` crossfade,
//! `TransportTransitions.kt` and `crossmix/*`): the public knobs the app
//! sends, plus the small fade curves shared by the engine, the decoder-side
//! mixer and the output callback.
//!
//! - **Gapless**: with a preloaded next track ([`crate::Command::PreloadNext`])
//!   the decoder runs straight from the last frame of one track into the
//!   first of the next, on the same output ring.
//! - **Crossfade** (Android AUTO / MANUAL): equal-power `sin`/`cos` ramps over
//!   the last N seconds, started at `duration − N` like
//!   `MusicService.scheduleCrossfade`.
//! - **Crossmix**: beat/energy-aware planning (see [`crate::crossmix`]): both
//!   songs are analysed, the planner picks style, length, hand-over bar,
//!   landing point and tempo match, and the decoder renders both lanes
//!   through per-lane filters / echo / reverb.
//! - **Transport fades**: smoothstep volume fades on play, pause, skip and
//!   seek, applied in the output callback (low latency).
//! - **Skip silence**: media3 `SilenceSkippingAudioProcessor(2 s, 20 ms, 256)`
//!   semantics, plus the "instant" 15 s seek hops.

use crate::{TrackInfo, TrackSource};

/// What happens between two consecutive queue items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransitionKind {
    /// Plain gapless hand-over (Android "Off").
    #[default]
    Gapless,
    /// Fixed-length equal-power crossfade (Android Auto = 5 s, Manual = 1–30 s).
    Crossfade,
    /// Planned, beat-matched DJ-style mix (Android "Crossmix").
    Crossmix,
}

/// Crossmix listener options (`CrossmixSettings.kt`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrossmixOptions {
    /// `CrossmixLength.scale`: 0.6 short, 1 medium, 1.5 long.
    pub length_scale: f32,
    /// `CrossmixTempoMatch.maxSpeedUp`: 0.006 off, 0.04 gentle, 0.10 full.
    pub max_speed_up: f32,
    /// One style for every mix (`None` = the planner picks).
    pub style: Option<crate::crossmix::Style>,
    /// Backspin-like stunts allowed when the energy jumps.
    pub stunts: bool,
    /// Every hand-over a real blend of both songs.
    pub always_mix: bool,
}

impl Default for CrossmixOptions {
    fn default() -> Self {
        Self { length_scale: 1.0, max_speed_up: 0.10, style: None, stunts: true, always_mix: true }
    }
}

/// Transition configuration (`Command::SetTransition`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransitionConfig {
    pub kind: TransitionKind,
    /// Crossfade length in seconds (used by [`TransitionKind::Crossfade`]).
    pub crossfade_secs: f32,
    pub crossmix: CrossmixOptions,
}

impl Default for TransitionConfig {
    fn default() -> Self {
        Self { kind: TransitionKind::Gapless, crossfade_secs: 5.0, crossmix: CrossmixOptions::default() }
    }
}

/// Pause / play / skip / seek fades (`TransportTransitions.kt`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportFade {
    pub enabled: bool,
    /// Fade length, seconds (Android 0.5–10, default 2).
    pub secs: f32,
}

impl Default for TransportFade {
    fn default() -> Self {
        Self { enabled: true, secs: TRANSPORT_FADE_DEFAULT_SECS }
    }
}

pub const TRANSPORT_FADE_DEFAULT_SECS: f32 = 2.0;
pub const TRANSPORT_FADE_MIN_SECS: f32 = 0.5;
pub const TRANSPORT_FADE_MAX_SECS: f32 = 10.0;
/// A skip shouldn't keep you waiting on the old song (`SkipOutMaxMs`).
pub const SKIP_OUT_MAX_SECS: f32 = 1.5;
/// Shortest swell into a skipped-to song.
pub const SKIP_IN_MIN_SECS: f32 = 0.3;
/// Seeks get a short dip either side (desktop addition, same switch).
pub const SEEK_FADE_MAX_SECS: f32 = 0.12;
/// Changing your mind mid-pause comes back over at least this long.
pub const RESUME_MIN_SECS: f32 = 0.2;

impl TransportFade {
    pub fn active(&self) -> bool {
        self.enabled && self.secs > 0.0
    }
    pub fn len(&self) -> f32 {
        self.secs.clamp(TRANSPORT_FADE_MIN_SECS, TRANSPORT_FADE_MAX_SECS)
    }
    /// Old song's fade-out on a skip: `min(len / 2, 1.5 s)`.
    pub fn skip_out(&self) -> f32 {
        (self.len() / 2.0).min(SKIP_OUT_MAX_SECS)
    }
    /// New song's swell on a skip: `max(len / 2, 0.3 s)`.
    pub fn skip_in(&self) -> f32 {
        (self.len() / 2.0).max(SKIP_IN_MIN_SECS)
    }
    pub fn seek_len(&self) -> f32 {
        (self.len() / 4.0).min(SEEK_FADE_MAX_SECS)
    }
    /// Swell length when play is pressed mid-pause at heard level `level`.
    pub fn resume_len(&self, level: f32) -> f32 {
        if level > 0.0 { (self.len() * (1.0 - level)).max(RESUME_MIN_SECS) } else { self.len() }
    }
}

/// Skip-silence switches (Android `skipSilence`, `skipSilenceInstant`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SkipSilence {
    pub enabled: bool,
    /// Jump through long silences with 15 s seeks instead of decoding them.
    pub instant: bool,
}

/// Per-song speed/pitch (`SongTuning.kt`); overrides the Audio Lab values
/// for one track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SongTuning {
    pub speed: f32,
    pub semitones: f32,
}

/// Loudness normalization for a preloaded track, as `SetReplayGain` would
/// set it at load time.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NextGain {
    pub gain_db: Option<f32>,
    pub peak: Option<f32>,
    pub preamp_db: f32,
    /// Use the file's REPLAYGAIN tags when `gain_db` is `None`.
    pub use_file_tags: bool,
}

/// A track the engine may run into at the end of the current one.
#[derive(Debug, Clone)]
pub struct NextTrack {
    pub source: TrackSource,
    pub info: TrackInfo,
    /// Echoed back in [`crate::Event::NextStarted`] (the app's queue uid).
    pub token: u64,
    pub gain: NextGain,
    pub tuning: Option<SongTuning>,
    /// `false` forces a plain gapless hand-over (same album, repeat one…).
    pub allow_mix: bool,
}

// ---------------------------------------------------------------------------
// Curves
// ---------------------------------------------------------------------------

/// Hermite smoothstep (the transport fades' shape).
#[inline]
pub fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Equal-power crossfade at progress `p`: `(outgoing, incoming)` gains =
/// `(cos, sin)` of `p·π/2` (Android end-of-song crossfade).
#[inline]
pub fn equal_power(p: f32) -> (f32, f32) {
    let a = p.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
    (a.cos(), a.sin())
}

/// Equal-gain S-curve (`cos²`, `sin²`): Android's skip blend.
#[inline]
pub fn equal_gain(p: f32) -> (f32, f32) {
    let (o, i) = equal_power(p);
    (o * o, i * i)
}

/// Volume under a leaving effect: held, then down over the last fifth
/// (`TransportTransitions.tailFade`).
#[inline]
pub fn tail_fade(p: f32) -> f32 {
    const FROM: f32 = 0.8;
    1.0 - smoothstep((p - FROM) / (1.0 - FROM))
}
