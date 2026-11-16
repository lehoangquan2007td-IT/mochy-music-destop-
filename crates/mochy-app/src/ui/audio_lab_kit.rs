//! Audio Lab v2 drawing kit (port of Android `LabVisualizer`, `LabHeaderCard`,
//! `FxCardStack` and the hub sketches in `AudioLabScreen`): a music level
//! follower, spectrum bars, the hub cards with their live sketches and the
//! fanned stack of effect cards. Everything is drawn with the egui painter.

use egui::epaint::TextShape;
use egui::{Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2, pos2, vec2};

use super::theme;

/// Hub card fill (Android `HubCard`) and sketch ink (`Sketch`).
pub const HUB_CARD: Color32 = Color32::from_rgb(0x22, 0x22, 0x25);
pub const SKETCH: Color32 = Color32::from_rgb(0x6A, 0x6A, 0x70);
pub const STATUS_ON: Color32 = Color32::from_rgb(0x8B, 0xE0, 0x7A);
pub const STATUS_OFF: Color32 = Color32::from_rgb(0x5C, 0x5C, 0x63);
pub const STATUS_TEXT: Color32 = Color32::from_rgb(0x8A, 0x8A, 0x8E);
/// Header card colours (Android `LabHeaderCard`).
pub const CARD_FILL: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x1F);
pub const CARD_EDGE: Color32 = Color32::from_rgb(0x36, 0x36, 0x3C);
pub const CARD_PANEL: Color32 = Color32::from_rgb(0x14, 0x14, 0x16);
pub const CARD_BARS: Color32 = Color32::from_rgb(0x5A, 0x5A, 0x60);
pub const CARD_INK: Color32 = Color32::from_rgb(0x7E, 0x7E, 0x84);
pub const BLUE: Color32 = Color32::from_rgb(0x6B, 0xBA, 0xF2);
pub const PINK: Color32 = Color32::from_rgb(0xF2, 0x8B, 0xA8);

pub fn white(a: f32) -> Color32 {
    Color32::from_white_alpha((a.clamp(0.0, 1.0) * 255.0) as u8)
}

pub fn alpha(c: Color32, a: f32) -> Color32 {
    c.gamma_multiply(a.clamp(0.0, 1.0))
}

// ---------------------------------------------------------------------------
// Music follower
// ---------------------------------------------------------------------------

/// The music's level and its beats, from the engine spectrum (Android
/// `LevelFollower`): `level` ≈ 0..1 loudness, `punch` jumps on transients.
#[derive(Debug, Clone, Default)]
pub struct LevelFollower {
    pub level: f32,
    pub punch: f32,
    slow: f32,
    pub live: bool,
}

impl LevelFollower {
    /// Feed the latest 64-band dBFS spectrum (empty = silence).
    pub fn update(&mut self, spectrum: &[f32], dt: f32) {
        let dt = dt.clamp(0.0, 0.1);
        let target = if spectrum.is_empty() {
            0.0
        } else {
            // Mean energy of the bands, weighted to the bass/mids where beats live.
            let n = spectrum.len();
            let mut sum = 0.0;
            let mut w = 0.0;
            for (i, &db) in spectrum.iter().enumerate() {
                let weight = if i < n / 3 { 1.5 } else { 1.0 };
                sum += 10f32.powf(db.clamp(-120.0, 6.0) / 20.0) * weight;
                w += weight;
            }
            ((sum / w) * 6.0).clamp(0.0, 1.5)
        };
        self.live = target > 0.01;
        let up = 1.0 - (-dt * 24.0).exp();
        let down = 1.0 - (-dt * 6.0).exp();
        self.level += (target - self.level) * if target > self.level { up } else { down };
        self.slow += (self.level - self.slow) * (1.0 - (-dt * 2.0).exp());
        let rise = ((self.level - self.slow) * 4.0).clamp(0.0, 1.0);
        self.punch = (self.punch - dt * 3.0).max(rise).clamp(0.0, 1.0);
    }
}

/// `count` bar heights 0..1 from the spectrum, tilted so the treble reads as
/// evenly as the bass; an idle wave when nothing plays (Android `LabSpectrum`).
pub fn spectrum_bars(spectrum: &[f32], count: usize, time: f32, out: &mut Vec<f32>, dt: f32) {
    let count = count.max(1);
    if out.len() != count {
        *out = vec![0.0; count];
    }
    let live = spectrum.iter().any(|&d| d > -85.0);
    let up = 1.0 - (-dt * 30.0).exp();
    let down = 1.0 - (-dt * 7.0).exp();
    for (i, v) in out.iter_mut().enumerate() {
        let target = if live {
            let n = spectrum.len();
            let from = i * n / count;
            let to = ((i + 1) * n / count).max(from + 1).min(n);
            let db = spectrum[from..to].iter().copied().fold(-120.0f32, f32::max);
            let tilt = 14.0 * i as f32 / (count.max(2) - 1) as f32;
            ((db + tilt + 66.0) / 54.0).clamp(0.0, 1.0)
        } else {
            0.17 + 0.1 * (time * 1.3 + i as f32 * 0.62).sin() + 0.05 * (time * 2.1 - i as f32 * 1.13).sin()
        };
        *v += (target - *v) * if target > *v { up } else { down };
    }
}

// ---------------------------------------------------------------------------
// Hub cards (Android `BigHubCard` / `SmallHubCard`)
// ---------------------------------------------------------------------------

/// A hub card that doubles as a tab: status line with a dot, a big title and
/// a live sketch on the right. Returns the click response.
pub fn hub_card(
    ui: &mut Ui,
    size: Vec2,
    title: &str,
    status: &str,
    on: bool,
    selected: bool,
    accent: Color32,
    sketch: impl FnOnce(&Painter, Rect),
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let press = ui.ctx().animate_bool_with_time(resp.id.with("press"), resp.is_pointer_button_down_on(), 0.12);
    let rect = rect.shrink(press * 3.0);
    let p = ui.painter_at(rect);
    let fill = if resp.hovered() { theme::ROW } else { HUB_CARD };
    p.rect_filled(rect, CornerRadius::same(24), fill);
    sketch(&p, rect);
    if selected {
        p.rect_stroke(rect.shrink(1.0), CornerRadius::same(23), Stroke::new(2.0, accent), egui::StrokeKind::Inside);
    }
    let small = size.y < 100.0;
    let left = rect.left() + if small { 18.0 } else { 22.0 };
    let dot_col = if on { STATUS_ON } else { STATUS_OFF };
    let status_y = rect.top() + if small { rect.height() * 0.62 } else { 26.0 };
    let title_y = if small { rect.top() + rect.height() * 0.36 } else { rect.bottom() - 34.0 };
    let max_w = rect.width() * 0.62;
    p.circle_filled(pos2(left + 3.5, status_y), 3.5, dot_col);
    let g = p.layout_no_wrap(truncate(status, (max_w / 6.6) as usize), FontId::monospace(10.0), STATUS_TEXT);
    p.galley(pos2(left + 13.0, status_y - g.size().y / 2.0), g, STATUS_TEXT);
    let font = theme::bold(ui.ctx(), if small { 18.0 } else { 26.0 });
    p.text(pos2(left, title_y), Align2::LEFT_CENTER, title, font, Color32::WHITE);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max.max(4) {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.max(4) - 1).collect();
        t.push('…');
        t
    }
}

/// Rotate `p` around `c` by `a` radians.
pub fn rot(p: Pos2, c: Pos2, a: f32) -> Pos2 {
    let (s, co) = a.sin_cos();
    let d = p - c;
    pos2(c.x + d.x * co - d.y * s, c.y + d.x * s + d.y * co)
}

/// Rounded rectangle as a polygon (for rotated drawing).
pub fn rounded_poly(rect: Rect, radius: f32, segments: usize) -> Vec<Pos2> {
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let corners = [
        (pos2(rect.right() - r, rect.top() + r), -std::f32::consts::FRAC_PI_2),
        (pos2(rect.right() - r, rect.bottom() - r), 0.0),
        (pos2(rect.left() + r, rect.bottom() - r), std::f32::consts::FRAC_PI_2),
        (pos2(rect.left() + r, rect.top() + r), std::f32::consts::PI),
    ];
    let mut pts = Vec::with_capacity(4 * (segments + 1));
    for (c, start) in corners {
        for k in 0..=segments {
            let a = start + std::f32::consts::FRAC_PI_2 * k as f32 / segments as f32;
            pts.push(pos2(c.x + r * a.cos(), c.y + r * a.sin()));
        }
    }
    pts
}

/// Three sliders on a tilted panel, set to the curve's lows, mids and highs.
pub fn eq_sketch(p: &Painter, rect: Rect, gains: [f32; 3], on: bool, punch: f32, t: f32) {
    let panel = rect.height() * 1.05;
    let origin = pos2(rect.right() - panel * 0.78, rect.top() + rect.height() * 0.05);
    let c = origin + Vec2::splat(panel / 2.0);
    let a = (-14f32).to_radians();
    let frame: Vec<Pos2> = rounded_poly(Rect::from_min_size(origin, Vec2::splat(panel)), panel * 0.2, 6)
        .into_iter()
        .map(|q| rot(q, c, a))
        .collect();
    p.add(Shape::closed_line(frame, Stroke::new(3.5, SKETCH)));
    for (i, g) in gains.iter().enumerate() {
        let x = origin.x + panel * (0.25 + i as f32 * 0.22);
        let top = origin.y + panel * 0.16;
        let bottom = origin.y + panel * 0.84;
        p.line_segment([rot(pos2(x, top), c, a), rot(pos2(x, bottom), c, a)], Stroke::new(4.0, SKETCH));
        let bob = punch * 4.0 * (t * 9.0 + i as f32 * 2.0).sin();
        let y = (top + bottom) / 2.0 - (g / 12.0).clamp(-1.0, 1.0) * (bottom - top) / 2.0 + bob;
        let k = rot(pos2(x, y), c, a);
        p.circle_filled(k, 11.0, HUB_CARD);
        p.circle_stroke(k, 11.0, Stroke::new(4.0, if on { Color32::from_rgb(0x9A, 0x9A, 0xA0) } else { SKETCH }));
    }
}

/// Big "FX" letters off the edge; a light runs through them while an effect is on.
pub fn fx_sketch(p: &Painter, rect: Rect, glow: f32, t: f32) {
    let size = rect.height() * 1.05;
    let font = FontId::proportional(size);
    let g = p.layout_no_wrap("FX".into(), font, SKETCH);
    let top_left = pos2(rect.right() - g.size().x * 0.86, rect.center().y - g.size().y / 2.0 + rect.height() * 0.06);
    p.add(TextShape::new(top_left, g.clone(), SKETCH).with_angle(-0.08));
    if glow > 0.01 {
        let sweep = (t * 0.35).rem_euclid(1.0);
        let x = top_left.x + g.size().x * (sweep * 1.6 - 0.3);
        let band = Rect::from_x_y_ranges((x - 40.0)..=(x + 40.0), rect.y_range());
        let lit = p.with_clip_rect(band.intersect(rect));
        lit.add(TextShape::new(top_left, g, white(0.85 * glow)).with_angle(-0.08));
    }
}

/// A little waveform, as coarse as the stream (Android `QualitySketch`).
pub fn quality_sketch(p: &Painter, rect: Rect, resolution: f32) {
    let area = Rect::from_min_max(pos2(rect.right() - 120.0, rect.top()), pos2(rect.right() - 14.0, rect.bottom()));
    let step = 5.0 - 3.6 * resolution;
    let levels = (3.0 + 12.0 * resolution).round();
    let mut x = 0.0;
    while x < area.width() {
        let pr = x / area.width();
        let a = (0.35 + 0.35 * (pr * 23.0).sin() + 0.3 * (pr * 57.0 + 1.0).sin()).clamp(0.0, 1.0) * (1.0 - pr * 0.6);
        let q = (a * levels).round() / levels;
        let half = q * area.height() * 0.32;
        let cx = area.left() + x;
        p.line_segment([pos2(cx, area.center().y - half), pos2(cx, area.center().y + half)], Stroke::new((step * 0.55).max(1.0), SKETCH));
        x += step;
    }
}

/// A headphone outline (AutoEq hub card), brighter while a correction plays.
pub fn headphone_sketch(p: &Painter, rect: Rect, on: bool) {
    let s = rect.height() * 0.62;
    let c = pos2(rect.right() - s * 0.75 - 10.0, rect.center().y + s * 0.08);
    let col = if on { Color32::from_rgb(0x9A, 0x9A, 0xA0) } else { SKETCH };
    let r = s * 0.42;
    let arc: Vec<Pos2> = (0..=24)
        .map(|k| {
            let a = std::f32::consts::PI + std::f32::consts::PI * k as f32 / 24.0;
            pos2(c.x + r * a.cos(), c.y + r * a.sin())
        })
        .collect();
    p.add(Shape::line(arc, Stroke::new(3.0, col)));
    for side in [-1.0f32, 1.0] {
        let cup = Rect::from_center_size(pos2(c.x + side * r, c.y + s * 0.14), vec2(s * 0.2, s * 0.36));
        p.rect_stroke(cup, CornerRadius::same(6), Stroke::new(3.0, col), egui::StrokeKind::Middle);
    }
}

// ---------------------------------------------------------------------------
// Fanned card stack (Android `FxCardStack` + `LabHeaderCard`)
// ---------------------------------------------------------------------------

/// The header's stack state: which card is in front and the fan animation.
#[derive(Default)]
pub struct CardStack {
    front: usize,
    shown_front: f32,
    last_turn: f64,
    count: usize,
}

pub const STACK_CARD: Vec2 = vec2(128.0, 186.0);

impl CardStack {
    /// Draw one card per active effect (`labels`), fanned, the front one
    /// turning to the back every few seconds or on click. `bars` are the
    /// spectrum heights shown on each card's panel.
    pub fn show(&mut self, ui: &mut Ui, labels: &[String], bars: &[f32], on_text: &str, idle_text: &str) -> Response {
        let size = vec2(STACK_CARD.x + 120.0, STACK_CARD.y + 50.0);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let now = ui.input(|i| i.time);
        let count = labels.len();
        if count != self.count {
            self.count = count;
            self.front = 0;
            self.shown_front = 0.0;
            self.last_turn = now;
        }
        if count > 1 && (resp.clicked() || now - self.last_turn > 3.2) {
            self.front += 1;
            self.last_turn = now;
        }
        let dt = ui.input(|i| i.stable_dt).clamp(0.0, 0.1);
        let target = self.front as f32;
        self.shown_front += (target - self.shown_front) * (1.0 - (-dt * 7.0).exp());
        if (target - self.shown_front).abs() < 0.002 {
            self.shown_front = target;
        } else {
            ui.ctx().request_repaint();
        }
        if !ui.is_rect_visible(rect) {
            return resp;
        }
        let p = ui.painter_at(rect.expand(30.0));
        let base = pos2(rect.left() + 22.0, rect.bottom() - 18.0);
        let n = count.max(1);
        // Paint back to front.
        let mut order: Vec<(f32, usize)> = (0..n)
            .map(|i| {
                let depth = if count == 0 { 0.0 } else { (i as f32 - self.shown_front).rem_euclid(n as f32) };
                (depth, i)
            })
            .collect();
        order.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (place, i) in order {
            let fade = (3.0 - place).clamp(0.0, 1.0);
            if fade <= 0.0 {
                continue;
            }
            let label = labels.get(i).map(String::as_str);
            let scale = 1.0 - 0.04 * place;
            let card_size = STACK_CARD * scale;
            let min = pos2(base.x + 20.0 * place, base.y - card_size.y - 10.0 * place);
            let card = Rect::from_min_size(min, card_size);
            let angle = (-10.0 + 5.0 * place).to_radians();
            let pivot = card.center();
            let poly: Vec<Pos2> = rounded_poly(card, 26.0 * scale, 6).into_iter().map(|q| rot(q, pivot, angle)).collect();
            p.add(Shape::convex_polygon(poly, alpha(CARD_FILL, fade.max(0.98)), Stroke::new(1.5, alpha(CARD_EDGE, fade))));
            let face = (1.0 - place).clamp(0.0, 1.0) * fade;
            // Panel with spectrum bars.
            let inner = card.shrink(12.0 * scale);
            let panel_top = inner.top() + if label.is_some() { 46.0 * scale } else { 0.0 };
            let panel = Rect::from_min_max(pos2(inner.left(), panel_top), inner.max);
            let panel_poly: Vec<Pos2> = rounded_poly(panel, 18.0 * scale, 5).into_iter().map(|q| rot(q, pivot, angle)).collect();
            p.add(Shape::convex_polygon(panel_poly, alpha(CARD_PANEL, fade), Stroke::NONE));
            let bar_area = Rect::from_min_max(panel.min + vec2(10.0, 14.0) * scale, panel.max - vec2(10.0, 0.0) * scale);
            let nb = bars.len().max(1);
            let slot = bar_area.width() / nb as f32;
            let bw = slot * 0.42;
            for (k, &v) in bars.iter().enumerate() {
                let x = bar_area.left() + k as f32 * slot + (slot - bw) / 2.0;
                let h = (v * bar_area.height()).max(bw);
                let r = Rect::from_min_size(pos2(x, bar_area.bottom() - h), vec2(bw, h));
                let q: Vec<Pos2> = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()].iter().map(|&c| rot(c, pivot, angle)).collect();
                p.add(Shape::convex_polygon(q, alpha(CARD_BARS, fade), Stroke::NONE));
            }
            if let Some(label) = label {
                let ink = alpha(CARD_INK, face);
                let small = FontId::monospace(9.0 * scale);
                let g = p.layout_no_wrap(on_text.to_string(), small.clone(), ink);
                p.add(TextShape::new(rot(inner.left_top() + vec2(4.0, 0.0), pivot, angle), g, ink).with_angle(angle));
                let pos_text = format!("{}/{}", i + 1, count);
                let g = p.layout_no_wrap(pos_text, small, ink);
                let w = g.size().x;
                p.add(TextShape::new(rot(pos2(inner.right() - 4.0 - w, inner.top()), pivot, angle), g, ink).with_angle(angle));
                let size = (20.0 * (7.0 / label.chars().count().max(1) as f32).min(1.0)).max(13.0) * scale;
                let big = FontId::proportional(size);
                let col = alpha(Color32::from_rgb(0xA2, 0xA2, 0xA8), face);
                let g = p.layout_no_wrap(label.to_uppercase(), big, col);
                p.add(TextShape::new(rot(inner.left_top() + vec2(4.0, 14.0 * scale), pivot, angle), g, col).with_angle(angle));
                // Further back: the name down the edge.
                let edge = (place).clamp(0.0, 1.0) * fade;
                if edge > 0.02 {
                    let col = alpha(CARD_INK, edge);
                    let g = p.layout_no_wrap(label.to_uppercase(), FontId::monospace(9.0 * scale), col);
                    let at = rot(pos2(card.right() - 6.0, card.top() + 18.0), pivot, angle);
                    p.add(TextShape::new(at, g, col).with_angle(angle + std::f32::consts::FRAC_PI_2));
                }
            }
        }
        if count == 0 {
            let g = p.layout_no_wrap(idle_text.to_string(), FontId::monospace(10.0), CARD_INK);
            p.galley(pos2(rect.left() + 170.0, rect.bottom() - 40.0), g, CARD_INK);
        } else if count > 3 {
            let txt = format!("+{}", count - 3);
            p.text(pos2(rect.left() + 26.0, rect.top() + 4.0), Align2::LEFT_TOP, txt, FontId::monospace(10.0), CARD_INK);
        }
        if count > 1 && resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        resp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follower_rises_and_falls() {
        let mut f = LevelFollower::default();
        for _ in 0..30 {
            f.update(&[-10.0; 64], 0.016);
        }
        assert!(f.level > 0.3 && f.live, "{}", f.level);
        for _ in 0..200 {
            f.update(&[], 0.016);
        }
        assert!(f.level < 0.02 && !f.live);
    }

    #[test]
    fn bars_follow_spectrum_and_idle() {
        let mut out = Vec::new();
        for _ in 0..60 {
            spectrum_bars(&[-20.0; 64], 10, 0.0, &mut out, 0.016);
        }
        assert_eq!(out.len(), 10);
        assert!(out.iter().all(|&v| v > 0.5));
        let mut idle = Vec::new();
        for _ in 0..60 {
            spectrum_bars(&[], 10, 1.0, &mut idle, 0.016);
        }
        assert!(idle.iter().all(|&v| (0.0..0.4).contains(&v)));
    }

    #[test]
    fn rounded_poly_and_rotation() {
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 50.0));
        let pts = rounded_poly(r, 10.0, 4);
        assert_eq!(pts.len(), 20);
        assert!(pts.iter().all(|q| r.expand(0.01).contains(*q)));
        let q = rot(pos2(1.0, 0.0), Pos2::ZERO, std::f32::consts::FRAC_PI_2);
        assert!((q.x).abs() < 1e-5 && (q.y - 1.0).abs() < 1e-5);
    }
}
