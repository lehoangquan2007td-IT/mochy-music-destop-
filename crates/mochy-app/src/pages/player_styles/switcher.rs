//! The in-player look panel opened by [`super::style_switcher`]: the four
//! styles as small pictures (like Settings → Now Playing), the backgrounds
//! as previews, and the few options of the current style. Every change is
//! visible at once behind the panel; the panel stays open until a click
//! outside it.

use egui::{Color32, CornerRadius, FontId, Mesh, Pos2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use mochy_core::player_look::{BedsideFace, CoverShape, PlayerBackground, PlayerLook, PlayerStyle, WheelTheme};

use super::settings_ui::{background_name, cover_name, face_name, paint_style_pic, style_desc, style_name, wheel_name};
use crate::ui::i18n::tr;
use crate::widgets::{self, TEXT, TEXT_DIM};

/// Panel width (4 style cards side by side).
pub(super) const WIDTH: f32 = 4.0 * CARD.x + 3.0 * GAP;
const CARD: egui::Vec2 = egui::vec2(124.0, 118.0);
const GAP: f32 = 8.0;

/// What the panel asks of its caller besides look changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PanelPick {
    OpenSettings,
}

/// Draw the panel over `look`; changes are made to `look` in place.
pub(super) fn panel(ui: &mut Ui, look: &mut PlayerLook) -> Option<PanelPick> {
    ui.set_width(WIDTH);
    ui.spacing_mut().item_spacing.y = 8.0;
    let accent = ui.visuals().selection.bg_fill;

    heading(ui, tr("Player style"));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        for st in PlayerStyle::ALL {
            if style_card(ui, st, look.style == st, accent).clicked() {
                look.style = st;
            }
        }
    });

    if matches!(look.style, PlayerStyle::Flagship | PlayerStyle::Turntable) {
        ui.add_space(2.0);
        heading(ui, tr("Background"));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            let w = (WIDTH - 4.0 * GAP) / 5.0;
            for b in PlayerBackground::ALL {
                if background_tile(ui, b, look.background == b, accent, w).clicked() {
                    look.background = b;
                }
            }
        });
    }

    match look.style {
        PlayerStyle::Flagship => {
            heading(ui, tr("Cover"));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                for c in CoverShape::ALL {
                    if cover_tile(ui, c, look.cover == c, accent).clicked() {
                        look.cover = c;
                    }
                }
            });
            artwork_size(ui, look);
            ui.checkbox(&mut look.show_side_panel, tr("Show lyrics / queue beside the player"));
        }
        PlayerStyle::Turntable => {
            artwork_size(ui, look);
            ui.checkbox(&mut look.spin_records, tr("Records turn while playing"));
            ui.checkbox(&mut look.show_side_panel, tr("Show lyrics / queue beside the player"));
        }
        PlayerStyle::ClickWheel => {
            heading(ui, tr("Colour"));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                for w in WheelTheme::ALL {
                    if wheel_swatch(ui, w, look.wheel_theme == w, accent).clicked() {
                        look.wheel_theme = w;
                    }
                }
            });
        }
        PlayerStyle::Bedside => {
            heading(ui, tr("Clock face"));
            ui.horizontal(|ui| {
                for f in BedsideFace::ALL {
                    if widgets::pill(ui, face_name(f), look.bedside_face == f).clicked() {
                        look.bedside_face = f;
                    }
                }
            });
            ui.checkbox(&mut look.bedside_24h, tr("24-hour clock"));
            ui.checkbox(&mut look.bedside_warm, tr("Warm ink at night"));
        }
    }

    ui.separator();
    let mut pick = None;
    ui.horizontal(|ui| {
        if ui.button(tr("More in Settings…")).clicked() {
            ui.close();
            pick = Some(PanelPick::OpenSettings);
        }
    });
    pick
}

fn heading(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.5).strong().color(TEXT_DIM));
}

fn artwork_size(ui: &mut Ui, look: &mut PlayerLook) {
    ui.horizontal(|ui| {
        ui.label(tr("Artwork size"));
        let mut pct = look.artwork_scale * 100.0;
        if ui.add(egui::Slider::new(&mut pct, 60.0..=100.0).suffix("%").fixed_decimals(0)).changed() {
            look.artwork_scale = pct / 100.0;
        }
    });
}

/// Card frame shared by the tiles: hover fill, accent ring when selected.
fn tile(ui: &mut Ui, size: egui::Vec2, selected: bool, accent: Color32) -> (Rect, egui::Response) {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    let fill = if resp.hovered() { widgets::SURFACE_3 } else { widgets::SURFACE_2 };
    p.rect_filled(rect, CornerRadius::same(12), fill);
    if selected {
        p.rect_stroke(rect, CornerRadius::same(12), Stroke::new(2.0, accent), egui::StrokeKind::Inside);
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    (rect, resp)
}

/// Centred name under a tile picture: up to two lines, then "…".
fn label_under(ui: &Ui, rect: Rect, y: f32, text: &str, size: f32, selected: bool) {
    let color = if selected { TEXT } else { TEXT_DIM };
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), FontId::proportional(size), color, rect.width() - 10.0);
    job.halign = egui::Align::Center;
    job.wrap.max_rows = 2;
    job.wrap.overflow_character = Some('…');
    let g = ui.painter().layout_job(job);
    ui.painter().galley(pos2(rect.center().x, y), g, color);
}

fn style_card(ui: &mut Ui, style: PlayerStyle, selected: bool, accent: Color32) -> egui::Response {
    let (rect, resp) = tile(ui, CARD, selected, accent);
    let pic = Rect::from_min_size(rect.min + vec2(8.0, 8.0), vec2(rect.width() - 16.0, 66.0));
    paint_style_pic(ui.painter(), pic, style);
    label_under(ui, rect, pic.bottom() + 7.0, style_name(style), 13.0, selected);
    resp.on_hover_text(style_desc(style))
}

/// Sample "cover colours" the background previews are painted with.
const SAMPLE_A: Color32 = Color32::from_rgb(0xD0, 0x5C, 0x8C);
const SAMPLE_B: Color32 = Color32::from_rgb(0x3C, 0x46, 0xA8);

fn background_tile(ui: &mut Ui, bg: PlayerBackground, selected: bool, accent: Color32, w: f32) -> egui::Response {
    let (rect, resp) = tile(ui, vec2(w, 86.0), selected, accent);
    let pic = Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(rect.width() - 12.0, 44.0));
    paint_background_pic(ui, pic, bg);
    label_under(ui, rect, pic.bottom() + 6.0, background_name(bg), 11.5, selected);
    resp
}

fn gradient(p: &egui::Painter, r: Rect, top: Color32, bottom: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(r.left_top(), top);
    m.colored_vertex(r.right_top(), top);
    m.colored_vertex(r.right_bottom(), bottom);
    m.colored_vertex(r.left_bottom(), bottom);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    p.add(m);
}

fn paint_background_pic(ui: &Ui, pic: Rect, bg: PlayerBackground) {
    let p = ui.painter().with_clip_rect(pic.intersect(ui.clip_rect()));
    let dark = Color32::from_rgb(0x10, 0x10, 0x14);
    match bg {
        PlayerBackground::CoverGradient => gradient(&p, pic, SAMPLE_A, SAMPLE_B),
        PlayerBackground::BlurredCover => {
            p.rect_filled(pic, 0, SAMPLE_B.lerp_to_gamma(dark, 0.35));
            super::radial(&p, pic.left_top() + vec2(pic.width() * 0.3, pic.height() * 0.4), pic.width() * 0.55, SAMPLE_A.gamma_multiply(0.8));
            super::radial(&p, pic.right_bottom() - vec2(pic.width() * 0.2, pic.height() * 0.2), pic.width() * 0.45, Color32::from_rgb(0xF0, 0xB0, 0x60).gamma_multiply(0.6));
            p.rect_filled(pic, 0, Color32::from_black_alpha(50));
        }
        PlayerBackground::Ambient => {
            p.rect_filled(pic, 0, dark);
            super::radial(&p, pic.left_top() + vec2(pic.width() * 0.25, pic.height() * 0.7), pic.width() * 0.4, SAMPLE_A.gamma_multiply(0.75));
            super::radial(&p, pic.left_top() + vec2(pic.width() * 0.75, pic.height() * 0.3), pic.width() * 0.4, SAMPLE_B.gamma_multiply(0.9));
        }
        PlayerBackground::DarkGradient => gradient(&p, pic, Color32::BLACK, SAMPLE_A.lerp_to_gamma(dark, 0.25)),
        PlayerBackground::Solid => {
            p.rect_filled(pic, 0, SAMPLE_A.lerp_to_gamma(dark, 0.45));
        }
    }
    // A tiny cover in the middle, so it reads as "behind the player".
    let art = Rect::from_center_size(pic.center(), vec2(16.0, 16.0));
    p.rect_filled(art, CornerRadius::same(3), Color32::from_white_alpha(200));
}

fn cover_tile(ui: &mut Ui, c: CoverShape, selected: bool, accent: Color32) -> egui::Response {
    let (rect, resp) = tile(ui, vec2(78.0, 82.0), selected, accent);
    let center = pos2(rect.center().x, rect.top() + 26.0);
    let p = ui.painter();
    let ink = Color32::from_gray(0xB0);
    match c {
        CoverShape::Rounded => {
            p.rect_filled(Rect::from_center_size(center, vec2(34.0, 34.0)), CornerRadius::same(8), ink);
        }
        CoverShape::Square => {
            p.rect_filled(Rect::from_center_size(center, vec2(34.0, 34.0)), 0, ink);
        }
        CoverShape::Disc => {
            p.circle_filled(center, 17.0, ink);
            p.circle_filled(center, 3.0, widgets::SURFACE_2);
        }
        CoverShape::DiscLabel => {
            p.circle_filled(center, 17.0, Color32::from_rgb(0x1A, 0x1A, 0x1C));
            p.circle_stroke(center, 12.0, Stroke::new(1.0, Color32::from_gray(0x55)));
            p.circle_filled(center, 8.0, ink);
            p.circle_filled(center, 1.6, widgets::SURFACE_2);
        }
    }
    label_under(ui, rect, rect.top() + 50.0, cover_name(c), 11.5, selected);
    resp
}

fn wheel_swatch(ui: &mut Ui, w: WheelTheme, selected: bool, accent: Color32) -> egui::Response {
    let (rect, resp) = tile(ui, vec2(78.0, 84.0), selected, accent);
    // A mini pocket player in the theme's colours.
    let (body, wheel, hub) = match w {
        WheelTheme::Silver => (Color32::from_gray(0xD9), Color32::WHITE, Color32::from_gray(0xC8)),
        WheelTheme::Black => (Color32::from_gray(0x1E), Color32::from_gray(0x2A), Color32::from_gray(0x7D)),
        WheelTheme::Red => (Color32::from_rgb(0xC8, 0x1E, 0x26), Color32::from_gray(0x1A), Color32::from_gray(0x5D)),
        WheelTheme::TrueDark => (Color32::BLACK, Color32::from_gray(0x12), Color32::from_gray(0x24)),
    };
    let b = Rect::from_center_size(pos2(rect.center().x, rect.top() + 32.0), vec2(30.0, 50.0));
    let p = ui.painter();
    p.rect_filled(b, CornerRadius::same(5), body);
    if w == WheelTheme::TrueDark {
        p.rect_stroke(b, CornerRadius::same(5), Stroke::new(1.0, Color32::from_gray(0x44)), egui::StrokeKind::Inside);
    }
    let screen = if w == WheelTheme::TrueDark { Color32::from_gray(0x30) } else { Color32::from_gray(0xF4) };
    p.rect_filled(Rect::from_min_size(b.min + vec2(3.0, 4.0), vec2(24.0, 16.0)), CornerRadius::same(2), screen);
    let wc: Pos2 = pos2(b.center().x, b.bottom() - 14.0);
    p.circle_filled(wc, 10.0, wheel);
    p.circle_filled(wc, 3.5, hub);
    label_under(ui, rect, rect.top() + 63.0, wheel_name(w), 11.5, selected);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_draws_every_style_and_changes_nothing() {
        let ctx = egui::Context::default();
        for st in PlayerStyle::ALL {
            let mut look = PlayerLook { style: st, ..Default::default() };
            let before = look.clone();
            for _ in 0..2 {
                let mut out = ctx.run_ui(Default::default(), |ui| assert_eq!(panel(ui, &mut look), None));
                out.textures_delta.clear();
            }
            assert_eq!(look, before);
        }
    }
}
