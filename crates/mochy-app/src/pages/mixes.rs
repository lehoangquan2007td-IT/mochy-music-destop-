//! "Made for you" ("Dành cho bạn") on Home: one card per taste mix
//! (`crate::taste::mixes`) with a 2×2 cover collage, and the mix page
//! (`Route::HomeSection("mix:<id>")`, same layout as the other home "see
//! all" lists).

use std::sync::Arc;

use egui::{Color32, CornerRadius, FontId, Rect, RichText, Sense, Ui, Vec2, pos2, vec2};
use mochy_core::models::Song;

use crate::actions::{Action, Ctx};
use crate::images::{ImageSrc, THUMB_PX};
use crate::nav::Route;
use crate::taste::mixes::{self, Mix};
use crate::ui::i18n::{self, tr};
use crate::widgets::{self, CARD_W, RowOpts, SURFACE_2, TEXT, TEXT_DIM, galley_trunc, icons, icons::Icon, paint_artwork, shelves};

/// Route key of a mix page.
pub fn route(mix: &Mix) -> Route {
    Route::HomeSection(format!("mix:{}", mix.id))
}

/// The Home shelf; nothing while there are no mixes (first generation
/// included) or the profile has no history.
pub fn shelf(ui: &mut Ui, cx: &mut Ctx) {
    let Some(set) = mixes::current() else { return };
    if set.mixes.is_empty() || crate::taste::current().is_some_and(|p| p.is_empty()) {
        return;
    }
    shelves::section_header(ui, cx, tr("Made for you"), None, None, false);
    egui::ScrollArea::horizontal().id_salt("home_mixes").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            for m in &set.mixes {
                mix_card(ui, cx, m);
            }
        });
    });
    ui.add_space(12.0);
}

/// First `n` distinct cover images of a song list.
fn covers(songs: &[Song], n: usize) -> Vec<ImageSrc> {
    let mut out: Vec<ImageSrc> = Vec::new();
    for s in songs {
        if out.len() >= n {
            break;
        }
        if let Some(src) = crate::images::song_src(s)
            && !out.contains(&src)
        {
            out.push(src);
        }
    }
    out
}

/// 2×2 collage of the first 4 distinct covers (a single cover when fewer).
fn paint_collage(ui: &Ui, cx: &mut Ctx, art: Rect, songs: &[Song], radius: f32) {
    let srcs = covers(songs, 4);
    if srcs.len() < 4 {
        let tex = srcs.first().and_then(|s| cx.images.get(s, THUMB_PX));
        paint_artwork(ui, art, tex.as_ref(), radius);
        return;
    }
    let half = art.size() / 2.0;
    let r = radius.clamp(0.0, half.x.min(255.0)) as u8;
    for (i, src) in srcs.iter().enumerate() {
        let (col, row) = ((i % 2) as f32, (i / 2) as f32);
        let cell = Rect::from_min_size(art.min + vec2(col * half.x, row * half.y), half);
        let cr = CornerRadius {
            nw: if i == 0 { r } else { 0 },
            ne: if i == 1 { r } else { 0 },
            sw: if i == 2 { r } else { 0 },
            se: if i == 3 { r } else { 0 },
        };
        match cx.images.get(src, THUMB_PX) {
            Some(t) => {
                egui::Image::from_texture(&t).uv(widgets::cover_uv(t.size_vec2(), cell.size())).corner_radius(cr).paint_at(ui, cell);
            }
            None => {
                ui.painter().rect_filled(cell, cr, SURFACE_2);
            }
        }
    }
}

/// Card like `widgets::card` (same size, lift, text), with the collage and
/// a play button on hover.
fn mix_card(ui: &mut Ui, cx: &mut Ctx, mix: &Mix) {
    let size = vec2(CARD_W, CARD_W + 46.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let art0 = Rect::from_min_size(rect.min, Vec2::splat(CARD_W));
    let btn = Rect::from_center_size(pos2(art0.right() - 30.0, art0.bottom() - 30.0), Vec2::splat(40.0));
    let play = ui.interact(btn, resp.id.with("mix_play"), Sense::click());
    let hovered = resp.hovered() || play.hovered() || resp.context_menu_opened();
    if ui.is_rect_visible(rect) {
        let lift = ui.ctx().animate_bool_with_time(resp.id.with("lift"), hovered, 0.12);
        let art = art0.translate(vec2(0.0, -3.0 * lift));
        paint_collage(ui, cx, art, &mix.songs, cx.radius);
        if hovered {
            ui.painter().rect_filled(art, CornerRadius::same(cx.radius.clamp(0.0, 255.0) as u8), Color32::from_black_alpha(60));
            let b = btn.translate(vec2(0.0, -3.0 * lift));
            let fill = if play.hovered() { widgets::accent_light(cx.accent) } else { cx.accent };
            ui.painter().circle_filled(b.center(), b.width() / 2.0, fill);
            icons::paint(ui.painter(), b.shrink(11.0), Icon::Play, widgets::contrast_fg(fill));
        }
        let ty = art.bottom() + 8.0;
        let g = galley_trunc(ui, &mix.title(), FontId::proportional(14.0), TEXT, CARD_W);
        ui.painter().galley(pos2(rect.left(), ty), g, TEXT);
        let g2 = galley_trunc(ui, &mix.subtitle(), FontId::proportional(12.5), TEXT_DIM, CARD_W);
        ui.painter().galley(pos2(rect.left(), ty + 19.0), g2, TEXT_DIM);
    }
    if play.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        cx.act(Action::PlayList { songs: mix.songs.clone(), start: 0, shuffle: false });
    } else if resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        cx.act(Action::Navigate(route(mix)));
    }
    resp.context_menu(|ui| menu(ui, cx, mix));
}

fn menu(ui: &mut Ui, cx: &mut Ctx, mix: &Mix) {
    ui.set_min_width(200.0);
    let item = |ui: &mut Ui, label: &str| {
        let c = ui.button(label).clicked();
        if c {
            ui.close();
        }
        c
    };
    if item(ui, tr("Open")) {
        cx.act(Action::Navigate(route(mix)));
    }
    ui.separator();
    if item(ui, tr("Play")) {
        cx.act(Action::PlayList { songs: mix.songs.clone(), start: 0, shuffle: false });
    }
    if item(ui, tr("Shuffle")) {
        cx.act(Action::PlayList { songs: mix.songs.clone(), start: 0, shuffle: true });
    }
    if item(ui, tr("Add to queue")) {
        cx.act(Action::AddToQueue(mix.songs.as_ref().clone()));
    }
    ui.separator();
    if item(ui, tr("Refresh mixes")) {
        mixes::request_refresh();
    }
}

/// Mix page: title, artists line + last update (+ refresh), Play / Shuffle,
/// the songs.
pub fn page(ui: &mut Ui, cx: &mut Ctx, id: &str) {
    let Some(mix) = mixes::get(id) else {
        widgets::page_title(ui, tr("Made for you"));
        if mixes::generating() {
            widgets::loading(ui, tr("Updating mixes…"));
        } else {
            ui.label(RichText::new(tr("Nothing here yet.")).color(TEXT_DIM));
        }
        return;
    };
    widgets::page_title(ui, &mix.title());
    ui.horizontal(|ui| {
        let at = mixes::current().map(|s| s.generated_at).unwrap_or(0);
        let ago = i18n::ago_label(((crate::browse::now_ms() - at).max(0) / 60_000) as u64);
        ui.label(RichText::new(format!("{} · {} {ago}", mix.subtitle(), tr("Updated"))).color(TEXT_DIM));
        if mixes::generating() {
            ui.spinner();
        } else if ui
            .add(egui::Button::new(RichText::new(tr("Refresh")).size(13.0).color(TEXT_DIM)).frame(false))
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            mixes::request_refresh();
        }
    });
    ui.add_space(6.0);
    let songs: Arc<Vec<Song>> = mix.songs.clone();
    super::explore::play_buttons(ui, cx, &songs);
    widgets::song_list(ui, cx, &songs, RowOpts { show_album: true, ..Default::default() });
}
