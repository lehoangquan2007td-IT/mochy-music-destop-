//! Album / artist / playlist (remote and local) detail pages.

use std::sync::Arc;

use egui::{RichText, Sense, Ui, vec2};
use mochy_core::models::{Album, Artist, Playlist, Shelf, ShelfItem, Song};

use crate::actions::{Action, Ctx, LoadState};
use crate::images::{ImageSrc, LARGE_PX};
use crate::widgets::{self, RowOpts, TEXT, TEXT_DIM, icons::Icon};

/// What the page wants from the app.
pub enum DetailEvent {
    Retry,
}

#[allow(clippy::too_many_arguments)]
fn header(
    ui: &mut Ui,
    cx: &mut Ctx,
    src: Option<&ImageSrc>,
    round: bool,
    kind: &str,
    title: &str,
    sub: &str,
    desc: Option<&str>,
    songs: Option<&Arc<Vec<Song>>>,
    extra: impl FnOnce(&mut Ui, &mut Ctx),
) {
    ui.horizontal(|ui| {
        let size = 200.0;
        let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
        let tex = src.and_then(|s| cx.images.get(s, LARGE_PX));
        widgets::paint_artwork(ui, rect, tex.as_ref(), if round { size / 2.0 } else { cx.radius * 1.4 });
        ui.add_space(20.0);
        ui.vertical(|ui| {
            ui.add_space(28.0);
            ui.label(RichText::new(kind).size(13.0).color(TEXT_DIM));
            ui.label(RichText::new(title).size(32.0).strong().color(TEXT));
            if !sub.is_empty() {
                ui.label(RichText::new(sub).size(14.0).color(TEXT_DIM));
            }
            if let Some(d) = desc.filter(|d| !d.trim().is_empty()) {
                let short: String = d.chars().take(220).collect();
                ui.label(RichText::new(if d.chars().count() > 220 { format!("{short}…") } else { short }).size(12.5).color(TEXT_DIM))
                    .on_hover_text(d);
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if let Some(list) = songs.filter(|l| !l.is_empty()) {
                    if widgets::accent_button(ui, Icon::Play, "Play", cx.accent, true).clicked() {
                        cx.act(Action::PlayList { songs: list.clone(), start: 0, shuffle: false });
                    }
                    if widgets::accent_button(ui, Icon::Shuffle, "Shuffle", cx.accent, false).clicked() {
                        cx.act(Action::PlayList { songs: list.clone(), start: 0, shuffle: true });
                    }
                    if widgets::accent_button(ui, Icon::Queue, "Add to queue", cx.accent, false).clicked() {
                        cx.act(Action::AddToQueue(list.as_ref().clone()));
                    }
                }
                extra(ui, cx);
            });
        });
    });
    ui.add_space(18.0);
}

fn total_duration(songs: &[Song]) -> String {
    let t: f64 = songs.iter().filter_map(|s| s.duration_secs).sum();
    if t <= 0.0 {
        return String::new();
    }
    let m = (t / 60.0).round() as u64;
    if m >= 60 { format!("{} h {} min", m / 60, m % 60) } else { format!("{m} min") }
}

fn state_guard<T>(ui: &mut Ui, st: Option<&LoadState<T>>) -> Result<Arc<T>, Option<DetailEvent>> {
    match st {
        None | Some(LoadState::Loading) => {
            widgets::loading(ui, "Loading…");
            Err(None)
        }
        Some(LoadState::Failed(e)) => Err(widgets::error_box(ui, e).then_some(DetailEvent::Retry)),
        Some(LoadState::Ready(v)) => Ok(v.clone()),
    }
}

pub fn album(ui: &mut Ui, cx: &mut Ctx, st: Option<&LoadState<(Album, Vec<Song>)>>, songs_arc: &mut Option<(String, Arc<Vec<Song>>)>) -> Option<DetailEvent> {
    let data = match state_guard(ui, st) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let (al, songs) = data.as_ref();
    let list = cached_arc(songs_arc, &al.id, songs);
    let artists: Vec<String> = songs.first().map(|s| s.artists.iter().map(|a| a.name.clone()).collect()).unwrap_or_default();
    let mut sub = artists.join(", ");
    if let Some(y) = al.year {
        sub.push_str(&format!(" · {y}"));
    }
    sub.push_str(&format!(" · {} songs", songs.len()));
    let d = total_duration(songs);
    if !d.is_empty() {
        sub.push_str(&format!(" · {d}"));
    }
    let src = al.thumbnail_url.clone().map(ImageSrc::Url);
    header(ui, cx, src.as_ref(), false, "Album", &al.title, &sub, al.description.as_deref(), Some(&list), |_, _| {});
    widgets::song_list(ui, cx, &list, RowOpts { numbered: true, ..Default::default() });
    None
}

pub fn artist(ui: &mut Ui, cx: &mut Ctx, st: Option<&LoadState<(Artist, Vec<Shelf>)>>, songs_arc: &mut Option<(String, Arc<Vec<Song>>)>) -> Option<DetailEvent> {
    let data = match state_guard(ui, st) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let (ar, shelves) = data.as_ref();
    // First all-song shelf = "Top songs".
    let top: Vec<Song> = shelves
        .iter()
        .find(|s| !s.items.is_empty() && s.items.iter().all(|i| matches!(i, ShelfItem::Song(_))))
        .map(|s| s.items.iter().filter_map(|i| if let ShelfItem::Song(s) = i { Some(s.clone()) } else { None }).collect())
        .unwrap_or_default();
    let list = cached_arc(songs_arc, &ar.id, &top);
    let src = ar.thumbnail_url.clone().map(ImageSrc::Url);
    let radio_seed = top.first().cloned();
    header(ui, cx, src.as_ref(), true, "Artist", &ar.name, "", None, Some(&list), |ui, cx| {
        if let Some(seed) = radio_seed
            && widgets::accent_button(ui, Icon::Radio, "Radio", cx.accent, false).clicked()
        {
            cx.act(Action::StartRadio(seed));
        }
    });
    let mut shown_top = false;
    for (i, shelf) in shelves.iter().enumerate() {
        if shelf.items.is_empty() {
            continue;
        }
        let all_songs = shelf.items.iter().all(|i| matches!(i, ShelfItem::Song(_)));
        if all_songs && !shown_top {
            shown_top = true;
            widgets::section_title(ui, &shelf.title);
            for idx in 0..list.len().min(10) {
                widgets::song_row(ui, cx, &list, idx, RowOpts { show_album: true, ..Default::default() });
            }
            ui.add_space(12.0);
        } else {
            widgets::shelf_row(ui, cx, shelf, ("artist", &ar.id, i));
        }
    }
    None
}

pub fn playlist(ui: &mut Ui, cx: &mut Ctx, st: Option<&LoadState<(Playlist, Vec<Song>)>>, songs_arc: &mut Option<(String, Arc<Vec<Song>>)>) -> Option<DetailEvent> {
    let data = match state_guard(ui, st) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let (pl, songs) = data.as_ref();
    let list = cached_arc(songs_arc, &pl.id, songs);
    let mut sub = format!("{} songs", songs.len());
    let d = total_duration(songs);
    if !d.is_empty() {
        sub.push_str(&format!(" · {d}"));
    }
    let src = pl.thumbnail_url.clone().or_else(|| songs.first().and_then(|s| s.thumbnail_url.clone())).map(ImageSrc::Url);
    header(ui, cx, src.as_ref(), false, "Playlist", &pl.name, &sub, pl.description.as_deref(), Some(&list), |_, _| {});
    widgets::song_list(ui, cx, &list, RowOpts { show_album: true, ..Default::default() });
    None
}

/// Local (DB) playlist page.
pub fn local_playlist(ui: &mut Ui, cx: &mut Ctx, pl: Option<&Playlist>, songs: Option<&Arc<Vec<Song>>>) {
    let Some(pl) = pl else {
        ui.label(RichText::new("Playlist not found").color(TEXT_DIM));
        return;
    };
    let Some(list) = songs else {
        widgets::loading(ui, "Loading playlist…");
        return;
    };
    let mut sub = format!("{} songs", list.len());
    let d = total_duration(list);
    if !d.is_empty() {
        sub.push_str(&format!(" · {d}"));
    }
    let src = list.first().and_then(crate::images::song_src);
    let id = pl.id.clone();
    header(ui, cx, src.as_ref(), false, "Your playlist", &pl.name, &sub, pl.description.as_deref(), Some(list), |ui, cx| {
        if widgets::accent_button(ui, Icon::More, "Rename", cx.accent, false).clicked() {
            cx.act(Action::RenamePlaylist(id.clone()));
        }
        if widgets::accent_button(ui, Icon::Trash, "Delete", cx.accent, false).clicked() {
            cx.act(Action::DeletePlaylist(id.clone()));
        }
    });
    if list.is_empty() {
        ui.label(RichText::new("This playlist is empty. Add songs from their ⋯ menu.").color(TEXT_DIM));
        return;
    }
    widgets::song_list(ui, cx, list, RowOpts { show_album: true, playlist_ctx: Some(&pl.id), ..Default::default() });
}

/// Reuse an `Arc<Vec<Song>>` across frames for the same page id.
fn cached_arc(slot: &mut Option<(String, Arc<Vec<Song>>)>, id: &str, songs: &[Song]) -> Arc<Vec<Song>> {
    if let Some((k, a)) = slot
        && k == id
        && a.len() == songs.len()
    {
        return a.clone();
    }
    let a = Arc::new(songs.to_vec());
    *slot = Some((id.to_string(), a.clone()));
    a
}
