//! Reusable app-shell widgets: text helpers, artwork, seek bar, song rows,
//! cards, shelves, song context menu.

pub mod icons;

use std::sync::Arc;

use egui::text::{LayoutJob, TextWrapping};
use egui::{Align2, Color32, CornerRadius, FontId, Galley, Pos2, Rect, Response, Sense, Stroke, TextureHandle, Ui, Vec2, pos2, vec2};
use mochy_core::models::{Album, Artist, Playlist, Shelf, ShelfItem, Song};

use crate::actions::{Action, Ctx};
use crate::images::{ImageSrc, THUMB_PX};
use crate::nav::Route;
use crate::ui::i18n::{self, tr};
use icons::Icon;

pub const TEXT: Color32 = Color32::from_rgb(0xF2, 0xF2, 0xF2);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0xA8, 0xA8, 0xAD);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x70, 0x70, 0x75);
pub const SURFACE: Color32 = Color32::from_rgb(0x16, 0x16, 0x18);
pub const SURFACE_2: Color32 = Color32::from_rgb(0x23, 0x23, 0x26);
pub const SURFACE_3: Color32 = Color32::from_rgb(0x3D, 0x3D, 0x41);

pub fn fmt_time(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "0:00".into();
    }
    let s = secs as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

pub fn artists_line(song: &Song) -> String {
    let names: Vec<&str> = song.artists.iter().map(|a| a.name.as_str()).filter(|n| !n.is_empty()).collect();
    if names.is_empty() { tr("Unknown artist").into() } else { names.join(", ") }
}

/// Single-line galley truncated with an ellipsis at `max_w`.
pub fn galley_trunc(ui: &Ui, text: &str, font: FontId, color: Color32, max_w: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping::truncate_at_width(max_w.max(8.0));
    ui.painter().layout_job(job)
}

/// Paint truncated text at `pos` (left-top); returns its rect.
pub fn paint_text(ui: &Ui, pos: Pos2, text: &str, font: FontId, color: Color32, max_w: f32) -> Rect {
    let g = galley_trunc(ui, text, font, color, max_w);
    let rect = Rect::from_min_size(pos, g.size());
    ui.painter().galley(pos, g, color);
    rect
}

/// Center-crop UV window (normalized 0..1) so a texture fills `rect` without
/// distortion — the egui equivalent of CSS `object-fit: cover`. Centered on
/// the image.
pub fn cover_uv(tex: Vec2, rect: Vec2) -> Rect {
    if !(tex.x > 0.0 && tex.y > 0.0) {
        return Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    }
    let scale = (rect.x / tex.x).max(rect.y / tex.y);
    let vis = (rect / scale).min(tex).max(Vec2::ZERO);
    let min = (((tex - vis) * 0.5) / tex).max(Vec2::ZERO);
    let max = (((tex + vis) * 0.5) / tex).min(Vec2::ONE).max(min);
    Rect::from_min_max(min.to_pos2(), max.to_pos2())
}

/// Artwork with placeholder while loading. Fills `rect` completely
/// (center-cropped), never stretched.
pub fn paint_artwork(ui: &Ui, rect: Rect, tex: Option<&TextureHandle>, radius: f32) {
    let cr = CornerRadius::same(radius.clamp(0.0, 255.0) as u8);
    match tex {
        Some(t) => {
            egui::Image::from_texture(t).uv(cover_uv(t.size_vec2(), rect.size())).corner_radius(cr).paint_at(ui, rect);
        }
        None => {
            ui.painter().rect_filled(rect, cr, SURFACE_2);
            let s = rect.width().min(rect.height()) * 0.36;
            icons::paint(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(s)), Icon::Note, TEXT_FAINT);
        }
    }
}

/// Rounded "pill" toggle (tabs / filters).
pub fn pill(ui: &mut Ui, text: &str, selected: bool) -> Response {
    let font = FontId::proportional(13.5);
    let g = ui.painter().layout_no_wrap(text.to_owned(), font, if selected { Color32::WHITE } else { TEXT_DIM });
    let size = vec2(g.size().x + 24.0, 30.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let fill = if selected {
        SURFACE_3
    } else if resp.hovered() {
        SURFACE_2
    } else {
        SURFACE
    };
    ui.painter().rect_filled(rect, CornerRadius::same(15), fill);
    ui.painter().galley(rect.center() - g.size() / 2.0, g, Color32::WHITE);
    resp
}

/// Accent-filled button with icon + label.
pub fn accent_button(ui: &mut Ui, icon: Icon, text: &str, accent: Color32, filled: bool) -> Response {
    let font = FontId::proportional(14.0);
    let fg = if filled { contrast_fg(accent) } else { Color32::WHITE };
    let g = ui.painter().layout_no_wrap(text.to_owned(), font, fg);
    let size = vec2(g.size().x + 52.0, 36.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let mut fill = if filled { accent } else { SURFACE_2 };
    if resp.hovered() {
        fill = fill.gamma_multiply(0.85);
    }
    ui.painter().rect_filled(rect, CornerRadius::same(18), fill);
    let ir = Rect::from_center_size(pos2(rect.left() + 22.0, rect.center().y), Vec2::splat(16.0));
    icons::paint(ui.painter(), ir, icon, fg);
    ui.painter().galley(pos2(rect.left() + 38.0, rect.center().y - g.size().y / 2.0), g, fg);
    resp
}

/// Black or white, whichever reads better on `c`.
pub fn contrast_fg(c: Color32) -> Color32 {
    let l = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
    if l > 150.0 { Color32::BLACK } else { Color32::WHITE }
}

#[cfg(test)]
mod tests {
    use super::cover_uv;
    use egui::{pos2, vec2};

    #[test]
    fn square_image_in_square_box_is_full_uv() {
        let uv = cover_uv(vec2(400.0, 400.0), vec2(200.0, 200.0));
        assert_eq!(uv.min, pos2(0.0, 0.0));
        assert_eq!(uv.max, pos2(1.0, 1.0));
    }

    #[test]
    fn wide_image_in_square_box_crops_sides() {
        // 16:9 image in a square box: full height, center 56% of the width.
        let uv = cover_uv(vec2(400.0, 225.0), vec2(200.0, 200.0));
        assert_eq!(uv.min.y, 0.0);
        assert_eq!(uv.max.y, 1.0);
        let w = uv.max.x - uv.min.x;
        assert!((w - 225.0 / 400.0).abs() < 1e-4, "w={w}");
        assert!((uv.min.x - (1.0 - w) / 2.0).abs() < 1e-4);
    }

    #[test]
    fn square_image_in_wide_box_crops_top_bottom() {
        // square image in a 2:1 banner: full width, center half of the height.
        let uv = cover_uv(vec2(400.0, 400.0), vec2(400.0, 200.0));
        assert_eq!(uv.min.x, 0.0);
        assert_eq!(uv.max.x, 1.0);
        let h = uv.max.y - uv.min.y;
        assert!((h - 0.5).abs() < 1e-4, "h={h}");
    }
}

pub fn section_title(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(text).size(20.0).strong().color(TEXT));
    ui.add_space(4.0);
}

pub fn page_title(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(30.0).strong().color(TEXT));
    ui.add_space(8.0);
}

pub fn loading(ui: &mut Ui, what: &str) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(egui::RichText::new(what).color(TEXT_DIM));
    });
}

pub fn error_box(ui: &mut Ui, err: &str) -> bool {
    let mut retry = false;
    egui::Frame::new().fill(Color32::from_rgb(0x2A, 0x16, 0x16)).corner_radius(10).inner_margin(12).show(ui, |ui| {
        ui.label(egui::RichText::new(tr("Something went wrong")).strong().color(Color32::from_rgb(0xFF, 0x9A, 0x9A)));
        ui.label(egui::RichText::new(err).color(TEXT_DIM).size(12.5));
        retry = ui.button(tr("Retry")).clicked();
    });
    retry
}

// ---------------------------------------------------------------------------
// Seek / slider
// ---------------------------------------------------------------------------

pub struct SeekOut {
    /// Committed seek target (click or drag release), seconds.
    pub commit: Option<f64>,
    /// Position to display (follows the drag).
    pub display: f64,
}

/// Seek bar. Drags preview locally and commit on release.
pub fn seek_bar(ui: &mut Ui, pos: f64, dur: f64, width: f32, accent: Color32) -> SeekOut {
    let h = 18.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width.max(40.0), h), Sense::click_and_drag());
    let frac_at = |x: f32| ((x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64;
    let mut frac = if dur > 0.0 { (pos / dur).clamp(0.0, 1.0) } else { 0.0 };
    let active = resp.dragged() || resp.is_pointer_button_down_on();
    if active && let Some(p) = resp.interact_pointer_pos() {
        frac = frac_at(p.x);
    }
    let mut commit = None;
    if dur > 0.0
        && (resp.drag_stopped() || resp.clicked())
        && let Some(p) = resp.interact_pointer_pos()
    {
        commit = Some(frac_at(p.x) * dur);
        frac = frac_at(p.x);
    }
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered() || active;
        let th = if hovered { 6.0 } else { 4.0 };
        let track = Rect::from_center_size(rect.center(), vec2(rect.width(), th));
        let painter = ui.painter();
        painter.rect_filled(track, CornerRadius::same(3), Color32::from_white_alpha(46));
        let mut filled = track;
        filled.set_right(track.left() + track.width() * frac as f32);
        painter.rect_filled(filled, CornerRadius::same(3), if hovered { accent_light(accent) } else { Color32::from_gray(235) });
        if hovered {
            painter.circle_filled(pos2(filled.right(), rect.center().y), 6.5, Color32::WHITE);
        }
        if resp.hovered()
            && !active
            && dur > 0.0
            && let Some(p) = resp.hover_pos()
        {
            let t = frac_at(p.x) * dur;
            let _ = resp.on_hover_text_at_pointer(fmt_time(t));
        }
    }
    SeekOut { commit, display: frac * dur }
}

/// Make sure an accent is light enough for a progress fill on dark bg.
pub fn accent_light(c: Color32) -> Color32 {
    let l = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
    if l >= 110.0 {
        c
    } else {
        let t = ((110.0 - l) / 255.0).clamp(0.0, 0.8);
        lerp_color(c, Color32::WHITE, t)
    }
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Horizontal 0..1 slider (volume). Emits while dragging; scroll wheel too.
pub fn level_slider(ui: &mut Ui, value: f32, width: f32, accent: Color32) -> Option<f32> {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 18.0), Sense::click_and_drag());
    let mut out = None;
    if (resp.dragged() || resp.clicked() || resp.is_pointer_button_down_on())
        && let Some(p) = resp.interact_pointer_pos()
    {
        out = Some(((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0));
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.smooth_scroll_delta.x);
        if scroll.abs() > 0.0 {
            out = Some((value + scroll / 600.0).clamp(0.0, 1.0));
        }
    }
    let v = out.unwrap_or(value);
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered() || resp.dragged();
        let track = Rect::from_center_size(rect.center(), vec2(rect.width(), if hovered { 6.0 } else { 4.0 }));
        ui.painter().rect_filled(track, CornerRadius::same(3), Color32::from_white_alpha(46));
        let mut f = track;
        f.set_right(track.left() + track.width() * v);
        ui.painter().rect_filled(f, CornerRadius::same(3), if hovered { accent_light(accent) } else { Color32::from_gray(225) });
        if hovered {
            ui.painter().circle_filled(pos2(f.right(), rect.center().y), 6.0, Color32::WHITE);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Song context menu
// ---------------------------------------------------------------------------

pub fn watch_url(song: &Song) -> Option<String> {
    song.video_id.as_ref().map(|v| format!("https://music.youtube.com/watch?v={v}"))
}

/// Shared "song actions" menu body.
pub fn song_menu(ui: &mut Ui, cx: &mut Ctx, song: &Song, playlist_ctx: Option<&str>) {
    ui.set_min_width(200.0);
    if ui.button(tr("Play")).clicked() {
        cx.act(Action::PlaySong(song.clone()));
        ui.close();
    }
    if ui.button(tr("Play next")).clicked() {
        cx.act(Action::PlayNext(song.clone()));
        ui.close();
    }
    if ui.button(tr("Add to queue")).clicked() {
        cx.act(Action::AddToQueue(vec![song.clone()]));
        ui.close();
    }
    if song.video_id.is_some() && ui.button(tr("Start radio")).clicked() {
        cx.act(Action::StartRadio(song.clone()));
        ui.close();
    }
    ui.separator();
    let liked = cx.is_liked(song);
    if ui.button(if liked { tr("Remove from liked songs") } else { tr("Add to liked songs") }).clicked() {
        cx.act(Action::ToggleLike(song.clone()));
        ui.close();
    }
    ui.menu_button(tr("Add to playlist"), |ui| {
        ui.set_min_width(180.0);
        if ui.button(tr("New playlist…")).clicked() {
            cx.act(Action::NewPlaylist(Some(song.clone())));
            ui.close();
        }
        if !cx.playlists.is_empty() {
            ui.separator();
        }
        let pls: Vec<(String, String)> = cx.playlists.iter().map(|p| (p.id.clone(), p.name.clone())).collect();
        for (id, name) in pls {
            if ui.button(name).clicked() {
                cx.act(Action::AddToPlaylist { playlist_id: id, song: song.clone() });
                ui.close();
            }
        }
    });
    if let Some(pl) = playlist_ctx
        && ui.button(tr("Remove from this playlist")).clicked()
    {
        cx.act(Action::RemoveFromPlaylist { playlist_id: pl.to_string(), song_id: song.id.clone() });
        ui.close();
    }
    let artist = song.artists.iter().find(|a| a.id.is_some());
    let album = song.album_id.clone();
    if artist.is_some() || album.is_some() {
        ui.separator();
    }
    if let Some(a) = artist
        && ui.button(i18n::goto_artist(&a.name)).clicked()
    {
        cx.act(Action::Navigate(Route::Artist(a.id.clone().unwrap_or_default())));
        ui.close();
    }
    if let Some(al) = album
        && ui.button(tr("Go to album")).clicked()
    {
        cx.act(Action::Navigate(Route::Album(al)));
        ui.close();
    }
    if let Some(url) = watch_url(song) {
        ui.separator();
        if ui.button(tr("Copy link")).clicked() {
            cx.act(Action::CopyText(url));
            ui.close();
        }
    }
}

// ---------------------------------------------------------------------------
// Song rows
// ---------------------------------------------------------------------------

#[derive(Default, Clone, Copy)]
pub struct RowOpts<'a> {
    /// Show "#n" instead of the thumbnail.
    pub numbered: bool,
    pub show_album: bool,
    /// Local playlist id, enables "remove from playlist".
    pub playlist_ctx: Option<&'a str>,
}

pub const ROW_H: f32 = 56.0;

/// One song row. Double-click (or click on the artwork) plays `list` from
/// `idx`; right click / "⋯" opens the song menu.
pub fn song_row(ui: &mut Ui, cx: &mut Ctx, list: &Arc<Vec<Song>>, idx: usize, opts: RowOpts) {
    let song = &list[idx];
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::click());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let id = ui.scope_id().with(("row", idx, &song.id));
    let is_current = cx.current_id == Some(song.id.as_str());
    let hovered = resp.hovered() || resp.context_menu_opened();
    if hovered {
        ui.painter().rect_filled(rect, CornerRadius::same(8), Color32::from_white_alpha(10));
    }
    let mut x = rect.left() + 8.0;
    let cy = rect.center().y;

    // Leading: number or artwork.
    let lead = Rect::from_min_size(pos2(x, cy - 20.0), Vec2::splat(40.0));
    if opts.numbered {
        let txt = if is_current && cx.playing { "♪".to_string() } else { format!("{}", idx + 1) };
        ui.painter().text(lead.center(), Align2::CENTER_CENTER, txt, FontId::proportional(14.0), if is_current { cx.accent } else { TEXT_DIM });
    } else {
        let tex = crate::images::song_src(song).and_then(|s| cx.images.get(&s, THUMB_PX));
        paint_artwork(ui, lead, tex.as_ref(), (cx.radius * 0.5).max(4.0));
    }
    let lead_resp = ui.interact(lead, id.with("lead"), Sense::click());
    if hovered || lead_resp.hovered() {
        ui.painter().rect_filled(lead, CornerRadius::same(6), Color32::from_black_alpha(120));
        icons::paint(ui.painter(), lead.shrink(11.0), Icon::Play, Color32::WHITE);
    }
    x += 52.0;

    // Trailing controls (right to left).
    let mut right = rect.right() - 8.0;
    let more_rect = Rect::from_center_size(pos2(right - 14.0, cy), Vec2::splat(28.0));
    right -= 32.0;
    let dur_txt = song.duration_secs.map(fmt_time).unwrap_or_default();
    let dur_w = 48.0;
    ui.painter().text(pos2(right, cy), Align2::RIGHT_CENTER, dur_txt, FontId::proportional(13.0), TEXT_DIM);
    right -= dur_w;
    let liked = cx.is_liked(song);
    let heart_rect = Rect::from_center_size(pos2(right - 12.0, cy), Vec2::splat(26.0));
    right -= 32.0;

    // Middle: title / artist (+ album column when wide).
    let album_w = if opts.show_album && width > 720.0 { (width * 0.26).min(280.0) } else { 0.0 };
    let text_w = (right - x - album_w - 12.0).max(40.0);
    let title_color = if is_current { accent_light(cx.accent) } else { TEXT };
    let mut title = song.title.clone();
    if song.explicit {
        title.push_str("  🅴");
    }
    paint_text(ui, pos2(x, cy - 18.0), &title, FontId::proportional(14.5), title_color, text_w);
    let artist_text = artists_line(song);
    let ar = paint_text(ui, pos2(x, cy + 2.0), &artist_text, FontId::proportional(12.5), TEXT_DIM, text_w);
    if let Some(aid) = song.artists.iter().find_map(|a| a.id.clone()) {
        let r = ui.interact(ar, id.with("artist"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        if r.hovered() {
            ui.painter().line_segment([ar.left_bottom(), ar.right_bottom()], Stroke::new(1.0, TEXT_DIM));
        }
        if r.clicked() {
            cx.act(Action::Navigate(Route::Artist(aid)));
        }
    }
    if album_w > 0.0
        && let Some(al) = &song.album_name
    {
        let ax = right - album_w;
        let alr = paint_text(ui, pos2(ax, cy - 8.0), al, FontId::proportional(13.0), TEXT_DIM, album_w - 12.0);
        if let Some(alid) = &song.album_id {
            let r = ui.interact(alr, id.with("album"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
            if r.clicked() {
                cx.act(Action::Navigate(Route::Album(alid.clone())));
            }
        }
    }

    // Heart + more buttons (visible on hover or when liked).
    let heart_resp = ui.interact(heart_rect, id.with("heart"), Sense::click());
    if hovered || liked || heart_resp.hovered() {
        let col = if liked { Color32::from_rgb(0xFF, 0x5A, 0x7A) } else if heart_resp.hovered() { Color32::WHITE } else { TEXT_DIM };
        icons::paint(ui.painter(), heart_rect.shrink(5.0), if liked { Icon::HeartFilled } else { Icon::Heart }, col);
    }
    if heart_resp.clicked() {
        cx.act(Action::ToggleLike(song.clone()));
    }
    let more_resp = ui.interact(more_rect, id.with("more"), Sense::click());
    if hovered || more_resp.hovered() {
        if more_resp.hovered() {
            ui.painter().circle_filled(more_rect.center(), 13.0, Color32::from_white_alpha(18));
        }
        icons::paint(ui.painter(), more_rect.shrink(6.0), Icon::More, TEXT);
    }
    egui::Popup::menu(&more_resp).show(|ui| song_menu(ui, cx, song, opts.playlist_ctx));
    resp.context_menu(|ui| song_menu(ui, cx, song, opts.playlist_ctx));

    if resp.double_clicked() || lead_resp.clicked() {
        cx.act(Action::PlayList { songs: list.clone(), start: idx, shuffle: false });
    }
}

/// A vertical list of song rows.
pub fn song_list(ui: &mut Ui, cx: &mut Ctx, list: &Arc<Vec<Song>>, opts: RowOpts) {
    for i in 0..list.len() {
        song_row(ui, cx, list, i, opts);
    }
}

// ---------------------------------------------------------------------------
// Cards & shelves
// ---------------------------------------------------------------------------

pub const CARD_W: f32 = 168.0;

/// Generic artwork card. Returns the click response of the whole card.
pub fn card(ui: &mut Ui, cx: &mut Ctx, src: Option<&ImageSrc>, title: &str, subtitle: &str, round: bool) -> Response {
    let size = vec2(CARD_W, CARD_W + 46.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let art = Rect::from_min_size(rect.min, Vec2::splat(CARD_W));
        let tex = src.and_then(|s| cx.images.get(s, THUMB_PX));
        let radius = if round { CARD_W / 2.0 } else { cx.radius };
        let lift = ui.ctx().animate_bool_with_time(resp.id.with("lift"), resp.hovered(), 0.12);
        let art = art.translate(vec2(0.0, -3.0 * lift));
        paint_artwork(ui, art, tex.as_ref(), radius);
        if resp.hovered() {
            ui.painter().rect_filled(art, CornerRadius::same(radius.min(255.0) as u8), Color32::from_black_alpha(60));
        }
        let ty = art.bottom() + 8.0;
        let align_x = if round { rect.center().x } else { rect.left() };
        let g = galley_trunc(ui, title, FontId::proportional(14.0), TEXT, CARD_W);
        let gx = if round { align_x - g.size().x / 2.0 } else { align_x };
        ui.painter().galley(pos2(gx, ty), g, TEXT);
        let g2 = galley_trunc(ui, subtitle, FontId::proportional(12.5), TEXT_DIM, CARD_W);
        let gx2 = if round { align_x - g2.size().x / 2.0 } else { align_x };
        ui.painter().galley(pos2(gx2, ty + 19.0), g2, TEXT_DIM);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn album_card(ui: &mut Ui, cx: &mut Ctx, a: &Album) {
    let src = a.thumbnail_url.clone().map(ImageSrc::Url);
    let sub = match a.year {
        Some(y) => format!("{} · {y}", tr("Album")),
        None => tr("Album").into(),
    };
    if card(ui, cx, src.as_ref(), &a.title, &sub, false).clicked() {
        cx.act(Action::Navigate(Route::Album(a.id.clone())));
    }
}

pub fn artist_card(ui: &mut Ui, cx: &mut Ctx, a: &Artist) {
    let src = a.thumbnail_url.clone().map(ImageSrc::Url);
    if card(ui, cx, src.as_ref(), &a.name, tr("Artist"), true).clicked() {
        cx.act(Action::Navigate(Route::Artist(a.id.clone())));
    }
}

pub fn playlist_card(ui: &mut Ui, cx: &mut Ctx, p: &Playlist) {
    let src = p.thumbnail_url.clone().map(ImageSrc::Url);
    let sub = if p.song_count > 0 { format!("{} · {}", tr("Playlist"), i18n::songs_count(p.song_count as usize)) } else { tr("Playlist").into() };
    if card(ui, cx, src.as_ref(), &p.name, &sub, false).clicked() {
        let id = p.browse_id.clone().unwrap_or_else(|| p.id.clone());
        if id.starts_with("LP") {
            cx.act(Action::Navigate(Route::LocalPlaylist(id)));
        } else {
            cx.act(Action::Navigate(Route::Playlist(id)));
        }
    }
}

/// Song card: click plays `list` from `idx` (`as_list`) or the song + its
/// radio; right click → menu.
pub fn song_card(ui: &mut Ui, cx: &mut Ctx, list: &Arc<Vec<Song>>, idx: usize, as_list: bool) {
    let s = &list[idx];
    let src = crate::images::song_src(s);
    let resp = card(ui, cx, src.as_ref(), &s.title, &artists_line(s), false);
    if resp.clicked() {
        if as_list {
            cx.act(Action::PlayList { songs: list.clone(), start: idx, shuffle: false });
        } else {
            cx.act(Action::PlaySong(s.clone()));
        }
    }
    resp.context_menu(|ui| song_menu(ui, cx, s, None));
}

/// Horizontal scrolling row of cards for a YTM shelf.
pub fn shelf_row(ui: &mut Ui, cx: &mut Ctx, shelf: &Shelf, salt: impl std::hash::Hash + std::fmt::Debug) {
    let songs: Arc<Vec<Song>> = Arc::new(
        shelf
            .items
            .iter()
            .filter_map(|i| match i {
                ShelfItem::Song(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
    );
    section_title(ui, &shelf.title);
    egui::ScrollArea::horizontal().id_salt(("shelf", salt)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            let mut si = 0;
            for item in &shelf.items {
                match item {
                    ShelfItem::Song(_) => {
                        song_card(ui, cx, &songs, si, false);
                        si += 1;
                    }
                    ShelfItem::Album(a) => album_card(ui, cx, a),
                    ShelfItem::Artist(a) => artist_card(ui, cx, a),
                    ShelfItem::Playlist(p) => playlist_card(ui, cx, p),
                }
            }
        });
    });
    ui.add_space(12.0);
}

/// Horizontal row of song cards (Recently played, Related…).
pub fn song_card_row(ui: &mut Ui, cx: &mut Ctx, title: &str, songs: &Arc<Vec<Song>>, salt: &str, as_list: bool) {
    if songs.is_empty() {
        return;
    }
    section_title(ui, title);
    egui::ScrollArea::horizontal().id_salt(("songrow", salt)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            for i in 0..songs.len() {
                song_card(ui, cx, songs, i, as_list);
            }
        });
    });
    ui.add_space(12.0);
}
