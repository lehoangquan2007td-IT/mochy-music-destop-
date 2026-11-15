//! mochy-storage — SQLite persistence + settings + paths (spec §5).
//!
//! Schema of record: Room v43 (spec §5.1) — keep table/column names, create
//! ALL 23 tables + 3 views (even ones unused in MVP) so future migrations and
//! the importer fit. PRAGMAs: WAL, busy_timeout=60000, cache_size=-16000,
//! wal_autocheckpoint=1000, synchronous=NORMAL. `user_version` = 43.

use std::path::{Path, PathBuf};

use mochy_core::models::{Album, Artist, ArtistRef, Playlist, QueueItem, Song};
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
    let base = dirs::data_local_dir().expect("no local data dir");
    base.join("MochyMusic")
}

/// Cache dir (album art LRU lives here).
pub fn cache_dir() -> PathBuf {
    let base = dirs::cache_dir().expect("no cache dir");
    base.join("MochyMusic")
}

/// `<data>/mochy.db`
pub fn db_path() -> PathBuf {
    data_dir().join("mochy.db")
}

/// `<data>/settings.json`
pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

/// `<cache>/art`
pub fn art_cache_dir() -> PathBuf {
    cache_dir().join("art")
}

/// Create all app dirs.
pub fn ensure_dirs() -> StorageResult<()> {
    std::fs::create_dir_all(data_dir())?;
    std::fs::create_dir_all(cache_dir())?;
    std::fs::create_dir_all(art_cache_dir())?;
    Ok(())
}

/// FNV-1a hash of a string, returned as a 16-hex-digit string.
fn fnv1a_hex(s: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}

/// Local file path for a remote artwork url (stable name by url hash).
pub fn art_cache_file(url: &str) -> PathBuf {
    art_cache_dir().join(fnv1a_hex(url))
}

// ---------------------------------------------------------------------------
// Schema DDL (v43, 23 tables + 3 views)
// ---------------------------------------------------------------------------

const SCHEMA_V43: &str = r#"
CREATE TABLE IF NOT EXISTS song (
    id TEXT PRIMARY KEY NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    duration INTEGER NOT NULL DEFAULT -1,
    thumbnailUrl TEXT,
    albumId TEXT,
    albumName TEXT,
    explicit INTEGER NOT NULL DEFAULT 0,
    year INTEGER,
    date TEXT,
    dateModified TEXT,
    liked INTEGER NOT NULL DEFAULT 0,
    likedDate INTEGER,
    totalPlayTime REAL NOT NULL DEFAULT 0.0,
    inLibrary INTEGER,
    dateDownload TEXT,
    isLocal INTEGER NOT NULL DEFAULT 0,
    libraryAddToken TEXT,
    libraryRemoveToken TEXT,
    lyricsOffset INTEGER NOT NULL DEFAULT 0,
    romanizeLyrics INTEGER NOT NULL DEFAULT 1,
    isDownloaded INTEGER NOT NULL DEFAULT 0,
    isUploaded INTEGER NOT NULL DEFAULT 0,
    isVideo INTEGER NOT NULL DEFAULT 0,
    localUri TEXT,
    addedByLike INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_song_albumId ON song(albumId);

CREATE TABLE IF NOT EXISTS artist (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    thumbnailUrl TEXT,
    channelId TEXT,
    lastUpdateTime INTEGER,
    bookmarkedAt INTEGER,
    isLocal INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS album (
    id TEXT PRIMARY KEY NOT NULL,
    playlistId TEXT,
    title TEXT NOT NULL DEFAULT '',
    year INTEGER,
    thumbnailUrl TEXT,
    themeColor INTEGER,
    songCount INTEGER,
    duration INTEGER,
    explicit INTEGER,
    lastUpdateTime INTEGER,
    bookmarkedAt INTEGER,
    likedDate INTEGER,
    inLibrary INTEGER,
    description TEXT,
    isLocal INTEGER,
    isUploaded INTEGER
);

CREATE TABLE IF NOT EXISTS playlist (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    browseId TEXT,
    createdAt INTEGER,
    lastUpdateTime INTEGER,
    isEditable INTEGER NOT NULL DEFAULT 1,
    bookmarkedAt INTEGER,
    remoteSongCount INTEGER,
    playEndpointParams TEXT,
    thumbnailUrl TEXT,
    shuffleEndpointParams TEXT,
    radioEndpointParams TEXT,
    isLocal INTEGER NOT NULL DEFAULT 0,
    isAutoSync INTEGER NOT NULL DEFAULT 0,
    description TEXT
);

CREATE TABLE IF NOT EXISTS song_artist_map (
    songId TEXT NOT NULL,
    artistId TEXT NOT NULL,
    position INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (songId, artistId),
    FOREIGN KEY (songId) REFERENCES song(id) ON DELETE CASCADE,
    FOREIGN KEY (artistId) REFERENCES artist(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS song_album_map (
    songId TEXT NOT NULL,
    albumId TEXT NOT NULL,
    "index" INTEGER,
    PRIMARY KEY (songId, albumId),
    FOREIGN KEY (songId) REFERENCES song(id) ON DELETE CASCADE,
    FOREIGN KEY (albumId) REFERENCES album(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS album_artist_map (
    albumId TEXT NOT NULL,
    artistId TEXT NOT NULL,
    "order" INTEGER,
    PRIMARY KEY (albumId, artistId),
    FOREIGN KEY (albumId) REFERENCES album(id) ON DELETE CASCADE,
    FOREIGN KEY (artistId) REFERENCES artist(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS playlist_song_map (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    playlistId TEXT NOT NULL,
    songId TEXT NOT NULL,
    position INTEGER NOT NULL DEFAULT 0,
    setVideoId TEXT,
    FOREIGN KEY (playlistId) REFERENCES playlist(id) ON DELETE CASCADE,
    FOREIGN KEY (songId) REFERENCES song(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_playlist_song_map_playlistId ON playlist_song_map(playlistId);
CREATE INDEX IF NOT EXISTS idx_playlist_song_map_songId ON playlist_song_map(songId);

CREATE TABLE IF NOT EXISTS search_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    query TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS format (
    id TEXT PRIMARY KEY NOT NULL,
    itag INTEGER,
    mimeType TEXT,
    codecs TEXT,
    bitrate INTEGER,
    sampleRate INTEGER,
    contentLength INTEGER,
    loudnessDb REAL,
    perceptualLoudnessDb REAL,
    playbackUrl TEXT
);

CREATE TABLE IF NOT EXISTS lyrics (
    id TEXT PRIMARY KEY NOT NULL,
    lyrics TEXT,
    provider TEXT NOT NULL DEFAULT 'Unknown',
    translatedLyrics TEXT NOT NULL DEFAULT '',
    translationLanguage TEXT NOT NULL DEFAULT '',
    translationMode TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS event (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    songId TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    playTime REAL NOT NULL DEFAULT 0.0,
    FOREIGN KEY (songId) REFERENCES song(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS related_song_map (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    songId TEXT NOT NULL,
    relatedSongId TEXT NOT NULL,
    FOREIGN KEY (songId) REFERENCES song(id) ON DELETE CASCADE,
    FOREIGN KEY (relatedSongId) REFERENCES song(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS set_video_id (
    videoId TEXT PRIMARY KEY NOT NULL,
    setVideoId TEXT
);

CREATE TABLE IF NOT EXISTS playCount (
    song TEXT NOT NULL,
    year INTEGER NOT NULL,
    month INTEGER NOT NULL,
    count INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (song, year, month)
);

CREATE TABLE IF NOT EXISTS recognition_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trackId TEXT,
    title TEXT,
    artist TEXT,
    album TEXT,
    coverArtUrl TEXT,
    coverArtHqUrl TEXT,
    genre TEXT,
    releaseDate TEXT,
    label TEXT,
    shazamUrl TEXT,
    appleMusicUrl TEXT,
    spotifyUrl TEXT,
    isrc TEXT,
    youtubeVideoId TEXT,
    recognizedAt INTEGER,
    liked INTEGER
);
CREATE INDEX IF NOT EXISTS idx_recognition_history_trackId ON recognition_history(trackId);

CREATE TABLE IF NOT EXISTS speed_dial_item (
    id TEXT PRIMARY KEY NOT NULL,
    secondaryId TEXT,
    title TEXT,
    subtitle TEXT,
    thumbnailUrl TEXT,
    type TEXT,
    explicit INTEGER,
    createDate INTEGER
);

CREATE TABLE IF NOT EXISTS playlist_event (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    playlistId TEXT NOT NULL,
    timestamp INTEGER,
    FOREIGN KEY (playlistId) REFERENCES playlist(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS imported_song (
    songId TEXT NOT NULL,
    source TEXT NOT NULL,
    importedAt INTEGER,
    PRIMARY KEY (songId, source)
);

CREATE TABLE IF NOT EXISTS import_session (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source TEXT,
    startedAt INTEGER
);

CREATE TABLE IF NOT EXISTS import_session_song (
    sessionId INTEGER NOT NULL,
    songId TEXT NOT NULL,
    addedToLibrary INTEGER,
    liked INTEGER,
    PRIMARY KEY (sessionId, songId)
);

CREATE TABLE IF NOT EXISTS imported_playlist (
    playlistId TEXT PRIMARY KEY NOT NULL,
    source TEXT,
    sessionId INTEGER,
    importedAt INTEGER,
    remoteId TEXT,
    remoteUrl TEXT,
    lastSyncedAt INTEGER
);
CREATE INDEX IF NOT EXISTS idx_imported_playlist_source ON imported_playlist(source);

CREATE TABLE IF NOT EXISTS imported_playlist_track (
    playlistId TEXT NOT NULL,
    trackKey TEXT NOT NULL,
    PRIMARY KEY (playlistId, trackKey)
);

CREATE TABLE IF NOT EXISTS song_edit (
    songId TEXT PRIMARY KEY NOT NULL,
    title TEXT,
    artists TEXT,
    album TEXT,
    year INTEGER,
    cover TEXT,
    videoId TEXT,
    noVideo INTEGER NOT NULL DEFAULT 0
);

-- Views
CREATE VIEW IF NOT EXISTS sorted_song_artist_map AS
    SELECT * FROM song_artist_map ORDER BY position;

CREATE VIEW IF NOT EXISTS sorted_song_album_map AS
    SELECT * FROM song_album_map ORDER BY "index";

CREATE VIEW IF NOT EXISTS playlist_song_map_preview AS
    SELECT * FROM playlist_song_map WHERE position <= 3;

-- Queue table for persistent queue
CREATE TABLE IF NOT EXISTS _queue (
    id INTEGER PRIMARY KEY CHECK (id = 0),
    json TEXT NOT NULL DEFAULT '{}',
    current_index INTEGER NOT NULL DEFAULT 0
);
"#;

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
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = rusqlite::Connection::open(path)?;

        // PRAGMAs
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 60000;
             PRAGMA cache_size = -16000;
             PRAGMA wal_autocheckpoint = 1000;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        )?;

        // Schema
        conn.execute_batch(SCHEMA_V43)?;

        // User version
        conn.pragma_update(None, "user_version", 43)?;

        Ok(Self {
            conn: std::sync::Mutex::new(conn),
        })
    }

    /// Open at the default path (`db_path()`), creating dirs.
    pub fn open_default() -> StorageResult<Self> {
        ensure_dirs()?;
        Self::open(&db_path())
    }

    // helper to lock the connection
    fn conn(&self) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
        self.conn.lock().expect("db mutex poisoned")
    }

    // -- songs ------------------------------------------------------------

    /// Insert or update a song (upsert by id; replaces artist/album maps for
    /// the song). Sets `inLibrary = true` when `add_to_library` is used.
    pub fn upsert_song(&self, song: &Song) -> StorageResult<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;

        let duration: i64 = song.duration_secs.map(|d| d as i64).unwrap_or(-1);
        let is_local: i64 = if song.local_path.is_some() { 1 } else { 0 };
        let local_uri: Option<String> = song.local_path.as_ref().map(|p| p.to_string_lossy().into_owned());
        let liked: i64 = if song.liked { 1 } else { 0 };
        let in_library: Option<i64> = if song.in_library { Some(1) } else { Some(0) };
        let explicit: i64 = if song.explicit { 1 } else { 0 };
        let is_video: i64 = if song.is_video { 1 } else { 0 };
        let romanize: i64 = if song.romanize_lyrics { 1 } else { 0 };

        tx.execute(
            "INSERT OR REPLACE INTO song (
                id, title, duration, thumbnailUrl, albumId, albumName,
                explicit, year, liked, likedDate, totalPlayTime,
                inLibrary, isLocal, localUri, isVideo, lyricsOffset,
                romanizeLyrics
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            rusqlite::params![
                song.id,
                song.title,
                duration,
                song.thumbnail_url,
                song.album_id,
                song.album_name,
                explicit,
                song.year,
                liked,
                song.liked_date,
                song.total_play_time,
                in_library,
                is_local,
                local_uri,
                is_video,
                song.lyrics_offset_ms,
                romanize,
            ],
        )?;

        // Delete old artist maps for this song, re-insert
        tx.execute(
            "DELETE FROM song_artist_map WHERE songId = ?1",
            rusqlite::params![song.id],
        )?;
        for (pos, artist_ref) in song.artists.iter().enumerate() {
            let artist_id = artist_ref
                .id
                .clone()
                .unwrap_or_else(|| format!("LA_unknown_{}", fnv1a_hex(&artist_ref.name)));
            // Ensure artist row exists
            tx.execute(
                "INSERT OR IGNORE INTO artist (id, name) VALUES (?1, ?2)",
                rusqlite::params![artist_id, artist_ref.name],
            )?;
            tx.execute(
                "INSERT OR REPLACE INTO song_artist_map (songId, artistId, position) VALUES (?1, ?2, ?3)",
                rusqlite::params![song.id, artist_id, pos as i64],
            )?;
        }

        // Album map
        tx.execute(
            "DELETE FROM song_album_map WHERE songId = ?1",
            rusqlite::params![song.id],
        )?;
        if let Some(ref album_id) = song.album_id {
            // Ensure album row exists
            tx.execute(
                "INSERT OR IGNORE INTO album (id, title) VALUES (?1, ?2)",
                rusqlite::params![album_id, song.album_name.as_deref().unwrap_or("")],
            )?;
            tx.execute(
                "INSERT OR REPLACE INTO song_album_map (songId, albumId) VALUES (?1, ?2)",
                rusqlite::params![song.id, album_id],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Helper: build a Song from a row that includes artist info via a
    /// separate query.
    fn song_from_row(conn: &rusqlite::Connection, row: &rusqlite::Row<'_>) -> rusqlite::Result<Song> {
        let id: String = row.get("id")?;
        let duration_raw: i64 = row.get("duration")?;
        let duration_secs = if duration_raw < 0 { None } else { Some(duration_raw as f64) };
        let liked_int: i64 = row.get("liked")?;
        let in_library_raw: Option<i64> = row.get("inLibrary")?;
        let explicit_int: i64 = row.get("explicit")?;
        let is_video_int: i64 = row.get("isVideo")?;
        let is_local_int: i64 = row.get("isLocal")?;
        let local_uri: Option<String> = row.get("localUri")?;
        let romanize_int: i64 = row.get("romanizeLyrics")?;
        let lyrics_offset: i32 = row.get("lyricsOffset")?;
        let year: Option<i32> = row.get("year")?;

        // Fetch artists for this song
        let mut stmt = conn.prepare(
            "SELECT a.id, a.name FROM song_artist_map m
             JOIN artist a ON a.id = m.artistId
             WHERE m.songId = ?1
             ORDER BY m.position"
        )?;
        let artists: Vec<ArtistRef> = stmt
            .query_map(rusqlite::params![id], |r| {
                let aid: String = r.get(0)?;
                let aname: String = r.get(1)?;
                Ok(ArtistRef { id: Some(aid), name: aname })
            })?
            .filter_map(|r| r.ok())
            .collect();

        // Determine video_id: for remote songs id == video_id; for local songs it's None
        let video_id = if is_local_int != 0 { None } else { Some(id.clone()) };

        Ok(Song {
            id,
            video_id,
            title: row.get("title")?,
            artists,
            album_id: row.get("albumId")?,
            album_name: row.get("albumName")?,
            duration_secs,
            thumbnail_url: row.get("thumbnailUrl")?,
            explicit: explicit_int != 0,
            is_video: is_video_int != 0,
            local_path: if is_local_int != 0 {
                local_uri.map(PathBuf::from)
            } else {
                None
            },
            liked: liked_int != 0,
            liked_date: row.get("likedDate")?,
            in_library: in_library_raw.unwrap_or(0) != 0,
            total_play_time: row.get("totalPlayTime")?,
            lyrics_offset_ms: lyrics_offset,
            romanize_lyrics: romanize_int != 0,
            year,
        })
    }

    pub fn get_song(&self, id: &str) -> StorageResult<Option<Song>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT * FROM song WHERE id = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(Self::song_from_row(&conn, row)?)),
            None => Ok(None),
        }
    }

    /// All songs marked `inLibrary` (library view).
    pub fn all_songs(&self) -> StorageResult<Vec<Song>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT * FROM song WHERE inLibrary = 1")?;
        let mut rows = stmt.query([])?;
        let mut songs = Vec::new();
        while let Some(row) = rows.next()? {
            songs.push(Self::song_from_row(&conn, row)?);
        }
        Ok(songs)
    }

    pub fn set_liked(&self, song_id: &str, liked: bool) -> StorageResult<()> {
        let conn = self.conn();
        let liked_val: i64 = if liked { 1 } else { 0 };
        let now_ms = epoch_millis();
        let liked_date: Option<i64> = if liked { Some(now_ms) } else { None };
        conn.execute(
            "UPDATE song SET liked = ?1, likedDate = ?2 WHERE id = ?3",
            rusqlite::params![liked_val, liked_date, song_id],
        )?;
        Ok(())
    }

    /// All liked songs, most recent `likedDate` first.
    pub fn liked_songs(&self) -> StorageResult<Vec<Song>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT * FROM song WHERE liked = 1 ORDER BY likedDate DESC")?;
        let mut rows = stmt.query([])?;
        let mut songs = Vec::new();
        while let Some(row) = rows.next()? {
            songs.push(Self::song_from_row(&conn, row)?);
        }
        Ok(songs)
    }

    pub fn set_in_library(&self, song_id: &str, in_library: bool) -> StorageResult<()> {
        let conn = self.conn();
        let val: Option<i64> = if in_library { Some(1) } else { Some(0) };
        conn.execute(
            "UPDATE song SET inLibrary = ?1 WHERE id = ?2",
            rusqlite::params![val, song_id],
        )?;
        Ok(())
    }

    /// Save a local file as a library song (id from `ids::song_id_for_path`,
    /// tag metadata read by the caller).
    pub fn add_local_song(&self, song: &Song) -> StorageResult<()> {
        // Upsert with inLibrary = true
        let mut s = song.clone();
        s.in_library = true;
        self.upsert_song(&s)
    }

    // -- play events / stats ----------------------------------------------

    /// Record a play: `event` row + `playCount` bump + `totalPlayTime` add.
    pub fn record_play(&self, song_id: &str, played_secs: f64) -> StorageResult<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let now = epoch_millis();

        // Insert event
        tx.execute(
            "INSERT INTO event (songId, timestamp, playTime) VALUES (?1, ?2, ?3)",
            rusqlite::params![song_id, now, played_secs],
        )?;

        // Update totalPlayTime on song
        tx.execute(
            "UPDATE song SET totalPlayTime = totalPlayTime + ?1 WHERE id = ?2",
            rusqlite::params![played_secs, song_id],
        )?;

        // Update playCount table (year/month)
        let (year, month) = current_year_month();
        tx.execute(
            "INSERT INTO playCount (song, year, month, count) VALUES (?1, ?2, ?3, 1)
             ON CONFLICT(song, year, month) DO UPDATE SET count = count + 1",
            rusqlite::params![song_id, year, month],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Recently played (distinct songs, latest `event` first).
    pub fn recent_history(&self, limit: usize) -> StorageResult<Vec<Song>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT s.* FROM song s
             INNER JOIN (
                 SELECT songId, MAX(timestamp) AS latest
                 FROM event
                 GROUP BY songId
             ) e ON s.id = e.songId
             ORDER BY e.latest DESC
             LIMIT ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![limit as i64])?;
        let mut songs = Vec::new();
        while let Some(row) = rows.next()? {
            songs.push(Self::song_from_row(&conn, row)?);
        }
        Ok(songs)
    }

    /// Most played by `playCount`/`totalPlayTime`.
    pub fn most_played(&self, limit: usize) -> StorageResult<Vec<Song>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT s.* FROM song s
             INNER JOIN (
                 SELECT song, SUM(count) AS total_count
                 FROM playCount
                 GROUP BY song
             ) pc ON s.id = pc.song
             ORDER BY pc.total_count DESC, s.totalPlayTime DESC
             LIMIT ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![limit as i64])?;
        let mut songs = Vec::new();
        while let Some(row) = rows.next()? {
            songs.push(Self::song_from_row(&conn, row)?);
        }
        Ok(songs)
    }

    // -- playlists ----------------------------------------------------------

    /// Create a playlist (`LP`+8 letters id), returns its id.
    pub fn create_playlist(&self, name: &str) -> StorageResult<String> {
        let id = mochy_core::ids::gen_playlist_id();
        let now = epoch_millis();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO playlist (id, name, createdAt, isEditable) VALUES (?1, ?2, ?3, 1)",
            rusqlite::params![id, name, now],
        )?;
        Ok(id)
    }

    /// All playlists (user playlists only; skip `LP_LIKED`/`LP_DOWNLOADED`).
    pub fn playlists(&self) -> StorageResult<Vec<Playlist>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT * FROM playlist WHERE id NOT IN ('LP_LIKED', 'LP_DOWNLOADED') ORDER BY createdAt DESC",
        )?;
        let mut rows = stmt.query([])?;
        let mut list = Vec::new();
        while let Some(row) = rows.next()? {
            list.push(playlist_from_row(row)?);
        }
        Ok(list)
    }

    /// Including special playlists.
    pub fn playlist_by_id(&self, id: &str) -> StorageResult<Option<Playlist>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT * FROM playlist WHERE id = ?1")?;
        let mut rows = stmt.query(rusqlite::params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(playlist_from_row(row)?)),
            None => Ok(None),
        }
    }

    pub fn rename_playlist(&self, id: &str, name: &str) -> StorageResult<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE playlist SET name = ?1 WHERE id = ?2",
            rusqlite::params![name, id],
        )?;
        Ok(())
    }

    pub fn delete_playlist(&self, id: &str) -> StorageResult<()> {
        let conn = self.conn();
        conn.execute("DELETE FROM playlist WHERE id = ?1", rusqlite::params![id])?;
        Ok(())
    }

    /// Ordered songs of a playlist (`playlist_song_map` by position).
    pub fn playlist_songs(&self, playlist_id: &str) -> StorageResult<Vec<Song>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT s.* FROM song s
             INNER JOIN playlist_song_map psm ON s.id = psm.songId
             WHERE psm.playlistId = ?1
             ORDER BY psm.position",
        )?;
        let mut rows = stmt.query(rusqlite::params![playlist_id])?;
        let mut songs = Vec::new();
        while let Some(row) = rows.next()? {
            songs.push(Self::song_from_row(&conn, row)?);
        }
        Ok(songs)
    }

    /// Append; assigns `position` = max+1 and stores `setVideoId` when given.
    pub fn add_to_playlist(
        &self,
        playlist_id: &str,
        song: &Song,
        set_video_id: Option<&str>,
    ) -> StorageResult<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;

        // Ensure the song exists
        let existing: bool = tx.query_row(
            "SELECT COUNT(*) FROM song WHERE id = ?1",
            rusqlite::params![song.id],
            |r| r.get::<_, i64>(0),
        )? > 0;
        if !existing {
            // Quick insert of the song
            drop(tx);
            self.upsert_song(song)?;
            let conn2 = self.conn();
            let tx2 = conn2.unchecked_transaction()?;
            let max_pos: i64 = tx2
                .query_row(
                    "SELECT COALESCE(MAX(position), -1) FROM playlist_song_map WHERE playlistId = ?1",
                    rusqlite::params![playlist_id],
                    |r| r.get(0),
                )?;
            tx2.execute(
                "INSERT INTO playlist_song_map (playlistId, songId, position, setVideoId) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![playlist_id, song.id, max_pos + 1, set_video_id],
            )?;
            tx2.commit()?;
        } else {
            let max_pos: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(position), -1) FROM playlist_song_map WHERE playlistId = ?1",
                    rusqlite::params![playlist_id],
                    |r| r.get(0),
                )?;
            tx.execute(
                "INSERT INTO playlist_song_map (playlistId, songId, position, setVideoId) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![playlist_id, song.id, max_pos + 1, set_video_id],
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn remove_from_playlist(&self, playlist_id: &str, song_id: &str) -> StorageResult<()> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM playlist_song_map WHERE playlistId = ?1 AND songId = ?2",
            rusqlite::params![playlist_id, song_id],
        )?;
        Ok(())
    }

    // -- albums / artists ---------------------------------------------------

    pub fn upsert_album(&self, album: &Album) -> StorageResult<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT OR REPLACE INTO album (id, playlistId, title, year, thumbnailUrl, songCount, duration, description)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                album.id,
                album.playlist_id,
                album.title,
                album.year,
                album.thumbnail_url,
                album.song_count.map(|c| c as i64),
                album.duration_secs.map(|d| d as i64),
                album.description,
            ],
        )?;
        Ok(())
    }

    pub fn upsert_artist(&self, artist: &Artist) -> StorageResult<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT OR REPLACE INTO artist (id, name, thumbnailUrl, channelId) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                artist.id,
                artist.name,
                artist.thumbnail_url,
                artist.channel_id,
            ],
        )?;
        Ok(())
    }

    /// Albums present in the library (from `song_album_map` joins).
    pub fn library_albums(&self) -> StorageResult<Vec<Album>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT a.* FROM album a
             INNER JOIN song_album_map sam ON a.id = sam.albumId
             INNER JOIN song s ON s.id = sam.songId
             WHERE s.inLibrary = 1
             ORDER BY a.title",
        )?;
        let mut rows = stmt.query([])?;
        let mut albums = Vec::new();
        while let Some(row) = rows.next()? {
            albums.push(album_from_row(row)?);
        }
        Ok(albums)
    }

    /// Artists present in the library.
    pub fn library_artists(&self) -> StorageResult<Vec<Artist>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT a.* FROM artist a
             INNER JOIN song_artist_map sam ON a.id = sam.artistId
             INNER JOIN song s ON s.id = sam.songId
             WHERE s.inLibrary = 1
             ORDER BY a.name",
        )?;
        let mut rows = stmt.query([])?;
        let mut artists = Vec::new();
        while let Some(row) = rows.next()? {
            artists.push(artist_from_row(row)?);
        }
        Ok(artists)
    }

    /// Songs of one album in track order (`song_album_map.index`).
    pub fn album_songs(&self, album_id: &str) -> StorageResult<Vec<Song>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT s.* FROM song s
             INNER JOIN song_album_map sam ON s.id = sam.songId
             WHERE sam.albumId = ?1
             ORDER BY sam.\"index\"",
        )?;
        let mut rows = stmt.query(rusqlite::params![album_id])?;
        let mut songs = Vec::new();
        while let Some(row) = rows.next()? {
            songs.push(Self::song_from_row(&conn, row)?);
        }
        Ok(songs)
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
        let conn = self.conn();
        conn.execute(
            "INSERT OR REPLACE INTO format (id, itag, mimeType, codecs, bitrate, sampleRate, contentLength, loudnessDb)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                song_id,
                itag,
                mime_type,
                codecs,
                bitrate,
                sample_rate,
                content_length,
                loudness_db,
            ],
        )?;
        Ok(())
    }

    /// Upsert lyrics for a video id (Room table `lyrics`).
    pub fn save_lyrics(&self, video_id: &str, provider: &str, lyrics: &str) -> StorageResult<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT OR REPLACE INTO lyrics (id, lyrics, provider) VALUES (?1, ?2, ?3)",
            rusqlite::params![video_id, lyrics, provider],
        )?;
        Ok(())
    }

    /// (provider, lyrics) or None. Sentinel `LYRICS_NOT_FOUND` is returned as
    /// None for both missing and sentinel rows.
    pub fn load_lyrics(&self, video_id: &str) -> StorageResult<Option<(String, String)>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT provider, lyrics FROM lyrics WHERE id = ?1")?;
        let mut rows = stmt.query(rusqlite::params![video_id])?;
        match rows.next()? {
            Some(row) => {
                let provider: String = row.get(0)?;
                let lyrics_text: Option<String> = row.get(1)?;
                let lyrics_text = lyrics_text.unwrap_or_default();
                // Sentinel check
                if provider == "LYRICS_NOT_FOUND" {
                    Ok(None)
                } else {
                    Ok(Some((provider, lyrics_text)))
                }
            }
            None => Ok(None),
        }
    }

    /// Search history (unique `query`, newest first, trimmed to 100).
    pub fn add_search_history(&self, query: &str) -> StorageResult<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;

        // Remove existing duplicate so it becomes the newest
        tx.execute(
            "DELETE FROM search_history WHERE query = ?1",
            rusqlite::params![query],
        )?;
        tx.execute(
            "INSERT INTO search_history (query) VALUES (?1)",
            rusqlite::params![query],
        )?;

        // Trim to 100 entries (keep highest ids = newest)
        tx.execute(
            "DELETE FROM search_history WHERE id NOT IN (
                SELECT id FROM search_history ORDER BY id DESC LIMIT 100
            )",
            [],
        )?;

        tx.commit()?;
        Ok(())
    }

    pub fn search_history(&self, limit: usize) -> StorageResult<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT query FROM search_history ORDER BY id DESC LIMIT ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![limit as i64])?;
        let mut results = Vec::new();
        while let Some(row) = rows.next()? {
            results.push(row.get(0)?);
        }
        Ok(results)
    }

    /// Persist a queue for `persistent_queue` restore.
    pub fn save_queue(&self, items: &[QueueItem], index: usize) -> StorageResult<()> {
        let json = serde_json::to_string(items)?;
        let conn = self.conn();
        conn.execute(
            "INSERT OR REPLACE INTO _queue (id, json, current_index) VALUES (0, ?1, ?2)",
            rusqlite::params![json, index as i64],
        )?;
        Ok(())
    }

    /// Restored queue + index (None when absent).
    pub fn load_queue(&self) -> StorageResult<Option<(Vec<QueueItem>, usize)>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT json, current_index FROM _queue WHERE id = 0")?;
        let mut rows = stmt.query([])?;
        match rows.next()? {
            Some(row) => {
                let json_str: String = row.get(0)?;
                let idx: i64 = row.get(1)?;
                let items: Vec<QueueItem> = serde_json::from_str(&json_str)?;
                Ok(Some((items, idx as usize)))
            }
            None => Ok(None),
        }
    }
}

// ---------------------------------------------------------------------------
// Row helpers
// ---------------------------------------------------------------------------

fn playlist_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Playlist> {
    let is_editable_int: i64 = row.get("isEditable")?;
    let song_count: Option<i64> = row.get("remoteSongCount")?;
    Ok(Playlist {
        id: row.get("id")?,
        name: row.get("name")?,
        browse_id: row.get("browseId")?,
        thumbnail_url: row.get("thumbnailUrl")?,
        song_count: song_count.unwrap_or(0) as u32,
        description: row.get("description")?,
        is_editable: is_editable_int != 0,
        created_at: row.get::<_, Option<i64>>("createdAt")?.unwrap_or(0),
    })
}

fn album_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Album> {
    let song_count: Option<i64> = row.get("songCount")?;
    let duration: Option<i64> = row.get("duration")?;
    Ok(Album {
        id: row.get("id")?,
        playlist_id: row.get("playlistId")?,
        title: row.get("title")?,
        year: row.get("year")?,
        thumbnail_url: row.get("thumbnailUrl")?,
        song_count: song_count.map(|c| c as u32),
        duration_secs: duration.map(|d| d as f64),
        description: row.get("description")?,
    })
}

fn artist_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Artist> {
    Ok(Artist {
        id: row.get("id")?,
        name: row.get("name")?,
        thumbnail_url: row.get("thumbnailUrl")?,
        channel_id: row.get("channelId")?,
    })
}

// ---------------------------------------------------------------------------
// Epoch helpers
// ---------------------------------------------------------------------------

fn epoch_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn current_year_month() -> (i32, i32) {
    // Approximate from epoch millis — good enough for play-count bucketing.
    let millis = epoch_millis();
    let secs = millis / 1000;
    // Days since epoch
    let days = secs / 86400;
    // Approximate year/month (good enough for bucketing)
    // Use a simple calculation: days since 1970-01-01
    let mut y = 1970i32;
    let mut remaining = days;
    loop {
        let days_in_year: i64 = if is_leap(y) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }
    let month_days: [i64; 12] = if is_leap(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut m = 1i32;
    for &md in &month_days {
        if remaining < md {
            break;
        }
        remaining -= md;
        m += 1;
    }
    (y, m)
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
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
        Self {
            path: settings_path(),
        }
    }

    pub fn load(&self) -> StorageResult<AppSettings> {
        if !self.path.exists() {
            return Ok(AppSettings::default());
        }
        let data = std::fs::read_to_string(&self.path)?;
        if data.trim().is_empty() {
            return Ok(AppSettings::default());
        }
        let settings: AppSettings = serde_json::from_str(&data)?;
        Ok(settings)
    }

    /// Atomic-ish write (temp file + rename), pretty JSON.
    pub fn save(&self, settings: &AppSettings) -> StorageResult<()> {
        let json = serde_json::to_string_pretty(settings)?;
        // Write to a temp file next to the target, then rename
        let tmp_path = self.path.with_extension("json.tmp");
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&tmp_path, json.as_bytes())?;
        std::fs::rename(&tmp_path, &self.path)?;
        Ok(())
    }
}
