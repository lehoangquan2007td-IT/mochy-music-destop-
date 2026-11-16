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

/// UI language (Settings → Appearance). UI strings are English by default;
/// the Vietnamese table lives in the app (`ui/i18n.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
    Vi,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// UI language (English / Tiếng Việt).
    pub language: Language,
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
            language: Language::default(),
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
    /// How much of the crossfeed / virtual speakers is mixed in, 0–1 (Android `amount`).
    pub amount: f32,
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
            amount: 1.0,
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
    /// Sync compensation applied to every song, on top of the per-song offset.
    /// Negative delays the lyrics — for output devices whose latency the OS
    /// does not report (Bluetooth, TV, DSP-heavy drivers).
    pub global_offset_ms: i32,
    /// Translation, romanization and display options (lyrics extras).
    #[serde(default)]
    pub extras: LyricsExtras,
}

impl Default for LyricsSettings {
    fn default() -> Self {
        Self {
            show: true,
            provider_order: Vec::new(),
            text_size: 1.0,
            line_spacing: 1.0,
            global_offset_ms: 0,
            extras: LyricsExtras::default(),
        }
    }
}

/// Horizontal placement of lyric lines (Android `LyricsPosition`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LyricsTextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Weight of the lyric lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LyricsFontWeight {
    Regular,
    Medium,
    Semibold,
    #[default]
    Bold,
}

/// Lyrics extras: readings, translation and display options (Android `LyricsRomanize*Key`,
/// `Translate*Key`, `LyricsTextPositionKey`, blur keys…).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LyricsExtras {
    /// Show a romanized reading under lines in Japanese, Korean, Chinese, Cyrillic, Hindi…
    pub romanize: bool,
    /// Languages switched off individually (mochy-lyrics `RomanizeLang::id`, e.g. `"zh"`).
    pub romanize_disabled: Vec<String>,
    /// Show the romanization as the line itself, with the original underneath.
    pub romanize_as_main: bool,
    /// Translate every song automatically (Android "translate the whole session").
    pub translate_auto: bool,
    /// Target language (Google Translate code, e.g. "en", "vi").
    pub translate_language: String,
    /// Source language, "auto" to detect.
    pub translate_source: String,
    pub align: LyricsTextAlign,
    pub font_weight: LyricsFontWeight,
    /// Soft blur on lines far from the active one.
    pub blur_inactive: bool,
    /// How much inactive lines are dimmed, 0 (not at all) – 1 (strongly).
    pub dim_inactive: f32,
    /// Size of the translation / romanization line relative to the lyric line.
    pub sub_line_scale: f32,
    /// Click a line to seek to it.
    pub click_to_seek: bool,
}

impl Default for LyricsExtras {
    fn default() -> Self {
        Self {
            romanize: true,
            romanize_disabled: Vec::new(),
            romanize_as_main: false,
            translate_auto: false,
            translate_language: "en".into(),
            translate_source: "auto".into(),
            align: LyricsTextAlign::Left,
            font_weight: LyricsFontWeight::Bold,
            blur_inactive: true,
            dim_inactive: 0.5,
            sub_line_scale: 0.55,
            click_to_seek: true,
        }
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
    /// Downloads / offline / stream cache (Settings → Downloads).
    #[serde(default)]
    pub downloads: crate::download_settings::DownloadSettings,
    /// Now Playing styles and look (desktop port of the player customisation).
    pub player: crate::player_look::PlayerLook,
    /// Discord presence + scrobbling (see `crate::integrations`).
    #[serde(default)]
    pub integrations: crate::integrations::IntegrationSettings,
    /// Transitions, transport fades, skip silence, sleep timer (the mode,
    /// crossfade length and skip-silence switch stay in `playback`).
    #[serde(default)]
    pub transitions: TransitionSettings,
}

/// `CrossmixLength` (scale 0.6 / 1 / 1.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CrossmixLength {
    Short,
    #[default]
    Medium,
    Long,
}

impl CrossmixLength {
    pub fn scale(self) -> f32 {
        match self {
            CrossmixLength::Short => 0.6,
            CrossmixLength::Medium => 1.0,
            CrossmixLength::Long => 1.5,
        }
    }
}

/// `CrossmixTempoMatch` (max speed-up 0.6 % / 4 % / 10 %).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CrossmixTempoMatch {
    Off,
    Gentle,
    #[default]
    Full,
}

impl CrossmixTempoMatch {
    pub fn max_speed_up(self) -> f32 {
        match self {
            CrossmixTempoMatch::Off => 0.006,
            CrossmixTempoMatch::Gentle => 0.04,
            CrossmixTempoMatch::Full => 0.10,
        }
    }
}

/// Transition / fade settings (Android `transportFade*`, `crossfadeGapless`,
/// `crossmix*`, `skipSilenceInstant`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TransitionSettings {
    /// Fades on play / pause / skip / seek.
    pub transport_fade: bool,
    /// Transport fade length, seconds (0.5–10, default 2).
    pub transport_fade_secs: f32,
    /// No crossfade between consecutive songs of one album (gapless instead).
    pub gapless_albums: bool,
    pub crossmix_length: CrossmixLength,
    pub crossmix_tempo_match: CrossmixTempoMatch,
    /// One style for every mix (`None` = the planner picks); a style id.
    pub crossmix_style: Option<String>,
    pub crossmix_stunts: bool,
    pub crossmix_always_mix: bool,
    /// Skip silence: jump through long silences with 15 s hops.
    pub skip_silence_instant: bool,
    /// Sleep timer fade-out, seconds.
    pub sleep_fade_secs: f32,
    /// Last minutes picked in the sleep timer.
    pub sleep_last_minutes: u32,
}

impl Default for TransitionSettings {
    fn default() -> Self {
        Self {
            transport_fade: true,
            transport_fade_secs: 2.0,
            gapless_albums: true,
            crossmix_length: CrossmixLength::Medium,
            crossmix_tempo_match: CrossmixTempoMatch::Full,
            crossmix_style: None,
            crossmix_stunts: true,
            crossmix_always_mix: true,
            skip_silence_instant: false,
            sleep_fade_secs: 10.0,
            sleep_last_minutes: 30,
        }
    }
}
