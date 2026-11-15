//! mochy-audio — playback engine (spec §4).
//!
//! Contract: the app (mochy-app) drives this via `Command`s and consumes
//! `Event`s on two std channels. One engine = one output stream = one
//! currently-loaded track. Decoding is symphonia-backed via rodio (AAC/FLAC/
//! MP3/Vorbis/WAV/PCM supported; Opus is NOT — prefer AAC formats upstream).
//!
//! DSP chain order (spec §4.6): decode → sinc resample (if needed) →
//! biquad EQ cascade → peak limiter (always on, spec §4.5) →
//! loudness processor (optional) → ReplayGain factor → volume curve.

use std::path::PathBuf;
use std::sync::mpsc::Sender;

use mochy_core::eq::EqSettings;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Where the audio bytes come from.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackSource {
    Local { path: PathBuf },
    /// Progressive HTTP(S) URL. `headers` are required on the media request
    /// for some YouTube clients (spec §2.6, e.g. Range + User-Agent).
    Remote { url: String, headers: Vec<(String, String)> },
}

/// Per-track context handed to the engine at load time.
#[derive(Debug, Clone, Default)]
pub struct TrackInfo {
    pub duration_secs: Option<f64>,
    pub video_id: Option<String>,
    /// ReplayGain track gain in dB (Vorbis comment / stream metadata).
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
    /// Playback speed 0.1–8.0 (no pitch preservation in MVP).
    SetSpeed { factor: f32 },
    /// Replace EQ design live (crossfaded in, no re-seek — spec §4.3).
    SetEq { eq: EqSettings },
    /// Loudness processor toggle/params (spec §4.6 LoudnessAudioProcessor).
    SetLoudness { enabled: bool, boost_db: f32, compression: f32 },
    /// Exponential volume curve (spec §4.6 VolumeCurve).
    SetVolumeCurve { enabled: bool, exponent: f32, limit: f32 },
    /// ReplayGain: gain in dB + peak (None = bypass).
    SetReplayGain { gain_db: Option<f32>, peak: Option<f32>, preamp_db: f32 },
    /// Graceful shutdown of the engine thread.
    Shutdown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Load started (fetching/buffering).
    Loading,
    Playing,
    Paused,
    /// Stopped without a finished track.
    Stopped,
    /// Track reached its natural end.
    Finished,
    /// Unrecoverable failure for the current track.
    Error(String),
    /// Emitted every ~66 ms while a track is loaded (spec §4.1 progress loop).
    Progress { pos_secs: f64, duration_secs: Option<f64> },
}

/// Handle used by the app to command the engine thread.
#[derive(Debug, Clone)]
pub struct EngineHandle {
    tx: std::sync::mpsc::Sender<Command>,
}

impl EngineHandle {
    pub fn new(tx: std::sync::mpsc::Sender<Command>) -> Self {
        Self { tx }
    }

    /// Send a command; the engine thread owns the receiving side.
    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
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

/// Spawn the engine thread. It opens the default output device immediately,
/// answers commands until `Command::Shutdown`, and emits `Event`s on `events`.
/// The thread runs to completion even if the handle is dropped.
pub fn spawn_engine(events: Sender<Event>) -> Result<EngineHandle, AudioError> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("mochy-audio-engine".into())
        .spawn(move || engine_loop(rx, events))
        .map_err(|e| AudioError::Other(e.to_string()))?;
    Ok(EngineHandle::new(tx))
}

fn engine_loop(rx: std::sync::mpsc::Receiver<Command>, events: Sender<Event>) {
    let _ = rx;
    let _ = events;
    todo!("engine_loop: implemented by mochy-audio agent (spec §4)")
}

// ---------------------------------------------------------------------------
// DSP building blocks (public for tests and the app's EQ response plot).
// ---------------------------------------------------------------------------

pub mod dsp {
    //! Pure-Rust DSP. All constants come from spec §4.5/§4.6.

    use mochy_core::eq::FilterType;

    /// RBJ cookbook biquad. Stateful, stereo-linking handled by callers.
    #[derive(Debug, Clone, Copy, Default)]
    pub struct Biquad {
        pub b0: f64,
        pub b1: f64,
        pub b2: f64,
        pub a1: f64,
        pub a2: f64,
        x1: f64,
        x2: f64,
        y1: f64,
        y2: f64,
    }

    impl Biquad {
        /// Design one section (spec §4.3: PK/LSC/HSC/LPQ/HPQ, RBJ).
        pub fn design(
            filter_type: FilterType,
            freq_hz: f32,
            gain_db: f32,
            q: f32,
            sample_rate: f32,
        ) -> Biquad {
            let _ = (filter_type, freq_hz, gain_db, q, sample_rate);
            todo!("Biquad::design")
        }

        pub fn reset(&mut self) {
            self.x1 = 0.0;
            self.x2 = 0.0;
            self.y1 = 0.0;
            self.y2 = 0.0;
        }

        /// Process one sample (Direct Form II transposed acceptable).
        pub fn process(&mut self, x: f64) -> f64 {
            let _ = x;
            todo!("Biquad::process")
        }
    }

    /// Always-on look-ahead peak limiter (spec §4.5 `PeakLimiter`):
    /// lookahead 5 ms, hold 25 ms, exponential release 150 ms, ceiling 1.0,
    /// min-gain via monotonic queue, resting state = pure delay.
    pub struct PeakLimiter {
        pub lookahead_frames: usize,
        pub hold_frames: usize,
        pub release_coeff: f64,
        pub ceiling: f64,
    }

    impl PeakLimiter {
        pub fn new(sample_rate: u32) -> Self {
            let _ = sample_rate;
            todo!("PeakLimiter::new")
        }

        /// Process interleaved frames in place (`channels` channels).
        pub fn process(&mut self, samples: &mut [f32], channels: usize) {
            let _ = (samples, channels);
            todo!("PeakLimiter::process")
        }
    }

    /// Polyphase kaiser-windowed sinc resampler (spec §4.6):
    /// stopband 120 dB, base taps 192, cap 200k coefficients.
    pub struct SincResampler {
        pub in_rate: u32,
        pub out_rate: u32,
    }

    impl SincResampler {
        pub fn new(in_rate: u32, out_rate: u32) -> Self {
            Self { in_rate, out_rate }
        }

        /// Returns number of output interleaved frames written.
        pub fn process(
            &mut self,
            input: &[f32],
            channels: usize,
            output: &mut Vec<f32>,
            input_done: bool,
        ) -> usize {
            let _ = (input, channels, output, input_done);
            todo!("SincResampler::process")
        }
    }

    /// Loudness processor (spec §4.6): threshold −10 − 24·c dB, slope
    /// 1 − 1/(1+3c), makeup = −threshold·slope·0.5, attack 8 ms, release
    /// 180 ms, 32-frame stereo-linked envelope, ceiling 0.944, knee 0.95 tanh.
    pub struct LoudnessProcessor {
        pub sample_rate: u32,
        pub channels: usize,
        pub boost_db: f32,
        pub compression: f32,
        pub enabled: bool,
    }

    impl LoudnessProcessor {
        pub fn new(sample_rate: u32, channels: usize, boost_db: f32, compression: f32) -> Self {
            todo!("LoudnessProcessor::new")
        }

        pub fn process(&mut self, samples: &mut [f32]) {
            let _ = samples;
            todo!("LoudnessProcessor::process")
        }
    }

    /// VolumeCurve: gain = limit · v^exponent (default exponent 2, limit 1).
    pub fn volume_curve_gain(linear: f32, exponent: f32, limit: f32) -> f32 {
        let _ = (linear, exponent, limit);
        todo!("volume_curve_gain")
    }

    /// ReplayGain factor (spec §4.6): 10^((gain+preamp)/20), capped by 1/peak,
    /// clamped to [0.03, 1].
    pub fn replay_gain_factor(gain_db: f32, peak: Option<f32>, preamp_db: f32) -> f32 {
        let _ = (gain_db, peak, preamp_db);
        todo!("replay_gain_factor")
    }
}

// ---------------------------------------------------------------------------
// HTTP progressive reader used for remote tracks (spec §2.6): memory-buffered
// with ranged re-requests on seek; probe/validate is the caller's job.
// ---------------------------------------------------------------------------

pub mod http_source {
    //! `HttpByteSource`: `Read + Seek` over a remote media URL.
    //! Strategy: buffer forward in memory (cap ~64 MB); on backward or
    //! out-of-buffer seek, issue a ranged request.

    pub struct HttpByteSource;

    impl HttpByteSource {
        pub fn new(url: String, headers: Vec<(String, String)>) -> std::io::Result<Self> {
            let _ = (url, headers);
            todo!("HttpByteSource::new")
        }
    }

    impl std::io::Read for HttpByteSource {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let _ = buf;
            todo!("HttpByteSource::read")
        }
    }

    impl std::io::Seek for HttpByteSource {
        fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
            let _ = pos;
            todo!("HttpByteSource::seek")
        }
    }
}
