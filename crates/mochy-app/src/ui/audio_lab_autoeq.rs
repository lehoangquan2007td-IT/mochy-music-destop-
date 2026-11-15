//! AutoEq `ParametricEQ.txt` import/export (spec §4.5, Android
//! `ParametricEQParser.kt`). Pure functions — no UI.
//!
//! Format (Equalizer APO dialect):
//! ```text
//! Preamp: -6.2 dB
//! Filter 1: ON PK Fc 105 Hz Gain 3.4 dB Q 0.70
//! Filter 2: ON LSC Fc 105 Hz Gain 5.0 dB Q 0.71
//! ```

use mochy_core::eq::{EqNode, EqSettings, FilterType, MAX_BANDS, MAX_GAIN_DB, effective_preamp_db};

/// Frequency range accepted by the editor (Hz).
pub const FREQ_RANGE: (f32, f32) = (20.0, 20_000.0);
/// Q range accepted by the editor.
pub const Q_RANGE: (f32, f32) = (0.1, 10.0);

/// Result of [`parse_autoeq`].
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedEq {
    pub preamp_db: f32,
    pub nodes: Vec<EqNode>,
    /// Human readable notes (clamped values, skipped filters…).
    pub warnings: Vec<String>,
}

fn parse_num(tok: &str) -> Option<f32> {
    let t = tok.trim().replace(',', ".");
    let t = t.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    t.parse::<f32>().ok().filter(|v| v.is_finite())
}

fn filter_type(code: &str) -> Option<FilterType> {
    Some(match code.to_ascii_uppercase().as_str() {
        "PK" | "PEQ" | "PEAK" | "PEAKING" => FilterType::Peaking,
        "LSC" | "LS" | "LSQ" | "LOWSHELF" => FilterType::LowShelf,
        "HSC" | "HS" | "HSQ" | "HIGHSHELF" => FilterType::HighShelf,
        "LPQ" | "LP" | "LOWPASS" => FilterType::LowPass,
        "HPQ" | "HP" | "HIGHPASS" => FilterType::HighPass,
        _ => return None,
    })
}

/// AutoEq/APO short code for a filter type.
pub fn type_code(t: FilterType) -> &'static str {
    match t {
        FilterType::Peaking => "PK",
        FilterType::LowShelf => "LSC",
        FilterType::HighShelf => "HSC",
        FilterType::LowPass => "LPQ",
        FilterType::HighPass => "HPQ",
    }
}

fn default_q(t: FilterType) -> f32 {
    match t {
        FilterType::Peaking => 1.41,
        FilterType::LowShelf | FilterType::HighShelf => 0.71,
        FilterType::LowPass | FilterType::HighPass => 0.707,
    }
}

/// Parse an AutoEq `ParametricEQ.txt`. Values are clamped to the editor's
/// ranges (freq 20–20 kHz, gain ±12 dB, Q 0.1–10, preamp ±12 dB, at most
/// `MAX_BANDS` filters) and every clamp is reported in `warnings`.
pub fn parse_autoeq(text: &str) -> Result<ParsedEq, String> {
    let mut preamp: Option<f32> = None;
    let mut nodes = Vec::new();
    let mut warnings = Vec::new();
    let mut skipped = 0usize;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        let Some((head, rest)) = line.split_once(':') else { continue };
        let head = head.trim().to_ascii_lowercase();
        if head == "preamp" {
            let v = rest.split_whitespace().next().and_then(parse_num);
            match v {
                Some(v) => preamp = Some(v),
                None => return Err(format!("Bad preamp line: \"{line}\"")),
            }
            continue;
        }
        if !head.starts_with("filter") {
            continue;
        }

        let toks: Vec<&str> = rest.split_whitespace().collect();
        let mut enabled = true;
        let mut ftype: Option<FilterType> = None;
        let (mut fc, mut gain, mut q) = (None, None, None);
        let mut i = 0;
        while i < toks.len() {
            let t = toks[i];
            let next = toks.get(i + 1).and_then(|n| parse_num(n));
            match t.to_ascii_lowercase().as_str() {
                "on" => enabled = true,
                "off" => enabled = false,
                "fc" | "f" | "freq" => {
                    fc = next;
                    i += 1;
                }
                "gain" | "g" => {
                    gain = next;
                    i += 1;
                }
                "q" => {
                    q = next;
                    i += 1;
                }
                "hz" | "db" => {}
                other => {
                    if ftype.is_none()
                        && let Some(ft) = filter_type(other)
                    {
                        ftype = Some(ft);
                    }
                }
            }
            i += 1;
        }

        let (Some(ft), Some(f)) = (ftype, fc) else {
            skipped += 1;
            continue;
        };
        if nodes.len() >= MAX_BANDS {
            skipped += 1;
            continue;
        }
        let mut node = EqNode {
            frequency_hz: f,
            gain_db: gain.unwrap_or(0.0),
            q: q.unwrap_or_else(|| default_q(ft)),
            filter_type: ft,
            enabled,
        };
        let n = nodes.len() + 1;
        if !(FREQ_RANGE.0..=FREQ_RANGE.1).contains(&node.frequency_hz) {
            warnings.push(format!("Filter {n}: frequency {:.0} Hz clamped", node.frequency_hz));
            node.frequency_hz = node.frequency_hz.clamp(FREQ_RANGE.0, FREQ_RANGE.1);
        }
        if node.gain_db.abs() > MAX_GAIN_DB {
            warnings.push(format!("Filter {n}: gain {:.1} dB clamped to ±12", node.gain_db));
            node.gain_db = node.gain_db.clamp(-MAX_GAIN_DB, MAX_GAIN_DB);
        }
        if !(Q_RANGE.0..=Q_RANGE.1).contains(&node.q) {
            warnings.push(format!("Filter {n}: Q {:.2} clamped", node.q));
            node.q = node.q.clamp(Q_RANGE.0, Q_RANGE.1);
        }
        nodes.push(node);
    }

    if skipped > 0 {
        warnings.push(format!("{skipped} unsupported or extra filter line(s) skipped"));
    }
    if nodes.is_empty() && preamp.is_none() {
        return Err("No \"Preamp:\" or \"Filter N:\" lines found".into());
    }
    let mut preamp_db = preamp.unwrap_or(0.0);
    if preamp_db.abs() > MAX_GAIN_DB {
        warnings.push(format!("Preamp {preamp_db:.1} dB clamped to ±12"));
        preamp_db = preamp_db.clamp(-MAX_GAIN_DB, MAX_GAIN_DB);
    }
    Ok(ParsedEq { preamp_db, nodes, warnings })
}

fn fmt_freq(f: f32) -> String {
    if (f - f.round()).abs() < 0.05 { format!("{:.0}", f.round()) } else { format!("{f:.1}") }
}

/// Export `eq` as AutoEq `ParametricEQ.txt`. The preamp line carries the
/// effective preamp (manual + auto headroom) so the file is self-contained.
pub fn export_autoeq(eq: &EqSettings) -> String {
    let mut out = format!("Preamp: {:.1} dB\n", effective_preamp_db(eq));
    for (i, n) in eq.nodes.iter().take(MAX_BANDS).enumerate() {
        out.push_str(&format!(
            "Filter {}: {} {} Fc {} Hz Gain {:.1} dB Q {:.2}\n",
            i + 1,
            if n.enabled { "ON" } else { "OFF" },
            type_code(n.filter_type),
            fmt_freq(n.frequency_hz),
            n.gain_db,
            n.q
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_autoeq_file() {
        let src = "Preamp: -6.2 dB\n\
                   Filter 1: ON PK Fc 105 Hz Gain 3.4 dB Q 0.70\n\
                   Filter 2: ON LSC Fc 105 Hz Gain 5.6 dB Q 0.71\n\
                   Filter 3: OFF HSC Fc 10000 Hz Gain -2.0 dB Q 0.71\n\
                   Filter 4: ON LS Fc 80 Hz Gain 1.5 dB\n\
                   Filter 5: ON HPQ Fc 25 Hz Q 0.707\n\
                   Filter 6: ON LPQ Fc 18000 Hz Gain 0 dB Q 0.71\n\
                   Filter 7: ON HS Fc 9000 Hz Gain -1.0 dB\n";
        let p = parse_autoeq(src).unwrap();
        assert_eq!(p.preamp_db, -6.2);
        assert_eq!(p.nodes.len(), 7);
        assert_eq!(p.nodes[0], EqNode { frequency_hz: 105.0, gain_db: 3.4, q: 0.70, filter_type: FilterType::Peaking, enabled: true });
        assert_eq!(p.nodes[1].filter_type, FilterType::LowShelf);
        assert!(!p.nodes[2].enabled);
        assert_eq!(p.nodes[2].filter_type, FilterType::HighShelf);
        assert_eq!(p.nodes[3].filter_type, FilterType::LowShelf);
        assert_eq!(p.nodes[3].q, 0.71);
        assert_eq!(p.nodes[4].filter_type, FilterType::HighPass);
        assert_eq!(p.nodes[4].gain_db, 0.0);
        assert_eq!(p.nodes[5].filter_type, FilterType::LowPass);
        assert_eq!(p.nodes[6].filter_type, FilterType::HighShelf);
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    }

    #[test]
    fn clamps_and_skips() {
        let src = "Preamp: -20 dB\nFilter 1: ON PK Fc 10 Hz Gain 15 dB Q 20\nFilter 2: ON BP Fc 100 Hz\n";
        let p = parse_autoeq(src).unwrap();
        assert_eq!(p.preamp_db, -12.0);
        assert_eq!(p.nodes[0].frequency_hz, 20.0);
        assert_eq!(p.nodes[0].gain_db, 12.0);
        assert_eq!(p.nodes[0].q, 10.0);
        assert_eq!(p.nodes.len(), 1);
        assert!(p.warnings.len() >= 4);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_autoeq("hello world\nnothing here").is_err());
        assert!(parse_autoeq("Preamp: abc dB").is_err());
    }

    #[test]
    fn caps_band_count() {
        let src: String =
            (1..=30).map(|i| format!("Filter {i}: ON PK Fc {} Hz Gain 1 dB Q 1\n", 20 * i)).collect();
        assert_eq!(parse_autoeq(&src).unwrap().nodes.len(), MAX_BANDS);
    }

    #[test]
    fn export_round_trip() {
        let eq = EqSettings {
            enabled: true,
            nodes: vec![
                EqNode { frequency_hz: 105.0, gain_db: 3.4, q: 0.7, filter_type: FilterType::Peaking, enabled: true },
                EqNode { frequency_hz: 31.5, gain_db: -2.5, q: 0.71, filter_type: FilterType::LowShelf, enabled: true },
                EqNode { frequency_hz: 8000.0, gain_db: 1.0, q: 1.41, filter_type: FilterType::HighShelf, enabled: false },
                EqNode { frequency_hz: 30.0, gain_db: 0.0, q: 0.71, filter_type: FilterType::HighPass, enabled: true },
                EqNode { frequency_hz: 19000.0, gain_db: 0.0, q: 0.71, filter_type: FilterType::LowPass, enabled: true },
            ],
            preamp_db: -4.5,
            auto_preamp: false,
            preset_id: None,
        };
        let text = export_autoeq(&eq);
        assert!(text.starts_with("Preamp: -4.5 dB\n"));
        assert!(text.contains("Filter 1: ON PK Fc 105 Hz Gain 3.4 dB Q 0.70"));
        let p = parse_autoeq(&text).unwrap();
        assert_eq!(p.preamp_db, -4.5);
        assert_eq!(p.nodes.len(), eq.nodes.len());
        for (a, b) in p.nodes.iter().zip(&eq.nodes) {
            assert_eq!(a.filter_type, b.filter_type);
            assert_eq!(a.enabled, b.enabled);
            assert!((a.frequency_hz - b.frequency_hz).abs() < 0.06);
            assert!((a.gain_db - b.gain_db).abs() < 0.051);
            assert!((a.q - b.q).abs() < 0.0051);
        }
    }
}
