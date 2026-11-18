//! UI → app intents, and the per-frame widget context.

use std::collections::HashSet;
use std::sync::Arc;

use egui::Color32;
use mochy_core::models::{Playlist, Song};

use crate::images::ImageCache;
use crate::nav::Route;

/// Something the user asked for; applied by the app after drawing.
#[derive(Debug, Clone)]
pub enum Action {
    Navigate(Route),
    Back,
    Forward,
    /// Replace the queue with `songs` and play `start`.
    PlayList { songs: Arc<Vec<Song>>, start: usize, shuffle: bool },
    /// Play one song followed by its radio.
    PlaySong(Song),
    PlayNext(Song),
    AddToQueue(Vec<Song>),
    StartRadio(Song),
    ToggleLike(Song),
    AddToPlaylist { playlist_id: String, song: Song },
    /// Open the "new playlist" dialog (optionally adding a song afterwards).
    NewPlaylist(Option<Song>),
    RemoveFromPlaylist { playlist_id: String, song_id: String },
    RenamePlaylist(String),
    DeletePlaylist(String),
    /// Pick a picture as a local playlist's cover.
    ChangePlaylistCover(String),
    /// Drop a local playlist's picked cover.
    RemovePlaylistCover(String),
    SetNowPlaying(bool),
    QueuePlay(usize),
    QueueRemove(usize),
    QueueMove { from: usize, to: usize },
    QueueClear,
    Seek(f64),
    CopyText(String),
    // Transport
    Toggle,
    Next,
    Prev,
    SetVolume(f32),
    ToggleMute,
    ToggleShuffle,
    CycleRepeat,
    /// Show/hide the right side panel on a tab.
    ToggleSidePanel(crate::pages::SideTab),
    /// Adjust the current song's lyrics offset (ms delta).
    LyricsOffset(i32),
    /// Sleep timer / per-song speed & pitch (`ui/transitions_ui.rs`).
    Transitions(crate::ui::transitions_ui::TransitionsAction),
    // ---- library/explore/stats ----
    /// Explore / library / stats intents, handled by `crate::browse::BrowseState`.
    Browse(crate::browse::BrowseAction),
}

/// Remote/async data slot.
#[derive(Debug, Clone)]
pub enum LoadState<T> {
    Loading,
    Ready(Arc<T>),
    Failed(String),
}

impl<T> LoadState<T> {
    pub fn from_result(r: Result<T, String>) -> Self {
        match r {
            Ok(v) => LoadState::Ready(Arc::new(v)),
            Err(e) => LoadState::Failed(e),
        }
    }
}

/// Everything widgets need, borrowed for one frame.
pub struct Ctx<'a> {
    pub images: &'a mut ImageCache,
    pub actions: &'a mut Vec<Action>,
    pub liked: &'a HashSet<String>,
    pub playlists: &'a [Playlist],
    pub accent: Color32,
    /// Artwork corner radius (Appearance → cover rounding).
    pub radius: f32,
    pub current_id: Option<&'a str>,
    pub playing: bool,
}

impl Ctx<'_> {
    pub fn act(&mut self, a: Action) {
        self.actions.push(a);
    }
    pub fn is_liked(&self, song: &Song) -> bool {
        self.liked.contains(&song.id)
    }
}
