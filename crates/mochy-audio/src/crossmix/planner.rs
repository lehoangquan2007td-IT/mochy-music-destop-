//! The crossmix planner (port of `CrossmixPlanner.kt`): scores every style ×
//! length × hand-over bar × landing point for a pair of songs and returns
//! the best plan. Deterministic: the same two songs always get the same mix.
//!
//! Desktop has no synced lyrics at the engine level, so voices come from the
//! analyser's vocal curve (Android's fallback when a song has no LRC).

use super::analysis::{LocalGrid, TrackAnalysis, harmony};
use super::styles::{FxParams, Role, Style, StyleUse};
use crate::transition::CrossmixOptions;

/// One planned hand-over on a clock counted in the outgoing song's beats:
/// beat 0 is `out_start_ms`, the bass changes hands and the next song's
/// landing (`in_cue_ms`) is heard at `swap_beat`, the outgoing song is gone
/// by `length_beats`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
    pub style: Style,
    pub variation: u32,
    pub length_beats: u32,
    pub swap_beat: u32,
    pub out_start_ms: i64,
    pub out_beat_ms: f64,
    pub in_cue_ms: i64,
    /// Incoming media ms per outgoing beat.
    pub in_media_per_beat: f64,
    /// Incoming speed during the mix vs the listener's own.
    pub in_speed: f32,
    /// Outgoing speed from just before the mix.
    pub out_speed: f32,
    pub beat_matched: bool,
    /// The transition beat the incoming lane starts on.
    pub in_start_beat: f32,
    /// 0..1 how far the two voices get bands of their own.
    pub yield_amount: f32,
}

impl Plan {
    pub fn in_media_at(&self, beat: f32) -> f64 {
        self.in_cue_ms as f64 + (beat as f64 - self.swap_beat as f64) * self.in_media_per_beat
    }
    pub fn in_start_ms(&self) -> i64 {
        (self.in_media_at(self.in_start_beat) as i64).max(0)
    }
    pub fn out_media_at(&self, beat: f32) -> f64 {
        self.out_start_ms as f64 + beat as f64 * self.out_beat_ms
    }
    /// Last beat any outgoing sound (tails included) is rendered on.
    pub fn end_beat(&self) -> f32 {
        (self.length_beats + self.style.tail_beats()) as f32 + 1.0
    }

    /// A plain equal-power crossfade of `secs` starting at `duration − secs`
    /// (`MusicService.scheduleCrossfade`), counted as 16 pseudo-beats.
    pub fn plain(duration_ms: i64, secs: f32) -> Option<Plan> {
        let len_ms = (secs.max(0.1) * 1000.0) as i64;
        if duration_ms <= len_ms {
            return None;
        }
        let beats = 16u32;
        let beat_ms = len_ms as f64 / beats as f64;
        Some(Plan {
            style: Style::PlainFade,
            variation: 0,
            length_beats: beats,
            swap_beat: 0,
            out_start_ms: duration_ms - len_ms,
            out_beat_ms: beat_ms,
            in_cue_ms: 0,
            in_media_per_beat: beat_ms,
            in_speed: 1.0,
            out_speed: 1.0,
            beat_matched: false,
            in_start_beat: 0.0,
            yield_amount: 0.0,
        })
    }
}

/// What the planner sees of each song (`duration_ms` 0 when unknown).
#[derive(Debug, Clone, Copy)]
pub struct TrackInput<'a> {
    pub duration_ms: i64,
    pub analysis: Option<&'a TrackAnalysis>,
}

pub const MAX_LEAD_MS: i64 = 30_000;
pub const MAX_ENTRY_MS: i64 = 120_000;
const CLASH_PER_BEAT: f32 = 0.35;
const YIELD_COST: f32 = 0.3;
const MAX_OVERLAP_MS: f64 = 16_000.0;
const MAX_CALM_OVERLAP_MS: f64 = 24_000.0;
const MAX_UNMATCHED_OVERLAP_MS: f64 = 8_500.0;
const MIN_OVERLAP_MS: f64 = 3_500.0;
const MIN_FAST_OVERLAP_MS: f64 = 6_000.0;
const MAX_SKIPPED_MUSIC_S: f32 = 12.0;
const SHARP_ENERGY_CHANGE: f32 = 0.3;
const CALM_ENERGY: f32 = 0.45;
const GESTURE_BONUS: f32 = 0.3;
const ALWAYS_MIX_MIN_OVERLAP_MS: f64 = 6_000.0;
const FORCED_BLEND_MS: f64 = 10_000.0;

struct Tempo {
    factor: f32,
    in_beat_ms: f64,
}

struct Match {
    in_beat_ms: f64,
    in_speed: f32,
    out_speed: f32,
}

struct Exit {
    swap_ms: i64,
    grid: LocalGrid,
}

struct Landing {
    time_ms: i64,
    score: f32,
}

/// Where a song's voice is (0..1), from the analyser's lead curve.
struct Voice<'a> {
    curve: Option<&'a [f32]>,
}

impl Voice<'_> {
    fn at(&self, ms: f64) -> f32 {
        let Some(values) = self.curve.filter(|c| !c.is_empty()) else { return 0.35 };
        let s = (ms / 1000.0) as i64;
        if s >= 0 && (s as usize) < values.len() { values[s as usize] * 0.85 } else { 0.0 }
    }
    fn average(&self, from_ms: f64, to_ms: f64) -> f32 {
        if to_ms <= from_ms {
            return self.at(from_ms);
        }
        let (mut sum, mut count) = (0f32, 0usize);
        let mut t = from_ms;
        while t <= to_ms {
            sum += self.at(t);
            count += 1;
            t += 500.0;
        }
        sum / count.max(1) as f32
    }
}

/// The one transition that suits these two songs best.
pub fn plan(outgoing: TrackInput, incoming: TrackInput, settings: &CrossmixOptions) -> Plan {
    plan_from(outgoing, incoming, settings, 0)
}

/// [`plan`], with the hand-over starting no earlier than `earliest_ms`
/// (desktop: a plan made late, once the song is already in its last 30 s).
pub fn plan_from(outgoing: TrackInput, incoming: TrackInput, settings: &CrossmixOptions, earliest_ms: i64) -> Plan {
    let out = outgoing.analysis;
    let inc = incoming.analysis;
    let natural_end = natural_end(&outgoing);
    let window_start = (outgoing.duration_ms - MAX_LEAD_MS).max(0).max(earliest_ms.min(natural_end));

    let no_plan = || {
        if settings.always_mix {
            forced_blend(&outgoing, &incoming, natural_end, out, window_start)
        } else {
            smooth_fade(&outgoing, &incoming, natural_end, window_start)
        }
    };
    let Some(out) = out.filter(|o| o.is_beat_reliable()) else { return no_plan() };
    let Some(out_grid) = out.local_grid((natural_end - 12_000).max(0)) else { return no_plan() };

    let max_speed_up = settings.max_speed_up;
    let tempo = tempo_factor(out, inc, max_speed_up);
    let key_distance = harmony::camelot_distance(out.key.as_deref(), inc.and_then(|i| i.key.as_deref()))
        .filter(|_| out.key_confidence > 0.3 && inc.map_or(0.0, |i| i.key_confidence) > 0.3);
    let key_clash = key_distance.is_some_and(|d| d >= 3);
    let bars = out.bar_starts_ms();
    let exits = exit_points(out, &bars, out_grid, window_start, natural_end);
    let out_voice = Voice { curve: Some(&out.vocal) };
    let in_voice = Voice { curve: inc.map(|i| i.vocal.as_slice()) };
    let vocal_pair = out_voice.average(window_start as f64, natural_end as f64) * in_voice.average(0.0, 60_000.0);
    let calm_match = tempo.is_some() && out.vibe.energy.max(inc.map_or(0.5, |i| i.vibe.energy)) < CALM_ENERGY;
    let latest_landing = if incoming.duration_ms > 0 { MAX_ENTRY_MS.min(incoming.duration_ms / 4) } else { MAX_ENTRY_MS };
    let gesture = pair_gesture(out, inc);
    let mut scratch_out = FxParams::default();
    let mut scratch_in = FxParams::default();
    let variation = if 60_000.0 / out_grid.period_ms >= 118.0 { 1 } else { 0 };

    let mut best: Option<(Plan, f32)> = None;
    let mut consider = |style: Style, picked: bool, best: &mut Option<(Plan, f32)>| {
        if style.needs_beat_match() && tempo.is_none() {
            return;
        }
        if settings.always_mix && !picked && !style.blends_both() {
            return;
        }
        let mut fit = style_fit(style, out, inc, tempo.is_some(), key_distance, natural_end, vocal_pair);
        if picked {
            fit = fit.max(0.0);
        } else if fit <= -5.0 {
            return;
        }
        let bonus = if tempo.is_none() && style == gesture { GESTURE_BONUS } else { 0.0 };
        let locked = tempo.is_some() && (style.needs_beat_match() || !style.enters_at_end());
        for &length in style.lengths() {
            if !style.enters_at_end() && !overlap_fits(length, length as f64 * out_grid.period_ms, tempo.is_some(), calm_match, settings) {
                continue;
            }
            let swap = style.swap_at(length);
            for exit in &exits {
                let period = exit.grid.period_ms;
                let start = (exit.swap_ms as f64 - swap as f64 * period) as i64;
                let end = (start as f64 + length as f64 * period) as i64;
                if start < window_start || end > natural_end - 150 {
                    continue;
                }
                let Some(exit_score) = exit_score(out, &bars, exit.grid, exit.swap_ms, end, natural_end, style) else { continue };
                let Some(landing) =
                    pick_landing(style, out, inc, exit.swap_ms, swap, period, tempo.as_ref(), locked, &out_voice, &in_voice, latest_landing)
                else {
                    continue;
                };
                let m = if locked { inc.zip(tempo.as_ref()).and_then(|(i, t)| beat_match(i, landing.time_ms, t, period, max_speed_up)) } else { None };
                if style.needs_beat_match() && m.is_none() {
                    continue;
                }
                let in_beat = m.as_ref().map_or(period, |m| m.in_beat_ms);
                let heard_from = if style.enters_at_end() { length as f32 } else { 0.0 };
                let heard_ms = landing.time_ms as f64 + (heard_from as f64 - swap as f64) * in_beat;
                if heard_ms < -1.0 {
                    continue;
                }
                let pre_roll = 4f32.min((heard_ms / in_beat) as f32).max(0.0);
                let plan = Plan {
                    style,
                    variation,
                    length_beats: length,
                    swap_beat: swap,
                    out_start_ms: start,
                    out_beat_ms: period,
                    in_cue_ms: landing.time_ms,
                    in_media_per_beat: in_beat,
                    in_speed: m.as_ref().map_or(1.0, |m| m.in_speed),
                    out_speed: m.as_ref().map_or(1.0, |m| m.out_speed),
                    beat_matched: m.is_some(),
                    in_start_beat: heard_from - pre_roll,
                    yield_amount: 0.0,
                };
                let length_fit = length_fit(style, length, out, inc, key_clash);
                let mut chosen = plan;
                let (clash_beats, shared) = vocal_clash(&plan, &out_voice, &in_voice, &mut scratch_out, &mut scratch_in);
                let mut clash = clash_beats * CLASH_PER_BEAT;
                if clash > 0.08 && !style.enters_at_end() && style != Style::FarAway && style != Style::Dampen {
                    let yielding = Plan { yield_amount: (shared / 0.5).clamp(0.35, 1.0), ..plan };
                    let y = vocal_clash(&yielding, &out_voice, &in_voice, &mut scratch_out, &mut scratch_in).0 * CLASH_PER_BEAT + YIELD_COST;
                    if y < clash {
                        chosen = yielding;
                        clash = y;
                    }
                }
                let score = fit + bonus + exit_score + landing.score + length_fit - clash;
                if better(&chosen, score, best.as_ref()) {
                    *best = Some((chosen, score));
                }
            }
        }
    };

    if let Some(style) = settings.style {
        consider(style, true, &mut best);
    }
    if best.is_none() {
        for style in Style::ALL {
            if style.style_use() == StyleUse::Flavour {
                continue;
            }
            if !settings.stunts && style.style_use() != StyleUse::Everyday {
                continue;
            }
            consider(style, false, &mut best);
        }
    }
    best.map(|b| b.0).unwrap_or_else(no_plan)
}

/// Best score wins; a dead heat goes to the longer, then earlier-listed
/// style, then the earlier start.
fn better(plan: &Plan, score: f32, best: Option<&(Plan, f32)>) -> bool {
    let Some((b, bs)) = best else { return true };
    if score != *bs {
        return score > *bs;
    }
    if plan.length_beats != b.length_beats {
        return plan.length_beats > b.length_beats;
    }
    let ord = |s: Style| Style::ALL.iter().position(|x| *x == s).unwrap_or(99);
    if ord(plan.style) != ord(b.style) {
        return ord(plan.style) < ord(b.style);
    }
    plan.out_start_ms < b.out_start_ms
}

fn clarity(p: &FxParams) -> f32 {
    let words = ((p.high_cut_hz / 400.0).ln() / (3_500f32 / 400.0).ln()).clamp(0.0, 1.0);
    let body = 1.0 - ((p.low_cut_hz.max(20.0) / 900.0).ln() / (3_200f32 / 900.0).ln()).clamp(0.0, 1.0);
    (p.gain * p.mid_band.clamp(0.0, 1.2) * words * body).max(0.0)
}

fn vocal_clash(plan: &Plan, out_voice: &Voice, in_voice: &Voice, a: &mut FxParams, b: &mut FxParams) -> (f32, f32) {
    let style = plan.style;
    let length = plan.length_beats as f32;
    let swap = plan.swap_beat as f32;
    let last = length + style.tail_beats() as f32;
    let (mut clash, mut shared) = (0f32, 0usize);
    let mut beat = 0f32;
    while beat <= last {
        style.evaluate(Role::Outgoing, beat, length, swap, plan.variation, a);
        style.evaluate(Role::Incoming, beat, length, swap, plan.variation, b);
        if plan.yield_amount > 0.0 {
            style.yield_vocals(Role::Outgoing, beat, length, swap, plan.yield_amount, a);
            style.yield_vocals(Role::Incoming, beat, length, swap, plan.yield_amount, b);
        }
        let out_sings = out_voice.at(plan.out_media_at(beat));
        let in_sings = in_voice.at(plan.in_media_at(beat));
        if beat < length && out_sings > 0.5 && in_sings > 0.5 {
            shared += 1;
        }
        let both = clarity(a) * clarity(b);
        if both > 0.02 {
            clash += both * out_sings * in_sings;
        }
        beat += 1.0;
    }
    (clash, shared as f32 / length.max(1.0))
}

/// How much two songs could pass for one piece (0..1).
pub(crate) fn continuity(out: &TrackAnalysis, inc: Option<&TrackAnalysis>, matchable: bool, key_distance: Option<u32>) -> f32 {
    let Some(inc) = inc.filter(|_| matchable) else { return 0.0 };
    let energy = 1.0 - ((inc.vibe.energy - out.vibe.energy).abs() / 0.35).clamp(0.0, 1.0);
    let groove = out.vibe.danceability.min(inc.vibe.danceability).clamp(0.0, 1.0);
    let key = match key_distance {
        None => 0.6,
        Some(d) if d <= 1 => 1.0,
        Some(2) => 0.5,
        Some(_) => 0.0,
    };
    let bass = 1.0 - (inc.vibe.bass_weight - out.vibe.bass_weight).abs().clamp(0.0, 1.0);
    (energy * 0.35 + groove * 0.3 + key * 0.25 + bass * 0.1).clamp(0.0, 1.0)
}

fn tempo_factor(out: &TrackAnalysis, inc: Option<&TrackAnalysis>, max_speed_up: f32) -> Option<Tempo> {
    let inc = inc.filter(|i| i.is_beat_reliable())?;
    let out_bpm = out.bpm?;
    let in_bpm = inc.bpm?;
    for factor in [1.0f32, 2.0, 0.5] {
        let ratio = out_bpm / (in_bpm * factor);
        if ratio >= 1.0 / (1.0 + max_speed_up) && ratio <= 1.0 + max_speed_up {
            return Some(Tempo { factor, in_beat_ms: 60_000.0 / (in_bpm * factor) as f64 });
        }
    }
    None
}

fn beat_match(inc: &TrackAnalysis, landing_ms: i64, tempo: &Tempo, out_beat_ms: f64, max_speed_up: f32) -> Option<Match> {
    let local = inc.local_grid(landing_ms)?.period_ms;
    let in_beat = local / tempo.factor as f64;
    let ratio = (in_beat / out_beat_ms) as f32;
    if (ratio - 1.0).abs() < 0.003 {
        return Some(Match { in_beat_ms: in_beat, in_speed: 1.0, out_speed: 1.0 });
    }
    if ratio > 1.0 {
        return if ratio - 1.0 > max_speed_up { None } else { Some(Match { in_beat_ms: in_beat, in_speed: ratio, out_speed: 1.0 }) };
    }
    let out_up = 1.0 / ratio;
    if out_up - 1.0 > max_speed_up { None } else { Some(Match { in_beat_ms: in_beat, in_speed: 1.0, out_speed: out_up }) }
}

/// The unmatched move this pair leans to, picked from tempos and keys.
fn pair_gesture(out: &TrackAnalysis, inc: Option<&TrackAnalysis>) -> Style {
    // a stable string hash (Java's), so the same pair always picks the same gesture
    fn java_hash(s: &str) -> i32 {
        s.encode_utf16().fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
    }
    let mut hash: i32 = 17;
    hash = hash.wrapping_mul(31).wrapping_add((out.bpm.unwrap_or(0.0) * 10.0).round() as i32);
    hash = hash.wrapping_mul(31).wrapping_add((inc.and_then(|i| i.bpm).unwrap_or(0.0) * 10.0).round() as i32);
    hash = hash.wrapping_mul(31).wrapping_add(out.key.as_deref().map_or(0, java_hash));
    hash = hash.wrapping_mul(31).wrapping_add(inc.and_then(|i| i.key.as_deref()).map_or(0, java_hash));
    const GESTURES: [Style; 3] = [Style::FilterSweep, Style::SilkBlend, Style::EchoOut];
    GESTURES[hash.rem_euclid(3) as usize]
}

fn style_fit(
    style: Style,
    out: &TrackAnalysis,
    inc: Option<&TrackAnalysis>,
    matchable: bool,
    key_distance: Option<u32>,
    natural_end: i64,
    vocal_pair: f32,
) -> f32 {
    let out_energy = out.energy_around(natural_end - 10_000, 16_000);
    let in_energy = inc.map_or(0.5, |i| i.vibe.energy);
    let delta = in_energy - out.vibe.energy;
    if style.style_use() == StyleUse::Lift && delta < SHARP_ENERGY_CHANGE {
        return -10.0;
    }
    if style.style_use() == StyleUse::Fall && -delta < SHARP_ENERGY_CHANGE {
        return -10.0;
    }
    let dance = out.vibe.danceability.min(inc.map_or(0.4, |i| i.vibe.danceability));
    let calm = 1.0 - out.vibe.energy.max(in_energy);
    let clash = key_distance.is_some_and(|d| d >= 3);
    let harmonic = key_distance.is_some_and(|d| d <= 1);
    // desktop has no lyric lines at the engine: "vocals near the end" comes from the vocal curve
    let vocals_near_end = out.vocal.len() as i64 > (natural_end - 8_000) / 1000
        && (((natural_end - 8_000).max(0) / 1000) as usize..out.vocal.len().min((natural_end / 1000).max(0) as usize))
            .any(|s| out.vocal[s] > 0.6);
    let outro = out_energy < 0.45;
    let overlaps = !style.enters_at_end();
    let continuous = continuity(out, inc, matchable, key_distance);
    let um = if !matchable { 1.0 } else { 0.0 };
    let mut score = match style {
        Style::SilkBlend => 0.55 + dance * 0.3 + calm * 0.2 + um * 0.25,
        Style::BassSwap => 0.4 + dance * 1.4 + out.vibe.bass_weight * 0.4,
        Style::FilterSweep => 0.5 + dance * 0.2 + um * 0.3 + if clash { 0.3 } else { 0.0 },
        Style::EchoOut => 0.38 + um * 0.3 + (-delta).max(0.0) * 0.4 + if vocals_near_end { 0.15 } else { 0.0 },
        Style::VocalHandoff => 0.1 + if vocals_near_end { 0.9 } else { -0.6 } + um * 0.3,
        Style::DropCut => 0.1 + dance * 0.9 + in_energy * 0.5 + delta.max(0.0) * 0.4,
        Style::HarmonicGlide => 0.2 + if harmonic { 1.1 } else { -0.8 } + dance * 0.3,
        Style::FarAway => 0.05 + vocal_pair * 1.5 + calm * 0.5 + (1.0 - dance) * 0.2,
        Style::Dampen => 0.05 + vocal_pair * 1.4 + dance * 0.1,
        Style::HighPassLift => 0.3 + delta * 1.4 + dance * 0.3,
        Style::ThunderDrop => 0.1 + delta * 1.3 + out.vibe.bass_weight * 0.4,
        _ => -10.0,
    };
    if overlaps && clash && style != Style::FilterSweep {
        score -= 0.6;
    }
    if outro && style.enters_at_end() && style != Style::VocalHandoff && style != Style::EchoOut {
        score -= 0.3;
    }
    score += match style {
        Style::BassSwap | Style::HarmonicGlide => continuous * 1.6,
        Style::SilkBlend => continuous,
        Style::FilterSweep => continuous * 0.5,
        Style::DropCut | Style::ThunderDrop | Style::HighPassLift => -continuous * 1.3,
        _ => -continuous * 0.4,
    };
    score
}

fn overlap_fits(length: u32, length_ms: f64, matched: bool, calm_match: bool, settings: &CrossmixOptions) -> bool {
    let longest = if !matched {
        MAX_UNMATCHED_OVERLAP_MS
    } else if calm_match {
        MAX_CALM_OVERLAP_MS
    } else {
        MAX_OVERLAP_MS
    } * settings.length_scale as f64;
    let counted = if 60_000.0 * length as f64 / length_ms >= 140.0 { MIN_FAST_OVERLAP_MS } else { MIN_OVERLAP_MS };
    let shortest = if settings.always_mix { counted.max(ALWAYS_MIX_MIN_OVERLAP_MS) } else { counted };
    let long_setting = settings.length_scale >= 1.5;
    length_ms >= shortest && length_ms <= (shortest * 1.35).max(longest) && (length < 32 || calm_match || long_setting)
}

fn exit_points(out: &TrackAnalysis, bars: &[i64], grid: LocalGrid, from_ms: i64, to_ms: i64) -> Vec<Exit> {
    let in_window: Vec<i64> = bars.iter().copied().filter(|b| (from_ms..=to_ms).contains(b)).collect();
    if !in_window.is_empty() {
        return in_window
            .into_iter()
            .map(|bar| {
                let local = out.local_grid(bar).unwrap_or(grid);
                Exit { swap_ms: local.snap(bar), grid: local }
            })
            .collect();
    }
    let mut points = Vec::new();
    let mut t = grid.snap(from_ms) as f64;
    while t <= to_ms as f64 {
        if t >= from_ms as f64 {
            points.push(Exit { swap_ms: t as i64, grid });
        }
        t += 4.0 * grid.period_ms;
    }
    points
}

fn exit_score(out: &TrackAnalysis, bars: &[i64], grid: LocalGrid, swap_ms: i64, end_ms: i64, natural_end: i64, _style: Style) -> Option<f32> {
    let skipped = out.audible_seconds(end_ms, natural_end);
    if skipped > MAX_SKIPPED_MUSIC_S {
        return None;
    }
    let lateness = -(skipped - 4.0).max(0.0) * 0.06;
    let bar_index = bars.iter().position(|&b| ((b - swap_ms).abs() as f64) < grid.period_ms / 3.0);
    let phrase = match bar_index.map(|i| out.phrase_level(i)) {
        Some(8) => 1.0,
        Some(4) => 0.6,
        Some(2) => 0.25,
        _ => 0.0,
    };
    let drop = (out.energy_around(swap_ms - 3_000, 4_000) - out.energy_around(swap_ms + 3_000, 4_000)).clamp(-0.3, 0.5);
    Some(phrase * 0.9 + lateness + drop * 0.6)
}

#[allow(clippy::too_many_arguments)]
fn pick_landing(
    style: Style,
    out: &TrackAnalysis,
    inc: Option<&TrackAnalysis>,
    swap_ms: i64,
    swap_beat: u32,
    out_beat_ms: f64,
    tempo: Option<&Tempo>,
    locked: bool,
    out_voice: &Voice,
    in_voice: &Voice,
    latest_ms: i64,
) -> Option<Landing> {
    let in_beat = tempo.map_or(out_beat_ms, |t| t.in_beat_ms);
    let run_up_ms = if style.enters_at_end() { 0.0 } else { swap_beat as f64 * in_beat };
    let Some(inc) = inc else { return Some(Landing { time_ms: run_up_ms as i64, score: 0.0 }) };
    let start = inc.leading_silence_end_ms;
    let bars = inc.bar_starts_ms();
    let out_energy = out.energy_around(swap_ms - 4_000, 4_000);
    let needs_grid = !style.enters_at_end() && tempo.is_some();
    let out_singing = if style.enters_at_end() {
        0.0
    } else {
        out_voice.average(swap_ms as f64 - swap_beat as f64 * out_beat_ms, swap_ms as f64)
    };
    let from_top = (start as f64 + run_up_ms) as i64;
    let mut candidates: Vec<i64> = Vec::new();
    if style.enters_at_end() {
        candidates.push(start);
    }
    if !style.enters_at_end() && from_top <= latest_ms {
        candidates.push(from_top);
    }
    for &bar in &bars {
        if bar >= start && bar <= latest_ms && bar as f64 - run_up_ms >= 0.0 {
            candidates.push(bar);
        }
    }
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|c| seen.insert(*c));
    let mut best: Option<Landing> = None;
    for time in candidates {
        let bar_index = bars.iter().position(|&b| (b - time).abs() < 60);
        let phrase = if style.enters_at_end() && time == start {
            0.7
        } else {
            match bar_index.map(|i| inc.phrase_level(i)) {
                None => 0.0,
                Some(8) => 1.0,
                Some(4) => 0.5,
                Some(_) => 0.1,
            }
        };
        let section = if bar_index.is_some_and(|i| inc.section_bars.contains(&i)) { 0.3 } else { 0.0 };
        let before = inc.energy_around(time - 3_000, 4_000);
        let after = inc.energy_around(time + 3_000, 4_000);
        let energy_fit = if style.enters_at_end() {
            (after - before).clamp(-0.2, 0.5) + (1.0 - (after - out_energy.max(0.6)).abs()) * 0.5
        } else {
            let bass_rise = inc.bass_around(time + 3_000, 4_000) - inc.bass_around(time - 3_000, 4_000);
            ((after - before) + 0.5 * bass_rise).clamp(-0.2, 0.6) * 1.2 + (1.0 - (after - out_energy.max(0.55)).abs()) * 0.3
        };
        let heard_from = (time as f64 - run_up_ms).max(0.0);
        let skip_penalty = (heard_from - start as f64).max(0.0) as f32 / 60_000.0 * 0.45;
        let in_singing = if style.enters_at_end() { 0.0 } else { in_voice.average(heard_from, time as f64) };
        let grid = inc.local_grid(time);
        let gridless = needs_grid && grid.is_none();
        let score = phrase * 0.8 + section + energy_fit - skip_penalty - in_singing * (0.25 + out_singing * 0.9)
            - if gridless { 2.0 } else { 0.0 };
        let landing = match grid {
            Some(g) if locked => g.snap(time),
            _ => time,
        };
        if best.as_ref().is_none_or(|b| score > b.score) {
            best = Some(Landing { time_ms: landing, score });
        }
    }
    best
}

fn length_fit(style: Style, length: u32, out: &TrackAnalysis, inc: Option<&TrackAnalysis>, key_clash: bool) -> f32 {
    let calm = 1.0 - out.vibe.energy.max(inc.map_or(0.5, |i| i.vibe.energy));
    let longest = *style.lengths().last().unwrap_or(&16) as f32;
    let mut fit = (length as f32 / longest - 0.5) * (calm - 0.4) * 0.6;
    if key_clash && !style.enters_at_end() && length > 8 {
        fit -= 0.3;
    }
    fit
}

/// No reliable beat: an equal-power-ish dampened blend over 9 s.
fn smooth_fade(_outgoing: &TrackInput, incoming: &TrackInput, natural_end: i64, earliest_start: i64) -> Plan {
    let fade_ms = 9_000i64;
    let latest_start = (natural_end - fade_ms).max(0);
    let start = latest_start.clamp(earliest_start, latest_start.max(earliest_start));
    let beat_ms = fade_ms as f64 / 16.0;
    let style = Style::Dampen;
    let swap = style.swap_at(16);
    let cue = incoming.analysis.map_or(0, |a| a.leading_silence_end_ms.min(MAX_ENTRY_MS));
    Plan {
        style,
        variation: 0,
        length_beats: 16,
        swap_beat: swap,
        out_start_ms: start,
        out_beat_ms: beat_ms,
        in_cue_ms: (cue as f64 + swap as f64 * beat_ms) as i64,
        in_media_per_beat: beat_ms,
        in_speed: 1.0,
        out_speed: 1.0,
        beat_matched: false,
        in_start_beat: 0.0,
        yield_amount: 0.0,
    }
}

/// Always-mix with no beat: the song sinks under a filter while the next
/// comes up through it.
fn forced_blend(_outgoing: &TrackInput, incoming: &TrackInput, natural_end: i64, out: Option<&TrackAnalysis>, earliest_start: i64) -> Plan {
    let style = Style::FilterSweep;
    let beats = 16u32;
    let beat_ms = out.and_then(|o| o.bpm).filter(|b| (70.0..=180.0).contains(b)).map_or(FORCED_BLEND_MS / beats as f64, |b| 60_000.0 / b as f64);
    let length_ms = (beat_ms * beats as f64) as i64;
    let latest_start = (natural_end - length_ms).max(0);
    let start = latest_start.clamp(earliest_start, latest_start.max(earliest_start));
    let swap = style.swap_at(beats);
    let cue = incoming.analysis.map_or(0, |a| a.leading_silence_end_ms.min(MAX_ENTRY_MS));
    Plan {
        style,
        variation: 0,
        length_beats: beats,
        swap_beat: swap,
        out_start_ms: start,
        out_beat_ms: beat_ms,
        in_cue_ms: (cue as f64 + swap as f64 * beat_ms) as i64,
        in_media_per_beat: beat_ms,
        in_speed: 1.0,
        out_speed: 1.0,
        beat_matched: false,
        in_start_beat: 0.0,
        yield_amount: 0.0,
    }
}

/// Where the song really ends: before any trailing silence.
pub(crate) fn natural_end(track: &TrackInput) -> i64 {
    let silence = track
        .analysis
        .filter(|a| a.complete && a.trailing_silence_start_ms > track.duration_ms / 2)
        .map(|a| a.trailing_silence_start_ms);
    track.duration_ms.min(silence.unwrap_or(track.duration_ms))
}
