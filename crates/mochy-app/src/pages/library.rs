//! Library (port of `ui/screens/library/*`): songs, liked songs, albums,
//! artists, playlists, recently added, history, most played, local files.
//! Each tab is a self-contained view dispatched by `match tab`; the list
//! views live in `library_views.rs`.

use std::sync::Arc;

use egui::{RichText, Ui};
use mochy_core::models::Song;

use super::Data;
use super::library_views::{self, ListKind};
use crate::actions::{Action, Ctx};
use crate::browse::BrowseState;
use crate::nav::{LibTab, Route};
use crate::ui::i18n::{self, tr};
use crate::widgets::{self, RowOpts, TEXT_DIM, icons::Icon};

#[derive(Default)]
pub struct LibUi {
    /// Status line of the running / last folder scan.
    pub scan_status: Option<String>,
    pub scanning: bool,
}

pub enum LibEvent {
    AddFolder,
    CancelScan,
    Refresh,
}

pub fn show(ui: &mut Ui, cx: &mut Ctx, tab: LibTab, data: &Data, lib: &LibUi, st: &mut BrowseState) -> Option<LibEvent> {
    let mut ev = None;
    ui.horizontal(|ui| {
        widgets::page_title(ui, tr("Library"));
        ui.add_space(12.0);
        if widgets::icons::icon_button(ui, Icon::Repeat, 28.0, false, cx.accent).on_hover_text(tr("Refresh")).clicked() {
            ev = Some(LibEvent::Refresh);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::accent_button(ui, Icon::Chart, tr("Stats"), cx.accent, false).clicked() {
                cx.act(Action::Navigate(Route::Stats));
            }
        });
    });
    ui.horizontal_wrapped(|ui| {
        for t in LibTab::ALL {
            if widgets::pill(ui, t.label(), t == tab).clicked() && t != tab {
                cx.act(Action::Navigate(Route::Library(t)));
            }
        }
    });
    ui.add_space(14.0);

    match tab {
        LibTab::Songs => library_views::songs(ui, cx, data, st),
        LibTab::Liked => {
            library_views::toolbar(ui, cx, st, ListKind::Liked);
            let list = library_views::filtered_songs(st, "liked", &data.liked, true);
            songs_tab(ui, cx, &list, tr("No liked songs yet — tap ♥ on any song."), true);
        }
        LibTab::Albums => library_views::albums(ui, cx, st),
        LibTab::Artists => library_views::artists(ui, cx, st),
        LibTab::Playlists => library_views::playlists(ui, cx, data, st),
        LibTab::RecentlyAdded => library_views::recently_added(ui, cx, st),
        LibTab::History => library_views::history(ui, cx, st),
        LibTab::MostPlayed => songs_tab(ui, cx, &data.most_played, tr("Nothing played yet."), false),
        LibTab::Local => {
            ui.horizontal(|ui| {
                if lib.scanning {
                    ui.spinner();
                    if ui.button(tr("Cancel scan")).clicked() {
                        ev = Some(LibEvent::CancelScan);
                    }
                } else if widgets::accent_button(ui, Icon::Folder, tr("Add folder"), cx.accent, true).clicked() {
                    ev = Some(LibEvent::AddFolder);
                }
                if let Some(s) = &lib.scan_status {
                    ui.label(RichText::new(s).color(TEXT_DIM));
                }
            });
            ui.add_space(12.0);
            songs_tab(ui, cx, &data.local, tr("No local files yet — add a music folder."), true);
        }
        LibTab::Downloaded => crate::downloads::ui::downloaded_tab(ui, cx),
    }
    ev
}

pub fn songs_tab(ui: &mut Ui, cx: &mut Ctx, list: &Arc<Vec<Song>>, empty: &str, buttons: bool) {
    if list.is_empty() {
        ui.label(RichText::new(empty).color(TEXT_DIM));
        return;
    }
    if buttons {
        ui.horizontal(|ui| {
            if widgets::accent_button(ui, Icon::Play, tr("Play"), cx.accent, true).clicked() {
                cx.act(Action::PlayList { songs: list.clone(), start: 0, shuffle: false });
            }
            if widgets::accent_button(ui, Icon::Shuffle, tr("Shuffle"), cx.accent, false).clicked() {
                cx.act(Action::PlayList { songs: list.clone(), start: 0, shuffle: true });
            }
            ui.label(RichText::new(i18n::songs_count(list.len())).color(TEXT_DIM));
        });
        ui.add_space(10.0);
    }
    widgets::song_list(ui, cx, list, RowOpts { show_album: true, ..Default::default() });
}
