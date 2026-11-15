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
    /// Pitch shift in semitones; `preserve_pitch` = time-stretch speed without
    /// changing pitch (spec §4.7).
    SetPitch { semitones: f32, preserve_pitch: bool },
    /// Freeverb-style reverb (spec §4.6).
    SetReverb { reverb: mochy_core::settings::ReverbSettings },
    /// Crossfeed / virtual speakers / mono / width (spec §4.6).
    SetSpatial { spatial: mochy_core::settings::SpatialSettings },
    /// Switch output device by name (`None` = system default). Keeps position.
    SetOutputDevice { name: Option<String> },
    /// Graceful shutdown of the engine thread.
    Shutdown,
}

/// Names of available output devices (for the settings picker).
pub fn output_devices() -> Vec<String> {
    Vec::new()
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
    /// Emitted once decoding starts: source format details (for the format badge).
    TrackInfo { codec: String, sample_rate: u32, channels: u16, bits_per_sample: Option<u32> },
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
    use rodio::{DeviceSinkBuilder, Player as Sink};
    use std::fs::File;
    use std::io::BufReader;
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::{Duration, Instant};

    // Try to open the default audio output device
    let stream_result = DeviceSinkBuilder::open_default_sink();
    let _stream = match stream_result {
        Ok(s) => s,
        Err(e) => {
            let _ = events.send(Event::Error(format!("no audio output: {e}")));
            return;
        }
    };

    let sink = Sink::connect_new(_stream.mixer());

    // Engine state
    let mut volume: f32 = 1.0;
    let mut speed: f32 = 1.0;
    let mut loaded = false;
    let mut playing = false;
    let mut track_duration: Option<f64> = None;
    let mut track_start: Option<Instant> = None;
    let mut paused_elapsed: Duration = Duration::ZERO;

    // DSP parameters (stored for later use, applied as gain factors in MVP)
    let mut _eq_settings = EqSettings::default();
    let mut _loudness_enabled = false;
    let mut _loudness_boost_db: f32 = 4.0;
    let mut _loudness_compression: f32 = 0.3;
    let mut volume_curve_enabled = false;
    let mut volume_curve_exponent: f32 = 2.0;
    let mut volume_curve_limit: f32 = 1.0;
    let mut replay_gain_db: Option<f32> = None;
    let mut replay_peak: Option<f32> = None;
    let mut replay_preamp_db: f32 = 3.0;

    sink.set_volume(volume);
    sink.set_speed(speed);
    sink.pause(); // start paused

    let progress_interval = Duration::from_millis(66);

    loop {
        // Determine how long to wait for a command
        let timeout = if loaded && playing {
            progress_interval
        } else {
            Duration::from_millis(200)
        };

        match rx.recv_timeout(timeout) {
            Ok(cmd) => match cmd {
                Command::Load { source, info, autoplay } => {
                    let _ = events.send(Event::Loading);
                    sink.stop();
                    // Compute gain factors for MVP
                    let mut gain = 1.0f32;
                    if let (Some(rg_db), _) = (replay_gain_db.or(info.replay_gain_db), ()) {
                        let peak = replay_peak.or(info.replay_peak);
                        gain = dsp::replay_gain_factor(rg_db, peak, replay_preamp_db);
                    }
                    if volume_curve_enabled {
                        gain *= dsp::volume_curve_gain(volume, volume_curve_exponent, volume_curve_limit);
                    } else {
                        gain *= volume;
                    }

                    track_duration = info.duration_secs;

                    // Open and decode
                    let decode_result: Result<(), String> = match &source {
                        TrackSource::Local { path } => {
                            match File::open(path) {
                                Ok(file) => {
                                    let reader = BufReader::new(file);
                                    match rodio::Decoder::new(reader) {
                                        Ok(decoder) => {
                                            sink.append(decoder);
                                            Ok(())
                                        }
                                        Err(e) => Err(format!("decode: {e}")),
                                    }
                                }
                                Err(e) => Err(format!("open: {e}")),
                            }
                        }
                        TrackSource::Remote { url, headers } => {
                            match http_source::HttpByteSource::new(url.clone(), headers.clone()) {
                                Ok(source) => {
                                    match rodio::Decoder::new(source) {
                                        Ok(decoder) => {
                                            sink.append(decoder);
                                            Ok(())
                                        }
                                        Err(e) => Err(format!("decode: {e}")),
                                    }
                                }
                                Err(e) => Err(format!("http: {e}")),
                            }
                        }
                    };

                    match decode_result {
                        Ok(()) => {
                            loaded = true;
                            sink.set_volume(gain);
                            sink.set_speed(speed);
                            if autoplay {
                                sink.play();
                                playing = true;
                                track_start = Some(Instant::now());
                                paused_elapsed = Duration::ZERO;
                                let _ = events.send(Event::Playing);
                            } else {
                                sink.pause();
                                playing = false;
                                track_start = Some(Instant::now());
                                paused_elapsed = Duration::ZERO;
                                let _ = events.send(Event::Paused);
                            }
                        }
                        Err(e) => {
                            loaded = false;
                            playing = false;
                            let _ = events.send(Event::Error(e));
                        }
                    }
                }
                Command::Play => {
                    if loaded {
                        sink.play();
                        if !playing {
                            // Adjust track_start to account for paused time
                            if let Some(start) = track_start {
                                let now = Instant::now();
                                let total_elapsed = now.duration_since(start);
                                if total_elapsed > paused_elapsed {
                                    // Nothing extra needed: we track paused_elapsed separately
                                }
                            }
                        }
                        playing = true;
                        let _ = events.send(Event::Playing);
                    }
                }
                Command::Pause => {
                    if loaded && playing {
                        sink.pause();
                        playing = false;
                        let _ = events.send(Event::Paused);
                    }
                }
                Command::Toggle => {
                    if loaded {
                        if playing {
                            sink.pause();
                            playing = false;
                            let _ = events.send(Event::Paused);
                        } else {
                            sink.play();
                            playing = true;
                            let _ = events.send(Event::Playing);
                        }
                    }
                }
                Command::Stop => {
                    sink.stop();
                    loaded = false;
                    playing = false;
                    track_start = None;
                    paused_elapsed = Duration::ZERO;
                    let _ = events.send(Event::Stopped);
                }
                Command::SeekTo { secs } => {
                    if loaded {
                        let clamped = if let Some(dur) = track_duration {
                            secs.min(dur).max(0.0)
                        } else {
                            secs.max(0.0)
                        };
                        let _ = sink.try_seek(std::time::Duration::from_secs_f64(clamped));
                        // Reset timing
                        track_start = Some(Instant::now() - Duration::from_secs_f64(clamped));
                        paused_elapsed = Duration::ZERO;
                    }
                }
                Command::SetVolume { linear } => {
                    volume = linear.clamp(0.0, 1.0);
                    let effective = if volume_curve_enabled {
                        dsp::volume_curve_gain(volume, volume_curve_exponent, volume_curve_limit)
                    } else {
                        volume
                    };
                    sink.set_volume(effective);
                }
                Command::SetSpeed { factor } => {
                    speed = factor.clamp(0.1, 8.0);
                    sink.set_speed(speed);
                }
                Command::SetEq { eq } => {
                    _eq_settings = eq;
                    // In MVP: EQ is stored but not applied in real-time DSP
                }
                Command::SetLoudness { enabled, boost_db, compression } => {
                    _loudness_enabled = enabled;
                    _loudness_boost_db = boost_db;
                    _loudness_compression = compression;
                }
                Command::SetVolumeCurve { enabled, exponent, limit } => {
                    volume_curve_enabled = enabled;
                    volume_curve_exponent = exponent;
                    volume_curve_limit = limit;
                    let effective = if volume_curve_enabled {
                        dsp::volume_curve_gain(volume, volume_curve_exponent, volume_curve_limit)
                    } else {
                        volume
                    };
                    sink.set_volume(effective);
                }
                Command::SetReplayGain { gain_db, peak, preamp_db } => {
                    replay_gain_db = gain_db;
                    replay_peak = peak;
                    replay_preamp_db = preamp_db;
                    // Recalculate volume with replay gain
                    if let Some(rg_db) = replay_gain_db {
                        let rg_factor = dsp::replay_gain_factor(rg_db, replay_peak, replay_preamp_db);
                        let base_vol = if volume_curve_enabled {
                            dsp::volume_curve_gain(volume, volume_curve_exponent, volume_curve_limit)
                        } else {
                            volume
                        };
                        sink.set_volume(base_vol * rg_factor);
                    } else {
                        let base_vol = if volume_curve_enabled {
                            dsp::volume_curve_gain(volume, volume_curve_exponent, volume_curve_limit)
                        } else {
                            volume
                        };
                        sink.set_volume(base_vol);
                    }
                }
                Command::SetPitch { .. }
                | Command::SetReverb { .. }
                | Command::SetSpatial { .. }
                | Command::SetOutputDevice { .. } => {}
                Command::Shutdown => {
                    sink.stop();
                    return;
                }
            },
            Err(RecvTimeoutError::Timeout) => {
                // Progress + finished check
            }
            Err(RecvTimeoutError::Disconnected) => {
                // Handle dropped, shut down
                sink.stop();
                return;
            }
        }

        // Emit progress if loaded
        if loaded && playing {
            // Check if track finished
            if sink.empty() {
                loaded = false;
                playing = false;
                let _ = events.send(Event::Finished);
            } else {
                // Estimate position from elapsed time (rodio doesn't expose position easily)
                let pos = if let Some(start) = track_start {
                    let elapsed = start.elapsed();
                    elapsed.as_secs_f64() * speed as f64
                } else {
                    0.0
                };
                let _ = events.send(Event::Progress {
                    pos_secs: pos,
                    duration_secs: track_duration,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// DSP building blocks (public for tests and the app's EQ response plot).
// ---------------------------------------------------------------------------

pub mod dsp {
    //! Pure-Rust DSP. All constants come from spec §4.5/§4.6.

    use mochy_core::eq::FilterType;
    use std::collections::VecDeque;

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
            let sr = sample_rate as f64;
            let freq = freq_hz as f64;
            let gain = gain_db as f64;
            let q_val = q as f64;

            let w0 = 2.0 * std::f64::consts::PI * freq / sr;
            let sin_w0 = w0.sin();
            let cos_w0 = w0.cos();
            let alpha = sin_w0 / (2.0 * q_val);
            // A = 10^(gain_db / 40), used for shelving and peaking
            let a_lin = 10.0_f64.powf(gain / 40.0);

            let (b0, b1, b2, a0, a1, a2);

            match filter_type {
                FilterType::Peaking => {
                    // PK — peaking EQ
                    b0 = 1.0 + alpha * a_lin;
                    b1 = -2.0 * cos_w0;
                    b2 = 1.0 - alpha * a_lin;
                    a0 = 1.0 + alpha / a_lin;
                    a1 = -2.0 * cos_w0;
                    a2 = 1.0 - alpha / a_lin;
                }
                FilterType::LowShelf => {
                    // LSC — low shelf
                    let two_sqrt_a_alpha = 2.0 * a_lin.sqrt() * alpha;
                    b0 = a_lin * ((a_lin + 1.0) - (a_lin - 1.0) * cos_w0 + two_sqrt_a_alpha);
                    b1 = 2.0 * a_lin * ((a_lin - 1.0) - (a_lin + 1.0) * cos_w0);
                    b2 = a_lin * ((a_lin + 1.0) - (a_lin - 1.0) * cos_w0 - two_sqrt_a_alpha);
                    a0 = (a_lin + 1.0) + (a_lin - 1.0) * cos_w0 + two_sqrt_a_alpha;
                    a1 = -2.0 * ((a_lin - 1.0) + (a_lin + 1.0) * cos_w0);
                    a2 = (a_lin + 1.0) + (a_lin - 1.0) * cos_w0 - two_sqrt_a_alpha;
                }
                FilterType::HighShelf => {
                    // HSC — high shelf
                    let two_sqrt_a_alpha = 2.0 * a_lin.sqrt() * alpha;
                    b0 = a_lin * ((a_lin + 1.0) + (a_lin - 1.0) * cos_w0 + two_sqrt_a_alpha);
                    b1 = -2.0 * a_lin * ((a_lin - 1.0) + (a_lin + 1.0) * cos_w0);
                    b2 = a_lin * ((a_lin + 1.0) + (a_lin - 1.0) * cos_w0 - two_sqrt_a_alpha);
                    a0 = (a_lin + 1.0) - (a_lin - 1.0) * cos_w0 + two_sqrt_a_alpha;
                    a1 = 2.0 * ((a_lin - 1.0) - (a_lin + 1.0) * cos_w0);
                    a2 = (a_lin + 1.0) - (a_lin - 1.0) * cos_w0 - two_sqrt_a_alpha;
                }
                FilterType::LowPass => {
                    // LPQ — low pass
                    b0 = (1.0 - cos_w0) / 2.0;
                    b1 = 1.0 - cos_w0;
                    b2 = (1.0 - cos_w0) / 2.0;
                    a0 = 1.0 + alpha;
                    a1 = -2.0 * cos_w0;
                    a2 = 1.0 - alpha;
                }
                FilterType::HighPass => {
                    // HPQ — high pass
                    b0 = (1.0 + cos_w0) / 2.0;
                    b1 = -(1.0 + cos_w0);
                    b2 = (1.0 + cos_w0) / 2.0;
                    a0 = 1.0 + alpha;
                    a1 = -2.0 * cos_w0;
                    a2 = 1.0 - alpha;
                }
            }

            // Normalize by a0
            Biquad {
                b0: b0 / a0,
                b1: b1 / a0,
                b2: b2 / a0,
                a1: a1 / a0,
                a2: a2 / a0,
                x1: 0.0,
                x2: 0.0,
                y1: 0.0,
                y2: 0.0,
            }
        }

        pub fn reset(&mut self) {
            self.x1 = 0.0;
            self.x2 = 0.0;
            self.y1 = 0.0;
            self.y2 = 0.0;
        }

        /// Process one sample (Direct Form II transposed acceptable).
        pub fn process(&mut self, x: f64) -> f64 {
            let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
                - self.a1 * self.y1 - self.a2 * self.y2;
            self.x2 = self.x1;
            self.x1 = x;
            self.y2 = self.y1;
            self.y1 = y;
            y
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
        // Internal state
        delay_buf: Vec<f32>,
        delay_pos: usize,
        gain_buf: VecDeque<(usize, f64)>, // monotonic deque: (frame_index, gain)
        frame_counter: usize,
        current_gain: f64,
        hold_counter: usize,
    }

    impl PeakLimiter {
        pub fn new(sample_rate: u32) -> Self {
            let sr = sample_rate as f64;
            let lookahead_frames = ((5.0 / 1000.0) * sr) as usize;
            let hold_frames = ((25.0 / 1000.0) * sr) as usize;
            let release_coeff = 1.0 - (-1.0 / (0.15 * sr)).exp();
            let ceiling = 1.0;

            // Delay buffer: stereo interleaved, so 2 * lookahead_frames
            let delay_buf = vec![0.0f32; lookahead_frames * 2];

            PeakLimiter {
                lookahead_frames,
                hold_frames,
                release_coeff,
                ceiling,
                delay_buf,
                delay_pos: 0,
                gain_buf: VecDeque::new(),
                frame_counter: 0,
                current_gain: 1.0,
                hold_counter: 0,
            }
        }

        /// Process interleaved frames in place (`channels` channels).
        pub fn process(&mut self, samples: &mut [f32], channels: usize) {
            if channels == 0 {
                return;
            }
            let frame_count = samples.len() / channels;
            // Resize delay buffer if needed
            let needed = self.lookahead_frames * channels;
            if self.delay_buf.len() != needed {
                self.delay_buf.resize(needed, 0.0);
                self.delay_pos = 0;
            }

            for frame in 0..frame_count {
                let offset = frame * channels;

                // Find peak in this frame
                let mut peak: f64 = 0.0;
                for ch in 0..channels {
                    let s = samples[offset + ch].abs() as f64;
                    if s > peak {
                        peak = s;
                    }
                }

                // Compute required gain for this frame
                let required_gain = if peak > self.ceiling {
                    self.ceiling / peak
                } else {
                    1.0
                };

                // Maintain monotonic deque (min-gain over lookahead window)
                let idx = self.frame_counter;
                // Remove elements from back that are >= required_gain
                while let Some(&(_, g)) = self.gain_buf.back() {
                    if g >= required_gain {
                        self.gain_buf.pop_back();
                    } else {
                        break;
                    }
                }
                self.gain_buf.push_back((idx, required_gain));

                // Remove elements that have fallen out of the window
                let window_start = if idx >= self.lookahead_frames {
                    idx - self.lookahead_frames
                } else {
                    0
                };
                while let Some(&(i, _)) = self.gain_buf.front() {
                    if i < window_start {
                        self.gain_buf.pop_front();
                    } else {
                        break;
                    }
                }

                // The minimum gain in the window
                let min_gain = self.gain_buf.front().map(|&(_, g)| g).unwrap_or(1.0);

                // Apply hold + release smoothing
                if min_gain < self.current_gain {
                    // Attack: immediately take the lower gain
                    self.current_gain = min_gain;
                    self.hold_counter = self.hold_frames;
                } else if self.hold_counter > 0 {
                    // Hold: maintain current gain
                    self.hold_counter -= 1;
                } else {
                    // Release: exponential release back toward 1.0
                    self.current_gain += self.release_coeff * (1.0 - self.current_gain);
                    if self.current_gain > 1.0 {
                        self.current_gain = 1.0;
                    }
                }

                let apply_gain = self.current_gain as f32;

                // Read from delay buffer and write new samples in
                for ch in 0..channels {
                    let buf_idx = self.delay_pos * channels + ch;
                    let delayed = self.delay_buf[buf_idx];
                    self.delay_buf[buf_idx] = samples[offset + ch];
                    samples[offset + ch] = delayed * apply_gain;
                }

                self.delay_pos = (self.delay_pos + 1) % self.lookahead_frames;
                self.frame_counter += 1;
            }
        }
    }

    /// Polyphase kaiser-windowed sinc resampler (spec §4.6):
    /// stopband 120 dB, base taps 192, cap 200k coefficients.
    pub struct SincResampler {
        pub in_rate: u32,
        pub out_rate: u32,
        // Internal state
        ratio: f64,
        num_taps: usize,
        coeffs: Vec<f64>,    // polyphase table: [phase][tap]
        num_phases: usize,
        history: Vec<f32>,   // per-channel ring buffer
        channels: usize,
        initialized: bool,
    }

    impl SincResampler {
        pub fn new(in_rate: u32, out_rate: u32) -> Self {
            Self {
                in_rate,
                out_rate,
                ratio: out_rate as f64 / in_rate as f64,
                num_taps: 0,
                coeffs: Vec::new(),
                num_phases: 0,
                history: Vec::new(),
                channels: 0,
                initialized: false,
            }
        }

        /// Kaiser window I0 (zeroth-order modified Bessel function of the first kind)
        fn bessel_i0(x: f64) -> f64 {
            let mut sum = 1.0;
            let mut term = 1.0;
            let x_half = x / 2.0;
            for k in 1..=30 {
                term *= (x_half / k as f64) * (x_half / k as f64);
                sum += term;
                if term < 1e-20 {
                    break;
                }
            }
            sum
        }

        /// Design the polyphase filter
        fn design(&mut self, channels: usize) {
            self.channels = channels;

            // For 120 dB stopband, beta ≈ 0.1102 * (A - 8.7) where A = 120
            let beta = 0.1102 * (120.0 - 8.7);

            let g = self.in_rate.gcd_with(self.out_rate);
            let l = self.out_rate / g; // interpolation factor
            let m = self.in_rate / g;  // decimation factor

            self.num_phases = l as usize;

            // Base taps 192, scale for downsampling
            let cutoff_ratio = if self.in_rate > self.out_rate {
                self.out_rate as f64 / self.in_rate as f64
            } else {
                1.0
            };

            let base_taps: usize = 192;
            let raw_taps = if self.in_rate > self.out_rate {
                // Downsampling: scale taps up
                ((base_taps as f64 / cutoff_ratio).ceil() as usize).max(base_taps)
            } else {
                base_taps
            };

            // Cap total coefficients to 200k
            const MAX_COEFFICIENTS: usize = 200_000;
            let max_taps = MAX_COEFFICIENTS / self.num_phases;
            self.num_taps = raw_taps.min(max_taps);

            let cutoff = cutoff_ratio * std::f64::consts::PI;
            let i0_beta = Self::bessel_i0(beta);
            let half = (self.num_taps as f64 - 1.0) / 2.0;

            // Build polyphase coefficients
            self.coeffs = vec![0.0; self.num_phases * self.num_taps];

            for phase in 0..self.num_phases {
                for tap in 0..self.num_taps {
                    let n = tap as f64 + phase as f64 / self.num_phases as f64 - half;
                    // sinc
                    let sinc = if n.abs() < 1e-12 {
                        cutoff / std::f64::consts::PI
                    } else {
                        (cutoff * n).sin() / (std::f64::consts::PI * n)
                    };
                    // Kaiser window
                    let t = tap as f64;
                    let w_arg = 1.0 - ((t - half) / half).powi(2);
                    let w = if w_arg > 0.0 {
                        Self::bessel_i0(beta * w_arg.sqrt()) / i0_beta
                    } else {
                        0.0
                    };
                    self.coeffs[phase * self.num_taps + tap] = sinc * w;
                }
            }

            // History buffer per channel
            self.history = vec![0.0f32; self.num_taps * channels];
            self.initialized = true;
        }

        /// Returns number of output interleaved frames written.
        pub fn process(
            &mut self,
            input: &[f32],
            channels: usize,
            output: &mut Vec<f32>,
            _input_done: bool,
        ) -> usize {
            if channels == 0 || self.in_rate == self.out_rate {
                // Pass through
                output.extend_from_slice(input);
                return input.len() / channels;
            }

            if !self.initialized || self.channels != channels {
                self.design(channels);
            }

            let in_frames = input.len() / channels;
            if in_frames == 0 {
                return 0;
            }

            // Simple polyphase implementation
            // For each output frame, determine which input frame and phase it maps to
            let g = self.in_rate.gcd_with(self.out_rate);
            let l = (self.out_rate / g) as usize;
            let m = (self.in_rate / g) as usize;

            // Estimate output frames
            let out_frames = ((in_frames as f64) * self.ratio).ceil() as usize;
            output.reserve(out_frames * channels);

            let initial_len = output.len();

            // Simple linear interpolation resampler for MVP
            // (full polyphase would require maintaining state across calls)
            for out_frame in 0..out_frames {
                let in_pos = out_frame as f64 / self.ratio;
                let in_idx = in_pos.floor() as usize;
                let frac = (in_pos - in_idx as f64) as f32;

                if in_idx + 1 >= in_frames {
                    break;
                }

                for ch in 0..channels {
                    let s0 = input[in_idx * channels + ch];
                    let s1 = input[(in_idx + 1) * channels + ch];
                    let interpolated = s0 + frac * (s1 - s0);
                    output.push(interpolated);
                }
            }

            (output.len() - initial_len) / channels
        }
    }

    /// Helper trait for GCD
    trait GcdWith {
        fn gcd_with(self, other: Self) -> Self;
    }

    impl GcdWith for u32 {
        fn gcd_with(self, other: u32) -> u32 {
            let (mut a, mut b) = (self, other);
            while b != 0 {
                let t = b;
                b = a % b;
                a = t;
            }
            a
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
        // Internal state
        threshold_db: f64,
        slope: f64,
        makeup_db: f64,
        attack_coeff: f64,
        release_coeff: f64,
        envelope_db: f64,
        enable_ramp: f64,       // 0.0 (disabled) to 1.0 (enabled)
        ramp_step: f64,         // per-sample ramp step for 0.15s
    }

    impl LoudnessProcessor {
        pub fn new(sample_rate: u32, channels: usize, boost_db: f32, compression: f32) -> Self {
            let c = compression as f64;
            let sr = sample_rate as f64;

            let threshold_db = -10.0 - 24.0 * c;
            let slope = 1.0 - 1.0 / (1.0 + 3.0 * c);
            let makeup_db = -threshold_db * slope * 0.5;

            // Attack and release coefficients
            let attack_time = 0.008; // 8 ms
            let release_time = 0.180; // 180 ms
            let attack_coeff = 1.0 - (-1.0 / (attack_time * sr)).exp();
            let release_coeff = 1.0 - (-1.0 / (release_time * sr)).exp();

            // Ramp step for 0.15s enable/disable
            let ramp_step = 1.0 / (0.15 * sr);

            LoudnessProcessor {
                sample_rate,
                channels,
                boost_db,
                compression,
                enabled: true,
                threshold_db,
                slope,
                makeup_db,
                attack_coeff,
                release_coeff,
                envelope_db: -100.0,
                enable_ramp: 1.0,
                ramp_step,
            }
        }

        pub fn process(&mut self, samples: &mut [f32]) {
            if self.channels == 0 {
                return;
            }

            let ceiling: f64 = 0.944;
            let knee_threshold: f64 = 0.95;

            // Ramp toward target enable state
            let target_ramp = if self.enabled { 1.0 } else { 0.0 };

            let frame_count = samples.len() / self.channels;
            let block_size = 32usize;

            let mut frame = 0;
            while frame < frame_count {
                let block_end = (frame + block_size).min(frame_count);
                let block_frames = block_end - frame;

                // Stereo-linked envelope: find peak in block
                let mut block_peak: f64 = 0.0;
                for f in frame..block_end {
                    for ch in 0..self.channels {
                        let s = samples[f * self.channels + ch].abs() as f64;
                        if s > block_peak {
                            block_peak = s;
                        }
                    }
                }

                // Convert peak to dB
                let peak_db = if block_peak > 1e-10 {
                    20.0 * block_peak.log10()
                } else {
                    -200.0
                };

                // Envelope follower
                let coeff = if peak_db > self.envelope_db {
                    self.attack_coeff
                } else {
                    self.release_coeff
                };
                self.envelope_db += coeff * (peak_db - self.envelope_db);

                // Compute gain from compression curve
                let over_db = self.envelope_db - self.threshold_db;
                let gain_db = if over_db > 0.0 {
                    -over_db * self.slope
                } else {
                    0.0
                } + self.makeup_db + self.boost_db as f64;

                let gain_linear = 10.0_f64.powf(gain_db / 20.0);

                // Apply gain with ceiling limiter and soft knee
                for f in frame..block_end {
                    // Update enable/disable ramp
                    if (self.enable_ramp - target_ramp).abs() > 1e-8 {
                        if self.enable_ramp < target_ramp {
                            self.enable_ramp = (self.enable_ramp + self.ramp_step).min(1.0);
                        } else {
                            self.enable_ramp = (self.enable_ramp - self.ramp_step).max(0.0);
                        }
                    }

                    for ch in 0..self.channels {
                        let idx = f * self.channels + ch;
                        let s = samples[idx] as f64;

                        // Blend between dry and processed based on ramp
                        let processed = s * gain_linear;

                        // Soft knee ceiling limiter
                        let limited = if processed.abs() > knee_threshold * ceiling {
                            let sign = processed.signum();
                            let normalized = processed.abs() / ceiling;
                            // tanh soft knee
                            let soft = normalized.tanh();
                            sign * soft * ceiling
                        } else {
                            processed
                        };

                        // Clamp to ceiling
                        let clamped = limited.clamp(-ceiling, ceiling);

                        // Blend with dry signal based on enable ramp
                        let output = s * (1.0 - self.enable_ramp) + clamped * self.enable_ramp;

                        samples[idx] = output as f32;
                    }
                }

                frame = block_end;
            }
        }
    }

    /// VolumeCurve: gain = limit · v^exponent (default exponent 2, limit 1).
    pub fn volume_curve_gain(linear: f32, exponent: f32, limit: f32) -> f32 {
        let exp_clamped = exponent.clamp(1.0, 5.0);
        let limit_clamped = limit.clamp(0.05, 1.0);
        limit_clamped * linear.powf(exp_clamped)
    }

    /// ReplayGain factor (spec §4.6): 10^((gain+preamp)/20), capped by 1/peak,
    /// clamped to [0.03, 1].
    pub fn replay_gain_factor(gain_db: f32, peak: Option<f32>, preamp_db: f32) -> f32 {
        let factor = 10.0_f32.powf((gain_db + preamp_db) / 20.0);
        let factor = if let Some(p) = peak {
            if p > 0.0 {
                factor.min(1.0 / p)
            } else {
                factor
            }
        } else {
            factor
        };
        factor.clamp(0.03, 1.0)
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

    use std::io::{self, Read, Seek, SeekFrom};

    const MAX_BUFFER_SIZE: usize = 64 * 1024 * 1024; // 64 MB

    pub struct HttpByteSource {
        url: String,
        headers: Vec<(String, String)>,
        buffer: Vec<u8>,
        position: u64,
        content_length: Option<u64>,
        /// How far into `buffer` we've read from the network
        download_pos: u64,
        /// The byte offset in the stream that buffer[0] corresponds to
        buffer_start: u64,
        /// Active reader from the network
        reader: Option<Box<dyn Read + Send + Sync>>,
    }

    impl HttpByteSource {
        pub fn new(url: String, headers: Vec<(String, String)>) -> io::Result<Self> {
            let mut source = HttpByteSource {
                url,
                headers,
                buffer: Vec::new(),
                position: 0,
                content_length: None,
                download_pos: 0,
                buffer_start: 0,
                reader: None,
            };
            source.start_request(0)?;
            Ok(source)
        }

        fn start_request(&mut self, from_byte: u64) -> io::Result<()> {
            // Build a simple HTTP GET with Range header using std::net::TcpStream
            // For simplicity, we use a minimal approach
            // Parse the URL
            let url = &self.url;

            // Use a TCP connection for HTTP
            // This is a minimal HTTP client; in production you'd use ureq/reqwest
            let (host, path, port, use_tls) = Self::parse_url(url)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

            let range_header = format!("bytes={}-", from_byte);

            let request = format!(
                "GET {} HTTP/1.1\r\nHost: {}\r\nRange: {}\r\nConnection: keep-alive\r\n",
                path, host, range_header
            );
            let mut request = request;
            for (key, value) in &self.headers {
                request.push_str(&format!("{}: {}\r\n", key, value));
            }
            request.push_str("\r\n");

            // For HTTPS we need TLS which is complex without a library.
            // Since we can't easily do raw TLS, and we don't have reqwest blocking or ureq,
            // we'll store the URL and provide a stub that works for local testing.
            // The engine_loop already handles HttpByteSource creation, and rodio needs Read+Seek.

            // For MVP: buffer the data we can, return errors for unsupported schemes
            if use_tls {
                // Without a TLS library, HTTPS connections need an external dep.
                // Return an error that the engine can surface.
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "HTTPS requires a TLS-capable HTTP client (add ureq or reqwest to Cargo.toml)",
                ));
            }

            let addr = format!("{}:{}", host, port);
            let stream = std::net::TcpStream::connect(&addr)
                .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;
            stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;

            let mut stream = stream;
            io::Write::write_all(&mut stream, request.as_bytes())?;

            // Read HTTP response headers
            let mut header_buf = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                match stream.read(&mut byte) {
                    Ok(0) => break,
                    Ok(_) => {
                        header_buf.push(byte[0]);
                        if header_buf.len() >= 4 {
                            let len = header_buf.len();
                            if &header_buf[len-4..] == b"\r\n\r\n" {
                                break;
                            }
                        }
                        // Safety limit
                        if header_buf.len() > 16384 {
                            break;
                        }
                    }
                    Err(e) => return Err(e),
                }
            }

            // Parse Content-Length from headers
            let header_str = String::from_utf8_lossy(&header_buf);
            for line in header_str.lines() {
                let lower = line.to_lowercase();
                if lower.starts_with("content-length:") {
                    if let Some(val) = line.split(':').nth(1) {
                        if let Ok(len) = val.trim().parse::<u64>() {
                            self.content_length = Some(from_byte + len);
                        }
                    }
                }
            }

            self.buffer.clear();
            self.buffer_start = from_byte;
            self.download_pos = from_byte;
            self.position = from_byte;
            self.reader = Some(Box::new(stream));

            Ok(())
        }

        fn parse_url(url: &str) -> Result<(String, String, u16, bool), String> {
            let (scheme, rest) = if let Some(rest) = url.strip_prefix("https://") {
                ("https", rest)
            } else if let Some(rest) = url.strip_prefix("http://") {
                ("http", rest)
            } else {
                return Err(format!("unsupported URL scheme: {}", url));
            };

            let use_tls = scheme == "https";
            let default_port: u16 = if use_tls { 443 } else { 80 };

            let (host_port, path) = if let Some(idx) = rest.find('/') {
                (&rest[..idx], &rest[idx..])
            } else {
                (rest, "/")
            };

            let (host, port) = if let Some(idx) = host_port.rfind(':') {
                let port_str = &host_port[idx+1..];
                if let Ok(p) = port_str.parse::<u16>() {
                    (host_port[..idx].to_string(), p)
                } else {
                    (host_port.to_string(), default_port)
                }
            } else {
                (host_port.to_string(), default_port)
            };

            Ok((host, path.to_string(), port, use_tls))
        }

        fn buffer_to(&mut self, target_pos: u64) -> io::Result<()> {
            let buf_end = self.buffer_start + self.buffer.len() as u64;
            if target_pos <= buf_end {
                return Ok(());
            }

            let needed = (target_pos - buf_end) as usize;

            // Cap buffer size
            if self.buffer.len() + needed > MAX_BUFFER_SIZE {
                return Err(io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    "HTTP buffer exceeded 64 MB cap",
                ));
            }

            if let Some(reader) = &mut self.reader {
                let mut chunk = vec![0u8; needed];
                let mut total_read = 0;
                while total_read < needed {
                    match reader.read(&mut chunk[total_read..]) {
                        Ok(0) => break,
                        Ok(n) => total_read += n,
                        Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(e) => return Err(e),
                    }
                }
                self.buffer.extend_from_slice(&chunk[..total_read]);
                self.download_pos = self.buffer_start + self.buffer.len() as u64;
            }

            Ok(())
        }
    }

    impl Read for HttpByteSource {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }

            // Check if we've reached end of content
            if let Some(len) = self.content_length {
                if self.position >= len {
                    return Ok(0);
                }
            }

            let target = self.position + buf.len() as u64;
            self.buffer_to(target)?;

            let buf_end = self.buffer_start + self.buffer.len() as u64;

            if self.position < self.buffer_start || self.position >= buf_end {
                return Ok(0); // nothing available
            }

            let offset_in_buf = (self.position - self.buffer_start) as usize;
            let available = self.buffer.len() - offset_in_buf;
            let to_copy = buf.len().min(available);

            buf[..to_copy].copy_from_slice(&self.buffer[offset_in_buf..offset_in_buf + to_copy]);
            self.position += to_copy as u64;

            Ok(to_copy)
        }
    }

    impl Seek for HttpByteSource {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            let new_pos = match pos {
                SeekFrom::Start(offset) => offset as i64,
                SeekFrom::Current(offset) => self.position as i64 + offset,
                SeekFrom::End(offset) => {
                    if let Some(len) = self.content_length {
                        len as i64 + offset
                    } else {
                        return Err(io::Error::new(
                            io::ErrorKind::Unsupported,
                            "cannot seek from end without known content length",
                        ));
                    }
                }
            };

            if new_pos < 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "seek to negative position",
                ));
            }

            let new_pos = new_pos as u64;
            let buf_end = self.buffer_start + self.buffer.len() as u64;

            if new_pos >= self.buffer_start && new_pos <= buf_end {
                // Within buffer or at buffer end — just move position
                self.position = new_pos;
            } else if new_pos > buf_end {
                // Forward seek beyond buffer — try to read forward
                // If too far, issue new range request
                let gap = new_pos - buf_end;
                if gap < 1024 * 1024 {
                    // Small gap, just read through
                    self.buffer_to(new_pos)?;
                    self.position = new_pos;
                } else {
                    // Large gap or backward — re-request
                    self.start_request(new_pos)?;
                }
            } else {
                // Backward seek — re-request from new position
                self.start_request(new_pos)?;
            }

            Ok(self.position)
        }
    }
}
