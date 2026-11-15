//! mochy-storage — SQLite persistence + settings + paths (spec §5).
//!
//! Schema of record: Room v43 (spec §5.1) — keep table/column names, create
//! ALL 23 tables + 3 views (even ones unused in MVP) so future migrations and
//! the importer fit. PRAGMAs: WAL, busy_timeout=60000, cache_size=-16000,
//! wal_autocheckpoint=1000, synchronous=NORMAL. `user_version` = 43.

use std::path::{Path, PathBuf};

use mochy_core::models::{Album, Artist, Playlist, QueueItem, Song};
use mochy_core::settings::AppSettings;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}

pub type StorageResult<T> = Result<T, StorageError>;

// ---------------------------------------------------------------------------
// Paths (dirs crate): %LOCALAPPDATA%/MochyMusic on Windows, XDG data on Linux.
// ---------------------------------------------------------------------------

/// Root data dir (created on first call if missing).
pub fn data_dir() -> PathBuf {
    todo!("data_dir")
}

/// Cache dir (album art LRU lives here).
pub fn cache_dir() -> PathBuf {
    todo!("cache_dir")
}

/// `<data>/mochy.db`
pub fn db_path() -> PathBuf {
    todo!("db_path")
}

/// `<data>/settings.json`
pub fn settings_path() -> PathBuf {
    todo!("settings_path")
}

/// `<cache>/art`
pub fn art_cache_dir() -> PathBuf {
    todo!("art_cache_dir")
}

/// Create all app dirs.
pub fn ensure_dirs() -> StorageResult<()> {
    todo!("ensure_dirs")
}

/// Local file path for a remote artwork url (stable name by url hash).
pub fn art_cache_file(url: &str) -> PathBuf {
    let _ = url;
    todo!("art_cache_file")
}

// ---------------------------------------------------------------------------
// Database
// ---------------------------------------------------------------------------

/// Handle around a `rusqlite::Connection` with a mutex; cheap to clone? No —
/// wrap in Arc at the call site. All methods take `&self`.
#[derive(Debug)]
pub struct Db {
    conn: std::sync::Mutex<rusqlite::Connection>,
}

impl Db {
    /// Open (or create) the database with the full v43 schema.
    pub fn open(path: &Path) -> StorageResult<Self> {
        let _ = path;
        todo!("Db::open")
    }

    /// Open at the default path (`db_path()`), creating dirs.
    pub fn open_default() -> StorageResult<Self> {
        todo!("Db::open_default")
    }

    // -- songs ------------------------------------------------------------

    /// Insert or update a song (upsert by id; replaces artist/album maps for
    /// the song). Sets `inLibrary = true` when `add_to_library` is used.
    pub fn upsert_song(&self, song: &Song) -> StorageResult<()> {
        let _ = song;
        todo!("Db::upsert_song")
    }

    pub fn get_song(&self, id: &str) -> StorageResult<Option<Song>> {
        let _ = id;
        todo!("Db::get_song")
    }

    /// All songs marked `inLibrary` (library view).
    pub fn all_songs(&self) -> StorageResult<Vec<Song>> {
        todo!("Db::all_songs")
    }

    pub fn set_liked(&self, song_id: &str, liked: bool) -> StorageResult<()> {
        let _ = (song_id, liked);
        todo!("Db::set_liked")
    }

    /// All liked songs, most recent `likedDate` first.
    pub fn liked_songs(&self) -> StorageResult<Vec<Song>> {
        todo!("Db::liked_songs")
    }

    pub fn set_in_library(&self, song_id: &str, in_library: bool) -> StorageResult<()> {
        let _ = (song_id, in_library);
        todo!("Db::set_in_library")
    }

    /// Save a local file as a library song (id from `ids::song_id_for_path`,
    /// tag metadata read by the caller).
    pub fn add_local_song(&self, song: &Song) -> StorageResult<()> {
        let _ = song;
        todo!("Db::add_local_song")
    }

    // -- play events / stats ----------------------------------------------

    /// Record a play: `event` row + `playCount` bump + `totalPlayTime` add.
    pub fn record_play(&self, song_id: &str, played_secs: f64) -> StorageResult<()> {
        let _ = (song_id, played_secs);
        todo!("Db::record_play")
    }

    /// Recently played (distinct songs, latest `event` first).
    pub fn recent_history(&self, limit: usize) -> StorageResult<Vec<Song>> {
        let _ = limit;
        todo!("Db::recent_history")
    }

    /// Most played by `playCount`/`totalPlayTime`.
    pub fn most_played(&self, limit: usize) -> StorageResult<Vec<Song>> {
        let _ = limit;
        todo!("Db::most_played")
    }

    // -- playlists ----------------------------------------------------------

    /// Create a playlist (`LP`+8 letters id), returns its id.
    pub fn create_playlist(&self, name: &str) -> StorageResult<String> {
        let _ = name;
        todo!("Db::create_playlist")
    }

    /// All playlists (user playlists only; skip `LP_LIKED`/`LP_DOWNLOADED`).
    pub fn playlists(&self) -> StorageResult<Vec<Playlist>> {
        todo!("Db::playlists")
    }

    /// Including special playlists.
    pub fn playlist_by_id(&self, id: &str) -> StorageResult<Option<Playlist>> {
        let _ = id;
        todo!("Db::playlist_by_id")
    }

    pub fn rename_playlist(&self, id: &str, name: &str) -> StorageResult<()> {
        let _ = (id, name);
        todo!("Db::rename_playlist")
    }

    pub fn delete_playlist(&self, id: &str) -> StorageResult<()> {
        let _ = id;
        todo!("Db::delete_playlist")
    }

    /// Ordered songs of a playlist (`playlist_song_map` by position).
    pub fn playlist_songs(&self, playlist_id: &str) -> StorageResult<Vec<Song>> {
        let _ = playlist_id;
        todo!("Db::playlist_songs")
    }

    /// Append; assigns `position` = max+1 and stores `setVideoId` when given.
    pub fn add_to_playlist(
        &self,
        playlist_id: &str,
        song: &Song,
        set_video_id: Option<&str>,
    ) -> StorageResult<()> {
        let _ = (playlist_id, song, set_video_id);
        todo!("Db::add_to_playlist")
    }

    pub fn remove_from_playlist(&self, playlist_id: &str, song_id: &str) -> StorageResult<()> {
        let _ = (playlist_id, song_id);
        todo!("Db::remove_from_playlist")
    }

    // -- albums / artists ---------------------------------------------------

    pub fn upsert_album(&self, album: &Album) -> StorageResult<()> {
        let _ = album;
        todo!("Db::upsert_album")
    }

    pub fn upsert_artist(&self, artist: &Artist) -> StorageResult<()> {
        let _ = artist;
        todo!("Db::upsert_artist")
    }

    /// Albums present in the library (from `song_album_map` joins).
    pub fn library_albums(&self) -> StorageResult<Vec<Album>> {
        todo!("Db::library_albums")
    }

    /// Artists present in the library.
    pub fn library_artists(&self) -> StorageResult<Vec<Artist>> {
        todo!("Db::library_artists")
    }

    /// Songs of one album in track order (`song_album_map.index`).
    pub fn album_songs(&self, album_id: &str) -> StorageResult<Vec<Song>> {
        let _ = album_id;
        todo!("Db::album_songs")
    }

    // -- format / lyrics / misc ----------------------------------------------

    /// Cache stream format info (Room table `format`, PK = song id).
    pub fn save_format(
        &self,
        song_id: &str,
        itag: Option<i64>,
        mime_type: &str,
        codecs: &str,
        bitrate: Option<i64>,
        sample_rate: Option<i64>,
        content_length: Option<i64>,
        loudness_db: Option<f64>,
    ) -> StorageResult<()> {
        let _ = (song_id, itag, mime_type, codecs, bitrate, sample_rate, content_length, loudness_db);
        todo!("Db::save_format")
    }

    /// Upsert lyrics for a video id (Room table `lyrics`).
    pub fn save_lyrics(&self, video_id: &str, provider: &str, lyrics: &str) -> StorageResult<()> {
        let _ = (video_id, provider, lyrics);
        todo!("Db::save_lyrics")
    }

    /// (provider, lyrics) or None. Sentinel `LYRICS_NOT_FOUND` is returned as
    /// `Some(("LYRICS_NOT_FOUND", ...))` by the caller contract? No — this
    /// method returns None for both missing and sentinel rows.
    pub fn load_lyrics(&self, video_id: &str) -> StorageResult<Option<(String, String)>> {
        let _ = video_id;
        todo!("Db::load_lyrics")
    }

    /// Search history (unique `query`, newest first, trimmed to 100).
    pub fn add_search_history(&self, query: &str) -> StorageResult<()> {
        let _ = query;
        todo!("Db::add_search_history")
    }

    pub fn search_history(&self, limit: usize) -> StorageResult<Vec<String>> {
        let _ = limit;
        todo!("Db::search_history")
    }

    /// Persist a queue for `persistent_queue` restore.
    pub fn save_queue(&self, items: &[QueueItem], index: usize) -> StorageResult<()> {
        let _ = (items, index);
        todo!("Db::save_queue")
    }

    /// Restored queue + index (None when absent).
    pub fn load_queue(&self) -> StorageResult<Option<(Vec<QueueItem>, usize)>> {
        todo!("Db::load_queue")
    }
}

// ---------------------------------------------------------------------------
// Settings store: AppSettings as pretty JSON at `settings_path()`.
// ---------------------------------------------------------------------------

pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn open(path: PathBuf) -> Self {
        Self { path }
    }

    /// Default path `settings_path()`; loads existing or default settings.
    pub fn open_default() -> Self {
        todo!("SettingsStore::open_default")
    }

    pub fn load(&self) -> StorageResult<AppSettings> {
        let _ = self.path.clone();
        todo!("SettingsStore::load")
    }

    /// Atomic-ish write (temp file + rename), pretty JSON.
    pub fn save(&self, settings: &AppSettings) -> StorageResult<()> {
        let _ = (self, settings);
        todo!("SettingsStore::save")
    }
}
