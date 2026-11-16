//! Shelf widgets for YouTube Music browse sections: section header with
//! "See all", cards with YTM's own subtitle line, 4-row song grids (Quick
//! picks / Trending), mood & genre tiles, wrapped card grids.

use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, FontId, Rect, Response, RichText, Sense, Ui, Vec2, pos2, vec2};
use mochy_core::models::{ShelfItem, Song};
use mochy_innertube::explore::{MoodItem, Section};

use super::{RowOpts, TEXT, TEXT_DIM, artists_line, card, icons, icons::Icon, paint_artwork, paint_text, song_menu};
use crate::actions::{Action, Ctx};
use crate::images::{ImageSrc, THUMB_PX};
use crate::ui::i18n::tr;
use crate::widgets::menus;

/// Section title with optional strapline / avatar and a "See all" link.
/// Returns `true` when "See all" was clicked.
pub fn section_header(ui: &mut Ui, cx: &mut Ctx, title: &str, strapline: Option<&str>, thumb: Option<&str>, see_all: bool) -> bool {
    let mut clicked = false;
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if let Some(t) = thumb {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::hover());
            let tex = cx.images.get(&ImageSrc::Url(t.to_string()), THUMB_PX);
            paint_artwork(ui, r, tex.as_ref(), 18.0);
            ui.add_space(4.0);
        }
        ui.vertical(|ui| {
            if let Some(s) = strapline.filter(|s| !s.is_empty()) {
                ui.label(RichText::new(s).size(12.0).color(TEXT_DIM));
            }
            ui.label(RichText::new(title).size(20.0).strong().color(TEXT));
        });
        if see_all {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                let r = ui.add(egui::Button::new(RichText::new(tr("See all")).size(13.0).color(TEXT_DIM)).frame(false));
                if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    clicked = true;
                }
            });
        }
    });
    ui.add_space(4.0);
    clicked
}

/// Songs of a section's items, in order (for row playback).
pub fn songs_of(items: &[ShelfItem]) -> Arc<Vec<Song>> {
    Arc::new(items.iter().filter_map(|i| if let ShelfItem::Song(s) = i { Some(s.clone()) } else { None }).collect())
}

/// One card for any item, using `subtitle` (YTM's line) when not empty.
pub fn item_card(ui: &mut Ui, cx: &mut Ctx, item: &ShelfItem, subtitle: &str) -> Response {
    match item {
        ShelfItem::Song(s) => {
            let src = crate::images::song_src(s);
            let sub = if subtitle.is_empty() { artists_line(s) } else { subtitle.to_string() };
            let resp = card(ui, cx, src.as_ref(), &s.title, &sub, false);
            if resp.clicked() {
                cx.act(Action::PlaySong(s.clone()));
            }
            resp.context_menu(|ui| song_menu(ui, cx, s, None));
            resp
        }
        ShelfItem::Album(a) => {
            let src = a.thumbnail_url.clone().map(ImageSrc::Url);
            let sub = if subtitle.is_empty() { tr("Album").to_string() } else { subtitle.to_string() };
            let resp = card(ui, cx, src.as_ref(), &a.title, &sub, false);
            if resp.clicked() {
                cx.act(Action::Navigate(menus::album_route(&a.id)));
            }
            resp.context_menu(|ui| menus::album_menu(ui, cx, a, &[]));
            resp
        }
        ShelfItem::Artist(a) => {
            let src = a.thumbnail_url.clone().map(ImageSrc::Url);
            let sub = if subtitle.is_empty() { tr("Artist").to_string() } else { subtitle.to_string() };
            let resp = card(ui, cx, src.as_ref(), &a.name, &sub, true);
            if resp.clicked() {
                cx.act(Action::Navigate(menus::artist_route(&a.id)));
            }
            resp.context_menu(|ui| menus::artist_menu(ui, cx, a));
            resp
        }
        ShelfItem::Playlist(p) => {
            let src = p.thumbnail_url.clone().map(ImageSrc::Url);
            let sub = if subtitle.is_empty() { tr("Playlist").to_string() } else { subtitle.to_string() };
            let resp = card(ui, cx, src.as_ref(), &p.name, &sub, false);
            if resp.clicked() {
                cx.act(Action::Navigate(menus::playlist_route(p)));
            }
            resp.context_menu(|ui| menus::playlist_menu(ui, cx, p));
            resp
        }
    }
}

/// Horizontal scrolling card row.
pub fn cards_row(ui: &mut Ui, cx: &mut Ctx, items: &[ShelfItem], subtitles: &[String], salt: impl std::hash::Hash + std::fmt::Debug) {
    egui::ScrollArea::horizontal().id_salt(("cards_row", salt)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            for (i, item) in items.iter().enumerate() {
                item_card(ui, cx, item, subtitles.get(i).map(String::as_str).unwrap_or(""));
            }
        });
    });
}

/// Wrapped card grid (see-all pages, library grids).
pub fn cards_grid(ui: &mut Ui, cx: &mut Ctx, items: &[ShelfItem], subtitles: &[String]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(16.0, 16.0);
        for (i, item) in items.iter().enumerate() {
            item_card(ui, cx, item, subtitles.get(i).map(String::as_str).unwrap_or(""));
        }
    });
}

const GRID_ROW_H: f32 = 56.0;

/// Compact song row for grids: art, title, subtitle; click plays the song
/// with its radio (Android `YouTubeListItem` in Quick picks / charts).
pub fn compact_song_row(ui: &mut Ui, cx: &mut Ctx, list: &Arc<Vec<Song>>, idx: usize, subtitle: Option<&str>, width: f32, as_list: bool) {
    let song = &list[idx];
    let (rect, resp) = ui.allocate_exact_size(vec2(width, GRID_ROW_H), Sense::click());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let id = ui.scope_id().with(("grid_row", idx, &song.id));
    let hovered = resp.hovered() || resp.context_menu_opened();
    if hovered {
        ui.painter().rect_filled(rect, CornerRadius::same(8), Color32::from_white_alpha(10));
    }
    let art = Rect::from_min_size(pos2(rect.left() + 6.0, rect.center().y - 21.0), Vec2::splat(42.0));
    let tex = crate::images::song_src(song).and_then(|s| cx.images.get(&s, THUMB_PX));
    paint_artwork(ui, art, tex.as_ref(), (cx.radius * 0.5).max(4.0));
    let is_current = cx.current_id == Some(song.id.as_str());
    if hovered || is_current {
        ui.painter().rect_filled(art, CornerRadius::same(6), Color32::from_black_alpha(120));
        icons::paint(ui.painter(), art.shrink(12.0), if is_current && cx.playing { Icon::Pause } else { Icon::Play }, Color32::WHITE);
    }
    let x = art.right() + 10.0;
    let more = Rect::from_center_size(pos2(rect.right() - 18.0, rect.center().y), Vec2::splat(28.0));
    let text_w = (more.left() - x - 6.0).max(30.0);
    let title_col = if is_current { super::accent_light(cx.accent) } else { TEXT };
    let mut title = song.title.clone();
    if song.explicit {
        title.push_str("  🅴");
    }
    paint_text(ui, pos2(x, rect.center().y - 18.0), &title, FontId::proportional(14.0), title_col, text_w);
    let sub = subtitle.filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| artists_line(song));
    paint_text(ui, pos2(x, rect.center().y + 2.0), &sub, FontId::proportional(12.5), TEXT_DIM, text_w);
    let more_resp = ui.interact(more, id.with("more"), Sense::click());
    if hovered || more_resp.hovered() {
        icons::paint(ui.painter(), more.shrink(6.0), Icon::More, TEXT);
    }
    let opts = RowOpts::default();
    egui::Popup::menu(&more_resp).show(|ui| {
        song_menu(ui, cx, song, None);
        menus::song_menu_extra(ui, cx, song, &opts);
    });
    resp.context_menu(|ui| {
        song_menu(ui, cx, song, None);
        menus::song_menu_extra(ui, cx, song, &opts);
    });
    if resp.clicked() {
        if is_current {
            cx.act(Action::Toggle);
        } else if as_list {
            cx.act(Action::PlayList { songs: list.clone(), start: idx, shuffle: false });
        } else {
            cx.act(Action::PlaySong(song.clone()));
        }
    }
}

/// Horizontally scrolling grid of song rows, `rows` high (column-major like
/// Android's `LazyHorizontalGrid`).
pub fn song_grid(ui: &mut Ui, cx: &mut Ctx, list: &Arc<Vec<Song>>, subtitles: Option<&[String]>, rows: usize, salt: impl std::hash::Hash + std::fmt::Debug, as_list: bool) {
    if list.is_empty() {
        return;
    }
    let avail = ui.available_width();
    let col_w = if avail * 0.475 >= 320.0 { (avail * 0.475).min(420.0) } else { (avail * 0.9).max(260.0) };
    let rows = rows.max(1).min(list.len());
    let cols = list.len().div_ceil(rows);
    egui::ScrollArea::horizontal().id_salt(("song_grid", salt)).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = vec2(12.0, 2.0);
            for c in 0..cols {
                ui.vertical(|ui| {
                    ui.set_width(col_w);
                    for r in 0..rows {
                        let i = c * rows + r;
                        if i < list.len() {
                            compact_song_row(ui, cx, list, i, subtitles.and_then(|s| s.get(i)).map(String::as_str), col_w, as_list);
                        }
                    }
                });
            }
        });
    });
}

/// Colour of a mood tile (ARGB from YTM, or a neutral fallback).
pub fn mood_color(argb: u32) -> Color32 {
    if argb == 0 {
        return Color32::from_rgb(0x55, 0x55, 0x5C);
    }
    Color32::from_rgb((argb >> 16) as u8, (argb >> 8) as u8, argb as u8)
}

/// Mood & genre tile: dark card with the YTM stripe colour on the left.
pub fn mood_tile(ui: &mut Ui, title: &str, argb: u32, size: Vec2) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let base = super::SURFACE_2;
        let fill = if resp.hovered() { super::SURFACE_3 } else { base };
        ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
        let stripe = Rect::from_min_size(rect.min, vec2(6.0, rect.height()));
        ui.painter().rect_filled(stripe, CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 }, mood_color(argb));
        let g = super::galley_trunc(ui, title, FontId::proportional(14.5), TEXT, rect.width() - 28.0);
        ui.painter().galley(pos2(rect.left() + 18.0, rect.center().y - g.size().y / 2.0), g, TEXT);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Rows of mood tiles; clicking navigates to the mood/genre page.
pub fn mood_tiles(ui: &mut Ui, cx: &mut Ctx, moods: &[MoodItem], tile_w: f32) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
        for m in moods {
            if mood_tile(ui, &m.title, m.color, vec2(tile_w, 52.0)).clicked() {
                cx.act(Action::Navigate(crate::browse::route_for_endpoint(&m.endpoint)));
            }
        }
    });
}

/// Horizontally scrolling mood tiles in `rows` rows (Explore carousel).
pub fn mood_carousel(ui: &mut Ui, cx: &mut Ctx, moods: &[MoodItem], rows: usize, salt: impl std::hash::Hash + std::fmt::Debug) {
    let rows = rows.max(1);
    let cols = moods.len().div_ceil(rows);
    egui::ScrollArea::horizontal().id_salt(("moods", salt)).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
            for c in 0..cols {
                ui.vertical(|ui| {
                    for r in 0..rows {
                        if let Some(m) = moods.get(c * rows + r)
                            && mood_tile(ui, &m.title, m.color, vec2(180.0, 52.0)).clicked()
                        {
                            cx.act(Action::Navigate(crate::browse::route_for_endpoint(&m.endpoint)));
                        }
                    }
                });
            }
        });
    });
}

/// Shortcut tile with an icon (Explore: New releases / Charts / Moods).
pub fn shortcut_tile(ui: &mut Ui, cx: &Ctx, icon: Icon, title: &str, width: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 64.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if resp.hovered() { super::SURFACE_3 } else { super::SURFACE_2 };
        ui.painter().rect_filled(rect, CornerRadius::same(12), fill);
        let ir = Rect::from_center_size(pos2(rect.left() + 32.0, rect.center().y), Vec2::splat(26.0));
        icons::paint(ui.painter(), ir, icon, super::accent_light(cx.accent));
        ui.painter().text(pos2(rect.left() + 58.0, rect.center().y), Align2::LEFT_CENTER, title, FontId::proportional(16.0), TEXT);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Whole YTM section: header (+ "See all") and body by layout.
/// `see_all` is the action for the header link (endpoint or local detail).
pub fn yt_section(ui: &mut Ui, cx: &mut Ctx, sec: &Section, salt: impl std::hash::Hash + std::fmt::Debug + Copy, see_all: Option<Action>) {
    if sec.items.is_empty() && sec.moods.is_empty() {
        return;
    }
    if section_header(ui, cx, &sec.title, sec.strapline.as_deref(), sec.thumbnail.as_deref(), see_all.is_some())
        && let Some(a) = see_all
    {
        cx.act(a);
    }
    if !sec.moods.is_empty() {
        mood_carousel(ui, cx, &sec.moods, 2, salt);
    } else if sec.rows && sec.items.iter().all(|i| matches!(i, ShelfItem::Song(_))) {
        let songs = songs_of(&sec.items);
        song_grid(ui, cx, &songs, Some(&sec.subtitles), 4, salt, false);
    } else {
        cards_row(ui, cx, &sec.items, &sec.subtitles, salt);
    }
    ui.add_space(12.0);
}

/// "See all" action for a section's endpoint.
pub fn see_all_action(sec: &Section) -> Option<Action> {
    sec.more.as_ref().map(|ep| Action::Navigate(crate::browse::route_for_endpoint(ep)))
}

/// Bottom sentinel for infinite scrolling: returns `true` when it is
/// visible (the user scrolled near the end).
pub fn load_more_sentinel(ui: &mut Ui, loading: bool) -> bool {
    ui.add_space(8.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    let visible = ui.is_rect_visible(rect);
    if loading {
        let r = Rect::from_center_size(rect.center(), Vec2::splat(22.0));
        ui.put(r, egui::Spinner::new());
    }
    visible && !loading
}
