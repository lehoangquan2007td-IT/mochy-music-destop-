//! Library list views (ports of `LibrarySongsScreen`, `LibraryAlbumsScreen`,
//! `LibraryArtistsScreen`, `LibraryPlaylistsScreen`,
//! `LibraryRecentlyAddedScreen`, `HistoryScreen`): search within the
//! library, sorting, grid/list layouts, history grouped by day with
//! "remove from history", and the local artist / album pages.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::{CornerRadius, FontId, Rect, Response, RichText, Sense, Ui, Vec2, pos2, vec2};
use mochy_core::models::{Album, Artist, Playlist, ShelfItem, Song};

use super::Data;
use super::explore::play_buttons;
use crate::actions::{Action, Ctx};
use crate::browse::{AlbumSort, ArtistSort, BrowseAction, BrowseState, PlaylistSort, SongFilter, SongSort};
use crate::images::{ImageSrc, LARGE_PX, THUMB_PX};
use crate::nav::Route;
use crate::ui::i18n::{self, tr};
use crate::widgets::shelves;
use crate::widgets::{self, RowOpts, TEXT, TEXT_DIM, artists_line, icons::Icon, menus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    Songs,
    Liked,
    Albums,
    Artists,
    Playlists,
    History,
}

fn contains(hay: &str, q: &str) -> bool {
    hay.to_lowercase().contains(q)
}

fn song_matches(s: &Song, q: &str) -> bool {
    q.is_empty()
        || contains(&s.title, q)
        || s.artists.iter().any(|a| contains(&a.name, q))
        || s.album_name.as_deref().is_some_and(|al| contains(al, q))
}

/// Search + sort + layout toolbar.
pub fn toolbar(ui: &mut Ui, cx: &mut Ctx, st: &mut BrowseState, kind: ListKind) {
    let v = &mut st.lib_view;
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut v.query).hint_text(tr("Search in library")).desired_width(240.0));
        if !v.query.is_empty() && ui.small_button("✕").on_hover_text(tr("Clear")).clicked() {
            v.query.clear();
        }
        ui.add_space(8.0);
        let sort_combo = |ui: &mut Ui, id: &str, current: &str, options: &mut dyn FnMut(&mut Ui)| {
            egui::ComboBox::from_id_salt(("lib_sort", id)).selected_text(format!("{}: {current}", tr("Sort"))).show_ui(ui, |ui| options(ui));
        };
        match kind {
            ListKind::Songs | ListKind::Liked => {
                let label = song_sort_label(v.song_sort);
                sort_combo(ui, "songs", label, &mut |ui| {
                    for s in [SongSort::Added, SongSort::Name, SongSort::Artist, SongSort::PlayTime] {
                        ui.selectable_value(&mut v.song_sort, s, song_sort_label(s));
                    }
                });
                reverse_toggle(ui, &mut v.song_desc);
            }
            ListKind::Albums => {
                let label = album_sort_label(v.album_sort);
                sort_combo(ui, "albums", label, &mut |ui| {
                    for s in [AlbumSort::Added, AlbumSort::Name, AlbumSort::Artist, AlbumSort::Year, AlbumSort::SongCount, AlbumSort::PlayTime] {
                        ui.selectable_value(&mut v.album_sort, s, album_sort_label(s));
                    }
                });
                reverse_toggle(ui, &mut v.album_desc);
            }
            ListKind::Artists => {
                let label = artist_sort_label(v.artist_sort);
                sort_combo(ui, "artists", label, &mut |ui| {
                    for s in [ArtistSort::Added, ArtistSort::Name, ArtistSort::SongCount, ArtistSort::PlayTime] {
                        ui.selectable_value(&mut v.artist_sort, s, artist_sort_label(s));
                    }
                });
                reverse_toggle(ui, &mut v.artist_desc);
            }
            ListKind::Playlists => {
                let label = playlist_sort_label(v.playlist_sort);
                sort_combo(ui, "playlists", label, &mut |ui| {
                    for s in [PlaylistSort::Added, PlaylistSort::Name, PlaylistSort::SongCount] {
                        ui.selectable_value(&mut v.playlist_sort, s, playlist_sort_label(s));
                    }
                });
                reverse_toggle(ui, &mut v.playlist_desc);
            }
            ListKind::History => {}
        }
        if matches!(kind, ListKind::Albums | ListKind::Artists) {
            ui.checkbox(&mut v.saved_only, tr("Saved only"));
        }
        if matches!(kind, ListKind::Albums | ListKind::Artists | ListKind::Playlists) {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (icon, tip) = if v.list_layout { (Icon::Grid, tr("Grid")) } else { (Icon::ListView, tr("List")) };
                if widgets::icons::icon_button(ui, icon, 30.0, false, cx.accent).on_hover_text(tip).clicked() {
                    v.list_layout = !v.list_layout;
                }
            });
        }
    });
    if kind == ListKind::Songs {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for (f, label) in [(SongFilter::Library, tr("Library")), (SongFilter::Liked, tr("Liked")), (SongFilter::Local, tr("Local files"))] {
                if widgets::pill(ui, label, v.song_filter == f).clicked() {
                    v.song_filter = f;
                }
            }
        });
    }
    ui.add_space(10.0);
}

fn reverse_toggle(ui: &mut Ui, desc: &mut bool) {
    let txt = if *desc { "↑" } else { "↓" };
    if ui.button(txt).on_hover_text(tr("Reverse order")).clicked() {
        *desc = !*desc;
    }
}

fn song_sort_label(s: SongSort) -> &'static str {
    match s {
        SongSort::Added => tr("Date added"),
        SongSort::Name => tr("Name"),
        SongSort::Artist => tr("Artist"),
        SongSort::PlayTime => tr("Play time"),
    }
}

fn album_sort_label(s: AlbumSort) -> &'static str {
    match s {
        AlbumSort::Added => tr("Date added"),
        AlbumSort::Name => tr("Name"),
        AlbumSort::Artist => tr("Artist"),
        AlbumSort::Year => tr("Year"),
        AlbumSort::SongCount => tr("Song count"),
        AlbumSort::PlayTime => tr("Play time"),
    }
}

fn artist_sort_label(s: ArtistSort) -> &'static str {
    match s {
        ArtistSort::Added => tr("Date added"),
        ArtistSort::Name => tr("Name"),
        ArtistSort::SongCount => tr("Song count"),
        ArtistSort::PlayTime => tr("Play time"),
    }
}

fn playlist_sort_label(s: PlaylistSort) -> &'static str {
    match s {
        PlaylistSort::Added => tr("Date added"),
        PlaylistSort::Name => tr("Name"),
        PlaylistSort::SongCount => tr("Song count"),
    }
}

/// `source` filtered by the search query and (optionally) sorted; cached
/// across frames per view.
pub fn filtered_songs(st: &mut BrowseState, view: &'static str, source: &Arc<Vec<Song>>, sort: bool) -> Arc<Vec<Song>> {
    let v = &st.lib_view;
    let q = v.query.trim().to_lowercase();
    if q.is_empty() && (!sort || (v.song_sort == SongSort::Added && !v.song_desc)) {
        return source.clone();
    }
    let mut h = DefaultHasher::new();
    (Arc::as_ptr(source) as usize, source.len(), &q, sort).hash(&mut h);
    if sort {
        (v.song_sort as u8, v.song_desc).hash(&mut h);
    }
    let key = h.finish();
    if let Some((k, list)) = st.view_cache.get(view)
        && *k == key
    {
        return list.clone();
    }
    let mut out: Vec<Song> = source.iter().filter(|s| song_matches(s, &q)).cloned().collect();
    if sort {
        match v.song_sort {
            SongSort::Added => {}
            SongSort::Name => out.sort_by_cached_key(|s| s.title.to_lowercase()),
            SongSort::Artist => out.sort_by_cached_key(|s| artists_line(s).to_lowercase()),
            SongSort::PlayTime => out.sort_by(|a, b| b.total_play_time.total_cmp(&a.total_play_time)),
        }
        if v.song_desc {
            out.reverse();
        }
    }
    let list = Arc::new(out);
    st.view_cache.insert(view, (key, list.clone()));
    list
}

// ---------------------------------------------------------------------------
// Songs
// ---------------------------------------------------------------------------

pub fn songs(ui: &mut Ui, cx: &mut Ctx, data: &Data, st: &mut BrowseState) {
    toolbar(ui, cx, st, ListKind::Songs);
    let (source, empty) = match st.lib_view.song_filter {
        SongFilter::Library => (st.lib.songs.clone(), tr("Your library is empty — add songs from their ⋯ menu.")),
        SongFilter::Liked => (data.liked.clone(), tr("No liked songs yet — tap ♥ on any song.")),
        SongFilter::Local => (data.local.clone(), tr("No local files yet — add a music folder.")),
    };
    let list = filtered_songs(st, "songs", &source, true);
    if list.is_empty() {
        let msg = if st.lib_view.query.trim().is_empty() { empty } else { tr("No results in your library.") };
        ui.label(RichText::new(msg).color(TEXT_DIM));
        return;
    }
    play_buttons(ui, cx, &list);
    widgets::song_list(ui, cx, &list, RowOpts { show_album: true, ..Default::default() });
}

// ---------------------------------------------------------------------------
// Albums / artists
// ---------------------------------------------------------------------------

/// List-layout row for an album / artist / playlist.
fn entity_row(ui: &mut Ui, cx: &mut Ctx, src: Option<&ImageSrc>, title: &str, sub: &str, round: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 60.0), Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() || resp.context_menu_opened() {
            ui.painter().rect_filled(rect, CornerRadius::same(8), egui::Color32::from_white_alpha(10));
        }
        let art = Rect::from_min_size(pos2(rect.left() + 8.0, rect.center().y - 24.0), Vec2::splat(48.0));
        let tex = src.and_then(|s| cx.images.get(s, THUMB_PX));
        widgets::paint_artwork(ui, art, tex.as_ref(), if round { 24.0 } else { (cx.radius * 0.5).max(4.0) });
        let x = art.right() + 14.0;
        let w = rect.right() - x - 12.0;
        widgets::paint_text(ui, pos2(x, rect.center().y - 19.0), title, FontId::proportional(14.5), TEXT, w);
        widgets::paint_text(ui, pos2(x, rect.center().y + 2.0), sub, FontId::proportional(12.5), TEXT_DIM, w);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn year_part(a: &Album) -> String {
    a.year.map(|y| format!(" • {y}")).unwrap_or_default()
}

pub fn albums(ui: &mut Ui, cx: &mut Ctx, st: &mut BrowseState) {
    toolbar(ui, cx, st, ListKind::Albums);
    let v = st.lib_view.clone();
    let q = v.query.trim().to_lowercase();
    let mut list: Vec<&mochy_storage::AlbumEntry> = st
        .lib
        .albums
        .iter()
        .filter(|e| !v.saved_only || e.saved_at.is_some())
        .filter(|e| q.is_empty() || contains(&e.album.title, &q) || contains(&e.artists, &q))
        .collect();
    match v.album_sort {
        AlbumSort::Added => list.sort_by(|a, b| b.added_at.cmp(&a.added_at)),
        AlbumSort::Name => list.sort_by_cached_key(|e| e.album.title.to_lowercase()),
        AlbumSort::Artist => list.sort_by_cached_key(|e| e.artists.to_lowercase()),
        AlbumSort::Year => list.sort_by(|a, b| b.album.year.cmp(&a.album.year)),
        AlbumSort::SongCount => list.sort_by(|a, b| b.library_songs.max(b.album.song_count.unwrap_or(0)).cmp(&a.library_songs.max(a.album.song_count.unwrap_or(0)))),
        AlbumSort::PlayTime => list.sort_by(|a, b| b.play_time_ms.cmp(&a.play_time_ms)),
    }
    if v.album_desc {
        list.reverse();
    }
    if list.is_empty() {
        ui.label(RichText::new(tr("No albums yet — save one from its page or ⋯ menu.")).color(TEXT_DIM));
        return;
    }
    if v.list_layout {
        for e in list {
            let src = e.album.thumbnail_url.clone().map(ImageSrc::Url);
            let sub = format!("{}{}{}", if e.artists.is_empty() { tr("Album") } else { &e.artists }, year_part(&e.album), saved_mark(e.saved_at));
            let resp = entity_row(ui, cx, src.as_ref(), &e.album.title, &sub, false);
            if resp.clicked() {
                cx.act(Action::Navigate(menus::album_route(&e.album.id)));
            }
            resp.context_menu(|ui| menus::album_menu(ui, cx, &e.album, &[]));
        }
    } else {
        let items: Vec<ShelfItem> = list.iter().map(|e| ShelfItem::Album(e.album.clone())).collect();
        let subs: Vec<String> = list.iter().map(|e| format!("{}{}", if e.artists.is_empty() { tr("Album") } else { &e.artists }, year_part(&e.album))).collect();
        shelves::cards_grid(ui, cx, &items, &subs);
    }
}

fn saved_mark(saved: Option<i64>) -> &'static str {
    if saved.is_some() { "  ★" } else { "" }
}

pub fn artists(ui: &mut Ui, cx: &mut Ctx, st: &mut BrowseState) {
    toolbar(ui, cx, st, ListKind::Artists);
    let v = st.lib_view.clone();
    let q = v.query.trim().to_lowercase();
    let mut list: Vec<&mochy_storage::ArtistEntry> = st
        .lib
        .artists
        .iter()
        .filter(|e| !v.saved_only || e.saved_at.is_some())
        .filter(|e| q.is_empty() || contains(&e.artist.name, &q))
        .collect();
    match v.artist_sort {
        ArtistSort::Added => list.sort_by(|a, b| b.added_at.cmp(&a.added_at)),
        ArtistSort::Name => list.sort_by_cached_key(|e| e.artist.name.to_lowercase()),
        ArtistSort::SongCount => list.sort_by(|a, b| b.song_count.cmp(&a.song_count)),
        ArtistSort::PlayTime => list.sort_by(|a, b| b.play_time_ms.cmp(&a.play_time_ms)),
    }
    if v.artist_desc {
        list.reverse();
    }
    if list.is_empty() {
        ui.label(RichText::new(tr("No artists yet.")).color(TEXT_DIM));
        return;
    }
    if v.list_layout {
        for e in list {
            let src = e.artist.thumbnail_url.clone().map(ImageSrc::Url);
            let sub = format!("{}{}", i18n::songs_count(e.song_count as usize), saved_mark(e.saved_at));
            let resp = entity_row(ui, cx, src.as_ref(), &e.artist.name, &sub, true);
            if resp.clicked() {
                cx.act(Action::Navigate(menus::artist_route(&e.artist.id)));
            }
            resp.context_menu(|ui| menus::artist_menu(ui, cx, &e.artist));
        }
    } else {
        let items: Vec<ShelfItem> = list.iter().map(|e| ShelfItem::Artist(e.artist.clone())).collect();
        let subs: Vec<String> = list.iter().map(|e| i18n::songs_count(e.song_count as usize)).collect();
        shelves::cards_grid(ui, cx, &items, &subs);
    }
}

// ---------------------------------------------------------------------------
// Playlists
// ---------------------------------------------------------------------------

pub fn playlists(ui: &mut Ui, cx: &mut Ctx, data: &Data, st: &mut BrowseState) {
    ui.horizontal(|ui| {
        if widgets::accent_button(ui, Icon::Plus, tr("New playlist"), cx.accent, true).clicked() {
            cx.act(Action::NewPlaylist(None));
        }
    });
    ui.add_space(10.0);
    toolbar(ui, cx, st, ListKind::Playlists);
    let v = st.lib_view.clone();
    let q = v.query.trim().to_lowercase();
    let count = |p: &Playlist| data.playlist_songs.get(&p.id).map(|s| s.len() as u32).unwrap_or(p.song_count);
    let mut list: Vec<&Playlist> = data.playlists.iter().filter(|p| q.is_empty() || contains(&p.name, &q)).collect();
    match v.playlist_sort {
        PlaylistSort::Added => {}
        PlaylistSort::Name => list.sort_by_cached_key(|p| p.name.to_lowercase()),
        PlaylistSort::SongCount => list.sort_by_key(|p| std::cmp::Reverse(count(p))),
    }
    if v.playlist_desc {
        list.reverse();
    }
    if list.is_empty() {
        ui.label(RichText::new(tr("No playlists yet.")).color(TEXT_DIM));
        return;
    }
    let src_of = |p: &Playlist| {
        p.thumbnail_url
            .clone()
            .map(ImageSrc::Url)
            .or_else(|| data.playlist_songs.get(&p.id).and_then(|s| s.first()).and_then(crate::images::song_src))
    };
    let sub_of = |p: &Playlist| {
        if menus::is_local_playlist(p) {
            i18n::songs_count(count(p) as usize)
        } else {
            format!("{} • YouTube Music", tr("Playlist"))
        }
    };
    if v.list_layout {
        for p in list {
            let resp = entity_row(ui, cx, src_of(p).as_ref(), &p.name, &sub_of(p), false);
            if resp.clicked() {
                cx.act(Action::Navigate(menus::playlist_route(p)));
            }
            resp.context_menu(|ui| menus::playlist_menu(ui, cx, p));
        }
    } else {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(16.0, 16.0);
            for p in list {
                let resp = widgets::card(ui, cx, src_of(p).as_ref(), &p.name, &sub_of(p), false);
                if resp.clicked() {
                    cx.act(Action::Navigate(menus::playlist_route(p)));
                }
                resp.context_menu(|ui| menus::playlist_menu(ui, cx, p));
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Recently added
// ---------------------------------------------------------------------------

pub fn recently_added(ui: &mut Ui, cx: &mut Ctx, st: &mut BrowseState) {
    let recent = st.lib.recent_added.clone();
    if recent.is_empty() {
        ui.label(RichText::new(tr("Nothing added yet.")).color(TEXT_DIM));
        return;
    }
    let off = st.lib.utc_offset_secs;
    let today = local_day_index(crate::browse::now_ms(), off);
    // Group by when it was added, like the history.
    let mut groups: Vec<(String, Vec<ShelfItem>, Vec<String>)> = Vec::new();
    for r in recent.iter() {
        // Rows written by old builds carry a boolean (1) instead of a date.
        let label = if r.at < 946_684_800_000 { tr("Earlier").to_string() } else { day_group_label(local_day_index(r.at, off), today) };
        let sub = match &r.item {
            ShelfItem::Song(s) => format!("{} • {}", tr("Song"), artists_line(s)),
            ShelfItem::Album(_) => tr("Album").to_string(),
            ShelfItem::Artist(_) => tr("Artist").to_string(),
            ShelfItem::Playlist(_) => tr("Playlist").to_string(),
        };
        match groups.last_mut() {
            Some((l, items, subs)) if *l == label => {
                items.push(r.item.clone());
                subs.push(sub);
            }
            _ => groups.push((label, vec![r.item.clone()], vec![sub])),
        }
    }
    for (label, items, subs) in groups {
        widgets::section_title(ui, &label);
        shelves::cards_grid(ui, cx, &items, &subs);
        ui.add_space(14.0);
    }
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

/// Local day number (days since 1970-01-01) of an epoch-ms instant.
pub fn local_day_index(ms: i64, utc_offset_secs: i64) -> i64 {
    (ms.div_euclid(1000) + utc_offset_secs).div_euclid(86_400)
}

/// (year, month) of a day number (Howard Hinnant's civil-from-days).
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn month_name(m: u32) -> &'static str {
    tr(match m {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        _ => "December",
    })
}

/// Android `DateAgo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayGroup {
    Today,
    Yesterday,
    ThisWeek,
    LastWeek,
    Month(i64, u32),
}

pub fn day_group(day: i64, today: i64) -> DayGroup {
    let weekday = (today + 3).rem_euclid(7); // 0 = Monday (1970-01-01 was a Thursday)
    let monday = today - weekday;
    if day >= today {
        DayGroup::Today
    } else if day == today - 1 {
        DayGroup::Yesterday
    } else if day >= monday {
        DayGroup::ThisWeek
    } else if day >= monday - 7 {
        DayGroup::LastWeek
    } else {
        let (y, m, _) = civil_from_days(day);
        DayGroup::Month(y, m)
    }
}

/// Today / Yesterday / This week / Last week / "Month Year".
pub fn day_group_label(day: i64, today: i64) -> String {
    match day_group(day, today) {
        DayGroup::Today => tr("Today").into(),
        DayGroup::Yesterday => tr("Yesterday").into(),
        DayGroup::ThisWeek => tr("This week").into(),
        DayGroup::LastWeek => tr("Last week").into(),
        DayGroup::Month(y, m) => format!("{} {y}", month_name(m)),
    }
}

pub fn history(ui: &mut Ui, cx: &mut Ctx, st: &mut BrowseState) {
    toolbar(ui, cx, st, ListKind::History);
    let entries = st.lib.history.clone();
    let songs = st.lib.history_songs.clone();
    if entries.is_empty() {
        ui.label(RichText::new(tr("Nothing played yet.")).color(TEXT_DIM));
        return;
    }
    ui.horizontal(|ui| {
        if widgets::accent_button(ui, Icon::Play, tr("Play"), cx.accent, true).clicked() {
            cx.act(Action::PlayList { songs: songs.clone(), start: 0, shuffle: false });
        }
        if widgets::accent_button(ui, Icon::Shuffle, tr("Shuffle"), cx.accent, false).clicked() {
            cx.act(Action::PlayList { songs: songs.clone(), start: 0, shuffle: true });
        }
        ui.menu_button(tr("Clear history…"), |ui| {
            ui.label(tr("Remove every play from the history?"));
            if ui.button(tr("Clear history")).clicked() {
                cx.act(Action::Browse(BrowseAction::ClearHistory));
                ui.close();
            }
        });
        ui.label(RichText::new(i18n::songs_count(entries.len())).color(TEXT_DIM));
    });
    ui.add_space(8.0);
    let q = st.lib_view.query.trim().to_lowercase();
    let off = st.lib.utc_offset_secs;
    let today = local_day_index(crate::browse::now_ms(), off);
    let mut last_label = String::new();
    let mut shown = 0;
    for (i, e) in entries.iter().enumerate() {
        if !song_matches(&e.song, &q) {
            continue;
        }
        let label = day_group_label(local_day_index(e.timestamp, off), today);
        if label != last_label {
            widgets::section_title(ui, &label);
            last_label = label;
        }
        widgets::song_row(ui, cx, &songs, i, RowOpts { show_album: true, history_event: Some(e.event_id), ..Default::default() });
        shown += 1;
    }
    if shown == 0 {
        ui.label(RichText::new(tr("No results in your library.")).color(TEXT_DIM));
    }
}

// ---------------------------------------------------------------------------
// Local artist / album pages
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn local_header(ui: &mut Ui, cx: &mut Ctx, src: Option<&ImageSrc>, round: bool, kind: &str, title: &str, list: &Arc<Vec<Song>>, open_remote: Option<Route>) {
    ui.horizontal(|ui| {
        let size = 180.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
        let tex = src.and_then(|s| cx.images.get(s, LARGE_PX));
        widgets::paint_artwork(ui, rect, tex.as_ref(), if round { size / 2.0 } else { cx.radius * 1.4 });
        ui.add_space(20.0);
        ui.vertical(|ui| {
            ui.add_space(24.0);
            ui.label(RichText::new(kind).size(13.0).color(TEXT_DIM));
            ui.label(RichText::new(title).size(30.0).strong().color(TEXT));
            let mins = (list.iter().filter_map(|s| s.duration_secs).sum::<f64>() / 60.0).round() as u64;
            let mut sub = i18n::songs_count(list.len());
            let d = i18n::duration_label(mins);
            if !d.is_empty() {
                sub.push_str(&format!(" · {d}"));
            }
            ui.label(RichText::new(sub).size(14.0).color(TEXT_DIM));
            ui.add_space(12.0);
            play_buttons(ui, cx, list);
            if let Some(r) = open_remote
                && widgets::accent_button(ui, Icon::Compass, tr("Open on YouTube Music"), cx.accent, false).clicked()
            {
                cx.act(Action::Navigate(r));
            }
        });
    });
    ui.add_space(16.0);
}

pub fn local_artist(ui: &mut Ui, cx: &mut Ctx, st: &BrowseState, id: &str) {
    let entry = st.lib.artists.iter().find(|e| e.artist.id == id);
    let list = st.local_songs.get(&format!("artist:{id}")).cloned().unwrap_or_default();
    let name = entry
        .map(|e| e.artist.name.clone())
        .or_else(|| list.first().and_then(|s| s.artists.iter().find(|a| a.id.as_deref() == Some(id)).map(|a| a.name.clone())))
        .unwrap_or_else(|| tr("Unknown artist").into());
    let artist = entry.map(|e| e.artist.clone()).unwrap_or(Artist { id: id.into(), name: name.clone(), ..Default::default() });
    let src = artist.thumbnail_url.clone().map(ImageSrc::Url).or_else(|| list.first().and_then(crate::images::song_src));
    let remote = menus::is_remote_artist(id).then(|| Route::Artist(id.to_string()));
    local_header(ui, cx, src.as_ref(), true, tr("Artist"), &name, &list, remote);
    if list.is_empty() {
        ui.label(RichText::new(tr("No songs from this artist in your library.")).color(TEXT_DIM));
        return;
    }
    widgets::song_list(ui, cx, &list, RowOpts { show_album: true, ..Default::default() });
}

pub fn local_album(ui: &mut Ui, cx: &mut Ctx, st: &BrowseState, id: &str) {
    let entry = st.lib.albums.iter().find(|e| e.album.id == id);
    let list = st.local_songs.get(&format!("album:{id}")).cloned().unwrap_or_default();
    let title = entry
        .map(|e| e.album.title.clone())
        .filter(|t| !t.is_empty())
        .or_else(|| list.first().and_then(|s| s.album_name.clone()))
        .unwrap_or_else(|| tr("Album").into());
    let src = entry
        .and_then(|e| e.album.thumbnail_url.clone())
        .map(ImageSrc::Url)
        .or_else(|| list.first().and_then(crate::images::song_src));
    let remote = menus::is_remote_album(id).then(|| Route::Album(id.to_string()));
    local_header(ui, cx, src.as_ref(), false, tr("Album"), &title, &list, remote);
    if list.is_empty() {
        ui.label(RichText::new(tr("No songs from this album in your library.")).color(TEXT_DIM));
        return;
    }
    widgets::song_list(ui, cx, &list, RowOpts { numbered: true, ..Default::default() });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_and_day_groups() {
        // 2026-11-17 is day 20774 (a Tuesday).
        assert_eq!(civil_from_days(20_774), (2026, 11, 17));
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        let today = 20_774;
        assert_eq!(day_group(today, today), DayGroup::Today);
        assert_eq!(day_group(today - 1, today), DayGroup::Yesterday);
        assert_eq!(day_group(today - 2, today), DayGroup::LastWeek, "Sunday before a Tuesday is last week");
        assert_eq!(day_group(today - 9, today), DayGroup::Month(2026, 11));
        assert_eq!(day_group(today - 30, today), DayGroup::Month(2026, 10));
        // Friday: Monday..Thursday are "this week".
        assert_eq!(day_group(today + 3 - 3, today + 3), DayGroup::ThisWeek);
        assert_eq!(local_day_index(86_400_000 + 1, 0), 1);
        assert_eq!(local_day_index(86_400_000 - 1000, 3600), 1);
    }
}
