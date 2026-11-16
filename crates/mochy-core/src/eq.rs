//! Equalizer definitions — spec §4.3/§4.4 (Audio Lab).
//! Parametric EQ, MAX_BANDS = 20, RBJ biquad filter types, ±12 dB UI gain.

use serde::{Deserialize, Serialize};

pub const MAX_BANDS: usize = 20;
/// Most filters a headphone correction (AutoEq) may add on top of the user's curve.
pub const MAX_CORRECTION_BANDS: usize = 16;
/// Biquad sections the engine can run at once: the user's curve plus the correction.
pub const MAX_DSP_BANDS: usize = MAX_BANDS + MAX_CORRECTION_BANDS;
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

/// Shape of a headphone in the AutoEq database (from its rig folder).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum HeadphoneForm {
    #[default]
    OverEar,
    InEar,
    Earbud,
}

/// How a headphone correction is fitted (Android `AutoEqTuning`): AutoEq's own
/// 10-band fit unless something here is changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoEqTuning {
    pub bands: u32,
    pub max_hz: u32,
    /// A low shelf at 105 Hz added to the target, dB.
    pub bass_db: f32,
    /// dB per octave around 1 kHz, positive = brighter.
    pub tilt_db_per_octave: f32,
}

impl Default for AutoEqTuning {
    fn default() -> Self {
        Self { bands: 10, max_hz: 16_000, bass_db: 0.0, tilt_db_per_octave: 0.0 }
    }
}

impl AutoEqTuning {
    pub const BAND_CHOICES: [u32; 4] = [5, 8, 10, 15];
    pub const MAX_HZ_CHOICES: [u32; 3] = [10_000, 16_000, 20_000];

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A headphone correction picked from the AutoEq database, with the filters it
/// came to (Android `AutoEqCorrection`). It plays under the user's own curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutoEqCorrection {
    pub model: String,
    pub source: String,
    #[serde(default)]
    pub rig: Option<String>,
    /// The result's folder under AutoEq's `results/`, decoded.
    pub path: String,
    #[serde(default)]
    pub form: HeadphoneForm,
    #[serde(default)]
    pub tuning: AutoEqTuning,
    pub filters: Vec<EqNode>,
    pub preamp_db: f32,
}

fn default_true() -> bool {
    true
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
    /// Headphone correction from the AutoEq database, heard under `nodes`.
    #[serde(default)]
    pub correction: Option<AutoEqCorrection>,
    #[serde(default = "default_true")]
    pub correction_enabled: bool,
}

impl Default for EqSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            nodes: Vec::new(),
            preamp_db: 0.0,
            auto_preamp: true,
            preset_id: None,
            correction: None,
            correction_enabled: true,
        }
    }
}

impl EqSettings {
    /// The correction, when it is switched on.
    pub fn active_correction(&self) -> Option<&AutoEqCorrection> {
        self.correction.as_ref().filter(|_| self.correction_enabled)
    }

    /// The user's curve alone (no headphone correction).
    pub fn without_correction(&self) -> EqSettings {
        EqSettings { correction: None, ..self.clone() }
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
        ..Default::default()
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
        ..Default::default()
    }
}

/// User-saved EQ preset (Android `EqPreset{id, name, nodes, preamp}`; ids `user_<millis>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserEqPreset {
    pub id: String,
    pub name: String,
    pub nodes: Vec<EqNode>,
    pub preamp_db: f32,
}

// ---------------------------------------------------------------------------
// Shared EQ math (used by the audio engine AND the UI response curve, so the
// plotted curve is exactly what is heard). RBJ cookbook, spec §4.3/§4.5.
// ---------------------------------------------------------------------------

/// Normalized biquad coefficients (a0 = 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiquadCoeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl BiquadCoeffs {
    pub const IDENTITY: BiquadCoeffs = BiquadCoeffs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    /// RBJ design. Frequency is clamped below Nyquist, q to >= 0.05.
    pub fn design(filter_type: FilterType, freq_hz: f32, gain_db: f32, q: f32, sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0) as f64;
        let f = (freq_hz as f64).clamp(1.0, sr * 0.499);
        let q = (q as f64).max(0.05);
        let a = 10f64.powf(gain_db as f64 / 40.0);
        let w0 = 2.0 * std::f64::consts::PI * f / sr;
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q);
        let (b0, b1, b2, a0, a1, a2) = match filter_type {
            FilterType::Peaking => (1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a),
            FilterType::LowShelf => {
                let k = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * c + k),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * c),
                    a * ((a + 1.0) - (a - 1.0) * c - k),
                    (a + 1.0) + (a - 1.0) * c + k,
                    -2.0 * ((a - 1.0) + (a + 1.0) * c),
                    (a + 1.0) + (a - 1.0) * c - k,
                )
            }
            FilterType::HighShelf => {
                let k = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * c + k),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                    a * ((a + 1.0) + (a - 1.0) * c - k),
                    (a + 1.0) - (a - 1.0) * c + k,
                    2.0 * ((a - 1.0) - (a + 1.0) * c),
                    (a + 1.0) - (a - 1.0) * c - k,
                )
            }
            FilterType::LowPass => ((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha),
            FilterType::HighPass => ((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha),
        };
        BiquadCoeffs { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
    }

    /// Magnitude response in dB at `freq_hz`.
    pub fn magnitude_db(&self, freq_hz: f32, sample_rate: f32) -> f32 {
        let w = 2.0 * std::f64::consts::PI * freq_hz as f64 / sample_rate.max(1.0) as f64;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        let num = nr * nr + ni * ni;
        let den = (dr * dr + di * di).max(1e-30);
        (10.0 * (num / den).max(1e-30).log10()) as f32
    }
}

/// Enabled, audible nodes of `eq` (at most `MAX_BANDS`).
/// Followed by the headphone correction's filters while it is on (at most
/// `MAX_CORRECTION_BANDS`), so the engine and the plotted curve both hear it.
pub fn active_nodes(eq: &EqSettings) -> impl Iterator<Item = &EqNode> {
    let correction = eq.active_correction().map(|c| c.filters.as_slice()).unwrap_or(&[]);
    eq.nodes
        .iter()
        .filter(|n| n.enabled)
        .take(MAX_BANDS)
        .chain(correction.iter().filter(|n| n.enabled).take(MAX_CORRECTION_BANDS))
}

/// Combined filter response in dB at `freq_hz`, EXCLUDING preamp.
pub fn filters_response_db(eq: &EqSettings, freq_hz: f32, sample_rate: f32) -> f32 {
    active_nodes(eq)
        .map(|n| {
            BiquadCoeffs::design(n.filter_type, n.frequency_hz, n.gain_db, n.q, sample_rate)
                .magnitude_db(freq_hz, sample_rate)
        })
        .sum()
}

/// Limiter boost room (spec §4.5): `4.0 + 3.0 * max(0, log2(f/1000))` dB.
pub fn boost_room_db(freq_hz: f32) -> f32 {
    4.0 + 3.0 * (freq_hz / 1000.0).log2().max(0.0)
}

/// `n` log-spaced frequencies over 20 Hz – 20 kHz (the plotted range).
pub fn log_freqs(n: usize) -> Vec<f32> {
    let n = n.max(2);
    (0..n).map(|i| 20.0 * 1000f32.powf(i as f32 / (n - 1) as f32)).collect()
}

/// Auto-preamp headroom in dB (<= 0): `-max(response(f) - boostRoom(f))`
/// over the plotted range, rounded to 0.1 dB (spec §4.5).
pub fn auto_preamp_db(eq: &EqSettings) -> f32 {
    let sr = 48_000.0;
    let over = log_freqs(256)
        .into_iter()
        .map(|f| filters_response_db(eq, f, sr) - boost_room_db(f))
        .fold(0.0f32, f32::max);
    let r = -((over * 10.0).round() / 10.0);
    if r == 0.0 { 0.0 } else { r }
}

/// Total preamp applied by the engine (Android `AudioLabStore.design`):
/// `preamp + auto headroom` with auto preamp on, else `preamp + the
/// correction's own preamp` while a headphone correction plays.
pub fn effective_preamp_db(eq: &EqSettings) -> f32 {
    let headroom = if eq.auto_preamp {
        auto_preamp_db(eq)
    } else {
        eq.active_correction().map_or(0.0, |c| c.preamp_db)
    };
    eq.preamp_db + headroom
}

/// What the listener hears (dB) at `freq_hz`: filters + effective preamp.
/// Returns 0 when the EQ is disabled.
pub fn total_response_db(eq: &EqSettings, freq_hz: f32, sample_rate: f32) -> f32 {
    if !eq.enabled {
        return 0.0;
    }
    filters_response_db(eq, freq_hz, sample_rate) + effective_preamp_db(eq)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaking_gain_at_center() {
        let c = BiquadCoeffs::design(FilterType::Peaking, 1000.0, 6.0, 1.41, 48_000.0);
        assert!((c.magnitude_db(1000.0, 48_000.0) - 6.0).abs() < 0.05);
        assert!(c.magnitude_db(20.0, 48_000.0).abs() < 0.2);
    }

    #[test]
    fn flat_preset_is_flat_with_no_headroom() {
        let eq = settings_from_preset(preset_by_id("flat").unwrap());
        assert!(total_response_db(&eq, 440.0, 48_000.0).abs() < 1e-3);
        assert_eq!(auto_preamp_db(&eq), 0.0);
    }

    #[test]
    fn auto_preamp_negative_for_big_bass_boost() {
        let eq = settings_from_gains([12.0, 12.0, 12.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert!(auto_preamp_db(&eq) < -5.0);
    }

    fn correction() -> AutoEqCorrection {
        AutoEqCorrection {
            model: "Test HP".into(),
            source: "oratory1990".into(),
            rig: None,
            path: "oratory1990/over-ear/Test HP".into(),
            form: HeadphoneForm::OverEar,
            tuning: AutoEqTuning::default(),
            filters: vec![
                EqNode { filter_type: FilterType::LowShelf, ..EqNode::peaking(105.0, 6.0) },
                EqNode::peaking(3000.0, -4.0),
            ],
            preamp_db: -6.4,
        }
    }

    #[test]
    fn correction_plays_under_the_curve() {
        let mut eq = settings_from_gains([0.0; 10]);
        eq.auto_preamp = false;
        assert_eq!(active_nodes(&eq).count(), 10);
        eq.correction = Some(correction());
        assert_eq!(active_nodes(&eq).count(), 12);
        // The correction is heard, with its own preamp while auto preamp is off.
        assert!((filters_response_db(&eq, 3000.0, 48_000.0) + 4.0).abs() < 0.3);
        assert!((effective_preamp_db(&eq) + 6.4).abs() < 1e-4);
        eq.correction_enabled = false;
        assert_eq!(active_nodes(&eq).count(), 10);
        assert_eq!(effective_preamp_db(&eq), 0.0);
        // Auto preamp covers the correction's bass shelf too.
        eq.correction_enabled = true;
        eq.auto_preamp = true;
        assert!(effective_preamp_db(&eq) < -1.0);
        assert_eq!(eq.without_correction().correction, None);
    }

    #[test]
    fn old_settings_without_correction_still_load() {
        let json = r#"{"enabled":true,"nodes":[],"preamp_db":-2.0,"auto_preamp":false,"preset_id":null}"#;
        let eq: EqSettings = serde_json::from_str(json).unwrap();
        assert!(eq.correction.is_none());
        assert!(eq.correction_enabled);
        let mut with = eq.clone();
        with.correction = Some(correction());
        let back: EqSettings = serde_json::from_str(&serde_json::to_string(&with).unwrap()).unwrap();
        assert_eq!(back, with);
    }

    #[test]
    fn presets_have_ten_bands() {
        assert_eq!(BUILT_IN_PRESETS.len(), 21);
        for p in BUILT_IN_PRESETS {
            assert_eq!(settings_from_preset(p).nodes.len(), 10);
        }
    }
}
