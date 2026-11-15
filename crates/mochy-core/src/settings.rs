//! App settings — a typed subset of the Android DataStore keys (spec §5.3).
//! Persisted as a JSON file by mochy-storage.

use serde::{Deserialize, Serialize};

/// YouTube audio quality tiers (spec §2.5). MVP uses Auto/Low/Medium/High.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AudioQuality {
    #[default]
    Auto,
    Low,
    Medium,
    High,
    Lossless,
    HiResLossless,
    DolbyAtmos,
}

/// Android `RepeatMode` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RepeatMode {
    Off,
    #[default]
    All,
    One,
}

/// Transition modes (spec §4.8). Desktop MVP implements simple fade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TransitionMode {
    AutoCrossfade,
    #[default]
    Crossmix,
    ManualCrossfade,
    Off,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// Dynamic accent extracted from artwork (Palette+Score, spec §7.4).
    pub dynamic_theme: bool,
    /// Manual accent override as `0xRRGGBB`, when not following artwork.
    pub selected_theme_color: Option<u32>,
    /// Pure black surfaces instead of near-black.
    pub pure_black: bool,
    pub thumbnail_corner_radius: f32,
    /// Motion scale multiplier (Android `animationSpeed`).
    pub animation_speed: f32,
    /// Reduce transparency/blur-heavy effects for performance.
    pub disable_blur: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            dynamic_theme: true,
            selected_theme_color: None,
            pure_black: false,
            thumbnail_corner_radius: 8.0,
            animation_speed: 1.0,
            disable_blur: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Playback {
    pub audio_quality: AudioQuality,
    /// Normalization via stream loudness (Android `audioNormalization`).
    pub audio_normalization: bool,
    pub skip_silence: bool,
    /// 0.0–1.0 linear volume, restored across runs.
    pub player_volume: f32,
    pub shuffle_mode: bool,
    pub repeat_mode: RepeatMode,
    /// Stream failure auto-skip timeout, ms (0 = disabled).
    pub stream_failure_timeout: u32,
    pub auto_skip_next_on_error: bool,
    pub transition_mode: TransitionMode,
    /// Crossfade duration seconds (used by MANUAL_CROSSFADE).
    pub crossfade_duration_secs: f32,
    pub data_saver: bool,
    /// Output device name (`None` = system default).
    pub output_device: Option<String>,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            audio_quality: AudioQuality::Auto,
            audio_normalization: false,
            skip_silence: false,
            player_volume: 1.0,
            shuffle_mode: false,
            repeat_mode: RepeatMode::All,
            stream_failure_timeout: 0,
            auto_skip_next_on_error: true,
            transition_mode: TransitionMode::Crossmix,
            crossfade_duration_secs: 5.0,
            data_saver: false,
            output_device: None,
        }
    }
}

/// Audio Lab (spec §4.2) — MVP subset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioLab {
    pub eq: crate::eq::EqSettings,
    /// Loudness processor.
    pub loudness_enabled: bool,
    /// LoudnessAudioProcessor boost, dB (clamp 0–12, default 4).
    pub loudness_boost_db: f32,
    /// LoudnessAudioProcessor compression 0–1 (default 0.3).
    pub loudness_compression: f32,
    /// Exponential volume curve: gain = limit * v^exponent (Android `VolumeCurve`).
    pub volume_curve_enabled: bool,
    pub volume_curve_exponent: f32,
    pub volume_curve_limit: f32,
    /// ReplayGain Track mode, preamp dB (default 3).
    pub normalize_replay_gain: bool,
    pub normalize_preamp_db: f32,
    /// Playback speed factor 0.1–8.0.
    pub speed: f32,
    pub preserve_pitch: bool,
    /// Pitch shift in semitones (−12..+12), applied with speed (spec §4.7).
    pub pitch_semitones: f32,
    pub reverb: ReverbSettings,
    pub spatial: SpatialSettings,
    /// User-saved EQ presets (Android `presets` key).
    pub user_presets: Vec<crate::eq::UserEqPreset>,
}

impl Default for AudioLab {
    fn default() -> Self {
        Self {
            eq: crate::eq::EqSettings::default(),
            loudness_enabled: false,
            loudness_boost_db: 4.0,
            loudness_compression: 0.3,
            volume_curve_enabled: false,
            volume_curve_exponent: 2.0,
            volume_curve_limit: 1.0,
            normalize_replay_gain: true,
            normalize_preamp_db: 3.0,
            speed: 1.0,
            preserve_pitch: false,
            pitch_semitones: 0.0,
            reverb: ReverbSettings::default(),
            spatial: SpatialSettings::default(),
            user_presets: Vec::new(),
        }
    }
}

/// Freeverb-style reverb (spec §4.2 `ReverbSettings`, presets §4.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReverbSettings {
    pub enabled: bool,
    pub mix: f32,
    pub room_size: f32,
    pub damping: f32,
    pub wet: f32,
    pub dry: f32,
    pub width: f32,
    pub pre_delay_ms: f32,
    pub low_cut_hz: f32,
    pub preset_id: String,
}

impl Default for ReverbSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mix: 0.5,
            room_size: 0.55,
            damping: 0.45,
            wet: 0.6,
            dry: 1.0,
            width: 1.0,
            pre_delay_ms: 12.0,
            low_cut_hz: 180.0,
            preset_id: "room".into(),
        }
    }
}

/// Reverb presets (spec §4.6):
/// (id, name, mix, room_size, damping, wet, width, pre_delay_ms, low_cut_hz).
#[rustfmt::skip]
pub const REVERB_PRESETS: &[(&str, &str, f32, f32, f32, f32, f32, f32, f32)] = &[
    ("room",      "Room",      0.5,  0.55, 0.45, 0.6,  1.0, 12.0, 180.0),
    ("studio",    "Studio",    0.4,  0.3,  0.65, 0.45, 0.8, 5.0,  250.0),
    ("bathroom",  "Bathroom",  0.5,  0.42, 0.05, 0.7,  0.6, 2.0,  120.0),
    ("plate",     "Plate",     0.45, 0.65, 0.1,  0.55, 1.0, 0.0,  300.0),
    ("hall",      "Hall",      0.5,  0.78, 0.35, 0.6,  1.0, 25.0, 150.0),
    ("cathedral", "Cathedral", 0.55, 0.95, 0.2,  0.7,  1.0, 40.0, 120.0),
    ("stadium",   "Stadium",   0.6,  0.9,  0.5,  0.65, 1.0, 60.0, 200.0),
    ("dreamy",    "Dreamy",    0.7,  0.93, 0.6,  0.85, 1.0, 30.0, 300.0),
];

impl ReverbSettings {
    /// Settings for a preset id (keeps `enabled`).
    pub fn from_preset(id: &str, enabled: bool) -> Option<Self> {
        REVERB_PRESETS.iter().find(|p| p.0 == id).map(|p| Self {
            enabled,
            mix: p.2,
            room_size: p.3,
            damping: p.4,
            wet: p.5,
            dry: 1.0,
            width: p.6,
            pre_delay_ms: p.7,
            low_cut_hz: p.8,
            preset_id: p.0.into(),
        })
    }
}

/// Spatial processor (spec §4.2 `SpatialSettings`, constants §4.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpatialSettings {
    /// Headphone crossfeed.
    pub crossfeed_enabled: bool,
    pub crossfeed_cutoff_hz: f32,
    pub crossfeed_feed_db: f32,
    /// Virtual speakers (spherical-head model).
    pub virtual_speakers: bool,
    pub speaker_angle_deg: f32,
    pub surround: bool,
    /// Sum to mono.
    pub mono: bool,
    /// Stereo width 0 (mono) – 2 (extra wide); 1 = unchanged.
    pub width: f32,
}

impl Default for SpatialSettings {
    fn default() -> Self {
        Self {
            crossfeed_enabled: false,
            crossfeed_cutoff_hz: 700.0,
            crossfeed_feed_db: 4.5,
            virtual_speakers: false,
            speaker_angle_deg: 30.0,
            surround: false,
            mono: false,
            width: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LyricsSettings {
    pub show: bool,
    /// Provider order as serialized ids (see mochy-lyrics ProviderId).
    pub provider_order: Vec<String>,
    /// Per-song/line offset feedback, ms.
    pub text_size: f32,
    pub line_spacing: f32,
}

impl Default for LyricsSettings {
    fn default() -> Self {
        Self { show: true, provider_order: Vec::new(), text_size: 1.0, line_spacing: 1.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Misc {
    /// YTM content country used for browse responses.
    pub content_country: String,
    /// Keep queued track list across restarts.
    pub persistent_queue: bool,
    /// Window size/position memory.
    pub window_width: f32,
    pub window_height: f32,
    pub window_maximized: bool,
}

impl Default for Misc {
    fn default() -> Self {
        Self {
            content_country: String::new(),
            persistent_queue: true,
            window_width: 0.0,
            window_height: 0.0,
            window_maximized: false,
        }
    }
}

/// Root settings object (JSON file `settings.json` in the data dir).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppSettings {
    pub appearance: Appearance,
    pub playback: Playback,
    pub audio_lab: AudioLab,
    pub lyrics: LyricsSettings,
    pub misc: Misc,
}
