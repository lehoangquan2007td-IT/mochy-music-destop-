//! Library: liked songs, local playlists, history, most played, local files.

use std::sync::Arc;

use egui::{RichText, Ui, vec2};
use mochy_core::models::Song;

use super::Data;
use crate::actions::{Action, Ctx};
use crate::images::ImageSrc;
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

pub fn show(ui: &mut Ui, cx: &mut Ctx, tab: LibTab, data: &Data, lib: &LibUi) -> Option<LibEvent> {
    let mut ev = None;
    ui.horizontal(|ui| {
        widgets::page_title(ui, tr("Library"));
        ui.add_space(12.0);
        if widgets::icons::icon_button(ui, Icon::Repeat, 28.0, false, cx.accent).on_hover_text(tr("Refresh")).clicked() {
            ev = Some(LibEvent::Refresh);
        }
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
        LibTab::Liked => songs_tab(ui, cx, &data.liked, tr("No liked songs yet — tap ♥ on any song."), true),
        LibTab::History => songs_tab(ui, cx, &data.history, tr("Nothing played yet."), false),
        LibTab::MostPlayed => songs_tab(ui, cx, &data.most_played, tr("Nothing played yet."), false),
        LibTab::Playlists => {
            if widgets::accent_button(ui, Icon::Plus, tr("New playlist"), cx.accent, true).clicked() {
                cx.act(Action::NewPlaylist(None));
            }
            ui.add_space(14.0);
            if data.playlists.is_empty() {
                ui.label(RichText::new(tr("No playlists yet.")).color(TEXT_DIM));
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(16.0, 16.0);
                for p in &data.playlists {
                    let songs = data.playlist_songs.get(&p.id);
                    let src = p
                        .thumbnail_url
                        .clone()
                        .map(ImageSrc::Url)
                        .or_else(|| songs.and_then(|s| s.first()).and_then(crate::images::song_src));
                    let count = songs.map(|s| s.len() as u32).unwrap_or(p.song_count);
                    let resp = widgets::card(ui, cx, src.as_ref(), &p.name, &i18n::songs_count(count as usize), false);
                    if resp.clicked() {
                        cx.act(Action::Navigate(Route::LocalPlaylist(p.id.clone())));
                    }
                    resp.context_menu(|ui| {
                        if ui.button(tr("Open")).clicked() {
                            cx.act(Action::Navigate(Route::LocalPlaylist(p.id.clone())));
                            ui.close();
                        }
                        if ui.button(tr("Rename…")).clicked() {
                            cx.act(Action::RenamePlaylist(p.id.clone()));
                            ui.close();
                        }
                        if ui.button(tr("Delete…")).clicked() {
                            cx.act(Action::DeletePlaylist(p.id.clone()));
                            ui.close();
                        }
                    });
                }
            });
        }
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
    }
    ev
}

fn songs_tab(ui: &mut Ui, cx: &mut Ctx, list: &Arc<Vec<Song>>, empty: &str, buttons: bool) {
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
