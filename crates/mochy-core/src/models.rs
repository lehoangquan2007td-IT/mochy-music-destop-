//! Shared data model, mirroring the Android Room entities (spec §5.1, §5.2).
//! Field names intentionally match the DB column names.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// An artist reference attached to a song/album. `id` is a YTM browse id
/// (`LA…`) when known, `None` for local files.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ArtistRef {
    pub id: Option<String>,
    pub name: String,
}

impl ArtistRef {
    pub fn new(name: impl Into<String>) -> Self {
        Self { id: None, name: name.into() }
    }
    pub fn with_id(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self { id: Some(id.into()), name: name.into() }
    }
}

/// A playable track. Mirrors Room table `song`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Song {
    /// Stable local id. For remote songs this is the YouTube video id.
    /// For local files: `ids::song_id_for_path`.
    pub id: String,
    /// YouTube video id; `None` for local files.
    pub video_id: Option<String>,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub album_id: Option<String>,
    pub album_name: Option<String>,
    /// Seconds; `None` = unknown (DB stores -1).
    pub duration_secs: Option<f64>,
    pub thumbnail_url: Option<String>,
    pub explicit: bool,
    pub is_video: bool,
    /// `Some` for local files (DB `isLocal`/`localUri`).
    pub local_path: Option<PathBuf>,
    pub liked: bool,
    /// Epoch millis.
    pub liked_date: Option<i64>,
    pub in_library: bool,
    /// Accumulated seconds (DB `totalPlayTime`).
    pub total_play_time: f64,
    /// Per-song lyrics offset, positive shows lines earlier (ms).
    pub lyrics_offset_ms: i32,
    pub romanize_lyrics: bool,
    pub year: Option<i32>,
}

impl Song {
    /// Remote song from a video id + title.
    pub fn remote(video_id: impl Into<String>, title: impl Into<String>) -> Self {
        let video_id: String = video_id.into();
        Self { id: video_id.clone(), video_id: Some(video_id), title: title.into(), ..Default::default() }
    }

    /// True when this song can be played without a network lookup.
    pub fn is_local(&self) -> bool {
        self.local_path.is_some()
    }

    /// Primary artist name ("" when none).
    pub fn artist_name(&self) -> &str {
        self.artists.first().map(|a| a.name.as_str()).unwrap_or("")
    }
}

/// Mirrors Room table `album`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Album {
    pub id: String,
    /// YTM audio playlist id when known.
    pub playlist_id: Option<String>,
    pub title: String,
    pub year: Option<i32>,
    pub thumbnail_url: Option<String>,
    pub song_count: Option<u32>,
    pub duration_secs: Option<f64>,
    pub description: Option<String>,
}

/// Mirrors Room table `artist`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub thumbnail_url: Option<String>,
    pub channel_id: Option<String>,
}

/// Mirrors Room table `playlist`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Playlist {
    /// `LP` + 8 letters (special: `LP_LIKED`).
    pub id: String,
    pub name: String,
    pub browse_id: Option<String>,
    pub thumbnail_url: Option<String>,
    pub song_count: u32,
    pub description: Option<String>,
    pub is_editable: bool,
    /// Epoch millis.
    pub created_at: i64,
}

/// Result of a search across filters (spec §2.3).
#[derive(Debug, Clone, Default)]
pub struct SearchResults {
    pub songs: Vec<Song>,
    pub videos: Vec<Song>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    pub playlists: Vec<Playlist>,
}

/// A home/explore section row.
#[derive(Debug, Clone)]
pub struct Shelf {
    pub title: String,
    pub items: Vec<ShelfItem>,
}

#[derive(Debug, Clone)]
pub enum ShelfItem {
    Song(Song),
    Album(Album),
    Artist(Artist),
    Playlist(Playlist),
}

/// A track inside a YTM queue (from `next`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QueueItem {
    pub video_id: String,
    pub playlist_id: Option<String>,
    pub set_video_id: Option<String>,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub duration_secs: Option<f64>,
    pub thumbnail_url: Option<String>,
}

/// Generic track description used by the lyrics subsystem (spec §3).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackMeta {
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration_secs: Option<f64>,
    pub video_id: Option<String>,
}

impl From<&Song> for TrackMeta {
    fn from(s: &Song) -> Self {
        TrackMeta {
            title: s.title.clone(),
            artists: s.artists.iter().map(|a| a.name.clone()).collect(),
            album: s.album_name.clone(),
            duration_secs: s.duration_secs,
            video_id: s.video_id.clone(),
        }
    }
}
