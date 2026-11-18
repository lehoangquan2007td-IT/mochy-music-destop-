//! Context menus of album / artist / playlist cards and the extra song-menu
//! entries (ports of `ui/menu/AlbumMenu.kt`, `YouTubeAlbumMenu.kt`,
//! `ArtistMenu.kt`, `PlaylistMenu.kt`, `YouTubePlaylistMenu.kt` and the
//! history / library items of `SongMenu.kt`).
//!
//! Which albums/artists/playlists are saved lives in egui memory
//! ([`set_saved`]), published by `BrowseState` whenever the DB changes, so
//! any widget can show "Save" vs "Remove from library" without new `Ctx` fields.

use std::sync::Arc;

use egui::{Id, Ui};
use mochy_core::models::{Album, Artist, ArtistRef, Playlist, Song};

use crate::actions::{Action, Ctx};
use crate::browse::{BrowseAction, CollectionOp, CollectionRef, Saved};
use crate::nav::Route;
use crate::ui::i18n::{self, tr};
use crate::widgets::RowOpts;

fn saved_id() -> Id {
    Id::unique("mochy_saved_ids")
}

/// Publish the saved ids (called by `BrowseState`).
pub fn set_saved(ctx: &egui::Context, saved: Arc<Saved>) {
    ctx.data_mut(|d| d.insert_temp(saved_id(), saved));
}

/// Current saved ids (empty until the library loaded).
pub fn saved(ui: &Ui) -> Arc<Saved> {
    ui.ctx().data(|d| d.get_temp::<Arc<Saved>>(saved_id())).unwrap_or_default()
}

fn browse(cx: &mut Ctx, a: BrowseAction) {
    cx.act(Action::Browse(a));
}

fn button(ui: &mut Ui, label: &str) -> bool {
    let clicked = ui.button(label).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

/// Play / Shuffle / Play next / Add to queue (+ Start radio) for a collection.
pub fn collection_items(ui: &mut Ui, cx: &mut Ctx, target: &CollectionRef, radio: bool) {
    let ops: [(&str, CollectionOp); 4] = [
        (tr("Play"), CollectionOp::Play),
        (tr("Shuffle"), CollectionOp::Shuffle),
        (tr("Play next"), CollectionOp::PlayNext),
        (tr("Add to queue"), CollectionOp::AddToQueue),
    ];
    for (label, op) in ops {
        if button(ui, label) {
            browse(cx, BrowseAction::Collection { target: target.clone(), op });
        }
    }
    if radio && button(ui, tr("Start radio")) {
        browse(cx, BrowseAction::Collection { target: target.clone(), op: CollectionOp::Radio });
    }
}

pub fn album_link(a: &Album) -> String {
    match &a.playlist_id {
        Some(p) => format!("https://music.youtube.com/playlist?list={p}"),
        None => format!("https://music.youtube.com/browse/{}", a.id),
    }
}

pub fn artist_link(a: &Artist) -> String {
    format!("https://music.youtube.com/channel/{}", a.channel_id.as_deref().unwrap_or(&a.id))
}

pub fn playlist_link(id: &str) -> String {
    format!("https://music.youtube.com/playlist?list={}", id.strip_prefix("VL").unwrap_or(id))
}

/// Whether an id is a YouTube Music id (vs a local DB / local-file id).
pub fn is_remote_album(id: &str) -> bool {
    id.starts_with("MPRE") || id.starts_with("OLAK")
}

pub fn is_remote_artist(id: &str) -> bool {
    id.starts_with("UC") || id.starts_with("LA") || id.starts_with("FEmusic_library_privately_owned_artist")
}

/// Route that opens an album (YTM page or the local library page).
pub fn album_route(id: &str) -> Route {
    if is_remote_album(id) { Route::Album(id.to_string()) } else { Route::LibraryAlbum(id.to_string()) }
}

/// Route that opens an artist (YTM page or the local library page).
pub fn artist_route(id: &str) -> Route {
    if is_remote_artist(id) { Route::Artist(id.to_string()) } else { Route::LibraryArtist(id.to_string()) }
}

/// Album card menu.
pub fn album_menu(ui: &mut Ui, cx: &mut Ctx, a: &Album, artists: &[ArtistRef]) {
    ui.set_min_width(200.0);
    let remote = is_remote_album(&a.id);
    if button(ui, tr("Open")) {
        cx.act(Action::Navigate(album_route(&a.id)));
    }
    ui.separator();
    if remote {
        collection_items(ui, cx, &CollectionRef::Album(a.id.clone()), true);
        ui.separator();
        let saved = saved(ui).albums.contains(&a.id);
        if button(ui, if saved { tr("Remove from library") } else { tr("Save to library") }) {
            browse(cx, BrowseAction::SaveAlbum { album: a.clone(), artists: artists.to_vec(), save: !saved });
        }
    }
    let linked: Vec<&ArtistRef> = artists.iter().filter(|x| x.id.is_some()).collect();
    if !linked.is_empty() {
        ui.separator();
        for ar in linked {
            if button(ui, &i18n::goto_artist(&ar.name)) {
                cx.act(Action::Navigate(artist_route(ar.id.as_deref().unwrap_or_default())));
            }
        }
    }
    if remote {
        ui.separator();
        if button(ui, tr("Copy link")) {
            cx.act(Action::CopyText(album_link(a)));
        }
    }
}

/// Artist card menu.
pub fn artist_menu(ui: &mut Ui, cx: &mut Ctx, a: &Artist) {
    ui.set_min_width(200.0);
    let remote = is_remote_artist(&a.id);
    if button(ui, tr("Open")) {
        cx.act(Action::Navigate(artist_route(&a.id)));
    }
    if !remote {
        return;
    }
    ui.separator();
    let target = CollectionRef::Artist(a.id.clone());
    for (label, op) in [(tr("Play"), CollectionOp::Play), (tr("Shuffle"), CollectionOp::Shuffle), (tr("Start radio"), CollectionOp::Radio)] {
        if button(ui, label) {
            browse(cx, BrowseAction::Collection { target: target.clone(), op });
        }
    }
    ui.separator();
    let saved = saved(ui).artists.contains(&a.id);
    if button(ui, if saved { tr("Remove from library") } else { tr("Save to library") }) {
        browse(cx, BrowseAction::SaveArtist { artist: a.clone(), save: !saved });
    }
    if button(ui, tr("Copy link")) {
        cx.act(Action::CopyText(artist_link(a)));
    }
}

/// Is `p` a local (editable DB) playlist?
pub fn is_local_playlist(p: &Playlist) -> bool {
    p.id.starts_with("LP") && p.browse_id.as_deref().is_none_or(|b| !b.starts_with("VL"))
}

/// Route that opens a playlist card.
pub fn playlist_route(p: &Playlist) -> Route {
    if is_local_playlist(p) {
        Route::LocalPlaylist(p.id.clone())
    } else {
        Route::Playlist(p.browse_id.clone().unwrap_or_else(|| format!("VL{}", p.id)))
    }
}

/// Playlist card menu (local and YouTube Music playlists).
pub fn playlist_menu(ui: &mut Ui, cx: &mut Ctx, p: &Playlist) {
    ui.set_min_width(200.0);
    let local = is_local_playlist(p);
    if button(ui, tr("Open")) {
        cx.act(Action::Navigate(playlist_route(p)));
    }
    ui.separator();
    let target = if local {
        CollectionRef::LocalPlaylist(p.id.clone())
    } else {
        CollectionRef::Playlist(p.browse_id.clone().unwrap_or_else(|| p.id.clone()))
    };
    collection_items(ui, cx, &target, true);
    ui.separator();
    if local {
        if button(ui, tr("Rename…")) {
            cx.act(Action::RenamePlaylist(p.id.clone()));
        }
        if crate::playlist_cover::can_edit(p) {
            if button(ui, tr("Change cover…")) {
                cx.act(Action::ChangePlaylistCover(p.id.clone()));
            }
            if crate::playlist_cover::has_custom(p) && button(ui, tr("Remove custom cover")) {
                cx.act(Action::RemovePlaylistCover(p.id.clone()));
            }
        }
        if button(ui, tr("Delete…")) {
            cx.act(Action::DeletePlaylist(p.id.clone()));
        }
    } else {
        let s = saved(ui);
        let saved = s.playlists.contains(&p.id) || p.browse_id.as_ref().is_some_and(|b| s.playlists.contains(b));
        if button(ui, if saved { tr("Remove from library") } else { tr("Save to library") }) {
            browse(cx, BrowseAction::SavePlaylist { playlist: p.clone(), save: !saved });
        }
        if button(ui, tr("Copy link")) {
            cx.act(Action::CopyText(playlist_link(p.browse_id.as_deref().unwrap_or(&p.id))));
        }
    }
}

/// Entries appended to the song menu: library toggle, remove from
/// history, other artists.
pub fn song_menu_extra(ui: &mut Ui, cx: &mut Ctx, song: &Song, opts: &RowOpts) {
    if song.is_local() && opts.history_event.is_none() {
        return;
    }
    ui.separator();
    if !song.is_local() {
        let in_lib = saved(ui).songs.contains(&song.id);
        if button(ui, if in_lib { tr("Remove from library") } else { tr("Add to library") }) {
            browse(cx, BrowseAction::SetInLibrary { song: song.clone(), add: !in_lib });
        }
    }
    if let Some(ev) = opts.history_event {
        if button(ui, tr("Remove from history")) {
            browse(cx, BrowseAction::RemoveHistoryEvent(ev));
        }
        if button(ui, tr("Remove all plays from history")) {
            browse(cx, BrowseAction::RemoveSongHistory(song.id.clone()));
        }
    }
    // The base menu links the first artist; offer the others too.
    let others: Vec<&ArtistRef> = song.artists.iter().filter(|a| a.id.is_some()).skip(1).collect();
    for a in others {
        if button(ui, &i18n::goto_artist(&a.name)) {
            cx.act(Action::Navigate(artist_route(a.id.as_deref().unwrap_or_default())));
        }
    }
}

/// What a detail page's save button toggles.
pub enum SaveTarget<'a> {
    Album(&'a Album, &'a [ArtistRef]),
    Artist(&'a Artist),
    Playlist(&'a Playlist),
}

/// "Save" / "Saved" toggle for album / artist / playlist pages.
pub fn save_button(ui: &mut Ui, cx: &mut Ctx, target: SaveTarget) {
    let s = saved(ui);
    let saved = match &target {
        SaveTarget::Album(a, _) => s.albums.contains(&a.id),
        SaveTarget::Artist(a) => s.artists.contains(&a.id),
        SaveTarget::Playlist(p) => s.playlists.contains(&p.id) || p.browse_id.as_ref().is_some_and(|b| s.playlists.contains(b)),
    };
    let icon = if saved { super::icons::Icon::BookmarkFilled } else { super::icons::Icon::Bookmark };
    let label = if saved { tr("In library") } else { tr("Save to library") };
    if super::accent_button(ui, icon, label, cx.accent, false).clicked() {
        let a = match target {
            SaveTarget::Album(a, artists) => BrowseAction::SaveAlbum { album: a.clone(), artists: artists.to_vec(), save: !saved },
            SaveTarget::Artist(a) => BrowseAction::SaveArtist { artist: a.clone(), save: !saved },
            SaveTarget::Playlist(p) => BrowseAction::SavePlaylist { playlist: p.clone(), save: !saved },
        };
        browse(cx, a);
    }
}

/// "⋯" button that opens `menu` as a popup.
pub fn more_button(ui: &mut Ui, cx: &mut Ctx, menu: impl FnOnce(&mut Ui, &mut Ctx)) {
    let r = super::icons::icon_button(ui, super::icons::Icon::More, 36.0, false, cx.accent).on_hover_text(tr("More"));
    egui::Popup::menu(&r).show(|ui| menu(ui, cx));
}
