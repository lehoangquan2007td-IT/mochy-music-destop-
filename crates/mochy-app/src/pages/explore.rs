//! Explore (port of `ExploreScreen.kt`), moods & genres
//! (`MoodAndGenresScreen`), generic browse pages (`BrowseScreen.kt`,
//! `YouTubeBrowseScreen.kt`, `ArtistItemsScreen`, charts, new releases) and
//! the home "see all" pages (`SectionDetailScreen.kt`).

use std::sync::Arc;

use egui::{RichText, Ui};
use mochy_core::models::{ShelfItem, Song};

use super::Data;
use crate::actions::{Action, Ctx};
use crate::browse::{BrowseAction, BrowseState, Slot, page_key};
use crate::nav::Route;
use crate::ui::i18n::{self, tr};
use crate::widgets::shelves::{self, see_all_action, songs_of};
use crate::widgets::{self, RowOpts, TEXT_DIM, icons::Icon};

fn retry(cx: &mut Ctx, key: &str) {
    cx.act(Action::Browse(BrowseAction::Retry(key.to_string())));
}

/// Loading / error placeholder; returns `true` when the content is ready.
fn guard<T>(ui: &mut Ui, cx: &mut Ctx, slot: Option<&Slot<T>>, key: &str) -> bool {
    match slot {
        None | Some(Slot::Loading) => {
            widgets::loading(ui, tr("Loading…"));
            false
        }
        Some(Slot::Failed(e)) => {
            if widgets::error_box(ui, e) {
                retry(cx, key);
            }
            false
        }
        Some(Slot::Ready(_)) => true,
    }
}

fn shortcut_icon(id: &str) -> Icon {
    match id {
        "FEmusic_charts" => Icon::Chart,
        "FEmusic_moods_and_genres" => Icon::Compass,
        _ => Icon::Note,
    }
}

/// Play / Shuffle / Add to queue buttons for a song list.
pub fn play_buttons(ui: &mut Ui, cx: &mut Ctx, list: &Arc<Vec<Song>>) {
    if list.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        if widgets::accent_button(ui, Icon::Play, tr("Play"), cx.accent, true).clicked() {
            cx.act(Action::PlayList { songs: list.clone(), start: 0, shuffle: false });
        }
        if widgets::accent_button(ui, Icon::Shuffle, tr("Shuffle"), cx.accent, false).clicked() {
            cx.act(Action::PlayList { songs: list.clone(), start: 0, shuffle: true });
        }
        if widgets::accent_button(ui, Icon::Queue, tr("Add to queue"), cx.accent, false).clicked() {
            cx.act(Action::AddToQueue(list.as_ref().clone()));
        }
        ui.label(RichText::new(i18n::songs_count(list.len())).color(TEXT_DIM));
    });
    ui.add_space(10.0);
}

// ---------------------------------------------------------------------------
// Explore
// ---------------------------------------------------------------------------

pub fn explore(ui: &mut Ui, cx: &mut Ctx, st: &BrowseState) {
    widgets::page_title(ui, tr("Explore"));

    // Shortcut tiles (from the page when loaded, else the usual three).
    let mut shortcuts: Vec<(String, Icon, Route)> = Vec::new();
    if let Some(Slot::Ready(p)) = &st.explore {
        for m in &p.shortcuts {
            shortcuts.push((m.title.clone(), shortcut_icon(&m.endpoint.browse_id), crate::browse::route_for_endpoint(&m.endpoint)));
        }
    }
    if shortcuts.is_empty() {
        shortcuts = vec![
            (tr("New releases").into(), Icon::Note, Route::NewReleases),
            (tr("Charts").into(), Icon::Chart, Route::Charts),
            (tr("Moods & genres").into(), Icon::Compass, Route::MoodsGenres),
        ];
    }
    let w = ((ui.available_width() - 24.0) / 3.0).clamp(160.0, 320.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        for (title, icon, route) in shortcuts {
            if shelves::shortcut_tile(ui, cx, icon, &title, w).clicked() {
                cx.act(Action::Navigate(route));
            }
        }
    });
    ui.add_space(14.0);

    if !guard(ui, cx, st.explore.as_ref(), "explore") {
        return;
    }
    let Some(Slot::Ready(page)) = &st.explore else { return };

    if !page.moods.is_empty() {
        if shelves::section_header(ui, cx, tr("Moods & genres"), None, None, true) {
            cx.act(Action::Navigate(Route::MoodsGenres));
        }
        shelves::mood_carousel(ui, cx, &page.moods, 3, "explore_moods");
        ui.add_space(12.0);
    }
    if !page.new_release_albums.is_empty() {
        if shelves::section_header(ui, cx, tr("New release albums"), None, None, true) {
            cx.act(Action::Navigate(Route::NewReleases));
        }
        let items: Vec<ShelfItem> = page.new_release_albums.iter().cloned().map(ShelfItem::Album).collect();
        shelves::cards_row(ui, cx, &items, &[], "explore_new_albums");
        ui.add_space(12.0);
    }
    for (i, sec) in page.sections.iter().enumerate() {
        shelves::yt_section(ui, cx, sec, ("explore", i), see_all_action(sec));
    }
    // Charts (Android shows the chart shelves on Explore).
    match st.pages.get("charts").map(|p| &p.status) {
        Some(Slot::Ready(())) => {
            let charts = &st.pages["charts"].page;
            for (i, sec) in charts.sections.iter().enumerate() {
                let see_all = see_all_action(sec).or(Some(Action::Navigate(Route::Charts)));
                shelves::yt_section(ui, cx, sec, ("explore_charts", i), see_all);
            }
        }
        Some(Slot::Loading) | None => widgets::loading(ui, tr("Loading charts…")),
        Some(Slot::Failed(_)) => {}
    }
    ui.add_space(24.0);
}

// ---------------------------------------------------------------------------
// Moods & genres
// ---------------------------------------------------------------------------

pub fn moods(ui: &mut Ui, cx: &mut Ctx, st: &BrowseState) {
    widgets::page_title(ui, tr("Moods & genres"));
    if !guard(ui, cx, st.moods.as_ref(), "moods") {
        return;
    }
    let Some(Slot::Ready(groups)) = &st.moods else { return };
    let avail = ui.available_width();
    let cols = ((avail + 12.0) / 212.0).floor().max(2.0);
    let tile_w = (avail - 12.0 * (cols - 1.0)) / cols;
    for g in groups {
        widgets::section_title(ui, &g.title);
        shelves::mood_tiles(ui, cx, &g.items, tile_w.floor());
        ui.add_space(16.0);
    }
}

// ---------------------------------------------------------------------------
// Generic browse page
// ---------------------------------------------------------------------------

fn route_title(route: &Route) -> String {
    match route {
        Route::Charts => tr("Charts").into(),
        Route::NewReleases => tr("New releases").into(),
        Route::ArtistMore { title, .. } => title.clone(),
        _ => String::new(),
    }
}

/// Charts / new releases / mood & genre pages / "see all" pages.
pub fn browse_page(ui: &mut Ui, cx: &mut Ctx, st: &BrowseState, route: &Route) {
    let Some(key) = page_key(route) else { return };
    let Some(slot) = st.pages.get(&key) else {
        widgets::loading(ui, tr("Loading…"));
        return;
    };
    let page = &slot.page;
    let title = if page.title.is_empty() { route_title(route) } else { page.title.clone() };
    if !title.is_empty() {
        widgets::page_title(ui, &title);
    }
    if !guard(ui, cx, Some(&slot.status), &key) {
        return;
    }
    // Artist "see all" that is a playlist of songs.
    if let Some(songs) = &slot.songs {
        play_buttons(ui, cx, songs);
        widgets::song_list(ui, cx, songs, RowOpts { show_album: true, ..Default::default() });
        return;
    }
    if page.sections.is_empty() {
        ui.label(RichText::new(tr("Nothing here yet.")).color(TEXT_DIM));
        return;
    }
    let single = page.sections.len() == 1;
    for (i, sec) in page.sections.iter().enumerate() {
        let all_songs = !sec.items.is_empty() && sec.items.iter().all(|i| matches!(i, ShelfItem::Song(_)));
        if single || sec.grid {
            if !sec.title.is_empty() && !(single && sec.title == title) {
                if shelves::section_header(ui, cx, &sec.title, sec.strapline.as_deref(), None, sec.more.is_some())
                    && let Some(a) = see_all_action(sec)
                {
                    cx.act(a);
                }
            }
            if !sec.moods.is_empty() {
                let w = ((ui.available_width() + 12.0) / 212.0).floor().max(2.0);
                shelves::mood_tiles(ui, cx, &sec.moods, ((ui.available_width() - 12.0 * (w - 1.0)) / w).floor());
            }
            if all_songs && sec.rows {
                let songs = songs_of(&sec.items);
                play_buttons(ui, cx, &songs);
                widgets::song_list(ui, cx, &songs, RowOpts { show_album: true, ..Default::default() });
            } else {
                shelves::cards_grid(ui, cx, &sec.items, &sec.subtitles);
            }
            ui.add_space(16.0);
        } else {
            shelves::yt_section(ui, cx, sec, (key.as_str(), i), see_all_action(sec));
        }
    }
    // Infinite scroll: page continuation, else the last shelf's.
    let last = page.sections.len().saturating_sub(1);
    let section = if page.continuation.is_some() {
        Some(None)
    } else if page.sections.get(last).is_some_and(|s| s.continuation.is_some()) {
        Some(Some(last))
    } else {
        None
    };
    if let Some(section) = section
        && shelves::load_more_sentinel(ui, slot.loading_more)
    {
        cx.act(Action::Browse(BrowseAction::PageMore { key: key.clone(), section }));
    } else if slot.loading_more {
        shelves::load_more_sentinel(ui, true);
    }
    ui.add_space(24.0);
}

// ---------------------------------------------------------------------------
// Home "see all"
// ---------------------------------------------------------------------------

/// Full list of a home shelf.
pub fn home_section(ui: &mut Ui, cx: &mut Ctx, st: &BrowseState, data: &Data, key: &str) {
    let (title, list): (String, Option<Arc<Vec<Song>>>) = match key {
        "quick_picks" => (tr("Quick picks").into(), Some(st.lib.quick_picks.clone())),
        "recent" => (tr("Recently played").into(), Some(if st.lib.recent_songs.is_empty() { data.history.clone() } else { st.lib.recent_songs.clone() })),
        "most" => (tr("Most played").into(), Some(data.most_played.clone())),
        "forgotten" => (tr("Forgotten favorites").into(), Some(st.lib.forgotten.clone())),
        k => {
            let idx: Option<usize> = k.strip_prefix("yt:").and_then(|i| i.parse().ok());
            let Some(sec) = idx.and_then(|i| st.home.sections.get(i)) else {
                ui.label(RichText::new(tr("Nothing here yet.")).color(TEXT_DIM));
                return;
            };
            widgets::page_title(ui, &sec.title);
            if sec.items.iter().all(|i| matches!(i, ShelfItem::Song(_))) && sec.rows {
                let songs = songs_of(&sec.items);
                play_buttons(ui, cx, &songs);
                widgets::song_list(ui, cx, &songs, RowOpts { show_album: true, ..Default::default() });
            } else {
                shelves::cards_grid(ui, cx, &sec.items, &sec.subtitles);
            }
            return;
        }
    };
    widgets::page_title(ui, &title);
    let list = list.unwrap_or_default();
    if list.is_empty() {
        ui.label(RichText::new(tr("Nothing played yet.")).color(TEXT_DIM));
        return;
    }
    play_buttons(ui, cx, &list);
    widgets::song_list(ui, cx, &list, RowOpts { show_album: true, ..Default::default() });
}
