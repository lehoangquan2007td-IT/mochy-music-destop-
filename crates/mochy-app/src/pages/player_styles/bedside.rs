//! The Bedside style (port of Android `BedsideTilesStyle` / `BedsideClock` /
//! `BedsideRest`): the nightstand as two tiles on black — the music on one,
//! a big quiet clock on the other. Left alone it rests: the ink dims (and
//! warms, if asked) and the tiles drift a few pixels so nothing burns in; a
//! mouse move wakes it. Double-click the clock to play / pause.

use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use mochy_core::player_look::BedsideFace;

use super::clock::{self, LocalTime};
use super::look;
use crate::actions::{Action, Ctx};
use crate::pages::NowInfo;
use crate::ui::i18n::tr;
use crate::ui::theme;
use crate::widgets::{self, icons, icons::Icon};

const MUSIC_SHARE: f32 = 0.54;
const WARM: Color32 = Color32::from_rgb(0xFF, 0x74, 0x52);
/// How dim the ink gets at rest (Android `InkDim`).
const INK_DIM: f32 = 0.62;

#[derive(Clone, Default)]
struct Rest {
    last_activity: f64,
    sleep: f32,
}

/// Ink colour for a sleep amount 0 (awake) .. 1 (resting).
pub fn ink(sleep: f32, warm: bool) -> Color32 {
    let base = if warm { widgets::lerp_color(Color32::WHITE, WARM, sleep) } else { Color32::WHITE };
    base.gamma_multiply(1.0 - INK_DIM * sleep)
}

pub fn show(ui: &mut Ui, cx: &mut Ctx, now: &NowInfo, _grad: (Color32, Color32)) {
    let rect = ui.max_rect();
    let l = look().player;
    ui.painter().rect_filled(rect, CornerRadius::ZERO, Color32::BLACK);
    // ---- rest ---------------------------------------------------------------
    let id = egui::Id::unique("mochy_bedside_rest");
    let time = ui.input(|i| i.time);
    let active = ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_down() || !i.events.is_empty());
    let mut rest: Rest = ui.ctx().data(|d| d.get_temp(id)).unwrap_or(Rest { last_activity: time, sleep: 0.0 });
    if active {
        rest.last_activity = time;
    }
    let dt = ui.input(|i| i.stable_dt).clamp(0.0, 0.1);
    let resting = l.bedside_dim_secs > 0 && time - rest.last_activity > l.bedside_dim_secs as f64;
    let target = if resting { 1.0 } else { 0.0 };
    let speed = if resting { 0.5 } else { 4.0 };
    rest.sleep += (target - rest.sleep) * (1.0 - (-dt * speed).exp());
    if (target - rest.sleep).abs() < 0.002 {
        rest.sleep = target;
    } else {
        ui.ctx().request_repaint();
    }
    let sleep = rest.sleep;
    ui.ctx().data_mut(|d| d.insert_temp(id, rest));
    let ink = ink(sleep, l.bedside_warm);
    let soft = ink.gamma_multiply(0.55);
    // Burn-in drift while resting.
    let t = time as f32;
    let drift = vec2((t / 37.0).sin() * 10.0, (t / 53.0).cos() * 8.0) * sleep;
    // The clock ticks every second; repaint just after the next one.
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));

    let area = Rect::from_min_max(rect.min + vec2(28.0, 54.0), rect.max - vec2(28.0, 24.0)).translate(drift);
    let wide = area.width() > area.height();
    let (music, info) = if wide {
        let split = area.left() + area.width() * MUSIC_SHARE;
        (Rect::from_min_max(area.min, pos2(split - 12.0, area.bottom())), Rect::from_min_max(pos2(split + 12.0, area.top()), area.max))
    } else {
        let split = area.top() + area.height() * 0.5;
        (Rect::from_min_max(area.min, pos2(area.right(), split - 12.0)), Rect::from_min_max(pos2(area.left(), split + 12.0), area.max))
    };
    let awake = 1.0 - sleep;
    music_tile(ui, cx, now, music, ink, soft, awake);
    let now_t = clock::now();
    let resp = ui.interact(info, egui::Id::unique("mochy_bedside_clock"), Sense::click());
    if resp.double_clicked() {
        cx.act(Action::Toggle);
    }
    clock_face(ui, info, l.bedside_face, &now_t, l.bedside_24h, ink, soft);
    if awake > 0.05 {
        super::close_button(ui, cx, rect.min + vec2(14.0, 10.0), Color32::WHITE.gamma_multiply(awake * 0.9));
    }
}

fn tile(ui: &Ui, r: Rect, awake: f32) {
    ui.painter().rect_filled(r, CornerRadius::same(28), Color32::from_gray(0x0E).gamma_multiply(0.4 + 0.6 * awake));
}

fn music_tile(ui: &mut Ui, cx: &mut Ctx, now: &NowInfo, r: Rect, ink: Color32, soft: Color32, awake: f32) {
    tile(ui, r, awake);
    let inner = r.shrink(28.0);
    let controls_h = 150.0;
    let side = (inner.height() - controls_h).min(inner.width()).max(80.0);
    let art = Rect::from_min_size(pos2(inner.center().x - side / 2.0, inner.top()), vec2(side, side));
    let tex = super::cover_tex(cx, now);
    widgets::paint_artwork(ui, art, tex.as_ref(), 18.0);
    // Dim the cover with the ink.
    ui.painter().rect_filled(art, CornerRadius::same(18), Color32::from_black_alpha(((1.0 - ink.a() as f32 / 255.0) * 200.0) as u8));
    let y = art.bottom() + 18.0;
    let w = inner.width();
    if let Some(song) = &now.song {
        let g = widgets::galley_trunc(ui, &song.title, theme::semibold(ui.ctx(), 22.0), ink, w);
        ui.painter().galley(pos2(inner.center().x - g.size().x / 2.0, y), g, ink);
        let g = widgets::galley_trunc(ui, &widgets::artists_line(song), FontId::proportional(16.0), soft, w);
        ui.painter().galley(pos2(inner.center().x - g.size().x / 2.0, y + 30.0), g, soft);
    } else {
        ui.painter().text(pos2(inner.center().x, y + 10.0), Align2::CENTER_TOP, tr("Nothing playing"), FontId::proportional(18.0), soft);
    }
    // A slim progress line.
    let bar = Rect::from_min_size(pos2(inner.left() + w * 0.1, y + 62.0), vec2(w * 0.8, 3.0));
    ui.painter().rect_filled(bar, CornerRadius::same(2), soft.gamma_multiply(0.35));
    let frac = if now.duration > 0.0 { (now.pos / now.duration).clamp(0.0, 1.0) as f32 } else { 0.0 };
    ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * frac, bar.height())), CornerRadius::same(2), ink);
    // Transport, fading out at rest.
    if awake > 0.05 {
        let cy = y + 100.0;
        let col = Color32::WHITE.gamma_multiply(awake);
        for (dx, icon, action) in [(-64.0, Icon::Prev, Action::Prev), (0.0, if now.playing { Icon::Pause } else { Icon::Play }, Action::Toggle), (64.0, Icon::Next, Action::Next)] {
            let b = Rect::from_center_size(pos2(inner.center().x + dx, cy), vec2(44.0, 44.0));
            let resp = ui.interact(b, egui::Id::unique(("mochy_bedside_btn", dx as i32)), Sense::click());
            if resp.hovered() {
                ui.painter().circle_filled(b.center(), 24.0, Color32::from_white_alpha((24.0 * awake) as u8));
            }
            icons::paint(ui.painter(), b.shrink(if dx == 0.0 { 8.0 } else { 11.0 }), icon, col);
            if resp.clicked() {
                cx.act(action);
            }
        }
    }
}

fn clock_face(ui: &Ui, r: Rect, face: BedsideFace, t: &LocalTime, h24: bool, ink: Color32, soft: Color32) {
    let p = ui.painter();
    match face {
        BedsideFace::Big => {
            let digits = (r.height() * 0.42).min(r.width() / 2.9);
            let text = format!("{}:{}", t.hours_text(h24), t.minutes_text());
            let g = p.layout_no_wrap(text, theme::semibold(ui.ctx(), digits), ink);
            let date = p.layout_no_wrap(t.date_text(), FontId::proportional((digits * 0.13).max(12.0)), soft);
            let total_h = date.size().y + digits * 0.04 + g.size().y;
            let top = r.center().y - total_h / 2.0;
            p.galley(pos2(r.center().x - date.size().x / 2.0, top), date.clone(), soft);
            let gx = r.center().x - g.size().x / 2.0;
            let gy = top + date.size().y + digits * 0.04;
            p.galley(pos2(gx, gy), g.clone(), ink);
            if let Some(pm) = t.period(h24) {
                p.text(pos2(gx + g.size().x + 6.0, gy + digits * 0.12), Align2::LEFT_TOP, pm, FontId::proportional(digits * 0.14), soft);
            }
        }
        BedsideFace::Stacked => {
            let line = (r.height() * 0.36).min(r.width() * 0.5);
            let font = theme::bold(ui.ctx(), line);
            let hours = p.layout_no_wrap(format!("{:0>2}", t.hours_text(h24)), font.clone(), ink);
            let mins = p.layout_no_wrap(t.minutes_text(), font, soft);
            let gap = -line * 0.12;
            let total = hours.size().y + mins.size().y + gap + line * 0.25;
            let top = r.center().y - total / 2.0;
            p.galley(pos2(r.center().x - hours.size().x / 2.0, top), hours.clone(), ink);
            let my = top + hours.size().y + gap;
            p.galley(pos2(r.center().x - mins.size().x / 2.0, my), mins.clone(), soft);
            let mut date = t.date_text();
            if let Some(pm) = t.period(h24) {
                date = format!("{date} · {pm}");
            }
            p.text(pos2(r.center().x, my + mins.size().y + line * 0.05), Align2::CENTER_TOP, date, FontId::proportional((line * 0.15).max(12.0)), soft);
        }
        BedsideFace::Analog => {
            let dial = (r.height() * 0.72).min(r.width() * 0.78);
            let c = r.center() - vec2(0.0, dial * 0.06);
            analog_dial(p, c, dial / 2.0, t, ink);
            p.text(pos2(c.x, c.y + dial / 2.0 + dial * 0.06), Align2::CENTER_TOP, t.date_text(), FontId::proportional((dial * 0.07).max(12.0)), soft);
        }
    }
}

/// Sixty ticks and two hands (Android `AnalogDial`).
fn analog_dial(p: &egui::Painter, c: Pos2, r: f32, t: &LocalTime, ink: Color32) {
    let faint = ink.gamma_multiply(0.35);
    for i in 0..60 {
        let a = (i as f32 * 6.0 - 90.0).to_radians();
        let bold = i % 5 == 0;
        let inner = if bold { r * 0.84 } else { r * 0.92 };
        let dir = vec2(a.cos(), a.sin());
        p.line_segment([c + dir * inner, c + dir * r * 0.98], Stroke::new(if bold { r * 0.022 } else { r * 0.01 }, if bold { ink } else { faint }));
    }
    let minute = (t.minute as f32 + t.second as f32 / 60.0) * 6.0;
    let hour = ((t.hour % 12) as f32 + t.minute as f32 / 60.0) * 30.0;
    for (deg, len, w) in [(hour, 0.48, 0.055), (minute, 0.74, 0.032)] {
        let a = (deg - 90.0).to_radians();
        p.line_segment([c, c + vec2(a.cos(), a.sin()) * r * len], Stroke::new(r * w, ink));
    }
    p.circle_filled(c, r * 0.045, ink);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ink_dims_and_warms_at_rest() {
        let awake = ink(0.0, true);
        assert_eq!(awake, Color32::WHITE);
        let asleep = ink(1.0, true);
        assert!(asleep.a() < 120);
        assert!(asleep.r() > asleep.b());
        let cold = ink(1.0, false);
        assert_eq!(cold.r(), cold.b());
    }
}
