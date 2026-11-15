//! Equalizer definitions — spec §4.3/§4.4 (Audio Lab).
//! Parametric EQ, MAX_BANDS = 20, RBJ biquad filter types, ±12 dB UI gain.

use serde::{Deserialize, Serialize};

pub const MAX_BANDS: usize = 20;
/// UI gain clamp in dB.
pub const MAX_GAIN_DB: f32 = 12.0;

/// RBJ cookbook filter types (Android `FilterType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FilterType {
    /// PK — peaking.
    #[default]
    Peaking,
    /// LSC — low shelf.
    LowShelf,
    /// HSC — high shelf.
    HighShelf,
    /// LPQ — low pass.
    LowPass,
    /// HPQ — high pass.
    HighPass,
}

/// One parametric band (Android `ParametricEQBand`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EqNode {
    pub frequency_hz: f32,
    pub gain_db: f32,
    /// Default 1.41 (`OctaveQ`).
    pub q: f32,
    pub filter_type: FilterType,
    pub enabled: bool,
}

impl EqNode {
    pub fn peaking(frequency_hz: f32, gain_db: f32) -> Self {
        Self { frequency_hz, gain_db, q: 1.41, filter_type: FilterType::Peaking, enabled: true }
    }
}

/// Android `EqSettings` from the `audio_lab` store (spec §4.2/§4.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EqSettings {
    pub enabled: bool,
    pub nodes: Vec<EqNode>,
    pub preamp_db: f32,
    /// Auto preamp headroom from the designed response (spec §4.5).
    pub auto_preamp: bool,
    pub preset_id: Option<String>,
}

impl Default for EqSettings {
    fn default() -> Self {
        Self { enabled: false, nodes: Vec::new(), preamp_db: 0.0, auto_preamp: true, preset_id: None }
    }
}

/// The 10 fixed band frequencies of the built-in presets (spec §4.4).
pub const BAND_FREQS_HZ: [f32; 10] =
    [31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];

/// Built-in 10-band preset (Android `EqPresets`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqPreset {
    pub id: &'static str,
    pub name: &'static str,
    /// Gains in dB, one per `BAND_FREQS_HZ` entry.
    pub gains: [f32; 10],
}

#[rustfmt::skip]
pub const BUILT_IN_PRESETS: &[EqPreset] = &[
    EqPreset { id: "flat",           name: "Flat",            gains: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] },
    EqPreset { id: "acoustic",       name: "Acoustic",        gains: [4.5, 4.5, 3.5, 1.0, 1.5, 1.5, 3.0, 3.5, 3.0, 1.5] },
    EqPreset { id: "bass_boost",     name: "Bass Booster",    gains: [5.5, 4.5, 3.5, 2.5, 1.2, 0.0, 0.0, 0.0, 0.0, 0.0] },
    EqPreset { id: "bass_cut",       name: "Bass Reducer",    gains: [-5.5, -4.5, -3.5, -2.5, -1.2, 0.0, 0.0, 0.0, 0.0, 0.0] },
    EqPreset { id: "classical",      name: "Classical",       gains: [4.5, 3.5, 3.0, 2.5, -1.5, -1.5, 0.0, 2.0, 3.0, 3.5] },
    EqPreset { id: "dance",          name: "Dance",           gains: [3.5, 6.5, 5.0, 0.0, 2.0, 3.5, 5.0, 4.5, 3.5, 0.0] },
    EqPreset { id: "deep",           name: "Deep",            gains: [5.0, 3.5, 1.5, 1.0, 3.0, 2.5, 1.5, -2.0, -3.5, -4.5] },
    EqPreset { id: "electronic",     name: "Electronic",      gains: [4.5, 4.0, 1.5, 0.0, -2.0, 2.0, 1.0, 1.5, 4.5, 5.0] },
    EqPreset { id: "hiphop",         name: "Hip-Hop",         gains: [5.0, 4.0, 1.5, 3.0, -1.0, -1.0, 1.5, -0.5, 2.0, 3.0] },
    EqPreset { id: "jazz",           name: "Jazz",            gains: [4.0, 3.0, 1.5, 2.0, -1.5, -1.5, 0.0, 1.5, 3.0, 3.5] },
    EqPreset { id: "loudness",       name: "Loudness",        gains: [6.0, 4.0, 0.0, 0.0, -2.0, 0.0, -1.0, -5.0, 5.0, 1.0] },
    EqPreset { id: "lounge",         name: "Lounge",          gains: [-3.0, -1.5, -0.5, 1.5, 4.0, 2.5, 0.0, -1.5, 2.0, 1.0] },
    EqPreset { id: "piano",          name: "Piano",           gains: [3.0, 2.0, 0.0, 2.5, 3.0, 1.5, 3.5, 4.5, 3.0, 3.5] },
    EqPreset { id: "pop",            name: "Pop",             gains: [-1.5, -1.0, 0.0, 2.0, 4.0, 4.0, 2.0, 0.0, -1.0, -1.5] },
    EqPreset { id: "rnb",            name: "R&B",             gains: [2.5, 7.0, 5.5, 1.5, -2.5, -1.5, 2.5, 3.0, 3.0, 3.5] },
    EqPreset { id: "rock",           name: "Rock",            gains: [5.0, 4.0, 3.0, 1.5, -0.5, -1.0, 0.5, 2.5, 3.5, 4.5] },
    EqPreset { id: "small_speakers", name: "Small Speakers",  gains: [5.5, 4.0, 3.5, 2.5, 1.0, 0.0, -1.0, -2.0, -3.5, -4.0] },
    EqPreset { id: "spoken",         name: "Spoken Word",     gains: [-3.5, -0.5, 0.0, 0.5, 3.5, 4.5, 5.0, 4.0, 2.5, 0.0] },
    EqPreset { id: "treble_boost",   name: "Treble Booster",  gains: [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.5, 3.5, 4.5, 5.5] },
    EqPreset { id: "treble_cut",     name: "Treble Reducer",  gains: [0.0, 0.0, 0.0, 0.0, 0.0, -1.0, -2.5, -3.5, -4.5, -5.5] },
    EqPreset { id: "vocal",          name: "Vocal Booster",   gains: [-1.5, -3.0, -3.0, 1.5, 3.5, 3.5, 3.0, 1.5, 0.0, -1.5] },
];

/// Look up a built-in preset by id.
pub fn preset_by_id(id: &str) -> Option<&'static EqPreset> {
    BUILT_IN_PRESETS.iter().find(|p| p.id == id)
}

/// Convert a built-in preset into `EqSettings` nodes (10 PK bands, Q = 1.41).
pub fn settings_from_preset(p: &EqPreset) -> EqSettings {
    EqSettings {
        enabled: true,
        nodes: BAND_FREQS_HZ
            .iter()
            .zip(p.gains)
            .map(|(&f, g)| EqNode::peaking(f, g))
            .collect(),
        preamp_db: 0.0,
        auto_preamp: true,
        preset_id: Some(p.id.to_string()),
    }
}

/// Convert arbitrary (non-preset) 10-band slider gains into settings.
pub fn settings_from_gains(gains: [f32; 10]) -> EqSettings {
    EqSettings {
        enabled: true,
        nodes: BAND_FREQS_HZ
            .iter()
            .zip(gains)
            .map(|(&f, g)| EqNode::peaking(f, g))
            .collect(),
        preamp_db: 0.0,
        auto_preamp: true,
        preset_id: None,
    }
}
