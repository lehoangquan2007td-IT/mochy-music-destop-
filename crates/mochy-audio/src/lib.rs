//! mochy-audio — playback engine (spec §4).
//!
//! Contract: the app (mochy-app) drives this via `Command`s and consumes
//! `Event`s on two std channels. One engine = one output stream = one
//! currently-loaded track.
//!
//! Architecture (all pure Rust):
//! - **decode thread** — symphonia 0.5 (AAC/M4A, MP3, FLAC, ALAC, Vorbis,
//!   WAV/PCM/ADPCM, AIFF, CAF, MKV/WebM) plus the pure-Rust `rusty-opus`
//!   for Opus in WebM (YouTube itag 251) and Ogg. Audio is folded to stereo
//!   f32, then time-stretched (WSOLA, for preserve-pitch / pitch shift) and
//!   resampled to the device rate by a Kaiser sinc resampler whose ratio
//!   also carries varispeed and pitch. The result goes into a lock-free SPSC
//!   ring (`rtrb`).
//! - **cpal callback** — never blocks, locks or allocates: pops the ring and
//!   runs the fixed-rate FX chain in spec §4.6 order: biquad EQ cascade
//!   (designs crossfade in over 30 ms) → PeakLimiter (always on) → Reverb
//!   (Freeverb) → Spatial (crossfeed / virtual speakers / mono / width) →
//!   Loudness → ReplayGain × VolumeCurve (ramped). Parameters arrive on a
//!   second SPSC ring as `Copy` messages; pause/seek are de-clicked.
//! - **engine thread** — commands, events (progress every 66 ms from frames
//!   the device actually consumed), spectrum FFT, device loss recovery.
//! - **remote sources** — [`http_source::HttpByteSource`]: HTTPS (rustls)
//!   ranged-chunk fetching with prefetch and retry.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use mochy_core::eq::EqSettings;
use thiserror::Error;

mod decoder;
mod engine;
mod mix;
mod output;
mod spectrum;

pub mod crossmix;
pub mod dsp;
pub mod http_source;
pub mod output_info;
pub mod silence;
pub mod transition;

pub use decoder::{DecodeSession, RateParams, RatePipeline, ReplayGainTags, SourceInfo};
pub use spectrum::SpectrumAnalyzer;
pub use transition::{
    CrossmixOptions, NextGain, NextTrack, SkipSilence, SongTuning, TransitionConfig, TransitionKind, TransportFade,
};

/// Where the audio bytes come from.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackSource {
    Local { path: PathBuf },
    /// Progressive HTTP(S) URL. `headers` are sent on every media request
    /// (spec §2.6, e.g. User-Agent / Origin for some YouTube clients); the
    /// engine sets `Range` itself (chunked ranged fetching).
    Remote { url: String, headers: Vec<(String, String)> },
}

/// Per-track context handed to the engine at load time.
#[derive(Debug, Clone, Default)]
pub struct TrackInfo {
    pub duration_secs: Option<f64>,
    pub video_id: Option<String>,
    /// ReplayGain track gain in dB (Vorbis comment / stream metadata).
    /// When `None`, the engine uses the file's REPLAYGAIN tags if present
    /// (unless `SetReplayGain` is sent for the track).
    pub replay_gain_db: Option<f32>,
    /// ReplayGain track peak (0–1+).
    pub replay_peak: Option<f32>,
}

#[derive(Debug, Clone)]
pub enum Command {
    /// Replace the current track and optionally start playing.
    Load { source: TrackSource, info: TrackInfo, autoplay: bool },
    Play,
    Pause,
    Toggle,
    /// Unload and idle (silence).
    Stop,
    /// Absolute seek in seconds (clamped to duration when known).
    SeekTo { secs: f64 },
    /// Linear volume 0.0–1.0 (pre-curve).
    SetVolume { linear: f32 },
    /// Playback speed 0.1–8.0. Pitch follows the speed (varispeed) unless
    /// `SetPitch { preserve_pitch: true, .. }` is active.
    SetSpeed { factor: f32 },
    /// Replace EQ design live (crossfaded in, no re-seek — spec §4.3).
    SetEq { eq: EqSettings },
    /// Loudness processor toggle/params (spec §4.6 LoudnessAudioProcessor).
    SetLoudness { enabled: bool, boost_db: f32, compression: f32 },
    /// Exponential volume curve (spec §4.6 VolumeCurve).
    SetVolumeCurve { enabled: bool, exponent: f32, limit: f32 },
    /// ReplayGain for the current track: gain in dB + peak (None = bypass)
    /// and the preamp (kept across tracks).
    SetReplayGain { gain_db: Option<f32>, peak: Option<f32>, preamp_db: f32 },
    /// Pitch shift in semitones; `preserve_pitch` = time-stretch speed without
    /// changing pitch (spec §4.7). Semitones apply only when preserving.
    SetPitch { semitones: f32, preserve_pitch: bool },
    /// Freeverb-style reverb (spec §4.6).
    SetReverb { reverb: mochy_core::settings::ReverbSettings },
    /// Crossfeed / virtual speakers / mono / width (spec §4.6).
    SetSpatial { spatial: mochy_core::settings::SpatialSettings },
    /// Switch output device by name (`None` = system default). Keeps position.
    SetOutputDevice { name: Option<String> },
    /// Graceful shutdown of the engine thread.
    Shutdown,
    // ---- transitions (see `transition.rs`) ----
    /// Open the track that follows the current one, so the engine can run
    /// into it gaplessly or through the configured crossfade / crossmix.
    /// Replaces any earlier preload. `Event::NextStarted` reports the
    /// switch; without it the current track ends with `Event::Finished`.
    PreloadNext { next: Box<transition::NextTrack> },
    /// Forget the preloaded track (the queue changed).
    ClearNext,
    /// Gapless / crossfade / crossmix between consecutive tracks.
    SetTransition { config: transition::TransitionConfig },
    /// Fades on play / pause / skip (Load while playing) / seek.
    SetTransportFade { fade: transition::TransportFade },
    SetSkipSilence { skip: transition::SkipSilence },
    /// Speed/pitch for the track of the most recent `Load` (`None` = the
    /// Audio Lab values). Send right after `Load`.
    SetSongTuning { tuning: Option<transition::SongTuning> },
    /// Fade out over `secs`, then pause (sleep timer).
    PauseFade { secs: f32 },
}

/// Names of available output devices (for the settings picker).
pub fn output_devices() -> Vec<String> {
    output::device_names()
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Load started (fetching/buffering).
    Loading,
    Playing,
    Paused,
    /// Stopped without a finished track.
    Stopped,
    /// Track reached its natural end (decoder EOF and the device played it all).
    Finished,
    /// Unrecoverable failure for the current track (or the output device).
    Error(String),
    /// Emitted every ~66 ms while a track is loaded (spec §4.1 progress loop).
    Progress { pos_secs: f64, duration_secs: Option<f64> },
    /// Emitted once decoding starts: source format details (for the format badge).
    TrackInfo { codec: String, sample_rate: u32, channels: u16, bits_per_sample: Option<u32> },
    /// The engine moved on to the track preloaded with `token`
    /// (gapless or crossfaded); the previous track counts as finished.
    NextStarted { token: u64 },
}

/// Number of log-spaced bands in [`EngineHandle::spectrum`] (20 Hz – 20 kHz).
pub const SPECTRUM_BANDS: usize = 64;

/// Handle used by the app to command the engine thread.
/// NOTE: the app must treat `Event` as non-exhaustive (use a `_ => {}` arm).
#[derive(Debug, Clone)]
pub struct EngineHandle {
    tx: std::sync::mpsc::Sender<Command>,
    spectrum: std::sync::Arc<std::sync::Mutex<Vec<f32>>>,
}

impl EngineHandle {
    pub fn new(tx: std::sync::mpsc::Sender<Command>) -> Self {
        Self { tx, spectrum: Default::default() }
    }

    fn with_spectrum(tx: Sender<Command>, spectrum: Arc<Mutex<Vec<f32>>>) -> Self {
        Self { tx, spectrum }
    }

    /// Send a command; the engine thread owns the receiving side.
    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    /// Latest post-DSP output spectrum: `SPECTRUM_BANDS` values in dBFS
    /// (≈ −90..0), log-spaced 20 Hz – 20 kHz, updated ~30×/s while playing.
    /// Empty when nothing is playing. Cheap; call every frame.
    pub fn spectrum(&self) -> Vec<f32> {
        self.spectrum.lock().map(|v| v.clone()).unwrap_or_default()
    }
}

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("no audio output device found")]
    NoOutputDevice,
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
    #[error("decode failed: {0}")]
    Decode(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

/// Spawn the engine thread. It opens the default output device (emitting
/// `Event::Error` and retrying later if there is none), answers commands
/// until `Command::Shutdown` (or the handle's channel closes), and emits
/// `Event`s on `events`.
pub fn spawn_engine(events: Sender<Event>) -> Result<EngineHandle, AudioError> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spectrum: Arc<Mutex<Vec<f32>>> = Default::default();
    let shared = spectrum.clone();
    std::thread::Builder::new()
        .name("mochy-audio-engine".into())
        .spawn(move || engine::run(rx, events, shared))
        .map_err(|e| AudioError::Other(e.to_string()))?;
    Ok(EngineHandle::with_spectrum(tx, spectrum))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod transition_tests;
