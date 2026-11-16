//! Which beat starts each bar and where sections change (port of
//! `CrossmixBars.kt`): a 4-state Viterbi over harmony change, texture
//! change, backbeat (kick vs snare) and bass accent per beat; sections are
//! bars where loudness / bass / brightness / density clearly move.

const HARMONY_WEIGHT: f32 = 1.0;
const THREE_SHARE: f32 = 0.35;
const BACKBEAT_WEIGHT: f32 = 0.6;
const TEXTURE_WEIGHT: f32 = 0.5;
const ACCENT_WEIGHT: f32 = 0.3;
const JUMP_COST: f32 = 8.0;
const MIN_SECTION_CHANGE: f32 = 2.5;
const SPREAD_FLOORS: [f32; 4] = [1.0, 0.02, 150.0, 0.02];

/// Per-frame analyser output (`chroma` holds 12 values per frame).
pub struct Frames<'a> {
    pub onset: &'a [f32],
    pub kick: &'a [f32],
    pub snap: &'a [f32],
    pub chroma: &'a [f32],
    pub rms: &'a [f32],
    pub low_share: &'a [f32],
    pub centroid_hz: &'a [f32],
}

/// `bars`: beat index of each bar start; `sections`: bar index of each section start.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    pub bars: Vec<usize>,
    pub sections: Vec<usize>,
}

pub fn find(beat_frames: &[i32], frames: &Frames) -> Found {
    if beat_frames.len() < 8 || frames.rms.is_empty() {
        return Found::default();
    }
    let period = median_gap(beat_frames);
    let beats = BeatSound::new(beat_frames, period, frames);
    let position = bar_positions(beat_frames, period, &beats);
    let bars: Vec<usize> = position.iter().enumerate().filter(|(_, p)| **p == 0).map(|(i, _)| i).collect();
    let sections = section_starts(&bars, &beats);
    Found { bars, sections }
}

fn mean(v: &[f32]) -> f32 {
    if v.is_empty() { 0.0 } else { v.iter().sum::<f32>() / v.len() as f32 }
}

fn bar_positions(beat_frames: &[i32], period: f32, beats: &BeatSound) -> Vec<usize> {
    let n = beats.size;
    let harmony = harmony_change(beats);
    let texture = texture_change(beats);
    let drum_scale = 2.0 / (mean(&beats.kick) + mean(&beats.snap)).max(1e-6);
    let accent_scale = 1.0 / mean(&beats.accent).max(1e-9);
    let mut fit = vec![0f32; n * 4];
    for i in 0..n {
        let backbeat = ((beats.kick[i] - beats.snap[i]) * drum_scale).clamp(-2.0, 2.0) * BACKBEAT_WEIGHT;
        let one = HARMONY_WEIGHT * harmony[i] + TEXTURE_WEIGHT * texture[i] + ACCENT_WEIGHT * beats.accent[i] * accent_scale;
        fit[i * 4] = one + backbeat;
        fit[i * 4 + 1] = -backbeat;
        fit[i * 4 + 2] = HARMONY_WEIGHT * THREE_SHARE * harmony[i] + backbeat;
        fit[i * 4 + 3] = -backbeat;
    }
    let mut score = [fit[0], fit[1], fit[2], fit[3]];
    let mut next = [0f32; 4];
    let mut back = vec![0usize; n * 4];
    for i in 1..n {
        let step = (((beat_frames[i] - beat_frames[i - 1]) as f32 / period).round() as i32).max(1);
        let mut leader = 0;
        for k in 1..4 {
            if score[k] > score[leader] {
                leader = k;
            }
        }
        let jump = score[leader] - JUMP_COST;
        for k in 0..4 {
            let from = (k as i32 - step).rem_euclid(4) as usize;
            let kept = score[from] >= jump;
            next[k] = (if kept { score[from] } else { jump }) + fit[i * 4 + k];
            back[i * 4 + k] = if kept { from } else { leader };
        }
        score = next;
    }
    let mut position = vec![0usize; n];
    let mut state = (0..4).max_by(|&a, &b| score[a].total_cmp(&score[b])).unwrap_or(0);
    for i in (0..n).rev() {
        position[i] = state;
        if i > 0 {
            state = back[i * 4 + state];
        }
    }
    position
}

fn harmony_change(beats: &BeatSound) -> Vec<f32> {
    let n = beats.size;
    let rooted: Vec<f32> = beats.chroma.iter().map(|v| v.sqrt()).collect();
    let mut change = vec![0f32; n];
    let mut before = [0f32; 12];
    let mut after = [0f32; 12];
    for i in 2..n.saturating_sub(1) {
        for k in 0..12 {
            before[k] = rooted[(i - 2) * 12 + k] + rooted[(i - 1) * 12 + k];
            after[k] = rooted[i * 12 + k] + rooted[(i + 1) * 12 + k];
        }
        change[i] = 1.0 - cosine(&before, &after);
    }
    let usual = mean(&change).max(0.03);
    change.iter_mut().for_each(|c| *c /= usual);
    change
}

fn texture_change(beats: &BeatSound) -> Vec<f32> {
    let db = beats.decibels();
    let change = sound_change(&[&db, &beats.low, &beats.bright, &beats.density], 4);
    (0..change.len())
        .map(|i| if change[i] >= MIN_SECTION_CHANGE && is_peak(&change, i, 2) { (change[i] - 1.5).min(6.0) } else { 0.0 })
        .collect()
}

fn section_starts(bars: &[usize], beats: &BeatSound) -> Vec<usize> {
    let count = bars.len();
    if count < 8 {
        return Vec::new();
    }
    let mut power = vec![0f32; count];
    let mut low = vec![0f32; count];
    let mut bright = vec![0f32; count];
    let mut density = vec![0f32; count];
    for b in 0..count {
        let from = bars[b];
        let to = if b + 1 < count { bars[b + 1] } else { beats.size.min(from + 4) };
        for i in from..to {
            power[b] += beats.power[i];
            low[b] += beats.low[i];
            bright[b] += beats.bright[i];
            density[b] += beats.density[i];
        }
        let k = (to.saturating_sub(from)).max(1) as f32;
        power[b] /= k;
        low[b] /= k;
        bright[b] /= k;
        density[b] /= k;
    }
    let db: Vec<f32> = power.iter().map(|p| (10.0 * (*p as f64 + 1e-12).log10()) as f32).collect();
    let change = sound_change(&[&db, &low, &bright, &density], 4);
    let mut order: Vec<usize> = (2..=count - 2).collect();
    order.sort_by(|&a, &b| change[b].total_cmp(&change[a]));
    let mut picked: Vec<usize> = Vec::new();
    for b in order {
        if change[b] < MIN_SECTION_CHANGE {
            break;
        }
        if is_peak(&change, b, 2) && picked.iter().all(|&p| p.abs_diff(b) >= 4) {
            picked.push(b);
        }
    }
    picked.sort_unstable();
    picked
}

fn sound_change(series: &[&[f32]], window: usize) -> Vec<f32> {
    let n = series[0].len();
    let mut change = vec![0f32; n];
    for (index, values) in series.iter().enumerate() {
        let spread = standard_deviation(values).max(SPREAD_FLOORS[index]);
        let mut prefix = vec![0f64; n + 1];
        for i in 0..n {
            prefix[i + 1] = prefix[i] + values[i] as f64;
        }
        for i in 0..n {
            let w = window.min(i.min(n - i));
            if w < 2 {
                continue;
            }
            let before = (prefix[i] - prefix[i - w]) / w as f64;
            let after = (prefix[i + w] - prefix[i]) / w as f64;
            change[i] += ((after - before).abs() / spread as f64) as f32;
        }
    }
    change
}

struct BeatSound {
    size: usize,
    kick: Vec<f32>,
    snap: Vec<f32>,
    accent: Vec<f32>,
    power: Vec<f32>,
    low: Vec<f32>,
    bright: Vec<f32>,
    density: Vec<f32>,
    chroma: Vec<f32>,
}

impl BeatSound {
    fn new(beat_frames: &[i32], period: f32, f: &Frames) -> Self {
        let size = beat_frames.len();
        let last = f.rms.len() as i32 - 1;
        let mut s = BeatSound {
            size,
            kick: vec![0.0; size],
            snap: vec![0.0; size],
            accent: vec![0.0; size],
            power: vec![0.0; size],
            low: vec![0.0; size],
            bright: vec![0.0; size],
            density: vec![0.0; size],
            chroma: vec![0.0; size * 12],
        };
        let peak_near = |values: &[f32], at: i32| -> f32 {
            let hi = (values.len() as i32 - 1).min(at + 2);
            let mut best = 0f32;
            let mut fr = (at - 2).max(0);
            while fr <= hi {
                best = best.max(values[fr as usize]);
                fr += 1;
            }
            best
        };
        for i in 0..size {
            let at = beat_frames[i].clamp(0, last);
            s.kick[i] = peak_near(f.kick, at);
            s.snap[i] = peak_near(f.snap, at);
            for fr in (at - 1).max(0)..=(last.min(at + 4)) {
                let fr = fr as usize;
                s.accent[i] = s.accent[i].max(f.rms[fr] * f.low_share[fr].sqrt());
            }
            let until = if i + 1 < size { beat_frames[i + 1] } else { (at as f32 + period).round() as i32 };
            let until = until.clamp(at + 1, last + 1);
            for fr in at..until {
                let fr = fr as usize;
                s.power[i] += f.rms[fr] * f.rms[fr];
                s.low[i] += f.low_share[fr];
                s.bright[i] += f.centroid_hz[fr];
                s.density[i] += f.onset[fr];
                for k in 0..12 {
                    s.chroma[i * 12 + k] += f.chroma.get(fr * 12 + k).copied().unwrap_or(0.0);
                }
            }
            let count = (until - at) as f32;
            s.power[i] /= count;
            s.low[i] /= count;
            s.bright[i] /= count;
            s.density[i] /= count;
        }
        s
    }

    fn decibels(&self) -> Vec<f32> {
        self.power.iter().map(|p| (10.0 * (*p as f64 + 1e-12).log10()) as f32).collect()
    }
}

fn median_gap(beat_frames: &[i32]) -> f32 {
    let mut gaps: Vec<i32> = beat_frames.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.sort_unstable();
    gaps[gaps.len() / 2].max(1) as f32
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let (mut dot, mut aa, mut bb) = (0f32, 0f32, 0f32);
    for k in 0..a.len() {
        dot += a[k] * b[k];
        aa += a[k] * a[k];
        bb += b[k] * b[k];
    }
    if aa <= 0.0 || bb <= 0.0 { 1.0 } else { dot / (aa * bb).sqrt() }
}

fn is_peak(values: &[f32], at: usize, reach: usize) -> bool {
    let lo = at.saturating_sub(reach);
    let hi = (values.len() - 1).min(at + reach);
    (lo..=hi).all(|i| values[i] <= values[at])
}

fn standard_deviation(values: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let m = values.iter().map(|&v| v as f64).sum::<f64>() / values.len() as f64;
    (values.iter().map(|&v| (v as f64 - m) * (v as f64 - m)).sum::<f64>() / values.len() as f64).sqrt() as f32
}
