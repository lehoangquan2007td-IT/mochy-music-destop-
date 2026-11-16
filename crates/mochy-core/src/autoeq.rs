//! The AutoEq headphone database (port of Android `eq/autoeq/AutoEqRepository`
//! parsing + `AutoEqFitter`). Pure functions — the app does the networking.
//!
//! - The index is `results/INDEX.md` of github.com/jaakkopasanen/AutoEq: one
//!   markdown line per measured result, `- [Name](./source/rig/Name) by source on rig`.
//! - A result's folder holds `<Name>.csv` (frequency, raw, target, …) and
//!   `<Name> ParametricEQ.txt` (AutoEq's own 10-band fit).
//! - [`fit`] re-fits filters for a different tuning (band count, upper limit,
//!   bass shelf, tilt) the way AutoEq does: shelves for the broad tilt, peaks
//!   placed greedily where the error is largest, then all tuned together.

use crate::eq::{AutoEqTuning, BiquadCoeffs, EqNode, FilterType, HeadphoneForm};

/// Where AutoEq's results live (raw files on GitHub, as upstream uses).
pub const RESULTS_BASE: &str = "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results";
/// The index is re-downloaded after a week (kept on disk for offline search).
pub const INDEX_MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;

/// One measured result: a headphone as one reviewer measured it on one rig.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoEqEntry {
    pub name: String,
    pub source: String,
    pub rig: Option<String>,
    /// The result's folder under `results/`, decoded.
    pub path: String,
    pub form: HeadphoneForm,
}

impl AutoEqEntry {
    /// "oratory1990" or "crinacle · GRAS 43AG-7".
    pub fn source_label(&self) -> String {
        match &self.rig {
            Some(r) => format!("{} · {r}", self.source),
            None => self.source.clone(),
        }
    }

    /// URL of a file in this result's folder (`csv` or `ParametricEQ.txt`).
    pub fn file_url(&self, file_name: &str) -> String {
        let folder = self.path.split('/').map(uri_encode).collect::<Vec<_>>().join("/");
        format!("{RESULTS_BASE}/{folder}/{}", uri_encode(file_name))
    }

    /// The folder's own name (the measurement files are named after it).
    pub fn folder(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    pub fn csv_url(&self) -> String {
        self.file_url(&format!("{}.csv", self.folder()))
    }

    pub fn parametric_url(&self) -> String {
        self.file_url(&format!("{} ParametricEQ.txt", self.folder()))
    }

    /// Stable cache file stem for this result.
    pub fn cache_key(&self) -> String {
        // FNV-1a: stable across runs (unlike std's hasher).
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in self.path.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        format!("{h:016x}")
    }
}

/// Every result for one headphone name — the unit the list shows.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoEqModel {
    pub name: String,
    pub entries: Vec<AutoEqEntry>,
    search_key: String,
    compact_key: String,
}

impl AutoEqModel {
    pub fn new(name: String, entries: Vec<AutoEqEntry>) -> Self {
        let search_key = normalize(&name);
        let compact_key = search_key.replace(' ', "");
        Self { name, entries, search_key, compact_key }
    }

    pub fn form(&self) -> HeadphoneForm {
        self.entries.first().map(|e| e.form).unwrap_or_default()
    }
}

/// Parse `INDEX.md` into models sorted A–Z (case-insensitive). Names and
/// paths may contain brackets; lines that don't match are skipped.
pub fn parse_index(text: &str) -> Vec<AutoEqModel> {
    let mut entries: Vec<AutoEqEntry> = Vec::with_capacity(9000);
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("- [") else { continue };
        let Some(name_end) = rest.find("](./") else { continue };
        let after = &rest[name_end + 4..];
        let Some(path_end) = after.find(") by ") else { continue };
        let name = &rest[..name_end];
        let path = percent_decode(&after[..path_end]);
        let by = after[path_end + 5..].trim();
        let (source, rig) = match by.find(" on ") {
            Some(i) => (by[..i].to_string(), Some(by[i + 4..].to_string())),
            None => (by.to_string(), None),
        };
        let rig_folder = path.split('/').nth(1).unwrap_or("").to_lowercase();
        let form = if rig_folder.contains("earbud") {
            HeadphoneForm::Earbud
        } else if rig_folder.contains("in-ear") {
            HeadphoneForm::InEar
        } else {
            HeadphoneForm::OverEar
        };
        entries.push(AutoEqEntry { name: name.to_string(), source, rig, path, form });
    }
    // Group by name, keeping first-seen order inside a group.
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<AutoEqEntry>> = std::collections::HashMap::new();
    for e in entries {
        if !groups.contains_key(&e.name) {
            order.push(e.name.clone());
        }
        groups.entry(e.name.clone()).or_default().push(e);
    }
    let mut models: Vec<AutoEqModel> =
        order.into_iter().map(|n| {
            let list = groups.remove(&n).unwrap_or_default();
            AutoEqModel::new(n, list)
        }).collect();
    models.sort_by_cached_key(|m| m.name.to_lowercase());
    models
}

/// Indices of the models whose name contains every typed word (or, ignoring
/// spaces and punctuation, the whole query): names that start with it first.
pub fn search(all: &[AutoEqModel], query: &str) -> Vec<usize> {
    let q = normalize(query);
    if q.is_empty() {
        return (0..all.len()).collect();
    }
    let words: Vec<&str> = q.split(' ').filter(|w| !w.is_empty()).collect();
    let compact = q.replace(' ', "");
    let mut hits: Vec<(u8, usize)> = all
        .iter()
        .enumerate()
        .filter(|(_, m)| words.iter().all(|w| m.search_key.contains(w)) || m.compact_key.contains(&compact))
        .map(|(i, m)| (u8::from(!(m.search_key.starts_with(&q) || m.compact_key.starts_with(&compact))), i))
        .collect();
    hits.sort_by_key(|h| h.0); // stable: A–Z inside each group
    hits.into_iter().map(|h| h.1).collect()
}

/// A result's measured curve and the target AutoEq fitted it to.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Measurement {
    pub frequencies: Vec<f64>,
    pub raw: Vec<f64>,
    pub target: Vec<f64>,
}

/// Parse a result's CSV (`frequency,raw,…,target,…`; target optional).
pub fn parse_measurement(csv: &str) -> Result<Measurement, String> {
    let mut lines = csv.lines().filter(|l| !l.trim().is_empty());
    let header: Vec<String> = lines
        .next()
        .ok_or("empty measurement")?
        .split(',')
        .map(|h| h.trim().to_lowercase())
        .collect();
    let col = |name: &str| header.iter().position(|h| h == name);
    let (Some(fi), Some(ri)) = (col("frequency"), col("raw")) else {
        return Err("Unreadable AutoEq measurement".into());
    };
    let ti = col("target");
    let mut m = Measurement::default();
    for row in lines {
        let cells: Vec<&str> = row.split(',').collect();
        let num = |i: usize| cells.get(i).and_then(|c| c.trim().parse::<f64>().ok()).filter(|v| v.is_finite());
        let (Some(f), Some(r)) = (num(fi), num(ri)) else { continue };
        m.frequencies.push(f);
        m.raw.push(r);
        m.target.push(ti.and_then(num).unwrap_or(0.0));
    }
    if m.frequencies.is_empty() {
        return Err("Unreadable AutoEq measurement".into());
    }
    Ok(m)
}

/// Percent escapes back to UTF-8 text; a literal `+` stays a plus.
pub fn percent_decode(text: &str) -> String {
    if !text.contains('%') {
        return text.to_string();
    }
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Android `Uri.encode`: everything but `A-Za-z0-9_-!.~'()*` is escaped.
pub fn uri_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3 / 2);
    for &byte in s.as_bytes() {
        let c = byte as char;
        if c.is_ascii_alphanumeric() || "_-!.~'()*".contains(c) {
            out.push(c);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Lowercase letters and digits, runs of anything else folded into one space.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = true;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    out.truncate(out.trim_end().len());
    out
}

// ---------------------------------------------------------------------------
// Fitter (Android `AutoEqFitter`)
// ---------------------------------------------------------------------------

const FIT_SR: f64 = 48_000.0;
const MAX_BOOST_DB: f64 = 8.0;
const MAX_CUT_DB: f64 = 12.0;

/// A fitted correction: filters plus the preamp that keeps it from clipping.
#[derive(Debug, Clone, PartialEq)]
pub struct Fit {
    pub preamp_db: f32,
    pub filters: Vec<EqNode>,
}

/// Magnitude in dB of `c` at a frequency given by its `cos(w)` and `cos(2w)`.
fn mag_db(c: &BiquadCoeffs, cos1: f64, cos2: f64) -> f64 {
    let num = c.b0 * c.b0 + c.b1 * c.b1 + c.b2 * c.b2 + 2.0 * (c.b0 * c.b1 + c.b1 * c.b2) * cos1 + 2.0 * c.b0 * c.b2 * cos2;
    let den = 1.0 + c.a1 * c.a1 + c.a2 * c.a2 + 2.0 * (c.a1 + c.a1 * c.a2) * cos1 + 2.0 * c.a2 * cos2;
    10.0 * (num.max(1e-30) / den.max(1e-30)).log10()
}

fn design(t: FilterType, f: f64, g: f64, q: f64) -> BiquadCoeffs {
    BiquadCoeffs::design(t, f as f32, g as f32, q as f32, FIT_SR as f32)
}

/// The measurement's target with the tuning's bass shelf and tilt applied.
pub fn tuned_target(m: &Measurement, tuning: &AutoEqTuning) -> Vec<f64> {
    let mut out = m.target.clone();
    if tuning.bass_db != 0.0 {
        let c = design(FilterType::LowShelf, 105.0, tuning.bass_db as f64, 0.7);
        for (o, &f) in out.iter_mut().zip(&m.frequencies) {
            let w = 2.0 * std::f64::consts::PI * f / FIT_SR;
            *o += mag_db(&c, w.cos(), (2.0 * w).cos());
        }
    }
    if tuning.tilt_db_per_octave != 0.0 {
        for (o, &f) in out.iter_mut().zip(&m.frequencies) {
            *o += tuning.tilt_db_per_octave as f64 * (f / 1000.0).log2();
        }
    }
    out
}

/// Fit for `tuning`. With the default tuning AutoEq's own fit (`autoeq_fit`,
/// from its ParametricEQ.txt) is used as is, like upstream.
pub fn correction(m: &Measurement, tuning: &AutoEqTuning, autoeq_fit: Option<&Fit>) -> Fit {
    if tuning.is_default()
        && let Some(f) = autoeq_fit.filter(|f| !f.filters.is_empty())
    {
        return f.clone();
    }
    fit(&m.frequencies, &m.raw, &tuned_target(m, tuning), tuning.bands as usize, tuning.max_hz as f64)
}

struct Filter {
    kind: FilterType,
    freq: f64,
    gain: f64,
    q: f64,
    curve: Vec<f64>,
}

impl Filter {
    fn new(kind: FilterType, freq: f64, gain: f64, q: f64, n: usize) -> Self {
        Filter { kind, freq, gain, q, curve: vec![0.0; n] }
    }
    /// Parameter j (0 log-frequency, 1 gain, 2 log-q) moved by h.
    fn nudged(&self, j: usize, h: f64) -> (f64, f64, f64) {
        match j {
            0 => (self.freq * 2f64.powf(h), self.gain, self.q),
            1 => (self.freq, self.gain + h, self.q),
            _ => (self.freq, self.gain, self.q * h.exp()),
        }
    }
    fn shift(&mut self, j: usize, delta: f64, max_f: f64) {
        match j {
            0 => self.freq = (self.freq * 2f64.powf(delta)).clamp(20.0, max_f),
            1 => self.gain = (self.gain + delta).clamp(-MAX_CUT_DB, MAX_BOOST_DB),
            _ => self.q = (self.q * delta.exp()).clamp(0.18, 6.0),
        }
    }
}

/// Moving average over `half_width(f)` octaves either side on the log grid.
fn smooth(values: &[f64], octave: &[f64], half_width: impl Fn(f64) -> f64) -> Vec<f64> {
    let n = values.len();
    let mut out = vec![0.0; n];
    for i in 0..n {
        let hw = half_width(2f64.powf(octave[i]));
        let mut sum = values[i];
        let mut count = 1.0;
        let mut j = i;
        while j > 0 && octave[i] - octave[j - 1] <= hw {
            j -= 1;
            sum += values[j];
            count += 1.0;
        }
        let mut j = i + 1;
        while j < n && octave[j] - octave[i] <= hw {
            sum += values[j];
            count += 1.0;
            j += 1;
        }
        out[i] = sum / count;
    }
    out
}

fn mean_over(values: &[f64], f: &[f64], from: f64, to: f64) -> f64 {
    let (mut sum, mut count) = (0.0, 0usize);
    for (v, &x) in values.iter().zip(f) {
        if (from..=to).contains(&x) {
            sum += v;
            count += 1;
        }
    }
    if count == 0 { 0.0 } else { sum / count as f64 }
}

/// Fit parametric filters bending `raw` onto `target` (Android `AutoEqFitter.fit`).
pub fn fit(frequencies: &[f64], raw: &[f64], target: &[f64], bands: usize, max_hz: f64) -> Fit {
    let n = frequencies.len().min(raw.len()).min(target.len());
    if n < 8 {
        return Fit { preamp_db: 0.0, filters: Vec::new() };
    }
    let freqs = &frequencies[..n];
    let octave: Vec<f64> = freqs.iter().map(|f| f.max(1e-6).log2()).collect();
    let cos1: Vec<f64> = freqs.iter().map(|f| (2.0 * std::f64::consts::PI * f / FIT_SR).cos()).collect();
    let cos2: Vec<f64> = freqs.iter().map(|f| (4.0 * std::f64::consts::PI * f / FIT_SR).cos()).collect();
    let response_into = |kind: FilterType, f: f64, g: f64, q: f64, into: &mut [f64]| {
        let c = design(kind, f, g, q);
        for i in 0..n {
            into[i] = mag_db(&c, cos1[i], cos2[i]);
        }
    };

    // The correction wanted, centred on the midrange (level is the preamp's job).
    let mut wanted: Vec<f64> = (0..n).map(|i| target[i] - raw[i]).collect();
    let mids: Vec<f64> = (0..n).filter(|&i| (250.0..=2500.0).contains(&freqs[i])).map(|i| wanted[i]).collect();
    let mid_mean = if mids.is_empty() { 0.0 } else { mids.iter().sum::<f64>() / mids.len() as f64 };
    for w in &mut wanted {
        *w -= mid_mean;
    }
    // Narrow treble detail is mostly the rig, not the headphone: smooth it harder.
    let mut e = smooth(&wanted, &octave, |f| if f < 6000.0 { 1.0 / 24.0 } else { 1.0 / 6.0 });
    for v in &mut e {
        *v = v.clamp(-MAX_CUT_DB, MAX_BOOST_DB);
    }
    // Full weight up to max_hz, fading to nothing a quarter-octave past it.
    let weight: Vec<f64> = freqs
        .iter()
        .map(|&f| {
            let over = (f / max_hz).log2();
            if over <= 0.0 {
                1.0
            } else if over >= 0.25 {
                0.0
            } else {
                0.5 + 0.5 * (std::f64::consts::PI * over / 0.25).cos()
            }
        })
        .collect();

    let mut filters: Vec<Filter> = Vec::new();
    filters.push(Filter::new(FilterType::LowShelf, 105.0, mean_over(&e, freqs, 20.0, 90.0), 0.7, n));
    if max_hz >= 12_000.0 {
        filters.push(Filter::new(FilterType::HighShelf, 10_000.0, mean_over(&e, freqs, 10_000.0, max_hz), 0.7, n));
    }
    for flt in &mut filters {
        flt.gain = flt.gain.clamp(-MAX_CUT_DB, MAX_BOOST_DB);
        let (k, f, g, q) = (flt.kind, flt.freq, flt.gain, flt.q);
        response_into(k, f, g, q, &mut flt.curve);
    }
    let mut total = vec![0.0; n];
    for flt in &filters {
        for i in 0..n {
            total[i] += flt.curve[i];
        }
    }

    // Peaks, greedily: each goes where the remaining error is biggest, as wide as that error is.
    let peaks = bands.saturating_sub(filters.len());
    let mut residual = vec![0.0; n];
    for _ in 0..peaks {
        for i in 0..n {
            residual[i] = e[i] - total[i];
        }
        let r = smooth(&residual, &octave, |_| 1.0 / 6.0);
        let mut best: Option<usize> = None;
        let mut best_value = 0.0;
        for i in 0..n {
            let f = freqs[i];
            if !(20.0..=max_hz).contains(&f) {
                continue;
            }
            let v = r[i].abs() * weight[i];
            if v > best_value {
                best_value = v;
                best = Some(i);
            }
        }
        let Some(best) = best.filter(|_| best_value >= 0.2) else { continue };
        let gain = r[best].clamp(-MAX_CUT_DB, MAX_BOOST_DB);
        let (mut lo, mut hi) = (best, best);
        while lo > 0 && r[lo - 1] * gain > 0.0 && r[lo - 1].abs() > gain.abs() / 2.0 {
            lo -= 1;
        }
        while hi < n - 1 && r[hi + 1] * gain > 0.0 && r[hi + 1].abs() > gain.abs() / 2.0 {
            hi += 1;
        }
        let bw = (octave[hi] - octave[lo]).clamp(0.15, 4.0);
        let q = (2f64.powf(bw).sqrt() / (2f64.powf(bw) - 1.0)).clamp(0.3, 6.0);
        let mut flt = Filter::new(FilterType::Peaking, freqs[best], gain, q, n);
        response_into(flt.kind, flt.freq, flt.gain, flt.q, &mut flt.curve);
        for i in 0..n {
            total[i] += flt.curve[i];
        }
        filters.push(flt);
    }

    optimize(&mut filters, &mut total, &e, &weight, freqs[n - 1], &response_into);

    let max_boost = total.iter().copied().fold(f64::MIN, f64::max).max(0.0);
    let out = filters
        .iter()
        .map(|f| EqNode {
            frequency_hz: f.freq.round() as f32,
            gain_db: ((f.gain * 10.0).round() / 10.0) as f32,
            q: ((f.q * 100.0).round() / 100.0) as f32,
            filter_type: f.kind,
            enabled: true,
        })
        .collect();
    Fit { preamp_db: (-(max_boost * 10.0).round() / 10.0) as f32, filters: out }
}

/// Adam on every filter's frequency (log), gain and Q (log) at once with
/// numeric gradients; only the nudged filter's curve is recomputed per probe.
fn optimize(
    filters: &mut [Filter],
    total: &mut [f64],
    e: &[f64],
    w: &[f64],
    max_f: f64,
    response_into: &dyn Fn(FilterType, f64, f64, f64, &mut [f64]),
) {
    let n = total.len();
    let weight_sum = w.iter().sum::<f64>().max(1e-9);
    let mut probe = vec![0.0; n];
    let loss_with = |total: &[f64], old: &[f64], replacement: &[f64]| -> f64 {
        let mut sum = 0.0;
        for i in 0..n {
            if w[i] == 0.0 {
                continue;
            }
            let d = total[i] - old[i] + replacement[i] - e[i];
            sum += w[i] * d * d;
        }
        sum / weight_sum
    };
    let params = filters.len() * 3;
    let mut m = vec![0.0; params];
    let mut v = vec![0.0; params];
    let mut grad = vec![0.0; params];
    let lr = [0.03, 0.18, 0.04];
    let step = [0.005, 0.02, 0.005];
    let iterations = 160;
    for t in 1..=iterations {
        for (k, flt) in filters.iter().enumerate() {
            let tunable: &[usize] = if flt.kind == FilterType::Peaking { &[0, 1, 2] } else { &[1] };
            for j in 0..3 {
                grad[k * 3 + j] = 0.0;
            }
            for &j in tunable {
                let h = step[j];
                let (f, g, q) = flt.nudged(j, h);
                response_into(flt.kind, f, g, q, &mut probe);
                let lp = loss_with(total, &flt.curve, &probe);
                let (f, g, q) = flt.nudged(j, -h);
                response_into(flt.kind, f, g, q, &mut probe);
                let lm = loss_with(total, &flt.curve, &probe);
                grad[k * 3 + j] = (lp - lm) / (2.0 * h) + if j == 1 { 2e-4 * flt.gain } else { 0.0 };
            }
        }
        let decay = 1.0 - 0.6 * t as f64 / iterations as f64;
        for (k, flt) in filters.iter_mut().enumerate() {
            for j in 0..3 {
                let i = k * 3 + j;
                if grad[i] == 0.0 {
                    continue;
                }
                m[i] = 0.9 * m[i] + 0.1 * grad[i];
                v[i] = 0.999 * v[i] + 0.001 * grad[i] * grad[i];
                let mh = m[i] / (1.0 - 0.9f64.powi(t));
                let vh = v[i] / (1.0 - 0.999f64.powi(t));
                let delta = -lr[j] * decay * mh / (vh.sqrt() + 1e-8);
                flt.shift(j, delta, max_f);
            }
            let (kind, f, g, q) = (flt.kind, flt.freq, flt.gain, flt.q);
            response_into(kind, f, g, q, &mut flt.curve);
        }
        total.iter_mut().for_each(|x| *x = 0.0);
        for flt in filters.iter() {
            for i in 0..n {
                total[i] += flt.curve[i];
            }
        }
    }
}

/// The response (dB) of `filters` on the measurement's own grid, so
/// `corrected = raw + response`.
pub fn response_on(frequencies: &[f64], filters: &[EqNode]) -> Vec<f64> {
    let mut out = vec![0.0; frequencies.len()];
    for n in filters.iter().filter(|n| n.enabled) {
        let c = BiquadCoeffs::design(n.filter_type, n.frequency_hz, n.gain_db, n.q, FIT_SR as f32);
        for (o, &f) in out.iter_mut().zip(frequencies) {
            let w = 2.0 * std::f64::consts::PI * f / FIT_SR;
            *o += mag_db(&c, w.cos(), (2.0 * w).cos());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = "# Index\n\nThis is a list of all the headphones.\n\n\
- [1MORE Triple Driver](./oratory1990/in-ear/1MORE%20Triple%20Driver) by oratory1990\n\
- [Sennheiser HD 650](./oratory1990/over-ear/Sennheiser%20HD%20650) by oratory1990\n\
- [Sennheiser HD 650](./crinacle/GRAS%2043AG-7%20over-ear/Sennheiser%20HD%20650) by crinacle on GRAS 43AG-7\n\
- [Apple AirPods (2nd gen)](./Rtings/earbud/Apple%20AirPods%20%282nd%20gen%29) by Rtings\n\
- [Sony WH-1000XM5 [ANC]](./Rtings/over-ear/Sony%20WH-1000XM5%20[ANC]) by Rtings\n\
not a result line\n";

    #[test]
    fn index_parses_and_groups() {
        let models = parse_index(INDEX);
        let names: Vec<&str> = models.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["1MORE Triple Driver", "Apple AirPods (2nd gen)", "Sennheiser HD 650", "Sony WH-1000XM5 [ANC]"]);
        let hd650 = &models[2];
        assert_eq!(hd650.entries.len(), 2);
        assert_eq!(hd650.entries[1].rig.as_deref(), Some("GRAS 43AG-7"));
        assert_eq!(hd650.entries[1].source_label(), "crinacle · GRAS 43AG-7");
        assert_eq!(hd650.entries[1].path, "crinacle/GRAS 43AG-7 over-ear/Sennheiser HD 650");
        assert_eq!(models[0].form(), HeadphoneForm::InEar);
        assert_eq!(models[1].form(), HeadphoneForm::Earbud);
        assert_eq!(models[1].entries[0].path, "Rtings/earbud/Apple AirPods (2nd gen)");
        assert_eq!(models[3].name, "Sony WH-1000XM5 [ANC]");
    }

    #[test]
    fn urls_are_encoded_like_android() {
        let e = &parse_index(INDEX)[1].entries[0];
        assert_eq!(
            e.csv_url(),
            format!("{RESULTS_BASE}/Rtings/earbud/Apple%20AirPods%20(2nd%20gen)/Apple%20AirPods%20(2nd%20gen).csv")
        );
        assert!(e.parametric_url().ends_with("/Apple%20AirPods%20(2nd%20gen)%20ParametricEQ.txt"));
        assert_eq!(e.cache_key().len(), 16);
        assert_eq!(e.cache_key(), e.cache_key());
    }

    #[test]
    fn search_matches_words_and_compact_text() {
        let models = parse_index(INDEX);
        let names = |q: &str| search(&models, q).into_iter().map(|i| models[i].name.clone()).collect::<Vec<_>>();
        assert_eq!(names("hd 650"), ["Sennheiser HD 650"]);
        assert_eq!(names("wh1000"), ["Sony WH-1000XM5 [ANC]"]);
        assert_eq!(names("sennheiser 650"), ["Sennheiser HD 650"]);
        assert_eq!(names(""), models.iter().map(|m| m.name.clone()).collect::<Vec<_>>());
        assert!(names("zzz").is_empty());
        // Names starting with the query lead.
        let models = parse_index("- [Pro X](./a/over-ear/Pro%20X) by a\n- [Audio Pro](./a/over-ear/Audio%20Pro) by a\n");
        assert_eq!(search(&models, "pro").into_iter().map(|i| models[i].name.as_str()).collect::<Vec<_>>(), ["Pro X", "Audio Pro"]);
    }

    #[test]
    fn helpers() {
        assert_eq!(normalize("  Sony WH-1000XM5 (ANC) "), "sony wh 1000xm5 anc");
        assert_eq!(percent_decode("a%20b+c%28"), "a b+c(");
        assert_eq!(percent_decode("bad%zzx"), "bad%zzx");
        assert_eq!(percent_decode("%C3%A9t%C3%A9"), "été");
        assert_eq!(uri_encode("A b(1)/é"), "A%20b(1)%2F%C3%A9");
    }

    /// A synthetic headphone: a bass shelf deficit, a 3 kHz peak and a treble dip.
    fn synthetic() -> Measurement {
        let freqs: Vec<f64> = (0..300).map(|i| 20.0 * 1000f64.powf(i as f64 / 299.0)).collect();
        let bump = |f: f64, c: f64, g: f64, bw: f64| g * (-((f / c).log2() / bw).powi(2)).exp();
        let raw: Vec<f64> = freqs
            .iter()
            .map(|&f| bump(f, 3000.0, 6.0, 0.4) - bump(f, 8000.0, 5.0, 0.3) - if f < 120.0 { 4.0 } else { 0.0 })
            .collect();
        Measurement { target: vec![0.0; freqs.len()], frequencies: freqs, raw }
    }

    fn rms_error(m: &Measurement, fit: &Fit, max_hz: f64) -> f64 {
        let resp = response_on(&m.frequencies, &fit.filters);
        let mut corrected: Vec<f64> = m.raw.iter().zip(&resp).map(|(r, c)| r + c).collect();
        let mid = mean_over(&corrected, &m.frequencies, 250.0, 2500.0);
        corrected.iter_mut().for_each(|c| *c -= mid);
        let (mut sum, mut n) = (0.0, 0.0);
        for (i, &f) in m.frequencies.iter().enumerate() {
            if f <= max_hz {
                sum += (corrected[i] - m.target[i]).powi(2);
                n += 1.0;
            }
        }
        (sum / n).sqrt()
    }

    #[test]
    fn fitter_flattens_a_synthetic_headphone() {
        let m = synthetic();
        let before = rms_error(&m, &Fit { preamp_db: 0.0, filters: vec![] }, 16_000.0);
        let fit = correction(&m, &AutoEqTuning::default(), None);
        assert_eq!(fit.filters.len(), 10);
        assert_eq!(fit.filters[0].filter_type, FilterType::LowShelf);
        assert_eq!(fit.filters[1].filter_type, FilterType::HighShelf);
        let after = rms_error(&m, &fit, 16_000.0);
        assert!(after < before * 0.35, "rms {before:.2} -> {after:.2}");
        // Cuts the 3 kHz peak somewhere near it.
        assert!(fit.filters.iter().any(|f| f.gain_db < -2.0 && (2000.0..4500.0).contains(&f.frequency_hz)));
        // The preamp covers the biggest boost.
        let resp = response_on(&m.frequencies, &fit.filters);
        let max = resp.iter().copied().fold(0.0, f64::max);
        assert!((fit.preamp_db as f64 + max).abs() < 0.11);
        for f in &fit.filters {
            assert!(f.gain_db <= MAX_BOOST_DB as f32 && f.gain_db >= -MAX_CUT_DB as f32);
            assert!((0.18..=6.0).contains(&f.q));
        }
    }

    #[test]
    fn fewer_bands_and_lower_limit() {
        let m = synthetic();
        let t = AutoEqTuning { bands: 5, max_hz: 10_000, ..Default::default() };
        let fit = correction(&m, &t, None);
        assert!(fit.filters.len() <= 5);
        // No high shelf below 12 kHz.
        assert!(fit.filters.iter().all(|f| f.filter_type != FilterType::HighShelf));
    }

    #[test]
    fn default_tuning_uses_autoeqs_own_fit() {
        let m = synthetic();
        let own = Fit { preamp_db: -3.0, filters: vec![EqNode::peaking(1000.0, 2.0)] };
        assert_eq!(correction(&m, &AutoEqTuning::default(), Some(&own)), own);
        let t = AutoEqTuning { bass_db: 4.0, ..Default::default() };
        assert_ne!(correction(&m, &t, Some(&own)), own);
    }

    #[test]
    fn tuned_target_adds_bass_and_tilt() {
        let m = synthetic();
        let t = AutoEqTuning { bass_db: 6.0, tilt_db_per_octave: 1.0, ..Default::default() };
        let tt = tuned_target(&m, &t);
        assert!((tt[0] - (6.0 - 5.64)).abs() < 0.3, "{}", tt[0]); // 20 Hz: shelf +6, tilt -5.64
        let k = m.frequencies.iter().position(|&f| f >= 8000.0).unwrap();
        assert!((tt[k] - 3.0).abs() < 0.2, "{}", tt[k]); // 8 kHz: +3 octaves of tilt
    }

    #[test]
    fn measurement_csv_parses() {
        let csv = "frequency,raw,error,smoothed,error_smoothed,equalization,parametric_eq,fixed_band_eq,equalized_raw,equalized_smoothed,target\n\
20.00,-3.5,1,1,1,1,1,1,1,1,0.5\n\
20.20,-3.4,1,1,1,1,1,1,1,1,0.4\n\
bad,row\n";
        let m = parse_measurement(csv).unwrap();
        assert_eq!(m.frequencies, [20.0, 20.2]);
        assert_eq!(m.raw, [-3.5, -3.4]);
        assert_eq!(m.target, [0.5, 0.4]);
        assert!(parse_measurement("frequency,x\n1,2\n").is_err());
        let no_target = parse_measurement("frequency,raw\n100,1\n").unwrap();
        assert_eq!(no_target.target, [0.0]);
    }
}
