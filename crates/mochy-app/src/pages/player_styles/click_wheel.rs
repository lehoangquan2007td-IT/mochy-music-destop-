//! The Click Wheel style (desktop port of Android's iPod player style, as an
//! original drawing — no trademarks or logos): a pocket player with a screen
//! and a click wheel. Turn the wheel (drag round it or scroll over it) for
//! the volume; click the centre to scrub instead; MENU closes the player and
//! the other three sides skip back, play/pause and skip forward.

use egui::{Align2, Color32, CornerRadius, FontId, Layout, Mesh, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use mochy_core::player_look::WheelTheme;

use super::{look, paint_background};
use crate::actions::{Action, Ctx, LoadState};
use crate::pages::now_playing::{NpUi, SideData};
use crate::pages::player_bar::in_rect;
use crate::pages::{NowInfo, SideTab};
use crate::ui::i18n::tr;
use crate::ui::lyrics_view::LyricsView;
use crate::widgets;

/// Body proportions (Android `IpodGeometry`, from the mockup).
const SCREEN_LEFT: f32 = 43.0 / 572.0;
const SCREEN_WIDTH: f32 = 486.0 / 572.0;
const SCREEN_ASPECT: f32 = 367.0 / 486.0;
const WHEEL_SHARE: f32 = 375.0 / 572.0;
const BODY_ASPECT: f32 = 1248.0 / 572.0;

struct Colors {
    body_top: Color32,
    body_bottom: Color32,
    bezel: Color32,
    wheel_face: Color32,
    wheel_edge: Color32,
    wheel_ink: Color32,
    center: Color32,
    center_pressed: Color32,
    screen_bg: Color32,
    ink: Color32,
    soft: Color32,
    header: [Color32; 3],
    header_line: Color32,
    track: [Color32; 2],
}

fn colors(theme: WheelTheme) -> Colors {
    let rgb = Color32::from_rgb;
    let light_screen = theme != WheelTheme::TrueDark;
    let (body_top, body_bottom, bezel) = match theme {
        WheelTheme::Silver => (rgb(0xE9, 0xE9, 0xE9), rgb(0xD9, 0xD9, 0xD9), rgb(0x8A, 0x8A, 0x8A)),
        WheelTheme::Black | WheelTheme::Red => (rgb(0x7D, 0x7C, 0x7D), rgb(0x1E, 0x1E, 0x1E), rgb(0x0E, 0x0E, 0x0E)),
        WheelTheme::TrueDark => (Color32::BLACK, Color32::BLACK, rgb(0x1C, 0x1C, 0x1C)),
    };
    let (wheel_face, wheel_edge, wheel_ink, center, center_pressed) = match theme {
        WheelTheme::Silver => (Color32::WHITE, rgb(0xB9, 0xB9, 0xB9), rgb(0xAF, 0xAF, 0xAF), rgb(0xE6, 0xE6, 0xE6), rgb(0xD4, 0xD4, 0xD4)),
        WheelTheme::Black => (rgb(0x2A, 0x2A, 0x2A), rgb(0x1A, 0x1A, 0x1A), Color32::WHITE, rgb(0x7D, 0x7C, 0x7D), rgb(0x65, 0x64, 0x65)),
        WheelTheme::Red => (rgb(0xDE, 0x20, 0x29), rgb(0x1A, 0x1A, 0x1A), Color32::WHITE, rgb(0x5D, 0x5C, 0x5D), rgb(0x4A, 0x49, 0x4A)),
        WheelTheme::TrueDark => (rgb(0x12, 0x12, 0x12), rgb(0x24, 0x24, 0x24), rgb(0x8A, 0x8A, 0x8A), rgb(0x05, 0x05, 0x05), rgb(0x1C, 0x1C, 0x1C)),
    };
    if light_screen {
        Colors {
            body_top,
            body_bottom,
            bezel,
            wheel_face,
            wheel_edge,
            wheel_ink,
            center,
            center_pressed,
            screen_bg: Color32::WHITE,
            ink: Color32::BLACK,
            soft: rgb(0x6F, 0x6F, 0x6F),
            header: [rgb(0xFD, 0xFD, 0xFD), rgb(0xE2, 0xE4, 0xE6), rgb(0xA9, 0xAE, 0xB3)],
            header_line: rgb(0x8C, 0x8C, 0x8C),
            track: [rgb(0xFD, 0xFD, 0xFD), rgb(0xD4, 0xD4, 0xD4)],
        }
    } else {
        Colors {
            body_top,
            body_bottom,
            bezel,
            wheel_face,
            wheel_edge,
            wheel_ink,
            center,
            center_pressed,
            screen_bg: rgb(0x0B, 0x0B, 0x0B),
            ink: rgb(0xF2, 0xF2, 0xF2),
            soft: rgb(0x9C, 0x9C, 0x9C),
            header: [rgb(0x3A, 0x3A, 0x3A), rgb(0x24, 0x24, 0x24), rgb(0x15, 0x15, 0x15)],
            header_line: rgb(0x2E, 0x2E, 0x2E),
            track: [rgb(0x3A, 0x3A, 0x3A), rgb(0x1C, 0x1C, 0x1C)],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Default)]
enum Mode {
    #[default]
    Playing,
    Volume,
    Scrub,
}

#[derive(Clone, Default)]
struct WheelState {
    last_angle: Option<f32>,
    acc: f32,
    mode: Mode,
    /// When the volume / scrub overlay goes back to the progress view.
    until: f64,
    scrub: Option<f64>,
    pressed: Option<u8>,
}

fn vgrad(p: &egui::Painter, r: Rect, stops: &[Color32]) {
    let n = stops.len().max(2) - 1;
    let mut m = Mesh::default();
    for (k, c) in stops.iter().enumerate() {
        let y = r.top() + r.height() * k as f32 / n as f32;
        m.colored_vertex(pos2(r.left(), y), *c);
        m.colored_vertex(pos2(r.right(), y), *c);
        if k > 0 {
            let i = (k * 2) as u32;
            m.add_triangle(i - 2, i - 1, i);
            m.add_triangle(i - 1, i, i + 1);
        }
    }
    p.add(Shape::mesh(m));
}

#[allow(clippy::too_many_arguments)]
pub fn show(ui: &mut Ui, cx: &mut Ctx, now: &NowInfo, d: &SideData, tab: &mut SideTab, np: &mut NpUi, lv: &mut LyricsView, grad: (Color32, Color32)) {
    let rect = ui.max_rect();
    let l = look();
    paint_background(ui, cx, now, rect, grad, mochy_core::player_look::PlayerBackground::DarkGradient, l.glass, true);
    super::close_button(ui, cx, rect.min + vec2(16.0, 12.0), widgets::TEXT);
    let side = l.player.show_side_panel && rect.width() > 980.0;
    let device_area = if side {
        Rect::from_min_max(rect.min + vec2(0.0, 56.0), pos2(rect.left() + rect.width() * 0.45, rect.bottom() - 20.0))
    } else {
        Rect::from_min_max(rect.min + vec2(0.0, 56.0), rect.max - vec2(0.0, 20.0))
    };
    device(ui, cx, now, device_area, colors(l.player.wheel_theme));
    if side {
        let right = Rect::from_min_max(pos2(device_area.right() + 24.0, rect.top() + 64.0), rect.max - vec2(32.0, 24.0));
        in_rect(ui, right, Layout::top_down(egui::Align::Min), |ui| {
            crate::pages::now_playing::tabs(ui, tab);
            ui.add_space(12.0);
            let r = ui.available_rect_before_wrap();
            ui.painter().rect_filled(r, CornerRadius::same(18), Color32::from_black_alpha(60));
            in_rect(ui, r.shrink(14.0), Layout::top_down(egui::Align::Min), |ui| {
                crate::pages::now_playing::tab_content(ui, cx, *tab, now, d, np, lv);
            });
        });
    }
    let _ = LoadState::<()>::Loading;
}

fn device(ui: &mut Ui, cx: &mut Ctx, now: &NowInfo, area: Rect, c: Colors) {
    let body_w = (area.width() * 0.86).min(area.height() * 0.96 / BODY_ASPECT);
    let body_h = body_w * BODY_ASPECT;
    let body = Rect::from_center_size(area.center(), vec2(body_w, body_h));
    let p = ui.painter().clone();
    let corner = CornerRadius::same((body_w * 0.1224).min(255.0) as u8);
    p.rect_filled(body.translate(vec2(0.0, 12.0)).expand(4.0), corner, Color32::from_black_alpha(110));
    p.rect_filled(body, corner, c.body_bottom);
    // Body gradient as a rounded fan, so the corners stay round.
    let outline = crate::ui::audio_lab::kit::rounded_poly(body, body_w * 0.1224, 10);
    let mut m = Mesh::default();
    let col_at = |y: f32| widgets::lerp_color(c.body_top, c.body_bottom, (y - body.top()) / body.height());
    m.colored_vertex(body.center(), col_at(body.center().y));
    for (k, q) in outline.iter().enumerate() {
        m.colored_vertex(*q, col_at(q.y));
        let n = outline.len() as u32;
        m.add_triangle(0, k as u32 + 1, (k as u32 + 1) % n + 1);
    }
    p.add(Shape::mesh(m));
    p.rect_stroke(body, corner, Stroke::new(1.0, Color32::from_black_alpha(60)), egui::StrokeKind::Inside);

    // Screen.
    let screen = Rect::from_min_size(
        pos2(body.left() + body_w * SCREEN_LEFT, body.top() + body_w * 0.1976),
        vec2(body_w * SCREEN_WIDTH, body_w * SCREEN_WIDTH * SCREEN_ASPECT),
    );
    let sc = CornerRadius::same((screen.width() * 0.045) as u8);
    p.rect_filled(screen.expand(screen.width() * 0.012), sc, c.bezel);
    p.rect_filled(screen, sc, c.screen_bg);
    let wheel_c = pos2(body.center().x, body.top() + body_w * 1.5752);
    let wheel_r = body_w * WHEEL_SHARE / 2.0;

    let id = ui.scope_id().with("click_wheel");
    let mut st: WheelState = ui.ctx().data(|d| d.get_temp(id)).unwrap_or_default();
    let time = ui.input(|i| i.time);
    wheel_input(ui, cx, now, wheel_c, wheel_r, id, &mut st, time);
    if st.mode != Mode::Playing && time > st.until {
        if st.mode == Mode::Scrub
            && let Some(t) = st.scrub.take()
        {
            cx.act(Action::Seek(t));
        }
        st.mode = Mode::Playing;
    }
    screen_content(ui, cx, now, screen, &c, &st);
    wheel_paint(&p, wheel_c, wheel_r, &c, st.pressed, now.playing);
    if st.mode != Mode::Playing {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, st));
}

#[allow(clippy::too_many_arguments)]
fn wheel_input(ui: &mut Ui, cx: &mut Ctx, now: &NowInfo, c: Pos2, r: f32, id: egui::Id, st: &mut WheelState, time: f64) {
    let rect = Rect::from_center_size(c, vec2(r * 2.0, r * 2.0));
    let resp = ui.interact(rect, id.with("wheel"), Sense::click_and_drag());
    let center_r = r * 0.36;
    let inside = |q: Pos2| q.distance(c) <= r;
    // One tick = a 15° turn or a scroll notch.
    let mut ticks = 0i32;
    if resp.dragged()
        && let Some(q) = resp.interact_pointer_pos()
        && q.distance(c) > center_r * 0.6
    {
        let a = (q.y - c.y).atan2(q.x - c.x);
        if let Some(last) = st.last_angle {
            let mut d = a - last;
            if d > std::f32::consts::PI {
                d -= std::f32::consts::TAU;
            } else if d < -std::f32::consts::PI {
                d += std::f32::consts::TAU;
            }
            st.acc += d;
        }
        st.last_angle = Some(a);
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else {
        st.last_angle = None;
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            st.acc += scroll / 40.0 * (15f32).to_radians();
            ui.input_mut(|i| i.smooth_scroll_delta.y = 0.0);
        }
    }
    let step = 15f32.to_radians();
    while st.acc >= step {
        st.acc -= step;
        ticks += 1;
    }
    while st.acc <= -step {
        st.acc += step;
        ticks -= 1;
    }
    if ticks != 0 {
        match st.mode {
            Mode::Scrub => {
                let dur = now.duration.max(1.0);
                let base = st.scrub.unwrap_or(now.pos);
                st.scrub = Some((base + ticks as f64 * (dur / 100.0).max(2.0)).clamp(0.0, dur));
            }
            _ => {
                st.mode = Mode::Volume;
                cx.act(Action::SetVolume((now.volume + ticks as f32 * 0.04).clamp(0.0, 1.0)));
            }
        }
        st.until = time + 1.6;
    }
    // Clicks: which part of the wheel.
    st.pressed = None;
    if let Some(q) = resp.interact_pointer_pos().filter(|_| resp.is_pointer_button_down_on()).filter(|q| inside(*q)) {
        st.pressed = Some(zone(q, c, center_r));
    }
    if resp.clicked()
        && let Some(q) = resp.interact_pointer_pos()
        && inside(q)
    {
        match zone(q, c, center_r) {
            0 => {
                // Centre: scrub mode on / off.
                if st.mode == Mode::Scrub {
                    if let Some(t) = st.scrub.take() {
                        cx.act(Action::Seek(t));
                    }
                    st.mode = Mode::Playing;
                } else {
                    st.mode = Mode::Scrub;
                    st.scrub = Some(now.pos);
                    st.until = time + 4.0;
                }
            }
            1 => {
                if st.mode != Mode::Playing {
                    st.mode = Mode::Playing;
                    st.scrub = None;
                } else {
                    cx.act(Action::SetNowPlaying(false));
                }
            }
            2 => cx.act(Action::Next),
            3 => cx.act(Action::Toggle),
            _ => cx.act(Action::Prev),
        }
    }
    if resp.hovered() && !resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

/// 0 centre, 1 menu (top), 2 next (right), 3 play (bottom), 4 previous (left).
fn zone(q: Pos2, c: Pos2, center_r: f32) -> u8 {
    let v = q - c;
    if v.length() <= center_r {
        return 0;
    }
    if v.y.abs() > v.x.abs() {
        if v.y < 0.0 { 1 } else { 3 }
    } else if v.x > 0.0 {
        2
    } else {
        4
    }
}

fn wheel_paint(p: &egui::Painter, c: Pos2, r: f32, col: &Colors, pressed: Option<u8>, playing: bool) {
    p.circle_filled(c, r, col.wheel_face);
    p.circle_stroke(c, r, Stroke::new(1.0, col.wheel_edge));
    if let Some(z) = pressed.filter(|z| *z != 0) {
        let a = match z {
            1 => -std::f32::consts::FRAC_PI_2,
            2 => 0.0,
            3 => std::f32::consts::FRAC_PI_2,
            _ => std::f32::consts::PI,
        };
        super::radial(p, c + vec2(a.cos(), a.sin()) * r * 0.68, r * 0.45, Color32::from_black_alpha(26));
    }
    let ink = col.wheel_ink;
    let u = r / 100.0;
    // MENU
    p.text(c - vec2(0.0, r * 0.72), Align2::CENTER_CENTER, "MENU", FontId::proportional(13.0 * u.max(0.8)), ink);
    // Next ▶▶|
    let n = c + vec2(r * 0.72, 0.0);
    tri(p, n - vec2(7.0 * u, 0.0), 6.0 * u, ink);
    tri(p, n + vec2(1.0 * u, 0.0), 6.0 * u, ink);
    p.line_segment([n + vec2(7.0 * u, -6.0 * u), n + vec2(7.0 * u, 6.0 * u)], Stroke::new(2.0 * u, ink));
    // Previous |◀◀
    let pv = c - vec2(r * 0.72, 0.0);
    tri_left(p, pv + vec2(7.0 * u, 0.0), 6.0 * u, ink);
    tri_left(p, pv - vec2(1.0 * u, 0.0), 6.0 * u, ink);
    p.line_segment([pv - vec2(7.0 * u, 6.0 * u), pv - vec2(7.0 * u, -6.0 * u)], Stroke::new(2.0 * u, ink));
    // Play / pause ▶ ||
    let pl = c + vec2(0.0, r * 0.72);
    tri(p, pl - vec2(5.0 * u, 0.0), 5.5 * u, ink);
    for dx in [4.0, 8.0] {
        p.line_segment([pl + vec2(dx * u, -5.5 * u), pl + vec2(dx * u, 5.5 * u)], Stroke::new(2.0 * u, ink));
    }
    let _ = playing;
    // Centre button.
    let cr = r * 0.36;
    let fill = if pressed == Some(0) { col.center_pressed } else { col.center };
    p.circle_filled(c, cr, fill);
    p.circle_stroke(c, cr, Stroke::new(1.0, col.wheel_edge));
}

fn tri(p: &egui::Painter, c: Pos2, s: f32, col: Color32) {
    p.add(Shape::convex_polygon(vec![c + vec2(-s * 0.6, -s), c + vec2(s * 0.8, 0.0), c + vec2(-s * 0.6, s)], col, Stroke::NONE));
}

fn tri_left(p: &egui::Painter, c: Pos2, s: f32, col: Color32) {
    p.add(Shape::convex_polygon(vec![c + vec2(s * 0.6, -s), c + vec2(s * 0.6, s), c + vec2(-s * 0.8, 0.0)], col, Stroke::NONE));
}

fn screen_content(ui: &mut Ui, cx: &mut Ctx, now: &NowInfo, screen: Rect, c: &Colors, st: &WheelState) {
    let p = ui.painter().with_clip_rect(screen);
    let u = screen.width() / 486.0;
    // Header bar.
    let header = Rect::from_min_size(screen.min, vec2(screen.width(), 38.0 * u));
    vgrad(&p, header, &c.header);
    p.hline(header.x_range(), header.bottom(), Stroke::new(1.0, c.header_line));
    p.text(header.center(), Align2::CENTER_CENTER, tr("Now Playing"), FontId::proportional(17.0 * u), c.ink);
    // Play state glyph (left) and a battery (right).
    let g = pos2(header.left() + 18.0 * u, header.center().y);
    if now.playing {
        tri(&p, g, 6.0 * u, c.ink);
    } else {
        for dx in [-3.0, 3.0] {
            p.line_segment([g + vec2(dx * u, -6.0 * u), g + vec2(dx * u, 6.0 * u)], Stroke::new(2.5 * u, c.ink));
        }
    }
    let bat = Rect::from_center_size(pos2(header.right() - 26.0 * u, header.center().y), vec2(28.0 * u, 13.0 * u));
    p.rect_stroke(bat, CornerRadius::same(2), Stroke::new(1.2 * u, c.ink), egui::StrokeKind::Middle);
    p.rect_filled(Rect::from_min_size(bat.min + vec2(2.0, 2.0) * u, vec2(bat.width() * 0.7, bat.height() - 4.0 * u)), CornerRadius::same(1), Color32::from_rgb(0x6B, 0xC2, 0x5A));
    p.rect_filled(Rect::from_center_size(pos2(bat.right() + 2.0 * u, bat.center().y), vec2(2.5 * u, 6.0 * u)), CornerRadius::ZERO, c.ink);

    let body = Rect::from_min_max(pos2(screen.left() + 16.0 * u, header.bottom() + 14.0 * u), pos2(screen.right() - 16.0 * u, screen.bottom() - 64.0 * u));
    let art_side = body.height().min(body.width() * 0.44);
    let art = Rect::from_min_size(pos2(body.left(), body.top() + (body.height() - art_side) / 2.0), vec2(art_side, art_side));
    let tex = super::cover_tex(cx, now);
    widgets::paint_artwork(ui, art, tex.as_ref(), 4.0 * u);
    // Reflection under the cover.
    let tx = art.right() + 16.0 * u;
    let tw = body.right() - tx;
    if let Some(song) = &now.song {
        let ty = art.top() + art_side * 0.18;
        widgets::paint_text(ui, pos2(tx, ty), &song.title, FontId::proportional(19.0 * u), c.ink, tw);
        widgets::paint_text(ui, pos2(tx, ty + 28.0 * u), &widgets::artists_line(song), FontId::proportional(15.0 * u), c.soft, tw);
        if let Some(a) = &song.album_name {
            widgets::paint_text(ui, pos2(tx, ty + 52.0 * u), a, FontId::proportional(15.0 * u), c.soft, tw);
        }
    } else {
        p.text(pos2(tx, art.center().y), Align2::LEFT_CENTER, tr("Nothing playing"), FontId::proportional(16.0 * u), c.soft);
    }

    // Bottom: progress / volume / scrub.
    let bar = Rect::from_min_size(pos2(screen.left() + 22.0 * u, screen.bottom() - 46.0 * u), vec2(screen.width() - 44.0 * u, 12.0 * u));
    vgrad(&p, bar, &c.track);
    p.rect_stroke(bar, CornerRadius::ZERO, Stroke::new(1.0, c.header_line), egui::StrokeKind::Middle);
    let fill = |frac: f32| Rect::from_min_size(bar.min, vec2(bar.width() * frac.clamp(0.0, 1.0), bar.height()));
    let blue = [Color32::from_rgb(0x9C, 0xC8, 0xF4), Color32::from_rgb(0x3E, 0x8E, 0xE0)];
    let font = FontId::proportional(12.0 * u);
    let dur = now.duration.max(0.0);
    match st.mode {
        Mode::Volume => {
            vgrad(&p, fill(now.volume), &blue);
            p.text(pos2(bar.left(), bar.bottom() + 12.0 * u), Align2::LEFT_CENTER, tr("Volume"), font, c.soft);
        }
        Mode::Scrub | Mode::Playing => {
            let pos = if st.mode == Mode::Scrub { st.scrub.unwrap_or(now.pos) } else { now.pos };
            let frac = if dur > 0.0 { (pos / dur) as f32 } else { 0.0 };
            vgrad(&p, fill(frac), &blue);
            if st.mode == Mode::Scrub {
                let x = bar.left() + bar.width() * frac.clamp(0.0, 1.0);
                let dy = bar.height() * 0.9;
                p.add(Shape::convex_polygon(
                    vec![pos2(x, bar.center().y - dy), pos2(x + dy * 0.7, bar.center().y), pos2(x, bar.center().y + dy), pos2(x - dy * 0.7, bar.center().y)],
                    c.ink,
                    Stroke::NONE,
                ));
            }
            p.text(pos2(bar.left(), bar.bottom() + 12.0 * u), Align2::LEFT_CENTER, widgets::fmt_time(pos), font.clone(), c.ink);
            let remain = if dur > 0.0 { format!("-{}", widgets::fmt_time((dur - pos).max(0.0))) } else { "--:--".into() };
            p.text(pos2(bar.right(), bar.bottom() + 12.0 * u), Align2::RIGHT_CENTER, remain, font, c.ink);
            if st.mode == Mode::Scrub {
                p.text(pos2(bar.center().x, bar.top() - 10.0 * u), Align2::CENTER_CENTER, tr("Scrubbing"), FontId::proportional(11.0 * u), c.soft);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_zones() {
        let c = pos2(100.0, 100.0);
        assert_eq!(zone(pos2(100.0, 100.0), c, 30.0), 0);
        assert_eq!(zone(pos2(100.0, 20.0), c, 30.0), 1);
        assert_eq!(zone(pos2(180.0, 100.0), c, 30.0), 2);
        assert_eq!(zone(pos2(100.0, 180.0), c, 30.0), 3);
        assert_eq!(zone(pos2(20.0, 100.0), c, 30.0), 4);
    }
}
