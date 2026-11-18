//! Settings → Now Playing and Settings → Motion & glass (port of Android
//! `NowPlayingSettingsScreen`, the player selector and the customisation
//! screen's motion / blur / accent switches).

use egui::{Align2, Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use mochy_core::player_look::{BedsideFace, CoverShape, PlayerBackground, PlayerStyle, WheelTheme};
use mochy_core::settings::AppSettings;

use crate::ui::i18n::tr;
use crate::widgets::{self, TEXT, TEXT_DIM};

fn group(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).size(18.0).strong().color(TEXT));
    ui.add_space(6.0);
    egui::Frame::new().fill(widgets::SURFACE).corner_radius(14).inner_margin(16).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 10.0;
        body(ui);
    });
}

fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(TEXT_DIM));
}

pub fn style_name(s: PlayerStyle) -> &'static str {
    tr(match s {
        PlayerStyle::Flagship => "Flagship",
        PlayerStyle::Turntable => "Turntable",
        PlayerStyle::ClickWheel => "Click wheel",
        PlayerStyle::Bedside => "Bedside clock",
    })
}

pub(super) fn style_desc(s: PlayerStyle) -> &'static str {
    tr(match s {
        PlayerStyle::Flagship => "Big cover, lyrics and queue beside it",
        PlayerStyle::Turntable => "A record player; its arm is the seek bar",
        PlayerStyle::ClickWheel => "A pocket player — turn the wheel for volume",
        PlayerStyle::Bedside => "A quiet clock for the nightstand",
    })
}

pub fn background_name(b: PlayerBackground) -> &'static str {
    tr(match b {
        PlayerBackground::CoverGradient => "Cover gradient",
        PlayerBackground::BlurredCover => "Blurred cover",
        PlayerBackground::Ambient => "Ambient",
        PlayerBackground::DarkGradient => "Dark gradient",
        PlayerBackground::Solid => "Solid",
    })
}

pub(super) fn cover_name(c: CoverShape) -> &'static str {
    tr(match c {
        CoverShape::Rounded => "Rounded",
        CoverShape::Square => "Square",
        CoverShape::Disc => "Disc",
        CoverShape::DiscLabel => "Record label",
    })
}

pub(super) fn face_name(f: BedsideFace) -> &'static str {
    tr(match f {
        BedsideFace::Big => "Big",
        BedsideFace::Stacked => "Stacked",
        BedsideFace::Analog => "Analog",
    })
}

pub(super) fn wheel_name(w: WheelTheme) -> &'static str {
    tr(match w {
        WheelTheme::Silver => "Silver",
        WheelTheme::Black => "Black",
        WheelTheme::TrueDark => "True dark",
        WheelTheme::Red => "Red",
    })
}

/// A selector card with a tiny drawing of the style.
fn style_card(ui: &mut Ui, style: PlayerStyle, selected: bool, accent: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(170.0, 150.0), Sense::click());
    let p = ui.painter();
    let fill = if resp.hovered() { widgets::SURFACE_3 } else { widgets::SURFACE_2 };
    p.rect_filled(rect, CornerRadius::same(16), fill);
    if selected {
        p.rect_stroke(rect, CornerRadius::same(16), Stroke::new(2.0, accent), egui::StrokeKind::Inside);
    }
    let pic = Rect::from_min_size(rect.min + vec2(12.0, 12.0), vec2(rect.width() - 24.0, 86.0));
    paint_style_pic(p, pic, style);
    p.text(pos2(rect.left() + 12.0, pic.bottom() + 10.0), Align2::LEFT_TOP, style_name(style), FontId::proportional(14.0), TEXT);
    let g = widgets::galley_trunc(ui, style_desc(style), FontId::proportional(10.5), TEXT_DIM, rect.width() - 20.0);
    ui.painter().galley(pos2(rect.left() + 12.0, pic.bottom() + 28.0), g, TEXT_DIM);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// The tiny drawing of a player style inside `pic` (drawn for an 86 px tall
/// picture, scaled to `pic`'s height; used by Settings and the in-player
/// switcher).
pub(super) fn paint_style_pic(p: &egui::Painter, pic: Rect, style: PlayerStyle) {
    let k = pic.height() / 86.0;
    let r = |v: f32| CornerRadius::same((v * k).round().clamp(0.0, 255.0) as u8);
    p.rect_filled(pic, r(10.0), Color32::from_rgb(0x0E, 0x0E, 0x10));
    let ink = Color32::from_gray(0x9A);
    let faint = Color32::from_gray(0x55);
    match style {
        PlayerStyle::Flagship => {
            let art = Rect::from_min_size(pic.min + vec2(10.0, 10.0) * k, vec2(52.0, 52.0) * k);
            p.rect_filled(art, r(8.0), ink);
            p.rect_filled(Rect::from_min_size(pos2(art.left(), art.bottom() + 6.0 * k), vec2(52.0, 4.0) * k), r(2.0), faint);
            for i in 0..5 {
                let w = (56.0 - (i * 9 % 23) as f32) * k;
                let at = pos2(pic.left() + 76.0 * k, pic.top() + (14.0 + i as f32 * 12.0) * k);
                p.rect_filled(Rect::from_min_size(at, vec2(w, 5.0 * k)), r(2.0), faint);
            }
        }
        PlayerStyle::Turntable => {
            let c = pic.center() - vec2(14.0 * k, 0.0);
            p.circle_filled(c, 34.0 * k, Color32::from_rgb(0x1A, 0x1A, 0x1C));
            p.circle_stroke(c, 24.0 * k, Stroke::new(1.0, faint));
            p.circle_filled(c, 12.0 * k, ink);
            p.line_segment([pos2(pic.right() - 20.0 * k, pic.top() + 12.0 * k), c + vec2(22.0, 14.0) * k], Stroke::new(3.0 * k, ink));
        }
        PlayerStyle::ClickWheel => {
            let body = Rect::from_center_size(pic.center(), vec2(40.0, 78.0) * k);
            p.rect_filled(body, r(6.0), Color32::from_gray(0xD9));
            p.rect_filled(Rect::from_min_size(body.min + vec2(4.0, 5.0) * k, vec2(32.0, 25.0) * k), r(2.0), Color32::WHITE);
            let wheel = pos2(body.center().x, body.bottom() - 22.0 * k);
            p.circle_filled(wheel, 14.0 * k, Color32::WHITE);
            p.circle_filled(wheel, 5.0 * k, Color32::from_gray(0xC8));
        }
        PlayerStyle::Bedside => {
            p.text(pos2(pic.center().x + 20.0 * k, pic.center().y), Align2::CENTER_CENTER, "22:41", FontId::proportional(24.0 * k), ink);
            p.rect_filled(Rect::from_min_size(pic.min + vec2(10.0, 16.0) * k, vec2(36.0, 36.0) * k), r(6.0), faint);
        }
    }
}

fn chip_row<T: Copy + PartialEq>(ui: &mut Ui, label: &str, all: &[T], value: &mut T, name: impl Fn(T) -> &'static str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(label);
        for &v in all {
            if widgets::pill(ui, name(v), *value == v).clicked() {
                *value = v;
            }
        }
    });
}

/// Settings → Now Playing: the player style and how it looks.
pub fn now_playing_section(ui: &mut Ui, s: &mut AppSettings) {
    let accent = ui.visuals().selection.bg_fill;
    group(ui, tr("Now Playing"), |ui| {
        ui.label(RichText::new(tr("Player style")).color(TEXT));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
            for st in PlayerStyle::ALL {
                if style_card(ui, st, s.player.style == st, accent).clicked() {
                    s.player.style = st;
                }
            }
        });
        let pl = &mut s.player;
        chip_row(ui, tr("Background"), &PlayerBackground::ALL, &mut pl.background, background_name);
        if pl.style == PlayerStyle::Flagship {
            chip_row(ui, tr("Cover"), &CoverShape::ALL, &mut pl.cover, cover_name);
        }
        if matches!(pl.style, PlayerStyle::Flagship | PlayerStyle::Turntable) {
            ui.horizontal(|ui| {
                ui.label(tr("Artwork size"));
                let mut pct = pl.artwork_scale * 100.0;
                if ui.add(egui::Slider::new(&mut pct, 60.0..=100.0).suffix("%")).changed() {
                    pl.artwork_scale = pct / 100.0;
                }
            });
            if pl.style == PlayerStyle::Flagship && pl.cover == CoverShape::Rounded {
                ui.horizontal(|ui| {
                    ui.label(tr("Artwork corner radius"));
                    ui.add(egui::Slider::new(&mut pl.artwork_radius, 0.0..=64.0).suffix(" px"));
                });
            }
            ui.checkbox(&mut pl.spin_records, tr("Records turn while playing"));
        }
        ui.checkbox(&mut pl.show_format_badge, tr("Audio format badge"));
        ui.add_enabled_ui(pl.show_format_badge, |ui| {
            ui.indent("np_rate", |ui| {
                ui.checkbox(&mut pl.show_sample_rate, tr("Show sample rate in the badge"));
            });
        });
        ui.checkbox(&mut pl.show_volume, tr("Show the volume slider"));
        ui.checkbox(&mut pl.show_side_panel, tr("Show lyrics / queue beside the player"));
        match pl.style {
            PlayerStyle::Bedside => {
                ui.separator();
                chip_row(ui, tr("Clock face"), &BedsideFace::ALL, &mut pl.bedside_face, face_name);
                ui.checkbox(&mut pl.bedside_24h, tr("24-hour clock"));
                ui.horizontal(|ui| {
                    ui.label(tr("Dim after"));
                    ui.add(egui::Slider::new(&mut pl.bedside_dim_secs, 0..=300).suffix(" s"));
                });
                hint(ui, tr("0 = never dim. A mouse move wakes the clock."));
                ui.checkbox(&mut pl.bedside_warm, tr("Warm ink at night"));
            }
            PlayerStyle::ClickWheel => {
                ui.separator();
                chip_row(ui, tr("Colour"), &WheelTheme::ALL, &mut pl.wheel_theme, wheel_name);
                hint(ui, tr("Turn the wheel (drag round it or scroll) for the volume, click the centre to scrub, MENU closes."));
            }
            _ => {}
        }
    });
}

const ACCENTS: [u32; 8] = [0x8E8E8E, 0xF28BA8, 0xF2A46B, 0xF2D46B, 0x8FD98B, 0x6BBAF2, 0x8C9EF2, 0xB98CF2];

/// Settings → Motion & glass: animations, their speed, glass and accent swatches.
pub fn appearance_section(ui: &mut Ui, s: &mut AppSettings) {
    // Screenshot automation: `MOCHY_SHOTS_SETTINGS=player` scrolls here.
    if std::env::var_os("MOCHY_SHOTS").is_some() && std::env::var("MOCHY_SHOTS_SETTINGS").is_ok_and(|v| v == "player") {
        ui.scroll_to_cursor(Some(egui::Align::TOP));
    }
    group(ui, tr("Motion & glass"), |ui| {
        ui.checkbox(&mut s.player.animations, tr("Animations"));
        ui.add_enabled_ui(s.player.animations, |ui| {
            ui.horizontal(|ui| {
                ui.label(tr("Animation speed"));
                ui.add(egui::Slider::new(&mut s.appearance.animation_speed, 0.25..=4.0).logarithmic(true).suffix("×").fixed_decimals(2));
                if (s.appearance.animation_speed - 1.0).abs() > 1e-3 && ui.small_button(tr("Reset")).clicked() {
                    s.appearance.animation_speed = 1.0;
                }
            });
        });
        let mut glass = !s.appearance.disable_blur;
        if ui.checkbox(&mut glass, tr("Glass effects (frosted cover backgrounds)")).changed() {
            s.appearance.disable_blur = !glass;
        }
        hint(ui, tr("Turn glass off on slow GPUs: the player backgrounds are drawn flat."));
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("Accent"));
            for c in ACCENTS {
                let col = Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8);
                let selected = !s.appearance.dynamic_theme && s.appearance.selected_theme_color == Some(c);
                let (r, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                ui.painter().circle_filled(r.center(), 10.0, col);
                if selected {
                    ui.painter().circle_stroke(r.center(), 12.0, Stroke::new(2.0, Color32::WHITE));
                }
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    s.appearance.dynamic_theme = false;
                    s.appearance.selected_theme_color = Some(c);
                }
            }
            if ui.selectable_label(s.appearance.dynamic_theme, tr("From artwork")).clicked() {
                s.appearance.dynamic_theme = true;
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_draw_and_change_nothing() {
        let ctx = egui::Context::default();
        let mut s = AppSettings::default();
        let before = s.clone();
        for _ in 0..2 {
            let mut out = ctx.run_ui(Default::default(), |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    now_playing_section(ui, &mut s);
                    appearance_section(ui, &mut s);
                });
            });
            out.textures_delta.clear();
        }
        assert_eq!(s, before);
        s.player.style = PlayerStyle::Bedside;
        let mut out = ctx.run_ui(Default::default(), |ui| now_playing_section(ui, &mut s));
        out.textures_delta.clear();
    }
}
