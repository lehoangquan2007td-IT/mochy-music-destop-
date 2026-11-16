//! Import data model (Android `ImportModels.kt`).

use std::collections::HashMap;

use mochy_core::models::Song;

/// One track from another service, before it is matched on YouTube Music.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImportedTrack {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    /// Exact YouTube id when the source already knows it (Mochy CSV, YouTube links).
    pub video_id: Option<String>,
    pub thumbnail_url: Option<String>,
    /// Full YouTube item when the source is YouTube itself.
    pub yt_item: Option<Song>,
    /// When it was added to the source's list (epoch ms), so imports keep that order.
    pub added_at_ms: Option<i64>,
    /// The track's own id on its service ("spotify:track:…").
    pub source_id: Option<String>,
}

impl ImportedTrack {
    pub fn new(title: impl Into<String>, artist: impl Into<String>, album: Option<String>) -> Self {
        Self { title: title.into(), artist: artist.into(), album, ..Default::default() }
    }

    /// A YouTube song needs no matching: the id is exact and the item is kept.
    pub fn from_youtube(song: &Song) -> Self {
        Self {
            title: song.title.clone(),
            artist: song.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", "),
            album: song.album_name.clone(),
            duration_ms: song.duration_secs.map(|d| (d * 1000.0) as i64),
            video_id: song.video_id.clone(),
            thumbnail_url: song.thumbnail_url.clone(),
            yt_item: Some(song.clone()),
            ..Default::default()
        }
    }
}

/// Where imported music came from; kept on every imported song.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImportSource {
    Spotify,
    AppleMusic,
    Tidal,
    YoutubeMusic,
    Deezer,
    /// A file or a service with no name of its own (TuneMyMusic CSV, Mochy backup).
    Other,
}

impl ImportSource {
    pub const ALL: [ImportSource; 6] = [
        ImportSource::Spotify,
        ImportSource::AppleMusic,
        ImportSource::Tidal,
        ImportSource::YoutubeMusic,
        ImportSource::Deezer,
        ImportSource::Other,
    ];

    pub fn key(self) -> &'static str {
        match self {
            ImportSource::Spotify => "spotify",
            ImportSource::AppleMusic => "apple_music",
            ImportSource::Tidal => "tidal",
            ImportSource::YoutubeMusic => "youtube_music",
            ImportSource::Deezer => "deezer",
            ImportSource::Other => "other",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ImportSource::Spotify => "Spotify",
            ImportSource::AppleMusic => "Apple Music",
            ImportSource::Tidal => "Tidal",
            ImportSource::YoutubeMusic => "YouTube Music",
            ImportSource::Deezer => "Deezer",
            ImportSource::Other => "other sources",
        }
    }

    /// The sources the library gives a category of their own.
    pub fn is_listed(self) -> bool {
        self != ImportSource::Other
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.key() == key)
    }
}

/// Where a playlist lives on its service; either is enough for re-sync.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlaylistOrigin {
    pub remote_id: Option<String>,
    pub remote_url: Option<String>,
}

/// What a source handed over, before matching.
#[derive(Debug, Clone, Default)]
pub struct ParsedImport {
    pub liked_songs: Vec<ImportedTrack>,
    /// Playlists in source order (name, tracks).
    pub playlists: Vec<(String, Vec<ImportedTrack>)>,
    /// Songs that go straight into the library, in no playlist.
    pub library_songs: Vec<ImportedTrack>,
    pub warnings: Vec<String>,
    pub provider: Option<ImportSource>,
    pub playlist_covers: HashMap<String, String>,
    pub playlist_origins: HashMap<String, PlaylistOrigin>,
}

impl ParsedImport {
    pub fn total_songs(&self) -> usize {
        self.liked_songs.len() + self.library_songs.len() + self.playlists.iter().map(|p| p.1.len()).sum::<usize>()
    }
    pub fn total_playlists(&self) -> usize {
        self.playlists.len()
    }
    pub fn playlist(&self, name: &str) -> Option<&Vec<ImportedTrack>> {
        self.playlists.iter().find(|p| p.0 == name).map(|p| &p.1)
    }
    /// Get-or-insert keeping first-seen order (Kotlin's linkedMapOf).
    pub fn playlist_mut(&mut self, name: &str) -> &mut Vec<ImportedTrack> {
        if let Some(i) = self.playlists.iter().position(|p| p.0 == name) {
            &mut self.playlists[i].1
        } else {
            self.playlists.push((name.to_string(), Vec::new()));
            &mut self.playlists.last_mut().unwrap().1
        }
    }
    /// A link reads one playlist, so origin is where that one lives.
    pub fn with_origin(mut self, origin: PlaylistOrigin) -> Self {
        self.playlist_origins = self.playlists.iter().map(|(n, _)| (n.clone(), origin.clone())).collect();
        self
    }
}

/// A track that couldn't be matched, with where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct UnmatchedTrack {
    pub track: ImportedTrack,
    pub playlist_name: Option<String>,
}

/// One re-sync run's summary.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResyncSummary {
    pub playlist_name: String,
    pub playlists: usize,
    pub done: usize,
    pub added: usize,
    pub playlists_changed: usize,
    pub failed: Vec<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportProgress {
    pub is_running: bool,
    pub total_tracks: usize,
    pub processed_tracks: usize,
    pub matched_tracks: usize,
    pub current_label: String,
    pub is_done: bool,
    pub cancelled: bool,
    pub source: Option<ImportSource>,
    pub playlists_created: usize,
    pub liked_added: usize,
    pub unmatched: Vec<UnmatchedTrack>,
    pub error: Option<String>,
    pub resync: Option<ResyncSummary>,
    /// Local playlists created or topped up by this run: (name, id).
    pub playlist_ids: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportError(pub String);

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ImportError {}

impl From<&str> for ImportError {
    fn from(s: &str) -> Self {
        ImportError(s.to_string())
    }
}

impl From<String> for ImportError {
    fn from(s: String) -> Self {
        ImportError(s)
    }
}

pub type ImportResult<T> = Result<T, ImportError>;
