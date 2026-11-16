//! Track analysis for crossmix (port of `CrossmixAnalysis.kt`): streaming
//! spectral-flux onsets → tempo (autocorrelation with a 120 BPM prior) →
//! Ellis DP beat tracker → bars/sections ([`super::bars`]) → key
//! (Krumhansl profiles), silence bounds, per-second loudness / bass /
//! brightness and a vocal-presence curve. Input is mono at
//! [`SAMPLE_RATE`].

use std::f32::consts::PI;
use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};

use super::bars;

pub const SAMPLE_RATE: u32 = 22_050;
const FRAME_SIZE: usize = 1_024;
const HOP_SIZE: usize = 256;
const BINS: usize = FRAME_SIZE / 2 + 1;
pub(crate) const FRAMES_PER_SECOND: f32 = SAMPLE_RATE as f32 / HOP_SIZE as f32;
/// Loudness above which a second counts as music (~16 dB under the peak).
const AUDIBLE_ENERGY: f32 = 0.35;

#[derive(Debug, Clone, PartialEq)]
pub struct Vibe {
    pub energy: f32,
    pub danceability: f32,
    pub brightness: f32,
    pub bass_weight: f32,
    pub minor: bool,
}

/// A stretch of steady beats: beat n falls at `first_beat_ms + n·period_ms`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalGrid {
    pub first_beat_ms: f64,
    pub period_ms: f64,
    pub first_index: usize,
}

impl LocalGrid {
    /// The beat time nearest `ms` on this grid.
    pub fn snap(&self, ms: i64) -> i64 {
        let n = ((ms as f64 - self.first_beat_ms) / self.period_ms).round();
        (self.first_beat_ms + n * self.period_ms).round() as i64
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackAnalysis {
    pub analyzed_ms: i64,
    pub complete: bool,
    pub bpm: Option<f32>,
    /// 0..1; below ~0.35 nothing is beat-matched to it.
    pub bpm_confidence: f32,
    pub beats_ms: Vec<i64>,
    /// Beats that start a bar (indices into `beats_ms`).
    pub bar_beats: Vec<usize>,
    /// Bars a new section starts on (indices into `bar_starts_ms()`).
    pub section_bars: Vec<usize>,
    pub key: Option<String>,
    pub key_confidence: f32,
    pub leading_silence_end_ms: i64,
    pub trailing_silence_start_ms: i64,
    pub energy: Vec<f32>,
    pub bass: Vec<f32>,
    pub brightness: Vec<f32>,
    pub vibe: Vibe,
    pub vocal: Vec<f32>,
}

impl TrackAnalysis {
    pub fn beat_ms(&self) -> Option<f32> {
        self.bpm.map(|b| 60_000.0 / b)
    }

    pub fn is_beat_reliable(&self) -> bool {
        self.bpm.is_some() && self.bpm_confidence >= 0.35 && self.beats_ms.len() >= 16
    }

    pub fn bar_starts_ms(&self) -> Vec<i64> {
        self.bar_beats.iter().map(|&i| self.beats_ms[i]).collect()
    }

    /// How big a phrase bar `index` starts (8, 4, 2 or 1 bars).
    pub fn phrase_level(&self, index: usize) -> u32 {
        let mut previous = 0usize;
        let mut next: Option<usize> = None;
        for &bar in &self.section_bars {
            if bar <= index {
                previous = bar;
            } else {
                next = Some(bar);
                break;
            }
        }
        let from_previous = phrase_level_of(index - previous);
        match next {
            None => from_previous,
            Some(n) => from_previous.max(phrase_level_of(n - index)),
        }
    }

    pub fn energy_around(&self, ms: i64, window_ms: i64) -> f32 {
        average_over(&self.energy, ms, window_ms)
    }

    pub fn bass_around(&self, ms: i64, window_ms: i64) -> f32 {
        average_over(&self.bass, ms, window_ms)
    }

    /// Seconds of real music between two points.
    pub fn audible_seconds(&self, from_ms: i64, to_ms: i64) -> f32 {
        if to_ms <= from_ms || self.energy.is_empty() {
            return 0.0;
        }
        let n = self.energy.len() as i64;
        let from = (from_ms / 1000).clamp(0, n);
        let to = (to_ms / 1000).clamp(from, n);
        (from..to).filter(|&s| self.energy[s as usize] >= AUDIBLE_ENERGY).count() as f32
    }

    /// Beat grid near `ms`, least squares over nearby beats.
    pub fn local_grid(&self, ms: i64) -> Option<LocalGrid> {
        self.local_grid_span(ms, 12)
    }

    pub fn local_grid_span(&self, ms: i64, span: usize) -> Option<LocalGrid> {
        if self.beats_ms.len() < 4 {
            return None;
        }
        let nearest = (0..self.beats_ms.len()).min_by_key(|&i| (self.beats_ms[i] - ms).abs())?;
        let from = nearest.saturating_sub(span);
        let to = (nearest + span).min(self.beats_ms.len() - 1);
        let count = to - from + 1;
        if count < 4 {
            return None;
        }
        let (mut sx, mut sy, mut sxx, mut sxy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for i in from..=to {
            let x = (i - from) as f64;
            let y = self.beats_ms[i] as f64;
            sx += x;
            sy += y;
            sxx += x * x;
            sxy += x * y;
        }
        let c = count as f64;
        let denominator = c * sxx - sx * sx;
        if denominator == 0.0 {
            return None;
        }
        let period = (c * sxy - sx * sy) / denominator;
        let intercept = (sy - period * sx) / c;
        if period <= 200.0 || period >= 1_500.0 {
            return None;
        }
        Some(LocalGrid { first_beat_ms: intercept, period_ms: period, first_index: from })
    }

    /// Shift every time by `offset_ms` (an analysis of a song's tail,
    /// placed at its real position).
    pub fn shifted(mut self, offset_ms: i64) -> Self {
        if offset_ms == 0 {
            return self;
        }
        for b in &mut self.beats_ms {
            *b += offset_ms;
        }
        self.leading_silence_end_ms += offset_ms;
        self.trailing_silence_start_ms += offset_ms;
        self.analyzed_ms += offset_ms;
        let pad = (offset_ms / 1000).max(0) as usize;
        // the unanalysed head counts as loud, middling music
        let prefix = |v: &mut Vec<f32>, fill: f32| {
            let mut out = vec![fill; pad];
            out.append(v);
            *v = out;
        };
        prefix(&mut self.energy, 0.8);
        prefix(&mut self.bass, 0.5);
        prefix(&mut self.brightness, 0.5);
        prefix(&mut self.vocal, 0.0);
        self
    }
}

fn phrase_level_of(bars: usize) -> u32 {
    if bars % 8 == 0 {
        8
    } else if bars % 4 == 0 {
        4
    } else if bars % 2 == 0 {
        2
    } else {
        1
    }
}

fn average_over(values: &[f32], ms: i64, window_ms: i64) -> f32 {
    if values.is_empty() {
        return 0.5;
    }
    let last = values.len() as i64 - 1;
    let from = ((ms - window_ms / 2) / 1000).clamp(0, last);
    let to = ((ms + window_ms / 2) / 1000).clamp(from, last);
    let sum: f32 = values[from as usize..=to as usize].iter().sum();
    sum / (to - from + 1) as f32
}

/// Streaming analyser: feed mono samples at [`SAMPLE_RATE`], then `finish`.
pub struct StreamingTrackAnalyzer {
    ring: Vec<f32>,
    write_index: usize,
    since_hop: usize,
    total_samples: u64,
    window: Vec<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
    fft_in: Vec<f32>,
    fft_out: Vec<realfft::num_complex::Complex<f32>>,
    fft_scratch: Vec<realfft::num_complex::Complex<f32>>,
    previous_log: Vec<f32>,
    band_rise: [f32; 4],
    chroma: [f32; 12],
    frame_chroma: [f32; 12],
    flux: Vec<f32>,
    rms: Vec<f32>,
    low: Vec<f32>,
    centroid: Vec<f32>,
    lead: Vec<f32>,
    lead_share: Vec<f32>,
    tonality: Vec<f32>,
    kick: Vec<f32>,
    snap: Vec<f32>,
    harmony: Vec<f32>,
    tables: Tables,
}

struct Tables {
    low_bins: usize,
    voice_from: usize,
    voice_to: usize,
    harmony_from: usize,
    pitch_class_of: Vec<i32>,
    band_of: Vec<usize>,
    band_bins: [f32; 4],
}

const BAND_EDGES_HZ: [f32; 3] = [160.0, 600.0, 3_000.0];
const BAND_WEIGHTS: [f32; 4] = [1.0, 0.8, 0.55, 0.2];
const MAJOR_PROFILE: [f32; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MINOR_PROFILE: [f32; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
pub const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

impl Tables {
    fn new() -> Self {
        let sr = SAMPLE_RATE as f32;
        let fs = FRAME_SIZE as f32;
        let hz = |bin: usize| bin as f32 * sr / fs;
        let pitch_class_of = (0..BINS)
            .map(|bin| {
                let f = hz(bin);
                if bin == 0 || !(55.0..=5_000.0).contains(&f) {
                    -1
                } else {
                    let midi = (69.0 + 12.0 * (f / 440.0).log2()).round() as i32;
                    midi.rem_euclid(12)
                }
            })
            .collect();
        let band_of: Vec<usize> =
            (0..BINS).map(|bin| BAND_EDGES_HZ.iter().position(|&e| hz(bin) < e).unwrap_or(BAND_EDGES_HZ.len())).collect();
        let mut band_bins = [0f32; 4];
        for (b, slot) in band_bins.iter_mut().enumerate() {
            *slot = ((1..BINS).filter(|&i| band_of[i] == b).count().max(1)) as f32;
        }
        Tables {
            low_bins: (150.0 * fs / sr).round() as usize,
            voice_from: (1_000.0 * fs / sr).round() as usize,
            voice_to: (4_000.0 * fs / sr).round() as usize,
            harmony_from: (300.0 * fs / sr).round() as usize,
            pitch_class_of,
            band_of,
            band_bins,
        }
    }
}

impl Default for StreamingTrackAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingTrackAnalyzer {
    pub fn new() -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FRAME_SIZE);
        let fft_in = fft.make_input_vec();
        let fft_out = fft.make_output_vec();
        let fft_scratch = fft.make_scratch_vec();
        StreamingTrackAnalyzer {
            ring: vec![0.0; FRAME_SIZE],
            write_index: 0,
            since_hop: 0,
            total_samples: 0,
            window: (0..FRAME_SIZE).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / (FRAME_SIZE - 1) as f32).cos()).collect(),
            fft,
            fft_in,
            fft_out,
            fft_scratch,
            previous_log: vec![0.0; BINS],
            band_rise: [0.0; 4],
            chroma: [0.0; 12],
            frame_chroma: [0.0; 12],
            flux: Vec::with_capacity(4096),
            rms: Vec::with_capacity(4096),
            low: Vec::with_capacity(4096),
            centroid: Vec::with_capacity(4096),
            lead: Vec::with_capacity(4096),
            lead_share: Vec::with_capacity(4096),
            tonality: Vec::with_capacity(4096),
            kick: Vec::with_capacity(4096),
            snap: Vec::with_capacity(4096),
            harmony: Vec::with_capacity(4096 * 12),
            tables: Tables::new(),
        }
    }

    /// Mono samples at [`SAMPLE_RATE`].
    pub fn feed(&mut self, samples: &[f32]) {
        for &s in samples {
            self.ring[self.write_index] = s;
            self.write_index = (self.write_index + 1) & (FRAME_SIZE - 1);
            self.total_samples += 1;
            if self.total_samples >= FRAME_SIZE as u64 {
                self.since_hop += 1;
                if self.since_hop >= HOP_SIZE {
                    self.since_hop = 0;
                    self.analyze_frame();
                }
            }
        }
    }

    pub fn analyzed_ms(&self) -> i64 {
        (self.total_samples * 1000 / SAMPLE_RATE as u64) as i64
    }

    fn analyze_frame(&mut self) {
        let mut energy = 0.0f64;
        for i in 0..FRAME_SIZE {
            let v = self.ring[(self.write_index + i) & (FRAME_SIZE - 1)];
            energy += (v * v) as f64;
            self.fft_in[i] = v * self.window[i];
        }
        let _ = self.fft.process_with_scratch(&mut self.fft_in, &mut self.fft_out, &mut self.fft_scratch);

        let t = &self.tables;
        self.band_rise = [0.0; 4];
        self.frame_chroma = [0.0; 12];
        let (mut low_energy, mut total_energy, mut weighted) = (0f32, 0f32, 0f32);
        let (mut voice_energy, mut voice_log_sum, mut voice_bins) = (0f32, 0f32, 0usize);
        for bin in 1..BINS {
            let c = self.fft_out[bin];
            let magnitude = (c.re * c.re + c.im * c.im).sqrt();
            let compressed = (1.0 + 1_000.0 * magnitude).ln();
            let rise = compressed - self.previous_log[bin];
            if rise > 0.0 {
                self.band_rise[t.band_of[bin]] += rise;
            }
            self.previous_log[bin] = compressed;
            let power = magnitude * magnitude;
            total_energy += power;
            weighted += power * bin as f32;
            if bin <= t.low_bins {
                low_energy += power;
            }
            if (t.voice_from..=t.voice_to).contains(&bin) {
                voice_energy += power;
                voice_log_sum += (power + 1e-10).ln();
                voice_bins += 1;
            }
            let pc = t.pitch_class_of[bin];
            if pc >= 0 {
                self.chroma[pc as usize] += magnitude;
                if bin >= t.harmony_from {
                    self.frame_chroma[pc as usize] += magnitude;
                }
            }
        }
        let mut onset = 0f32;
        for band in 0..4 {
            onset += BAND_WEIGHTS[band] * self.band_rise[band] / t.band_bins[band];
        }
        self.flux.push(onset);
        self.kick.push(self.band_rise[0] / t.band_bins[0]);
        self.snap.push(0.5 * (self.band_rise[2] / t.band_bins[2] + self.band_rise[3] / t.band_bins[3]));
        self.harmony.extend_from_slice(&self.frame_chroma);
        self.rms.push((energy / FRAME_SIZE as f64).sqrt() as f32);
        self.low.push(if total_energy > 0.0 { low_energy / total_energy } else { 0.0 });
        self.centroid.push(if total_energy > 0.0 {
            (weighted / total_energy) * SAMPLE_RATE as f32 / FRAME_SIZE as f32
        } else {
            0.0
        });
        self.lead.push(voice_energy);
        self.lead_share.push(if total_energy > 0.0 { voice_energy / total_energy } else { 0.0 });
        let flatness = if voice_bins > 0 && voice_energy > 0.0 {
            ((voice_log_sum / voice_bins as f32).exp() / (voice_energy / voice_bins as f32)).clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.tonality.push(1.0 - flatness);
    }

    /// The analysis, or `None` with under 8 s of audio.
    pub fn finish(&self, complete: bool) -> Option<TrackAnalysis> {
        let frames = self.flux.len();
        if (frames as f32) < FRAMES_PER_SECOND * 8.0 {
            return None;
        }
        let onset = onset_envelope(&self.flux);
        let tempo = estimate_tempo(&onset);
        let beats = tempo.map(|(bpm, _)| track_beats(&onset, bpm)).unwrap_or_default();
        let found = if beats.len() >= 8 {
            let beat_frames: Vec<i32> = beats.iter().map(|&b| ms_to_frame(b)).collect();
            bars::find(
                &beat_frames,
                &bars::Frames {
                    onset: &onset,
                    kick: &self.kick,
                    snap: &self.snap,
                    chroma: &self.harmony,
                    rms: &self.rms,
                    low_share: &self.low,
                    centroid_hz: &self.centroid,
                },
            )
        } else {
            bars::Found::default()
        };
        let (key, key_conf) = estimate_key(&self.chroma);
        let analyzed_ms = self.analyzed_ms();
        let (lead_in, trail) = silence_bounds(&self.rms, analyzed_ms);
        let (loud, bass, bright) = per_second_curves(&self.rms, &self.low, &self.centroid);
        let vocal = vocal_curve(&self.lead, &self.lead_share, &self.tonality, &self.rms);
        let vibe = Vibe {
            energy: overall_energy(&self.rms, &onset),
            danceability: (tempo.map_or(0.0, |t| t.1) * 0.7 + pulse_strength(&beats, &onset) * 0.3).clamp(0.0, 1.0),
            brightness: average_of_loud(&bright, &loud),
            bass_weight: average_of_loud(&bass, &loud),
            minor: key.as_deref().is_some_and(|k| k.ends_with("minor")),
        };
        Some(TrackAnalysis {
            analyzed_ms,
            complete,
            bpm: tempo.map(|t| t.0),
            bpm_confidence: tempo.map_or(0.0, |t| t.1),
            beats_ms: beats,
            bar_beats: found.bars,
            section_bars: found.sections,
            key,
            key_confidence: key_conf,
            leading_silence_end_ms: lead_in,
            trailing_silence_start_ms: trail,
            energy: loud,
            bass,
            brightness: bright,
            vibe,
            vocal,
        })
    }
}

// flux peaks when a hit is about 3/4 along the analysis window
const ONSET_LAG_SAMPLES: f32 = FRAME_SIZE as f32 * 0.27;

pub(crate) fn frame_to_ms(frame: f32) -> i64 {
    ((frame * HOP_SIZE as f32 + FRAME_SIZE as f32 / 2.0 + ONSET_LAG_SAMPLES) * 1_000.0 / SAMPLE_RATE as f32) as i64
}

pub(crate) fn ms_to_frame(ms: i64) -> i32 {
    ((ms as f32 * SAMPLE_RATE as f32 / 1_000.0 - FRAME_SIZE as f32 / 2.0 - ONSET_LAG_SAMPLES) / HOP_SIZE as f32).round() as i32
}

/// Spectral flux minus its local average, normalised to 0..1.
pub(crate) fn onset_envelope(raw: &[f32]) -> Vec<f32> {
    let half = (FRAMES_PER_SECOND / 2.0).round() as usize;
    let mut prefix = vec![0f64; raw.len() + 1];
    for (i, &v) in raw.iter().enumerate() {
        prefix[i + 1] = prefix[i] + v as f64;
    }
    let mut out = vec![0f32; raw.len()];
    let mut peak = 0f32;
    for i in 0..raw.len() {
        let from = i.saturating_sub(half);
        let to = (i + half).min(raw.len());
        let mean = ((prefix[to] - prefix[from]) / (to - from) as f64) as f32;
        let v = (raw[i] - mean).max(0.0);
        out[i] = v;
        peak = peak.max(v);
    }
    if peak > 0.0 {
        out.iter_mut().for_each(|v| *v /= peak);
    }
    out
}

/// Beat period from the onset autocorrelation (prior around 120 BPM):
/// `(bpm, confidence)`.
pub(crate) fn estimate_tempo(onset: &[f32]) -> Option<(f32, f32)> {
    let fps = FRAMES_PER_SECOND;
    let min_lag = (60.0 / 200.0 * fps).round() as usize;
    let max_lag = ((60.0 / 60.0 * fps).round() as usize).min(onset.len() / 3);
    if max_lag <= min_lag + 2 {
        return None;
    }
    let n = onset.len();
    let mut acf = vec![0f32; max_lag + 2];
    for (lag, slot) in acf.iter_mut().enumerate().take(max_lag + 2).skip(min_lag) {
        let mut sum = 0f64;
        for i in 0..n.saturating_sub(lag) {
            sum += (onset[i] * onset[i + lag]) as f64;
        }
        *slot = (sum / (n - lag) as f64) as f32;
    }
    let (mut best_lag, mut best_score, mut total) = (0usize, 0f32, 0f32);
    for (lag, &a) in acf.iter().enumerate().take(max_lag + 1).skip(min_lag) {
        let bpm = 60.0 * fps / lag as f32;
        let z = (bpm / 120.0).log2() / 0.9;
        let prior = (-0.5 * z * z).exp();
        let score = a * prior;
        total += a;
        if score > best_score {
            best_score = score;
            best_lag = lag;
        }
    }
    if best_lag == 0 || best_score <= 0.0 {
        return None;
    }
    let (y0, y1, y2) = (acf[best_lag - 1], acf[best_lag], acf[best_lag + 1]);
    let curvature = y0 - 2.0 * y1 + y2;
    let shift = if curvature < 0.0 { (0.5 * (y0 - y2) / curvature).clamp(-0.5, 0.5) } else { 0.0 };
    let mut bpm = 60.0 * fps / (best_lag as f32 + shift);
    while bpm < 78.0 {
        bpm *= 2.0;
    }
    while bpm > 168.0 {
        bpm /= 2.0;
    }
    let mean = total / (max_lag - min_lag + 1) as f32;
    let confidence = if mean > 0.0 { ((acf[best_lag] / mean - 1.2) / 2.5).clamp(0.0, 1.0) } else { 0.0 };
    Some((bpm, confidence))
}

/// Ellis dynamic-programming beat tracker; beats in ms.
pub(crate) fn track_beats(onset: &[f32], bpm: f32) -> Vec<i64> {
    let period = 60.0 * FRAMES_PER_SECOND / bpm;
    let n = onset.len();
    let mut score = vec![0f32; n];
    let mut back = vec![-1i64; n];
    let tightness = 400f32;
    let longest = (2.0 * period).round() as usize;
    let shortest = ((period / 2.0).round() as usize).max(1);
    let penalty: Vec<f32> = (0..=longest)
        .map(|d| {
            if d == 0 {
                0.0
            } else {
                let l = (d as f32 / period).ln();
                -tightness * l * l
            }
        })
        .collect();
    for t in 0..n {
        let mut best = 0f32;
        let mut best_from: i64 = -1;
        let from = t.saturating_sub(longest);
        if t >= shortest {
            let to = t - shortest;
            for tau in from..=to {
                let candidate = score[tau] + penalty[t - tau];
                if best_from < 0 || candidate > best {
                    best = candidate;
                    best_from = tau as i64;
                }
            }
        }
        score[t] = onset[t] + if best_from >= 0 { best.max(0.0) } else { 0.0 };
        back[t] = if best_from >= 0 && best > 0.0 { best_from } else { -1 };
    }
    let mut t = n.saturating_sub(period.round() as usize);
    let mut end = t;
    let mut end_score = -1f32;
    while t < n {
        if score[t] > end_score {
            end_score = score[t];
            end = t;
        }
        t += 1;
    }
    let mut frames = Vec::new();
    let mut cursor = end as i64;
    while cursor >= 0 {
        frames.push(cursor as usize);
        cursor = back[cursor as usize];
    }
    frames.reverse();
    frames
        .iter()
        .map(|&f| {
            let y0 = if f > 0 { onset[f - 1] } else { 0.0 };
            let y1 = onset[f];
            let y2 = onset.get(f + 1).copied().unwrap_or(0.0);
            let curvature = y0 - 2.0 * y1 + y2;
            let shift = if curvature < 0.0 { (0.5 * (y0 - y2) / curvature).clamp(-0.5, 0.5) } else { 0.0 };
            frame_to_ms(f as f32 + shift)
        })
        .collect()
}

fn pulse_strength(beats: &[i64], onset: &[f32]) -> f32 {
    if beats.is_empty() || onset.is_empty() {
        return 0.0;
    }
    let last = onset.len() as i32 - 1;
    let sum: f32 = beats.iter().map(|&b| onset[ms_to_frame(b).clamp(0, last) as usize]).sum();
    let on_beat = sum / beats.len() as f32;
    let overall = (onset.iter().sum::<f32>() / onset.len() as f32).max(1e-4);
    ((on_beat / overall - 1.0) / 3.0).clamp(0.0, 1.0)
}

fn estimate_key(chroma: &[f32; 12]) -> (Option<String>, f32) {
    if chroma.iter().sum::<f32>() <= 0.0 {
        return (None, 0.0);
    }
    let mut best_label = None;
    let mut best_score = -1f32;
    for root in 0..12 {
        for (suffix, template) in [(" major", &MAJOR_PROFILE), (" minor", &MINOR_PROFILE)] {
            let s = correlation(chroma, template, root);
            if s > best_score {
                best_score = s;
                best_label = Some(format!("{}{}", NOTE_NAMES[root], suffix));
            }
        }
    }
    (best_label, best_score.clamp(0.0, 1.0))
}

fn correlation(chroma: &[f32; 12], template: &[f32; 12], rotation: usize) -> f32 {
    let x_mean = chroma.iter().sum::<f32>() / 12.0;
    let y_mean = template.iter().sum::<f32>() / 12.0;
    let (mut num, mut xe, mut ye) = (0f32, 0f32, 0f32);
    for i in 0..12 {
        let x = chroma[(i + rotation) % 12] - x_mean;
        let y = template[i] - y_mean;
        num += x * y;
        xe += x * x;
        ye += y * y;
    }
    if xe == 0.0 || ye == 0.0 { 0.0 } else { num / (xe * ye).sqrt() }
}

/// Where sound starts and stops: 45 dB under the loudest frame.
fn silence_bounds(rms: &[f32], analyzed_ms: i64) -> (i64, i64) {
    let threshold = rms.iter().copied().fold(0f32, f32::max) * 0.0056;
    let first = rms.iter().position(|&v| v > threshold).unwrap_or(0);
    let last = rms.iter().rposition(|&v| v > threshold).unwrap_or(first).max(first);
    (frame_to_ms(first as f32), frame_to_ms(last as f32 + 1.0).min(analyzed_ms))
}

/// Per second: loudness vs the track's peak (0..1), bass share, brightness.
fn per_second_curves(rms: &[f32], low: &[f32], centroid: &[f32]) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let seconds = ((rms.len() as f32 / FRAMES_PER_SECOND) as usize).max(1);
    let mut db = vec![0f32; seconds];
    let mut bass = vec![0f32; seconds];
    let mut bright = vec![0f32; seconds];
    for s in 0..seconds {
        let from = ((s as f32 * FRAMES_PER_SECOND) as usize).min(rms.len());
        let to = (((s + 1) as f32 * FRAMES_PER_SECOND) as usize).min(rms.len());
        let (mut e, mut l, mut c) = (0f64, 0f32, 0f32);
        for f in from..to {
            e += (rms[f] * rms[f]) as f64;
            l += low[f];
            c += centroid[f];
        }
        let count = (to - from).max(1);
        db[s] = (10.0 * (e / count as f64 + 1e-12).log10()) as f32;
        bass[s] = (l / count as f32 * 2.5).clamp(0.0, 1.0);
        bright[s] = ((c / count as f32 - 600.0) / 3_400.0).clamp(0.0, 1.0);
    }
    let peak = db.iter().copied().fold(f32::MIN, f32::max);
    let loud = db.iter().map(|&d| ((d - (peak - 24.0)) / 24.0).clamp(0.0, 1.0)).collect();
    (loud, bass, bright)
}

fn overall_energy(rms: &[f32], onset: &[f32]) -> f32 {
    let mut sorted = rms.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let loud = &sorted[(sorted.len() as f32 * 0.4) as usize..];
    let ms = loud.iter().map(|&v| (v * v) as f64).sum::<f64>() / loud.len().max(1) as f64;
    let db = 10.0 * (ms + 1e-12).log10();
    let loudness = (((db + 30.0) / 22.0) as f32).clamp(0.0, 1.0);
    let seconds = onset.len() as f32 / FRAMES_PER_SECOND;
    let density = (onset.iter().filter(|&&v| v > 0.3).count() as f32 / seconds / 6.0).clamp(0.0, 1.0);
    (loudness * 0.65 + density * 0.35).clamp(0.0, 1.0)
}

fn average_of_loud(values: &[f32], loudness: &[f32]) -> f32 {
    let (mut sum, mut count) = (0f32, 0usize);
    for (i, &v) in values.iter().enumerate() {
        if loudness.get(i).copied().unwrap_or(0.0) > 0.5 {
            sum += v;
            count += 1;
        }
    }
    if count == 0 {
        if values.is_empty() { 0.5 } else { values.iter().sum::<f32>() / values.len() as f32 }
    } else {
        sum / count as f32
    }
}

/// Each value placed between the 20th and 95th percentile.
fn normalised(values: &[f32]) -> Vec<f32> {
    if values.is_empty() {
        return Vec::new();
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let at = |q: f32| sorted[((sorted.len() as f32 * q) as usize).min(sorted.len() - 1)];
    let (low, high) = (at(0.2), at(0.95));
    let span = high - low;
    if span <= 1e-6 {
        return vec![0.0; values.len()];
    }
    values.iter().map(|&v| ((v - low) / span).clamp(0.0, 1.0)).collect()
}

fn vocal_curve(lead: &[f32], share: &[f32], tonal: &[f32], rms: &[f32]) -> Vec<f32> {
    let seconds = ((lead.len() as f32 / FRAMES_PER_SECOND) as usize).max(1);
    let mut lead_db = vec![0f32; seconds];
    let mut share_mean = vec![0f32; seconds];
    let mut tonal_mean = vec![0f32; seconds];
    let mut quiet = vec![0f32; seconds];
    let peak_rms = rms.iter().copied().fold(0f32, f32::max).max(1e-6);
    for s in 0..seconds {
        let from = ((s as f32 * FRAMES_PER_SECOND) as usize).min(lead.len());
        let to = (((s + 1) as f32 * FRAMES_PER_SECOND) as usize).min(lead.len());
        let (mut power, mut sh, mut to_sum, mut level) = (0f64, 0f32, 0f32, 0f32);
        for f in from..to {
            power += lead[f] as f64;
            sh += share[f];
            to_sum += tonal[f];
            level = level.max(rms[f]);
        }
        let count = (to - from).max(1);
        lead_db[s] = (10.0 * (power / count as f64 + 1e-12).log10()) as f32;
        share_mean[s] = sh / count as f32;
        tonal_mean[s] = to_sum / count as f32;
        quiet[s] = if level < peak_rms * 0.03 { 0.0 } else { 1.0 };
    }
    let (ln, sn, tn) = (normalised(&lead_db), normalised(&share_mean), normalised(&tonal_mean));
    let raw: Vec<f32> = (0..seconds).map(|i| (0.45 * ln[i] + 0.35 * sn[i] + 0.2 * tn[i]) * quiet[i]).collect();
    (0..seconds)
        .map(|s| {
            let a = raw[s.saturating_sub(1)];
            let b = raw[s];
            let c = raw[(s + 1).min(seconds - 1)];
            ((a + 2.0 * b + c) / 4.0).clamp(0.0, 1.0)
        })
        .collect()
}

/// Key names to pitch classes and Camelot distances (`Harmony`).
pub mod harmony {
    use super::NOTE_NAMES;

    pub fn pitch_class(key: Option<&str>) -> Option<usize> {
        let root = key?.split(' ').next()?;
        NOTE_NAMES.iter().position(|n| *n == root)
    }

    /// 0 same key, 1 neighbour / relative, more = clash.
    pub fn camelot_distance(a: Option<&str>, b: Option<&str>) -> Option<u32> {
        let (na, ma) = camelot(a)?;
        let (nb, mb) = camelot(b)?;
        let d = (na as i32 - nb as i32).unsigned_abs();
        let ring = d.min(12 - d);
        Some(if ma == mb { ring } else if ring == 0 { 1 } else { ring + 1 })
    }

    fn camelot(key: Option<&str>) -> Option<(usize, bool)> {
        let pc = pitch_class(key)?;
        let minor = key?.ends_with("minor");
        let major_root = if minor { (pc + 3) % 12 } else { pc };
        Some(((major_root * 7) % 12, minor))
    }
}
