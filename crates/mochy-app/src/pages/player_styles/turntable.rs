//! The Turntable style's cover (port of Android `TurntableArt` +
//! `TurntableArm`, original drawing): a record player whose record carries
//! the cover as its label and whose tone arm is the seek bar — at the song's
//! start the stylus sits in the lead-in by the rim and it swings in toward the
//! label as the song plays. Hold the arm and swing it to scrub; let go to play
//! from there. Clicking the record plays / pauses.

use egui::{Align2, Color32, CornerRadius, FontId, Mesh, Pos2, Rect, Sense, Shape, Stroke, TextureHandle, Ui, pos2, vec2};

use super::{record, spin, textured_disc};
use crate::actions::{Action, Ctx};
use crate::pages::NowInfo;
use crate::widgets;

/// Where everything sits, in fractions of the square's side.
pub struct Geometry {
    pub center: Pos2,
    pub radius: f32,
    pub pivot: Pos2,
    pub arm: f32,
    /// Stylus distance from the centre at the song's start and end.
    pub r_start: f32,
    pub r_end: f32,
}

impl Geometry {
    pub fn new(art: Rect) -> Self {
        let s = art.width().min(art.height());
        let o = art.center() - vec2(s, s) / 2.0;
        let radius = s * 0.39;
        Geometry {
            center: o + vec2(s * 0.43, s * 0.53),
            radius,
            pivot: o + vec2(s * 0.86, s * 0.15),
            arm: s * 0.64,
            r_start: radius * 0.94,
            r_end: radius * 0.42,
        }
    }

    /// Arm angle (screen radians) that puts the stylus `r` from the centre.
    pub fn angle_for(&self, r: f32) -> f32 {
        let v = self.center - self.pivot;
        let d = v.length().max(1e-3);
        let cos = ((self.arm * self.arm + d * d - r * r) / (2.0 * self.arm * d)).clamp(-1.0, 1.0);
        v.y.atan2(v.x) - cos.acos()
    }

    pub fn stylus(&self, angle: f32) -> Pos2 {
        self.pivot + vec2(angle.cos(), angle.sin()) * self.arm
    }

    /// Song fraction 0..1 → stylus radius, and back.
    pub fn radius_at(&self, frac: f32) -> f32 {
        self.r_start + (self.r_end - self.r_start) * frac.clamp(0.0, 1.0)
    }

    pub fn frac_at(&self, r: f32) -> f32 {
        ((self.r_start - r) / (self.r_start - self.r_end)).clamp(0.0, 1.0)
    }
}

pub fn show(ui: &mut Ui, cx: &mut Ctx, now: &NowInfo, art: Rect, tex: Option<&TextureHandle>, spin_records: bool) {
    let g = Geometry::new(art);
    let p = ui.painter().clone();
    let s = art.width().min(art.height());
    let plinth = Rect::from_center_size(art.center(), vec2(s, s));
    // Plinth: soft shadow, graphite body, a lighter top edge.
    p.rect_filled(plinth.expand(4.0).translate(vec2(0.0, 10.0)), CornerRadius::same(30), Color32::from_black_alpha(90));
    let mut m = Mesh::default();
    let (top, bottom) = (Color32::from_rgb(0x34, 0x34, 0x38), Color32::from_rgb(0x1A, 0x1A, 0x1D));
    m.colored_vertex(plinth.left_top(), top);
    m.colored_vertex(plinth.right_top(), top);
    m.colored_vertex(plinth.left_bottom(), bottom);
    m.colored_vertex(plinth.right_bottom(), bottom);
    m.add_triangle(0, 1, 2);
    m.add_triangle(1, 3, 2);
    p.with_clip_rect(plinth).add(Shape::mesh(m));
    p.rect_stroke(plinth, CornerRadius::same(26), Stroke::new(1.0, Color32::from_white_alpha(22)), egui::StrokeKind::Inside);
    // Corner the plinth by painting the outside corners with the backdrop's dark.
    // (The mesh is square; a rounded stroke on top reads as rounded.)
    // Platter rim + record.
    p.circle_filled(g.center, g.radius * 1.04, Color32::from_rgb(0x0B, 0x0B, 0x0D));
    p.circle_stroke(g.center, g.radius * 1.04, Stroke::new(1.5, Color32::from_white_alpha(30)));
    let angle = spin(ui, "turntable", now.playing && spin_records, 3.49);
    record(&p, g.center, g.radius, angle);
    textured_disc(&p, tex, g.center, g.radius * 0.36, angle, Color32::from_rgb(0x50, 0x50, 0x55));
    p.circle_filled(g.center, g.radius * 0.025, Color32::from_rgb(0xD8, 0xD8, 0xDC));
    // Speed dots and a start/stop knob, bottom right.
    let knob = pos2(plinth.right() - s * 0.1, plinth.bottom() - s * 0.1);
    p.circle_filled(knob, s * 0.035, Color32::from_rgb(0x2A, 0x2A, 0x2E));
    p.circle_stroke(knob, s * 0.035, Stroke::new(1.0, Color32::from_white_alpha(40)));
    let lamp = if now.playing { Color32::from_rgb(0xF2, 0x6B, 0x6B) } else { Color32::from_gray(0x55) };
    p.circle_filled(knob - vec2(s * 0.07, 0.0), s * 0.008, lamp);

    // Clicking the record plays / pauses.
    let rec_rect = Rect::from_center_size(g.center, vec2(g.radius * 2.0, g.radius * 2.0));
    let rec_resp = ui.interact(rec_rect, ui.scope_id().with("tt_record"), Sense::click());

    // ---- the arm -------------------------------------------------------------
    let id = ui.scope_id().with("tt_arm");
    let dur = now.duration.max(0.0);
    let frac = if dur > 0.0 { (now.pos / dur).clamp(0.0, 1.0) as f32 } else { 0.0 };
    let held_frac: Option<f32> = ui.ctx().data(|d| d.get_temp(id));
    let shown_frac = held_frac.unwrap_or(frac);
    let arm_angle = g.angle_for(g.radius_at(shown_frac));
    let stylus = g.stylus(arm_angle);
    let grab = Rect::from_center_size(stylus, vec2(48.0, 48.0)).union(Rect::from_two_pos(g.pivot, stylus).expand(6.0));
    let resp = ui.interact(grab, id.with("drag"), Sense::click_and_drag());
    let near_arm = |q: Pos2| dist_to_segment(q, g.pivot, stylus) < 14.0 || q.distance(stylus) < 24.0;
    let hovering = resp.hover_pos().is_some_and(near_arm);
    if resp.drag_started() || resp.dragged() {
        if let Some(q) = resp.interact_pointer_pos() {
            let f = g.frac_at(q.distance(g.center));
            ui.ctx().data_mut(|d| d.insert_temp(id, f));
        }
    }
    if resp.drag_stopped() {
        if let Some(f) = held_frac
            && dur > 0.0
        {
            cx.act(Action::Seek(f as f64 * dur));
        }
        ui.ctx().data_mut(|d| d.remove::<f32>(id));
    }
    if rec_resp.clicked() && !resp.dragged() {
        cx.act(Action::Toggle);
    }
    let held = held_frac.is_some();
    let lift = ui.ctx().animate_bool_with_time(id.with("lift"), held || hovering, 0.15);
    if hovering || held {
        ui.ctx().set_cursor_icon(if held { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
    } else if rec_resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    // The time arc round the stylus's path while held.
    if held && dur > 0.0 {
        let mut pts = Vec::new();
        for k in 0..=40 {
            let f = k as f32 / 40.0;
            pts.push(g.stylus(g.angle_for(g.radius_at(f))));
        }
        let split = (shown_frac * 40.0).round() as usize;
        p.add(Shape::line(pts[..=split.min(40)].to_vec(), Stroke::new(3.0, widgets::accent_light(cx.accent))));
        p.add(Shape::line(pts[split.min(40)..].to_vec(), Stroke::new(3.0, Color32::from_white_alpha(40))));
        let t = shown_frac as f64 * dur;
        let label = format!("{} / {}", widgets::fmt_time(t), widgets::fmt_time(dur));
        let at = stylus + vec2(-20.0, -26.0);
        let galley = p.layout_no_wrap(label, FontId::proportional(13.0), Color32::WHITE);
        let bg = Rect::from_center_size(at, galley.size() + vec2(14.0, 8.0));
        p.rect_filled(bg, CornerRadius::same(8), Color32::from_black_alpha(200));
        p.galley(bg.center() - galley.size() / 2.0, galley, Color32::WHITE);
    }

    // Arm base.
    let base_r = s * 0.065;
    p.circle_filled(g.pivot + vec2(0.0, 4.0), base_r * 1.1, Color32::from_black_alpha(80));
    p.circle_filled(g.pivot, base_r, Color32::from_rgb(0x3A, 0x3A, 0x3F));
    p.circle_stroke(g.pivot, base_r, Stroke::new(1.0, Color32::from_white_alpha(50)));
    // Shadow, counterweight, tube, headshell.
    let dir = vec2(arm_angle.cos(), arm_angle.sin());
    let shadow_off = vec2(4.0, 6.0) * (1.0 + 2.0 * lift);
    let tube = Stroke::new(s * 0.016, Color32::from_rgb(0xC8, 0xC8, 0xCE));
    p.line_segment([g.pivot + shadow_off, stylus + shadow_off], Stroke::new(s * 0.018, Color32::from_black_alpha(70)));
    let weight_end = g.pivot - dir * s * 0.09;
    p.line_segment([g.pivot, weight_end], Stroke::new(s * 0.05, Color32::from_rgb(0x55, 0x55, 0x5B)));
    p.circle_filled(weight_end, s * 0.025, Color32::from_rgb(0x55, 0x55, 0x5B));
    let shell_start = g.pivot + dir * (g.arm - s * 0.07);
    p.line_segment([g.pivot, shell_start], tube);
    let bend = arm_angle + 0.45;
    let head_dir = vec2(bend.cos(), bend.sin());
    let shell_end = shell_start + head_dir * s * 0.075;
    p.line_segment([shell_start, shell_end], Stroke::new(s * 0.028, Color32::from_rgb(0x2C, 0x2C, 0x30)));
    p.circle_filled(g.pivot, base_r * 0.45, Color32::from_rgb(0xD0, 0xD0, 0xD6));
    if lift > 0.01 {
        p.circle_stroke(stylus, 16.0 + 4.0 * lift, Stroke::new(1.5, Color32::from_white_alpha((60.0 * lift) as u8)));
    }
    let _ = Align2::CENTER_CENTER;
}

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_reaches_the_record_and_round_trips() {
        let g = Geometry::new(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 400.0)));
        for f in [0.0, 0.3, 0.7, 1.0] {
            let r = g.radius_at(f);
            let stylus = g.stylus(g.angle_for(r));
            assert!((stylus.distance(g.center) - r).abs() < 0.5, "f {f}: {} vs {r}", stylus.distance(g.center));
            assert!((g.frac_at(stylus.distance(g.center)) - f).abs() < 0.01);
        }
        // The stylus moves inward as the song plays.
        assert!(g.radius_at(1.0) < g.radius_at(0.0));
        assert!(dist_to_segment(pos2(5.0, 5.0), pos2(0.0, 0.0), pos2(10.0, 10.0)) < 1e-4);
    }
}
