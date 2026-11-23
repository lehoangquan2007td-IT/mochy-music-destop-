//! Home (port of `HomeScreen.kt` / `HomeViewModel.kt`): greeting, YTM mood
//! chips, local shelves (Quick picks = most played of the last 30 days,
//! Recently played, Forgotten favorites, Most played) with "see all", then
//! the YTM feed (cards / 4-row song grids, "see all" endpoints) with
//! continuation loading when scrolled to the bottom. Signed in to YouTube,
//! the youtube.com mixes / music videos and the YouTube history are added
//! (`crate::account::home_extras`).

use egui::{RichText, Ui};

use super::Data;
use crate::actions::{Action, Ctx};
use crate::browse::{BrowseAction, BrowseState, Slot};
use crate::nav::Route;
use crate::ui::i18n::tr;
use crate::widgets::shelves::{self, see_all_action};
use crate::widgets::{self, TEXT_DIM};

/// Returns `true` when the user asked to retry loading the home feed.
pub fn show(ui: &mut Ui, cx: &mut Ctx, data: &Data, st: &BrowseState) -> bool {
    let mut retry = false;
    widgets::page_title(ui, greeting(st.lib.utc_offset_secs));

    // Mood chips.
    if !st.home.chips.is_empty() {
        egui::ScrollArea::horizontal().id_salt("home_chips").show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, c) in st.home.chips.iter().enumerate() {
                    let sel = st.home.selected == Some(i);
                    if widgets::pill(ui, &c.title, sel).clicked() {
                        cx.act(Action::Browse(BrowseAction::HomeChip(if sel { None } else { Some(i) })));
                    }
                }
            });
        });
        ui.add_space(10.0);
    }

    // Taste mixes ("Made for you"), above the YouTube shelves.
    if st.home.selected.is_none() {
        super::mixes::shelf(ui, cx);
    }

    // Signed-in YouTube account: mixes / music videos from youtube.com, YouTube history.
    let extras = crate::account::home_extras();
    // Signed in, YouTube Music's own "Quick picks" (the first song-row shelf of
    // the feed) is personal: show it on top instead of the local one.
    let yt_quick = (extras.is_some() && st.home.selected.is_none() && matches!(st.home.status, Some(Slot::Ready(()))))
        .then(|| {
            st.home.sections.iter().take(3).position(|s| {
                s.rows && !s.items.is_empty() && s.items.iter().all(|i| matches!(i, mochy_core::models::ShelfItem::Song(_)))
            })
        })
        .flatten();
    if let Some(i) = yt_quick {
        let sec = &st.home.sections[i];
        let see_all = see_all_action(sec).or_else(|| Some(Action::Navigate(Route::HomeSection(format!("yt:{i}")))));
        shelves::yt_section(ui, cx, sec, ("home", i), see_all);
    }
    if st.home.selected.is_none()
        && let Some(x) = &extras
    {
        if x.flags.home_mixes && !x.mixes.items.is_empty() {
            shelves::yt_section(ui, cx, &x.mixes, ("home_ytmix", 0usize), None);
        }
        if x.flags.home_music_videos && !x.videos.items.is_empty() {
            shelves::yt_section(ui, cx, &x.videos, ("home_ytvid", 0usize), None);
        }
    }

    if st.home.selected.is_none() {
        let lib = &st.lib;
        if !lib.quick_picks.is_empty() && yt_quick.is_none() {
            if shelves::section_header(ui, cx, tr("Quick picks"), None, None, true) {
                cx.act(Action::Navigate(Route::HomeSection("quick_picks".into())));
            }
            shelves::song_grid(ui, cx, &lib.quick_picks, None, 4, "home_quick", true);
            ui.add_space(12.0);
        }
        let recent = if lib.recent_songs.is_empty() { &data.history } else { &lib.recent_songs };
        if !recent.is_empty() {
            if shelves::section_header(ui, cx, tr("Recently played"), None, None, true) {
                cx.act(Action::Navigate(Route::HomeSection("recent".into())));
            }
            card_row(ui, cx, recent, "recent");
        }
        if let Some(x) = &extras
            && x.flags.home_history
            && !x.history.is_empty()
        {
            shelves::section_header(ui, cx, tr("Recently on YouTube"), None, None, false);
            card_row(ui, cx, &x.history, "yt_history");
        }
        if !lib.forgotten.is_empty() {
            if shelves::section_header(ui, cx, tr("Forgotten favorites"), None, None, true) {
                cx.act(Action::Navigate(Route::HomeSection("forgotten".into())));
            }
            shelves::song_grid(ui, cx, &lib.forgotten, None, 4, "home_forgotten", true);
            ui.add_space(12.0);
        }
        if !data.most_played.is_empty() {
            if shelves::section_header(ui, cx, tr("Most played"), None, None, true) {
                cx.act(Action::Navigate(Route::HomeSection("most".into())));
            }
            card_row(ui, cx, &data.most_played, "most");
        }
    }

    match &st.home.status {
        None | Some(Slot::Loading) => {
            ui.add_space(12.0);
            widgets::loading(ui, tr("Loading YouTube Music…"));
        }
        Some(Slot::Failed(e)) => {
            ui.add_space(12.0);
            retry = widgets::error_box(ui, e);
        }
        Some(Slot::Ready(())) => {
            if st.home.sections.is_empty() {
                ui.label(RichText::new(tr("YouTube Music returned no shelves.")).color(TEXT_DIM));
                retry = ui.button(tr("Reload")).clicked();
            }
            for (i, sec) in st.home.sections.iter().enumerate() {
                if Some(i) == yt_quick {
                    continue;
                }
                let see_all = see_all_action(sec).or_else(|| Some(Action::Navigate(Route::HomeSection(format!("yt:{i}")))));
                shelves::yt_section(ui, cx, sec, ("home", i), see_all);
            }
            if st.home.continuation.is_some() && shelves::load_more_sentinel(ui, st.home.loading_more) {
                cx.act(Action::Browse(BrowseAction::HomeMore));
            } else if st.home.loading_more {
                shelves::load_more_sentinel(ui, true);
            }
        }
    }
    ui.add_space(24.0);
    retry
}

fn card_row(ui: &mut Ui, cx: &mut Ctx, songs: &std::sync::Arc<Vec<mochy_core::models::Song>>, salt: &str) {
    egui::ScrollArea::horizontal().id_salt(("home_row", salt)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            for i in 0..songs.len().min(30) {
                widgets::song_card(ui, cx, songs, i, true);
            }
        });
    });
    ui.add_space(12.0);
}

/// Time-of-day greeting from the local clock.
fn greeting(utc_offset_secs: i64) -> &'static str {
    let now = crate::browse::now_ms() / 1000 + utc_offset_secs;
    let hour = now.rem_euclid(86_400) / 3600;
    match hour {
        5..=11 => tr("Good morning"),
        12..=16 => tr("Good afternoon"),
        17..=21 => tr("Good evening"),
        _ => tr("Good night"),
    }
}
