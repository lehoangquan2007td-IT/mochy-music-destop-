//! Pages and the shared data they render.

pub mod detail;
pub mod home;
pub mod library;
pub mod now_playing;
pub mod player_bar;
pub mod player_styles;
pub mod search;
pub mod settings;
pub mod sidebar;
// ---- library/explore/stats ----
pub mod explore;
pub mod library_views;
pub mod stats;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use mochy_core::models::{Album, Artist, Playlist, Shelf, Song};

use crate::actions::LoadState;

/// Data fetched from YTM / the DB, keyed by id.
#[derive(Default)]
pub struct Data {
    pub home: Option<LoadState<Vec<Shelf>>>,
    pub albums: HashMap<String, LoadState<(Album, Vec<Song>)>>,
    pub artists: HashMap<String, LoadState<(Artist, Vec<Shelf>)>>,
    pub playlists_remote: HashMap<String, LoadState<(Playlist, Vec<Song>)>>,
    pub related: HashMap<String, LoadState<Vec<Song>>>,
    pub liked: Arc<Vec<Song>>,
    pub liked_ids: HashSet<String>,
    pub history: Arc<Vec<Song>>,
    pub most_played: Arc<Vec<Song>>,
    pub local: Arc<Vec<Song>>,
    pub playlists: Vec<Playlist>,
    pub playlist_songs: HashMap<String, Arc<Vec<Song>>>,
    pub search_history: Vec<String>,
}

/// Side panel / now-playing tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideTab {
    Lyrics,
    Queue,
    Related,
}

impl SideTab {
    pub const ALL: [SideTab; 3] = [SideTab::Lyrics, SideTab::Queue, SideTab::Related];
    pub fn label(self) -> &'static str {
        crate::ui::i18n::tr(match self {
            SideTab::Lyrics => "Lyrics",
            SideTab::Queue => "Queue",
            SideTab::Related => "Related",
        })
    }
}

/// Snapshot of the player used by the bar / now-playing view.
#[derive(Clone)]
pub struct NowInfo {
    pub song: Option<Song>,
    pub state: crate::controller::PlayState,
    pub playing: bool,
    pub pos: f64,
    pub duration: f64,
    pub volume: f32,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: mochy_core::settings::RepeatMode,
    pub format: Option<String>,
}
