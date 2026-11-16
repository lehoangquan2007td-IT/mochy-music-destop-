//! Audio Lab → Audio FX (port of Android `AudioFxScreen`, `ReverbCard`,
//! `AudioFxPictures`): one card per effect with a live picture of what it does
//! to the sound, a switch, quick presets and the fine sliders.

use egui::epaint::{CubicBezierShape, QuadraticBezierShape};
use egui::{Align, Color32, CornerRadius, Layout, Painter, Pos2, Rect, RichText, Sense, Shape, Stroke, Ui, pos2, vec2};
use mochy_core::settings::{AudioLab, ReverbSettings, REVERB_PRESETS};

use super::kit::{self, BLUE, CardStack, LevelFollower, PINK, alpha, white};
use super::{chip, percent_row, section_label, segmented, slider_row, theme, toggle, toggle_row};
use crate::ui::i18n::tr;

/// Persistent picture state for the FX page.
#[derive(Default)]
pub struct FxState {
    waves: RoomWaves,
    loud_hist: LevelHistory,
    spin_angle: f32,
    spin_phase: f32,
    pub stack: CardStack,
    stack_bars: Vec<f32>,
    /// Spatial mode remembered while spatial is off (false = crossfeed).
    speakers_mode: bool,
}

/// An effect that's on, for the header cards and the hub status line.
pub fn active_effects(lab: &AudioLab) -> Vec<&'static str> {
    let mut v = Vec::new();
    if lab.reverb.enabled {
        v.push("Reverb");
    }
    if lab.loudness_enabled {
        v.push("Loudness");
    }
    if lab.spatial.crossfeed_enabled || lab.spatial.virtual_speakers {
        v.push("Spatial");
    }
    if lab.spatial.mono {
        v.push("Mono");
    }
    if (lab.spatial.width - 1.0).abs() > 1e-3 {
        v.push("Width");
    }
    if (lab.speed - 1.0).abs() > 1e-3 || (lab.preserve_pitch && lab.pitch_semitones.abs() > 1e-3) {
        v.push("Playback");
    }
    if lab.volume_curve_enabled {
        v.push("Volume");
    }
    v
}

/// "REVERB, LOUDNESS and 2 more" / "ALL OFF".
pub fn effects_status(lab: &AudioLab) -> String {
    let names: Vec<String> = active_effects(lab).iter().map(|n| tr(n).to_uppercase()).collect();
    match names.len() {
        0 => tr("ALL OFF").into(),
        1 | 2 => names.join(", "),
        n => tr("{a} and {n} more").replace("{a}", &names[..2].join(", ")).replace("{n}", &(n - 2).to_string()),
    }
}

pub struct Frame<'a> {
    pub follower: &'a LevelFollower,
    pub spectrum: &'a [f32],
    pub t: f32,
    pub dt: f32,
    pub volume: f32,
    pub accent: Color32,
}

/// The header: the fanned cards of what's on, beside the page title.
pub fn header(ui: &mut Ui, lab: &AudioLab, st: &mut FxState, f: &Frame) {
    kit::spectrum_bars(f.spectrum, 10, f.t, &mut st.stack_bars, f.dt);
    let labels: Vec<String> = active_effects(lab).iter().map(|n| tr(n).to_string()).collect();
    ui.horizontal(|ui| {
        st.stack.show(ui, &labels, &st.stack_bars, tr("ON"), tr("Nothing on — every effect is off"));
        ui.vertical(|ui| {
            ui.add_space(40.0);
            ui.label(RichText::new(tr("Audio")).font(theme::semibold(ui.ctx(), 34.0)).color(theme::TEXT_SECONDARY));
            ui.label(RichText::new("FX").font(theme::bold(ui.ctx(), 72.0)).color(Color32::WHITE));
            ui.label(RichText::new(effects_status(lab)).monospace().size(11.0).color(kit::STATUS_TEXT));
        });
    });
}

pub fn page(ui: &mut Ui, lab: &mut AudioLab, st: &mut FxState, f: &Frame) {
    header(ui, lab, st, f);
    ui.add_space(6.0);
    if ui.available_width() >= 760.0 {
        ui.columns(2, |cols| {
            reverb_card(&mut cols[0], lab, st, f);
            spatial_card(&mut cols[0], lab, st, f);
            mono_card(&mut cols[0], lab, f);
            loudness_card(&mut cols[1], lab, st, f);
            speed_card(&mut cols[1], lab, st, f);
            volume_card(&mut cols[1], lab, f);
        });
    } else {
        reverb_card(ui, lab, st, f);
        loudness_card(ui, lab, st, f);
        spatial_card(ui, lab, st, f);
        mono_card(ui, lab, f);
        speed_card(ui, lab, st, f);
        volume_card(ui, lab, f);
    }
}

// ---------------------------------------------------------------------------
// Card shell
// ---------------------------------------------------------------------------

/// Picture stage on top, then `body` (the header and the folded options).
fn effect_card(ui: &mut Ui, picture: impl FnOnce(&Painter, Rect), body: impl FnOnce(&mut Ui)) {
    super::card(ui, |ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 132.0), Sense::hover());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, CornerRadius::same(18), theme::STAGE);
        p.rect_stroke(rect, CornerRadius::same(18), Stroke::new(1.0, theme::STAGE_LINE), egui::StrokeKind::Inside);
        picture(&p, rect);
        ui.add_space(4.0);
        body(ui);
    });
}

/// Options that fold away while their effect is off.
fn folded(ui: &mut Ui, id: &str, expanded: bool, add: impl FnOnce(&mut Ui)) {
    let open = ui.ctx().animate_bool_with_time(ui.scope_id().with(("fx_open", id)), expanded, 0.2);
    if open > 0.01 {
        ui.scope(|ui| {
            ui.multiply_opacity(open);
            add(ui);
        });
    }
}

fn panel(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new().fill(theme::ROW).corner_radius(CornerRadius::same(14)).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
}

fn note(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).small().color(theme::TEXT_SECONDARY));
}

fn chips(ui: &mut Ui, items: &[(&str, &str)], selected: Option<&str>, accent: Color32) -> Option<String> {
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for (key, label) in items {
            if chip(ui, label, selected == Some(*key), accent).clicked() {
                picked = Some(key.to_string());
            }
        }
    });
    picked
}

fn feq(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

// ---------------------------------------------------------------------------
// Reverb
// ---------------------------------------------------------------------------

#[derive(Default)]
struct RoomWaves {
    born: [f32; 14],
    strength: [f32; 14],
    next: usize,
    next_emit: f32,
    last_emit: f32,
    init: bool,
}

impl RoomWaves {
    fn emit(&mut self, t: f32, s: f32) {
        self.born[self.next] = t;
        self.strength[self.next] = s;
        self.next = (self.next + 1) % self.born.len();
        self.last_emit = t;
    }
}

fn reverb_name(id: &str) -> &'static str {
    tr(match id {
        "room" => "Room",
        "studio" => "Studio",
        "bathroom" => "Bathroom",
        "plate" => "Plate",
        "hall" => "Hall",
        "cathedral" => "Cathedral",
        "stadium" => "Stadium",
        "dreamy" => "Dreamy",
        _ => "Custom",
    })
}

fn reverb_card(ui: &mut Ui, lab: &mut AudioLab, st: &mut FxState, f: &Frame) {
    let on = ui.ctx().animate_bool_with_time(ui.scope_id().with("rv_on"), lab.reverb.enabled, 0.4);
    let room = ui.ctx().animate_value_with_time(ui.scope_id().with("rv_room"), lab.reverb.room_size, 0.5);
    let rv_copy = lab.reverb.clone();
    let accent = f.accent;
    effect_card(
        ui,
        |p, rect| reverb_picture(p, rect, &rv_copy, on, room, &mut st.waves, f),
        |ui| {
            toggle_row(ui, tr("Reverb"), Some(tr("Put the music in a room")), &mut lab.reverb.enabled, accent);
            folded(ui, "reverb", lab.reverb.enabled, |ui| {
                let rv = &mut lab.reverb;
                let mut items: Vec<(&str, &str)> = REVERB_PRESETS.iter().map(|p| (p.0, reverb_name(p.0))).collect();
                let custom = !REVERB_PRESETS.iter().any(|p| p.0 == rv.preset_id);
                if custom {
                    items.push(("custom", tr("Custom")));
                }
                let sel = if custom { "custom".to_string() } else { rv.preset_id.clone() };
                if let Some(id) = chips(ui, &items, Some(&sel), accent)
                    && let Some(s) = ReverbSettings::from_preset(&id, rv.enabled)
                {
                    *rv = s;
                }
                let before = rv.clone();
                panel(ui, |ui| {
                    percent_row(ui, tr("Mix"), &mut rv.mix);
                    percent_row(ui, tr("Room size"), &mut rv.room_size);
                    percent_row(ui, tr("Damping"), &mut rv.damping);
                    percent_row(ui, tr("Wet"), &mut rv.wet);
                    section_label(ui, tr("Fine tune"));
                    percent_row(ui, tr("Dry"), &mut rv.dry);
                    percent_row(ui, tr("Width"), &mut rv.width);
                    slider_row(ui, tr("Pre-delay"), &mut rv.pre_delay_ms, 0.0..=120.0, " ms", 0, false);
                    slider_row(ui, tr("Low cut"), &mut rv.low_cut_hz, 20.0..=1000.0, " Hz", 0, true);
                });
                if *rv != before && rv.preset_id == before.preset_id {
                    rv.preset_id = "custom".into();
                }
            });
        },
    );
}

/// Waves leave the speaker, bounce between the walls and die away; the wall
/// moves with the room size (Android `ReverbPicture`).
fn reverb_picture(p: &Painter, rect: Rect, s: &ReverbSettings, on: f32, room: f32, waves: &mut RoomWaves, f: &Frame) {
    let (w, h) = (rect.width(), rect.height());
    let cy = rect.center().y;
    let x0 = rect.left();
    let speaker_x = x0 + w * 0.15;
    let back_x = x0 + w * 0.05;
    let wall_x = x0 + w * (0.52 + 0.4 * room);
    let speed = w * 0.42;
    let t = f.t;
    if !waves.init {
        waves.born = [-100.0; 14];
        waves.init = true;
    }
    let live = f.follower.live;
    let punch = f.follower.punch;
    if t >= waves.next_emit || (live && punch > 0.55 && t - waves.last_emit > 0.28) || t < waves.last_emit {
        waves.emit(t, if live { 0.55 + 0.45 * (f.follower.level * 3.0 + punch).min(1.0) } else { 0.75 });
        waves.next_emit = t + if live { 0.9 } else { 1.3 };
    }
    let reflectivity = (0.35 + 0.55 * s.room_size) * (1.0 - 0.55 * s.damping);
    let echo_alpha = on * (0.35 + 0.65 * s.wet) * (0.4 + 0.6 * s.mix);
    let span = wall_x - back_x;
    let mut flash: f32 = 0.0;
    for i in 0..waves.born.len() {
        let age = t - waves.born[i];
        if !(0.0..=8.0).contains(&age) {
            continue;
        }
        let d = age * speed;
        let first = wall_x - speaker_x;
        if d <= first || on < 0.01 {
            let x = speaker_x + d;
            if x > rect.right() + 20.0 {
                continue;
            }
            let a = waves.strength[i] * (-d / (w * 1.6)).exp();
            draw_wave(p, x, cy, h, d, a * if d > first { 1.0 - on } else { 1.0 }, true, 0.0);
            continue;
        }
        let past = d - first;
        let bounces = 1 + (past / span) as i32;
        let in_leg = past % span;
        let rightward = bounces % 2 == 0;
        let x = if rightward { back_x + in_leg } else { wall_x - in_leg };
        let a = waves.strength[i] * reflectivity.powi(bounces) * (-d / (w * 2.2)).exp() * echo_alpha;
        if a < 0.02 {
            continue;
        }
        if in_leg < span * 0.08 && !rightward {
            flash = flash.max(a * (1.0 - in_leg / (span * 0.08)));
        }
        draw_wave(p, x, cy, h, d, a, rightward, s.damping);
    }
    let wall_h = h * 0.62;
    p.rect_filled(
        Rect::from_min_size(pos2(wall_x, cy - wall_h / 2.0), vec2(5.0, wall_h)),
        CornerRadius::same(3),
        white((0.18 + 0.55 * on + 0.6 * flash).min(1.0)),
    );
    draw_speaker(p, pos2(speaker_x, cy), 1.0 + punch * 0.12);
}

fn draw_wave(p: &Painter, x: f32, cy: f32, h: f32, travelled: f32, a: f32, rightward: bool, softness: f32) {
    if a <= 0.005 {
        return;
    }
    let reach = (h * 0.42).min(14.0 + travelled * 0.18);
    let bulge = reach * 0.42 * if rightward { 1.0 } else { -1.0 };
    let pts = [pos2(x - bulge * 0.2, cy - reach), pos2(x + bulge, cy), pos2(x - bulge * 0.2, cy + reach)];
    if softness > 0.15 {
        p.add(QuadraticBezierShape::from_points_stroke(pts, false, Color32::TRANSPARENT, Stroke::new(7.0 * softness, white(a * 0.25 * softness))));
    }
    p.add(QuadraticBezierShape::from_points_stroke(pts, false, Color32::TRANSPARENT, Stroke::new(2.2 - softness, white(a.min(1.0)))));
}

fn draw_speaker(p: &Painter, c: Pos2, scale: f32) {
    let s = 9.0 * scale;
    let body = vec![
        pos2(c.x - s * 1.1, c.y - s * 0.45),
        pos2(c.x - s * 0.4, c.y - s * 0.45),
        pos2(c.x + s * 0.5, c.y - s * 1.1),
        pos2(c.x + s * 0.5, c.y + s * 1.1),
        pos2(c.x - s * 0.4, c.y + s * 0.45),
        pos2(c.x - s * 1.1, c.y + s * 0.45),
    ];
    p.add(Shape::closed_line(body, Stroke::new(1.6, white(0.92))));
}

// ---------------------------------------------------------------------------
// Loudness (+ normalization footer)
// ---------------------------------------------------------------------------

struct LevelHistory {
    values: [f32; 56],
    head: usize,
    acc: f32,
}

impl Default for LevelHistory {
    fn default() -> Self {
        Self { values: [0.05; 56], head: 0, acc: 0.0 }
    }
}

const LOUDNESS_PRESETS: [(&str, &str, f32, f32); 4] =
    [("gentle", "Gentle", 2.0, 0.15), ("punchy", "Punchy", 4.0, 0.3), ("night", "Night", 3.0, 0.85), ("max", "Max", 9.0, 0.5)];

fn loudness_card(ui: &mut Ui, lab: &mut AudioLab, st: &mut FxState, f: &Frame) {
    let on = ui.ctx().animate_bool_with_time(ui.scope_id().with("ld_on"), lab.loudness_enabled, 0.4);
    let (boost, comp) = (lab.loudness_boost_db, lab.loudness_compression);
    let accent = f.accent;
    effect_card(
        ui,
        |p, rect| loudness_picture(p, rect, boost, comp, on, &mut st.loud_hist, f),
        |ui| {
            toggle_row(ui, tr("Loudness"), Some(tr("Louder, with the peaks held down")), &mut lab.loudness_enabled, accent);
            folded(ui, "loudness", lab.loudness_enabled, |ui| {
                let items: Vec<(&str, &str)> = LOUDNESS_PRESETS.iter().map(|p| (p.0, tr(p.1))).collect();
                let sel = LOUDNESS_PRESETS.iter().find(|p| feq(p.2, lab.loudness_boost_db) && feq(p.3, lab.loudness_compression)).map(|p| p.0);
                if let Some(k) = chips(ui, &items, sel, accent)
                    && let Some(p) = LOUDNESS_PRESETS.iter().find(|p| p.0 == k)
                {
                    lab.loudness_boost_db = p.2;
                    lab.loudness_compression = p.3;
                }
                panel(ui, |ui| {
                    slider_row(ui, tr("Boost"), &mut lab.loudness_boost_db, 0.0..=12.0, " dB", 1, false);
                    percent_row(ui, tr("Compression"), &mut lab.loudness_compression);
                    note(ui, tr("More compression brings quiet parts up and keeps loud parts from jumping out."));
                });
            });
        },
    );
    // Footer: even out songs (ReplayGain), always visible like upstream.
    super::card(ui, |ui| {
        toggle_row(
            ui,
            tr("Even out songs"),
            Some(tr("ReplayGain tags of local files, so every song plays at the same level")),
            &mut lab.normalize_replay_gain,
            accent,
        );
        if lab.normalize_replay_gain {
            panel(ui, |ui| {
                slider_row(ui, tr("Preamp"), &mut lab.normalize_preamp_db, -6.0..=6.0, " dB", 1, false);
                note(ui, tr("ReplayGain aims at −18 LUFS; the preamp lifts everything back up. Applies from the next song."));
            });
        }
        let mut yt = crate::stream_facts::playback_now().youtube_normalization;
        if toggle_row(ui, tr("YouTube loudness"), Some(tr("Use YouTube's loudness value for streamed songs")), &mut yt, accent).changed() {
            crate::stream_facts::request_youtube_normalization(yt);
        }
    });
}

/// The music's level, lifted by the boost and flattened under the ceiling.
fn loudness_picture(p: &Painter, rect: Rect, boost: f32, compression: f32, on: f32, hist: &mut LevelHistory, f: &Frame) {
    hist.acc += f.dt;
    if hist.acc > 1.0 / 24.0 {
        hist.acc = 0.0;
        let t = f.t;
        let v = if f.follower.live {
            f.follower.level * 2.6
        } else {
            0.18 + 0.12 * (t * 2.1).sin() + 0.1 * (t * 5.3 + 1.0).sin() + 0.06 * (t * 11.0).sin()
        };
        hist.values[hist.head] = v.clamp(0.005, 1.4);
        hist.head = (hist.head + 1) % hist.values.len();
    }
    let top = rect.top() + 18.0;
    let bottom = rect.bottom() - 14.0;
    let y_of = |db: f32| bottom - ((db + 42.0) / 42.0).clamp(0.0, 1.0) * (bottom - top);
    let ceiling = y_of(-0.5);
    let threshold = -10.0 - 24.0 * compression;
    let slope = 1.0 - 1.0 / (1.0 + 3.0 * compression);
    let makeup = if compression > 0.0 { -threshold * slope * 0.5 } else { 0.0 };
    let n = hist.values.len();
    let bar_w = (rect.width() - 24.0) / n as f32;
    let mid = (top + bottom) / 2.0;
    for k in 0..n {
        let v = hist.values[(hist.head + k) % n];
        let in_db = 20.0 * v.max(1e-4).log10();
        let reduced = if in_db > threshold { (threshold - in_db) * slope } else { 0.0 };
        let out_db = (in_db + (boost + makeup + reduced) * on).min(-0.5);
        let x = rect.left() + 12.0 + k as f32 * bar_w;
        let in_half = (bottom - y_of(in_db.min(0.0))) / 2.0;
        let out_half = (bottom - y_of(out_db)) / 2.0;
        let cr = CornerRadius::same((bar_w * 0.3) as u8);
        p.rect_filled(Rect::from_min_size(pos2(x, mid - in_half), vec2(bar_w * 0.62, in_half * 2.0)), cr, white(0.12));
        let limited = on > 0.5 && out_db >= -0.6;
        p.rect_filled(
            Rect::from_min_size(pos2(x, mid - out_half), vec2(bar_w * 0.62, out_half * 2.0)),
            cr,
            if limited { Color32::WHITE } else { white(0.72) },
        );
    }
    let half = (bottom - ceiling) / 2.0;
    let col = white(0.3 * on + 0.08);
    for y in [mid - half, mid + half] {
        p.extend(Shape::dashed_line(&[pos2(rect.left() + 10.0, y), pos2(rect.right() - 10.0, y)], Stroke::new(1.0, col), 4.0, 4.0));
    }
}

// ---------------------------------------------------------------------------
// Spatial
// ---------------------------------------------------------------------------

const CROSSFEED_LEVELS: [(&str, &str, f32, f32); 3] =
    [("light", "Light", 700.0, 4.5), ("natural", "Natural", 700.0, 6.0), ("strong", "Strong", 650.0, 9.5)];

fn spatial_card(ui: &mut Ui, lab: &mut AudioLab, st: &mut FxState, f: &Frame) {
    let sp = &mut lab.spatial;
    let enabled = sp.crossfeed_enabled || sp.virtual_speakers;
    if enabled {
        st.speakers_mode = sp.virtual_speakers;
    }
    let ctx = ui.ctx().clone();
    let on = ctx.animate_value_with_time(ui.scope_id().with("sp_on"), if enabled { sp.amount } else { 0.0 }, 0.4);
    let speakers = ctx.animate_bool_with_time(ui.scope_id().with("sp_mode"), st.speakers_mode, 0.5);
    let angle = ctx.animate_value_with_time(ui.scope_id().with("sp_angle"), sp.speaker_angle_deg, 0.4);
    let feed = ctx.animate_value_with_time(ui.scope_id().with("sp_feed"), sp.crossfeed_feed_db, 0.3);
    let accent = f.accent;
    let mut want = enabled;
    effect_card(
        ui,
        |p, rect| spatial_picture(p, rect, on, speakers, angle, feed, f),
        |ui| {
            toggle_row(ui, tr("Spatial"), Some(tr("Music in front of you instead of inside your head")), &mut want, accent);
            folded(ui, "spatial", enabled, |ui| {
                let cur = usize::from(st.speakers_mode);
                if let Some(i) = segmented(ui, &[tr("Crossfeed"), tr("Speakers")], cur) {
                    st.speakers_mode = i == 1;
                }
                if !st.speakers_mode {
                    let items: Vec<(&str, &str)> = CROSSFEED_LEVELS.iter().map(|l| (l.0, tr(l.1))).collect();
                    let sel = CROSSFEED_LEVELS.iter().find(|l| feq(l.2, sp.crossfeed_cutoff_hz) && feq(l.3, sp.crossfeed_feed_db)).map(|l| l.0);
                    if let Some(k) = chips(ui, &items, sel, accent)
                        && let Some(l) = CROSSFEED_LEVELS.iter().find(|l| l.0 == k)
                    {
                        sp.crossfeed_cutoff_hz = l.2;
                        sp.crossfeed_feed_db = l.3;
                    }
                    panel(ui, |ui| {
                        slider_row(ui, tr("Head shadow"), &mut sp.crossfeed_cutoff_hz, 300.0..=1500.0, " Hz", 0, false);
                        slider_row(ui, tr("Crossfeed"), &mut sp.crossfeed_feed_db, 1.0..=15.0, " dB", 1, false);
                        percent_row(ui, tr("Amount"), &mut sp.amount);
                    });
                } else {
                    panel(ui, |ui| {
                        slider_row(ui, tr("Speaker angle"), &mut sp.speaker_angle_deg, 10.0..=90.0, "°", 0, false);
                        percent_row(ui, tr("Amount"), &mut sp.amount);
                        note(ui, tr("Two speakers in front of you, each heard by both ears — the far one a little later and darker."));
                    });
                }
                panel(ui, |ui| {
                    slider_row(ui, tr("Stereo width"), &mut sp.width, 0.0..=2.0, "", 2, false);
                });
            });
        },
    );
    // Apply the switch / mode.
    let sp = &mut lab.spatial;
    if want {
        sp.virtual_speakers = st.speakers_mode;
        sp.crossfeed_enabled = !st.speakers_mode;
    } else {
        sp.virtual_speakers = false;
        sp.crossfeed_enabled = false;
    }
}

/// A head from above, and where the sound reaches it from.
fn spatial_picture(p: &Painter, rect: Rect, on: f32, speakers: f32, angle: f32, feed: f32, f: &Frame) {
    let (w, h) = (rect.width(), rect.height());
    let head = pos2(rect.center().x, rect.top() + h * 0.66);
    let r = 17.0;
    let ear_l = head - vec2(r, 0.0);
    let ear_r = head + vec2(r, 0.0);
    let pulse = f.follower.punch;
    let t = f.t;
    p.circle_stroke(head, r, Stroke::new(1.6, white(0.9)));
    p.add(Shape::line(
        vec![pos2(head.x - 4.0, head.y - r + 1.0), pos2(head.x, head.y - r - 6.0), pos2(head.x + 4.0, head.y - r + 1.0)],
        Stroke::new(1.6, white(0.9)),
    ));
    p.circle_filled(ear_l, 3.0, Color32::WHITE);
    p.circle_filled(ear_r, 3.0, Color32::WHITE);
    let inside = 1.0 - on;
    if inside > 0.01 {
        p.circle_filled(pos2(head.x - r * 0.45, head.y), 5.0 + 2.0 * pulse, alpha(BLUE, 0.7 * inside));
        p.circle_filled(pos2(head.x + r * 0.45, head.y), 5.0 + 2.0 * pulse, alpha(PINK, 0.7 * inside));
    }
    let ring = (w * 0.36).min(h * 0.55);
    let cf = on * (1.0 - speakers);
    let spk = on * speakers;
    let rad = angle.to_radians();
    let sp_l = pos2(head.x - rad.sin() * ring, head.y - rad.cos() * ring);
    let sp_r = pos2(head.x + rad.sin() * ring, head.y - rad.cos() * ring);
    if spk > 0.01 {
        for (side, (sp, col)) in [(sp_l, BLUE), (sp_r, PINK)].into_iter().enumerate() {
            let (near, far) = if side == 0 { (ear_l, ear_r) } else { (ear_r, ear_l) };
            flow_line(p, sp, near, alpha(col, 0.8 * spk), t, 1.0);
            flow_line(p, sp, far, alpha(col, 0.32 * spk), t - 0.08, 0.6);
            p.rect_filled(Rect::from_center_size(sp, vec2(12.0, 16.0)), CornerRadius::same(3), alpha(col, spk));
        }
    }
    if cf > 0.01 {
        let strength = (1.0 - (feed - 1.0) / 14.0).clamp(0.2, 1.0);
        let lift = r * 2.4;
        for (from, to, col) in [(ear_l, ear_r, BLUE), (ear_r, ear_l, PINK)] {
            let pts = [from - vec2(0.0, 4.0), pos2(from.x, from.y - lift), pos2(to.x, to.y - lift), to - vec2(0.0, 4.0)];
            let curve = CubicBezierShape::from_points_stroke(pts, false, Color32::TRANSPARENT, Stroke::NONE);
            let line: Vec<Pos2> = curve.flatten(Some(0.5));
            // Running dashes.
            let col = alpha(col, 0.55 * cf * strength);
            let total: f32 = line.windows(2).map(|s| s[0].distance(s[1])).sum();
            let off = (t * 40.0).rem_euclid(10.0);
            let mut acc = 0.0;
            for s in line.windows(2) {
                let d = s[0].distance(s[1]);
                let phase = (acc - off).rem_euclid(10.0);
                if phase < 5.0 {
                    p.line_segment([s[0], s[1]], Stroke::new(1.8, col));
                }
                acc += d;
            }
            let _ = total;
        }
        p.circle_filled(ear_l - vec2(10.0, 0.0), 5.0 + 2.0 * pulse, alpha(BLUE, 0.8 * cf));
        p.circle_filled(ear_r + vec2(10.0, 0.0), 5.0 + 2.0 * pulse, alpha(PINK, 0.8 * cf));
    }
}

fn flow_line(p: &Painter, from: Pos2, to: Pos2, col: Color32, t: f32, weight: f32) {
    p.line_segment([from, to], Stroke::new(1.2 * weight, col.gamma_multiply(0.35)));
    for k in 0..3 {
        let q = (t * 0.9 + k as f32 / 3.0).rem_euclid(1.0);
        p.circle_filled(from + (to - from) * q, 2.2 * weight, col);
    }
}

fn mono_card(ui: &mut Ui, lab: &mut AudioLab, f: &Frame) {
    super::card(ui, |ui| {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, if lab.spatial.mono { kit::STATUS_ON } else { kit::STATUS_OFF });
            ui.vertical(|ui| {
                ui.label(RichText::new(tr("Mono audio")).color(Color32::WHITE));
                ui.label(RichText::new(tr("Both ears hear the whole mix")).small().color(theme::TEXT_SECONDARY));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| toggle(ui, &mut lab.spatial.mono, f.accent));
        });
    });
}

// ---------------------------------------------------------------------------
// Speed & pitch
// ---------------------------------------------------------------------------

/// (key, label, speed, preserve pitch, with reverb)
const SPEED_PRESETS: [(&str, &str, f32, bool, bool); 8] = [
    ("normal", "Normal", 1.0, true, false),
    ("slowed", "Slowed", 0.85, false, false),
    ("slowed_reverb", "Slowed + reverb", 0.85, false, true),
    ("nightcore", "Nightcore", 1.25, false, false),
    ("0.75", "0.75×", 0.75, true, false),
    ("1.25", "1.25×", 1.25, true, false),
    ("1.5", "1.5×", 1.5, true, false),
    ("2", "2×", 2.0, true, false),
];

fn speed_card(ui: &mut Ui, lab: &mut AudioLab, st: &mut FxState, f: &Frame) {
    let ctx = ui.ctx().clone();
    let speed_anim = ctx.animate_value_with_time(ui.scope_id().with("spd"), lab.speed, 0.4);
    let pitch_ratio = if lab.preserve_pitch { 2f32.powf(lab.pitch_semitones / 12.0) } else { lab.speed };
    let pitch_anim = ctx.animate_value_with_time(ui.scope_id().with("pitch"), pitch_ratio, 0.4);
    let accent = f.accent;
    effect_card(
        ui,
        |p, rect| speed_picture(p, rect, speed_anim, pitch_anim, st, f),
        |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(tr("Speed & pitch")).strong().color(Color32::WHITE));
                    let sub = if lab.preserve_pitch { tr("Pitch stays put") } else { tr("Pitch follows the speed, like a record") };
                    ui.label(RichText::new(sub).small().color(theme::TEXT_SECONDARY));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if step_button(ui, "+").clicked() {
                        lab.speed = ((lab.speed + 0.05) * 100.0).round() / 100.0;
                    }
                    ui.label(RichText::new(format!("{:.2}×", lab.speed)).font(theme::bold(ui.ctx(), 17.0)).color(Color32::WHITE));
                    if step_button(ui, "−").clicked() {
                        lab.speed = ((lab.speed - 0.05) * 100.0).round() / 100.0;
                    }
                });
            });
            folded(ui, "speed", true, |ui| {
                let items: Vec<(&str, &str)> = SPEED_PRESETS.iter().map(|p| (p.0, tr(p.1))).collect();
                let sel = if feq(lab.speed, 0.85) && !lab.preserve_pitch {
                    Some(if lab.reverb.enabled { "slowed_reverb" } else { "slowed" })
                } else {
                    SPEED_PRESETS
                        .iter()
                        .find(|p| !p.4 && feq(p.2, lab.speed) && p.3 == lab.preserve_pitch && (!p.3 || lab.pitch_semitones == 0.0))
                        .map(|p| p.0)
                };
                if let Some(k) = chips(ui, &items, sel, accent)
                    && let Some(p) = SPEED_PRESETS.iter().find(|p| p.0 == k)
                {
                    lab.speed = p.2;
                    lab.preserve_pitch = p.3;
                    lab.pitch_semitones = 0.0;
                    if p.4 {
                        if let Some(hall) = ReverbSettings::from_preset("hall", true) {
                            lab.reverb = hall;
                        }
                    } else if k == "slowed" && lab.reverb.enabled {
                        lab.reverb.enabled = false;
                    }
                }
                panel(ui, |ui| {
                    slider_row(ui, tr("Speed"), &mut lab.speed, 0.1..=8.0, "×", 2, true);
                    toggle_row(ui, tr("Keep pitch"), Some(tr("Change the speed without changing the pitch")), &mut lab.preserve_pitch, accent);
                    if lab.preserve_pitch {
                        let mut st = lab.pitch_semitones;
                        if slider_row(ui, tr("Pitch"), &mut st, -12.0..=12.0, " st", 0, false).changed() {
                            lab.pitch_semitones = st.round();
                        }
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let normal = feq(lab.speed, 1.0) && lab.pitch_semitones == 0.0;
                    if ui.add_enabled(!normal, egui::Button::new(tr("Reset"))).clicked() {
                        lab.speed = 1.0;
                        lab.pitch_semitones = 0.0;
                        lab.preserve_pitch = true;
                    }
                });
            });
        },
    );
}

fn step_button(ui: &mut Ui, label: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    let fill = if resp.hovered() { theme::ROW_SELECTED } else { theme::ROW };
    ui.painter().circle_filled(rect.center(), 16.0, fill);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, theme::bold(ui.ctx(), 16.0), Color32::WHITE);
    resp
}

/// A record turning at the speed, a wave at the pitch.
fn speed_picture(p: &Painter, rect: Rect, speed: f32, pitch: f32, st: &mut FxState, f: &Frame) {
    st.spin_angle = (st.spin_angle + f.dt * 200.0 * speed).rem_euclid(360.0);
    st.spin_phase += f.dt * 2.4 * speed;
    let h = rect.height();
    let disc = h * 0.36;
    let c = pos2(rect.left() + disc + 18.0, rect.center().y);
    p.circle_filled(c, disc, Color32::from_rgb(0x16, 0x16, 0x1A));
    for g in 1..=5 {
        p.circle_stroke(c, disc * (0.45 + g as f32 * 0.1), Stroke::new(1.0, white(0.06)));
    }
    let a0 = st.spin_angle.to_radians();
    // Sheen wedge.
    let mut wedge = vec![c];
    for k in 0..=12 {
        let a = a0 + 70f32.to_radians() * k as f32 / 12.0;
        wedge.push(c + vec2(a.cos(), a.sin()) * disc);
    }
    p.add(Shape::convex_polygon(wedge, white(0.10), Stroke::NONE));
    p.circle_filled(c, disc * 0.3, white(0.9));
    let dot = a0 - std::f32::consts::FRAC_PI_2;
    p.circle_filled(c + vec2(dot.cos(), dot.sin()) * disc * 0.16, disc * 0.06, Color32::BLACK);
    p.circle_filled(c, disc * 0.05, Color32::BLACK);
    let left = c.x + disc + 22.0;
    let right = rect.right() - 16.0;
    let amp = h * (0.16 + 0.12 * f.follower.level.min(1.0));
    let cycles = 2.5 * pitch;
    let mut pts = Vec::new();
    let mut x = left;
    while x <= right {
        let q = (x - left) / (right - left);
        let env = (q * std::f32::consts::PI).sin();
        let y = rect.center().y + ((q * cycles * std::f32::consts::TAU) - st.spin_phase).sin() * amp * env;
        pts.push(pos2(x, y));
        x += 2.0;
    }
    p.add(Shape::line(pts, Stroke::new(2.0, white(0.9))));
}

// ---------------------------------------------------------------------------
// Volume curve
// ---------------------------------------------------------------------------

const CURVE_SHAPES: [(&str, &str, f32); 3] = [("gentle", "Gentle", 1.6), ("exponential", "Exponential", 2.2), ("steep", "Steep", 3.2)];

fn volume_card(ui: &mut Ui, lab: &mut AudioLab, f: &Frame) {
    let ctx = ui.ctx().clone();
    let on = lab.volume_curve_enabled;
    let exp = ctx.animate_value_with_time(ui.scope_id().with("vc_exp"), if on { lab.volume_curve_exponent } else { 1.0 }, 0.35);
    let lim = ctx.animate_value_with_time(ui.scope_id().with("vc_lim"), if on { lab.volume_curve_limit } else { 1.0 }, 0.3);
    let vol = ctx.animate_value_with_time(ui.scope_id().with("vc_vol"), f.volume, 0.2);
    let accent = f.accent;
    effect_card(
        ui,
        |p, rect| volume_picture(p, rect, exp, lim, vol, f.follower.punch),
        |ui| {
            toggle_row(ui, tr("Volume curve"), Some(tr("Finer steps at low volume")), &mut lab.volume_curve_enabled, accent);
            folded(ui, "volume", lab.volume_curve_enabled, |ui| {
                let items: Vec<(&str, &str)> = CURVE_SHAPES.iter().map(|c| (c.0, tr(c.1))).collect();
                let sel = CURVE_SHAPES.iter().find(|c| feq(c.2, lab.volume_curve_exponent)).map(|c| c.0);
                if let Some(k) = chips(ui, &items, sel, accent)
                    && let Some(c) = CURVE_SHAPES.iter().find(|c| c.0 == k)
                {
                    lab.volume_curve_exponent = c.2;
                }
                panel(ui, |ui| {
                    slider_row(ui, tr("Curve"), &mut lab.volume_curve_exponent, 1.0..=4.0, "", 1, false);
                    percent_row(ui, tr("Volume limit"), &mut lab.volume_curve_limit);
                    note(ui, tr("The slider's quiet half gets most of the room, so small changes stay small. The limit caps the loudest setting."));
                });
            });
        },
    );
}

/// The slider's travel against the gain it gives.
fn volume_picture(p: &Painter, rect: Rect, exponent: f32, limit: f32, volume: f32, punch: f32) {
    let plot = rect.shrink2(vec2(16.0, 14.0));
    let at = |q: f32| pos2(plot.left() + q * plot.width(), plot.bottom() - limit * q.powf(exponent) * plot.height());
    p.extend(Shape::dashed_line(&[plot.left_bottom(), plot.right_top()], Stroke::new(1.0, white(0.16)), 4.0, 4.0));
    if limit < 0.999 {
        let y = plot.bottom() - limit * plot.height();
        p.extend(Shape::dashed_line(&[pos2(plot.left(), y), pos2(plot.right(), y)], Stroke::new(1.0, white(0.3)), 2.0, 3.0));
    }
    let pts: Vec<Pos2> = (0..=60).map(|i| at(i as f32 / 60.0)).collect();
    // Soft fill under the curve.
    let mut mesh = egui::Mesh::default();
    for (k, q) in pts.iter().enumerate() {
        mesh.colored_vertex(*q, white(0.14));
        mesh.colored_vertex(pos2(q.x, plot.bottom()), white(0.0));
        if k > 0 {
            let i = (k * 2) as u32;
            mesh.add_triangle(i - 2, i - 1, i);
            mesh.add_triangle(i - 1, i, i + 1);
        }
    }
    p.add(Shape::mesh(mesh));
    p.add(Shape::line(pts, Stroke::new(2.2, Color32::WHITE)));
    for i in 1..=9 {
        let q = at(i as f32 / 10.0);
        p.line_segment([pos2(q.x, plot.bottom()), q], Stroke::new(1.0, white(0.25)));
    }
    let dot = at(volume.clamp(0.0, 1.0));
    p.circle_filled(dot, 11.0, white(0.18 + 0.2 * punch));
    p.circle_filled(dot, 5.0, Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_effects_and_status() {
        let mut lab = AudioLab::default();
        assert!(active_effects(&lab).is_empty());
        lab.reverb.enabled = true;
        lab.loudness_enabled = true;
        lab.spatial.virtual_speakers = true;
        lab.speed = 1.25;
        assert_eq!(active_effects(&lab), ["Reverb", "Loudness", "Spatial", "Playback"]);
        crate::ui::i18n::set_lang(mochy_core::settings::Language::En);
        let s = effects_status(&lab);
        assert!(s.contains("2"), "{s}");
    }

    #[test]
    fn fx_page_draws_headless() {
        let ctx = egui::Context::default();
        let mut lab = AudioLab::default();
        lab.reverb.enabled = true;
        lab.spatial.crossfeed_enabled = true;
        let mut st = FxState::default();
        let follower = LevelFollower::default();
        for _ in 0..3 {
            let mut out = ctx.run_ui(Default::default(), |ui| {
                let f = Frame { follower: &follower, spectrum: &[], t: 1.0, dt: 0.016, volume: 0.5, accent: Color32::RED };
                egui::ScrollArea::vertical().show(ui, |ui| page(ui, &mut lab, &mut st, &f));
            });
            out.textures_delta.clear();
        }
        // Drawing alone changes nothing.
        assert!(lab.reverb.enabled && lab.spatial.crossfeed_enabled && !lab.spatial.virtual_speakers);
    }
}
