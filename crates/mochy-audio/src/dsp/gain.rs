//! Gain stages: VolumeCurve and ReplayGain (spec §4.6, `VolumeCurve.kt`,
//! `ReplayGain.kt`).

/// VolumeCurve: `gain = limit · v^exponent` (exponent clamped 1–5, limit
/// clamped 0.05–1, v clamped 0–1). Callers use plain `v` when disabled.
pub fn volume_curve_gain(linear: f32, exponent: f32, limit: f32) -> f32 {
    let v = linear.clamp(0.0, 1.0);
    limit.clamp(0.05, 1.0) * v.powf(exponent.clamp(1.0, 5.0))
}

/// ReplayGain factor: `10^((gain+preamp)/20)`, capped by `1/peak`, clamped
/// to [0.03, 1] — it can only bring a song down and never takes a peak past
/// full scale.
pub fn replay_gain_factor(gain_db: f32, peak: Option<f32>, preamp_db: f32) -> f32 {
    let mut factor = 10f32.powf((gain_db + preamp_db) / 20.0);
    if let Some(p) = peak.filter(|p| *p > 0.0) {
        factor = factor.min(1.0 / p);
    }
    factor.clamp(0.03, 1.0)
}

/// ReplayGain tag number parser (`[-+]?\d+(?:\.\d+)?`): "-8.89 dB",
/// "1.000000", "+2.1 db".
pub fn parse_replay_gain_number(text: &str) -> Option<f32> {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let start = i;
        let mut j = i;
        if b[j] == b'-' || b[j] == b'+' {
            j += 1;
        }
        let digits_start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > digits_start {
            if j + 1 < b.len() && b[j] == b'.' && b[j + 1].is_ascii_digit() {
                j += 1;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
            }
            return text[start..j].parse().ok();
        }
        i += 1;
    }
    None
}
