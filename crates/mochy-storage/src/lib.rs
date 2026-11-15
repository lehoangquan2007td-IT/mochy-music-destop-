//! mochy-storage — SQLite persistence + settings + paths (spec §5).
//!
//! Schema of record: Room v43 (spec §5.1) — same table/column names, types
//! and NOT NULL constraints as the Android `InternalDatabase` (Room v34
//! exported schema + hand-written migrations 35→43), all 24 entity tables +
//! 3 views, plus one desktop-only table (`_queue`). PRAGMAs: WAL,
//! busy_timeout=60000, cache_size=-16000, wal_autocheckpoint=1000,
//! synchronous=NORMAL, foreign_keys=ON. `user_version` = 43.
//!
//! Units follow Room: timestamps (`likedDate`, `inLibrary`, `createdAt`,
//! `event.timestamp`, …) are epoch **milliseconds**; `inLibrary` is NULL when
//! the song is not in the library; `totalPlayTime`/`event.playTime` are
//! **milliseconds** (the `Song` model exposes seconds).
//!
//! [`Db`] is `Send + Sync` (an internal `Mutex<Connection>`), so the app can
//! share one `Arc<Db>` across threads; every method takes `&self`.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use mochy_core::models::{Album, Artist, ArtistRef, Playlist, QueueItem, Song};
use mochy_core::settings::AppSettings;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::Value;
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

/// Room `user_version` of the schema created here.
pub const SCHEMA_VERSION: i32 = 43;

/// Lyrics text Android stores when no provider found lyrics.
pub const LYRICS_NOT_FOUND: &str = "LYRICS_NOT_FOUND";

/// Special playlist ids (spec §5.1).
pub const LIKED_PLAYLIST_ID: &str = "LP_LIKED";
pub const DOWNLOADED_PLAYLIST_ID: &str = "LP_DOWNLOADED";

const SEARCH_HISTORY_MAX: i64 = 100;

// ---------------------------------------------------------------------------
// Paths (dirs crate): %LOCALAPPDATA%/MochyMusic on Windows, XDG data on Linux.
// ---------------------------------------------------------------------------

/// Root data dir (created by [`ensure_dirs`]).
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

/// FNV-1a hash of a string.
fn fnv1a(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Local file path for a remote artwork url (stable name by url hash).
pub fn art_cache_file(url: &str) -> PathBuf {
    art_cache_dir().join(format!("{:016x}", fnv1a(url)))
}

/// Stable `LA` + 8 letters id for an artist known only by name (local files,
/// rows without a browse id) so the same name maps to the same artist row.
pub fn local_artist_id(name: &str) -> String {
    const LETTERS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut h = fnv1a(&name.trim().to_lowercase());
    let mut out = String::from("LA");
    for _ in 0..8 {
        out.push(LETTERS[(h % LETTERS.len() as u64) as usize] as char);
        h /= LETTERS.len() as u64;
        if h == 0 {
            h = fnv1a(&out);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Schema DDL (Room v43: 24 tables + 3 views; statements as Room generates them)
// ---------------------------------------------------------------------------

const SCHEMA_V43: &str = r#"
CREATE TABLE IF NOT EXISTS `song` (`id` TEXT NOT NULL, `title` TEXT NOT NULL, `duration` INTEGER NOT NULL, `thumbnailUrl` TEXT, `albumId` TEXT, `albumName` TEXT, `explicit` INTEGER NOT NULL DEFAULT 0, `year` INTEGER, `date` INTEGER, `dateModified` INTEGER, `liked` INTEGER NOT NULL, `likedDate` INTEGER, `totalPlayTime` INTEGER NOT NULL, `inLibrary` INTEGER, `dateDownload` INTEGER, `isLocal` INTEGER NOT NULL DEFAULT false, `libraryAddToken` TEXT, `libraryRemoveToken` TEXT, `lyricsOffset` INTEGER NOT NULL DEFAULT 0, `romanizeLyrics` INTEGER NOT NULL DEFAULT true, `isDownloaded` INTEGER NOT NULL DEFAULT 0, `isUploaded` INTEGER NOT NULL DEFAULT false, `isVideo` INTEGER NOT NULL DEFAULT false, `localUri` TEXT DEFAULT NULL, `addedByLike` INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(`id`));
CREATE INDEX IF NOT EXISTS `index_song_albumId` ON `song` (`albumId`);

CREATE TABLE IF NOT EXISTS `artist` (`id` TEXT NOT NULL, `name` TEXT NOT NULL, `thumbnailUrl` TEXT, `channelId` TEXT, `lastUpdateTime` INTEGER NOT NULL, `bookmarkedAt` INTEGER, `isLocal` INTEGER NOT NULL DEFAULT false, PRIMARY KEY(`id`));

CREATE TABLE IF NOT EXISTS `album` (`id` TEXT NOT NULL, `playlistId` TEXT, `title` TEXT NOT NULL, `year` INTEGER, `thumbnailUrl` TEXT, `themeColor` INTEGER, `songCount` INTEGER NOT NULL, `duration` INTEGER NOT NULL, `explicit` INTEGER NOT NULL DEFAULT 0, `lastUpdateTime` INTEGER NOT NULL, `bookmarkedAt` INTEGER, `likedDate` INTEGER, `inLibrary` INTEGER, `description` TEXT, `isLocal` INTEGER NOT NULL DEFAULT false, `isUploaded` INTEGER NOT NULL DEFAULT false, PRIMARY KEY(`id`));

CREATE TABLE IF NOT EXISTS `playlist` (`id` TEXT NOT NULL, `name` TEXT NOT NULL, `browseId` TEXT, `createdAt` INTEGER, `lastUpdateTime` INTEGER, `isEditable` INTEGER NOT NULL DEFAULT true, `bookmarkedAt` INTEGER, `remoteSongCount` INTEGER, `playEndpointParams` TEXT, `thumbnailUrl` TEXT, `shuffleEndpointParams` TEXT, `radioEndpointParams` TEXT, `isLocal` INTEGER NOT NULL DEFAULT false, `isAutoSync` INTEGER NOT NULL DEFAULT false, `description` TEXT DEFAULT NULL, PRIMARY KEY(`id`));

CREATE TABLE IF NOT EXISTS `song_artist_map` (`songId` TEXT NOT NULL, `artistId` TEXT NOT NULL, `position` INTEGER NOT NULL, PRIMARY KEY(`songId`, `artistId`), FOREIGN KEY(`songId`) REFERENCES `song`(`id`) ON UPDATE NO ACTION ON DELETE CASCADE , FOREIGN KEY(`artistId`) REFERENCES `artist`(`id`) ON UPDATE NO ACTION ON DELETE CASCADE );
CREATE INDEX IF NOT EXISTS `index_song_artist_map_songId` ON `song_artist_map` (`songId`);
CREATE INDEX IF NOT EXISTS `index_song_artist_map_artistId` ON `song_artist_map` (`artistId`);

CREATE TABLE IF NOT EXISTS `song_album_map` (`songId` TEXT NOT NULL, `albumId` TEXT NOT NULL, `index` INTEGER NOT NULL, PRIMARY KEY(`songId`, `albumId`), FOREIGN KEY(`songId`) REFERENCES `song`(`id`) ON UPDATE NO ACTION ON DELETE CASCADE , FOREIGN KEY(`albumId`) REFERENCES `album`(`id`) ON UPDATE NO ACTION ON DELETE CASCADE );
CREATE INDEX IF NOT EXISTS `index_song_album_map_songId` ON `song_album_map` (`songId`);
CREATE INDEX IF NOT EXISTS `index_song_album_map_albumId` ON `song_album_map` (`albumId`);

CREATE TABLE IF NOT EXISTS `album_artist_map` (`albumId` TEXT NOT NULL, `artistId` TEXT NOT NULL, `order` INTEGER NOT NULL, PRIMARY KEY(`albumId`, `artistId`), FOREIGN KEY(`albumId`) REFERENCES `album`(`id`) ON UPDATE NO ACTION ON DELETE CASCADE , FOREIGN KEY(`artistId`) REFERENCES `artist`(`id`) ON UPDATE NO ACTION ON DELETE CASCADE );
CREATE INDEX IF NOT EXISTS `index_album_artist_map_albumId` ON `album_artist_map` (`albumId`);
CREATE INDEX IF NOT EXISTS `index_album_artist_map_artistId` ON `album_artist_map` (`artistId`);

CREATE TABLE IF NOT EXISTS `playlist_song_map` (`id` INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, `playlistId` TEXT NOT NULL, `songId` TEXT NOT NULL, `position` INTEGER NOT NULL, `setVideoId` TEXT, FOREIGN KEY(`playlistId`) REFERENCES `playlist`(`id`) ON UPDATE NO ACTION ON DELETE CASCADE , FOREIGN KEY(`songId`) REFERENCES `song`(`id`) ON UPDATE NO ACTION ON DELETE CASCADE );
CREATE INDEX IF NOT EXISTS `index_playlist_song_map_playlistId` ON `playlist_song_map` (`playlistId`);
CREATE INDEX IF NOT EXISTS `index_playlist_song_map_songId` ON `playlist_song_map` (`songId`);

CREATE TABLE IF NOT EXISTS `search_history` (`id` INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, `query` TEXT NOT NULL);
CREATE UNIQUE INDEX IF NOT EXISTS `index_search_history_query` ON `search_history` (`query`);

CREATE TABLE IF NOT EXISTS `format` (`id` TEXT NOT NULL, `itag` INTEGER NOT NULL, `mimeType` TEXT NOT NULL, `codecs` TEXT NOT NULL, `bitrate` INTEGER NOT NULL, `sampleRate` INTEGER, `contentLength` INTEGER NOT NULL, `loudnessDb` REAL, `perceptualLoudnessDb` REAL, `playbackUrl` TEXT, PRIMARY KEY(`id`));

CREATE TABLE IF NOT EXISTS `lyrics` (`id` TEXT NOT NULL, `lyrics` TEXT NOT NULL, `provider` TEXT NOT NULL DEFAULT 'Unknown', `translatedLyrics` TEXT NOT NULL DEFAULT '', `translationLanguage` TEXT NOT NULL DEFAULT '', `translationMode` TEXT NOT NULL DEFAULT '', PRIMARY KEY(`id`));

CREATE TABLE IF NOT EXISTS `event` (`id` INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, `songId` TEXT NOT NULL, `timestamp` INTEGER NOT NULL, `playTime` INTEGER NOT NULL, FOREIGN KEY(`songId`) REFERENCES `song`(`id`) ON UPDATE NO ACTION ON DELETE CASCADE );
CREATE INDEX IF NOT EXISTS `index_event_songId` ON `event` (`songId`);

CREATE TABLE IF NOT EXISTS `related_song_map` (`id` INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, `songId` TEXT NOT NULL, `relatedSongId` TEXT NOT NULL, FOREIGN KEY(`songId`) REFERENCES `song`(`id`) ON UPDATE NO ACTION ON DELETE CASCADE , FOREIGN KEY(`relatedSongId`) REFERENCES `song`(`id`) ON UPDATE NO ACTION ON DELETE CASCADE );
CREATE INDEX IF NOT EXISTS `index_related_song_map_songId` ON `related_song_map` (`songId`);
CREATE INDEX IF NOT EXISTS `index_related_song_map_relatedSongId` ON `related_song_map` (`relatedSongId`);

CREATE TABLE IF NOT EXISTS `set_video_id` (`videoId` TEXT NOT NULL, `setVideoId` TEXT, PRIMARY KEY(`videoId`));

CREATE TABLE IF NOT EXISTS `playCount` (`song` TEXT NOT NULL, `year` INTEGER NOT NULL, `month` INTEGER NOT NULL, `count` INTEGER NOT NULL, PRIMARY KEY(`song`, `year`, `month`));

CREATE TABLE IF NOT EXISTS `recognition_history` (`id` INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, `trackId` TEXT NOT NULL, `title` TEXT NOT NULL, `artist` TEXT NOT NULL, `album` TEXT, `coverArtUrl` TEXT, `coverArtHqUrl` TEXT, `genre` TEXT, `releaseDate` TEXT, `label` TEXT, `shazamUrl` TEXT, `appleMusicUrl` TEXT, `spotifyUrl` TEXT, `isrc` TEXT, `youtubeVideoId` TEXT, `recognizedAt` INTEGER NOT NULL, `liked` INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS `index_recognition_history_trackId` ON `recognition_history` (`trackId`);

CREATE TABLE IF NOT EXISTS `speed_dial_item` (`id` TEXT NOT NULL, `secondaryId` TEXT, `title` TEXT NOT NULL, `subtitle` TEXT, `thumbnailUrl` TEXT, `type` TEXT NOT NULL, `explicit` INTEGER NOT NULL, `createDate` INTEGER NOT NULL, PRIMARY KEY(`id`));

CREATE TABLE IF NOT EXISTS `playlist_event` (`id` INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, `playlistId` TEXT NOT NULL, `timestamp` INTEGER NOT NULL, FOREIGN KEY(`playlistId`) REFERENCES `playlist`(`id`) ON UPDATE NO ACTION ON DELETE CASCADE );
CREATE INDEX IF NOT EXISTS `index_playlist_event_playlistId` ON `playlist_event` (`playlistId`);

CREATE TABLE IF NOT EXISTS `imported_song` (`songId` TEXT NOT NULL, `source` TEXT NOT NULL, `importedAt` INTEGER NOT NULL, PRIMARY KEY(`songId`, `source`));
CREATE INDEX IF NOT EXISTS `index_imported_song_source` ON `imported_song` (`source`);

CREATE TABLE IF NOT EXISTS `import_session` (`id` INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, `source` TEXT NOT NULL, `startedAt` INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS `import_session_song` (`sessionId` INTEGER NOT NULL, `songId` TEXT NOT NULL, `addedToLibrary` INTEGER NOT NULL, `liked` INTEGER NOT NULL, PRIMARY KEY(`sessionId`, `songId`));
CREATE INDEX IF NOT EXISTS `index_import_session_song_songId` ON `import_session_song` (`songId`);

CREATE TABLE IF NOT EXISTS `imported_playlist` (`playlistId` TEXT NOT NULL, `source` TEXT NOT NULL, `sessionId` INTEGER, `importedAt` INTEGER NOT NULL, `remoteId` TEXT, `remoteUrl` TEXT, `lastSyncedAt` INTEGER, PRIMARY KEY(`playlistId`));
CREATE INDEX IF NOT EXISTS `index_imported_playlist_source` ON `imported_playlist` (`source`);

CREATE TABLE IF NOT EXISTS `imported_playlist_track` (`playlistId` TEXT NOT NULL, `trackKey` TEXT NOT NULL, PRIMARY KEY(`playlistId`, `trackKey`));

CREATE TABLE IF NOT EXISTS `song_edit` (`songId` TEXT NOT NULL, `title` TEXT, `artists` TEXT, `album` TEXT, `year` INTEGER, `cover` TEXT, `videoId` TEXT, `noVideo` INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(`songId`));

CREATE VIEW IF NOT EXISTS `sorted_song_artist_map` AS SELECT * FROM song_artist_map ORDER BY position;
CREATE VIEW IF NOT EXISTS `sorted_song_album_map` AS SELECT * FROM song_album_map ORDER BY `index`;
CREATE VIEW IF NOT EXISTS `playlist_song_map_preview` AS SELECT * FROM playlist_song_map WHERE position <= 3 ORDER BY position;

-- Desktop-only: persistent play queue (`Misc.persistent_queue`).
CREATE TABLE IF NOT EXISTS `_queue` (`id` INTEGER PRIMARY KEY CHECK (`id` = 0), `json` TEXT NOT NULL DEFAULT '[]', `current_index` INTEGER NOT NULL DEFAULT 0);
"#;

/// Columns added by Room migrations after a table first existed; applied to
/// databases created by older desktop builds (`ALTER TABLE ADD COLUMN`).
const LATE_COLUMNS: &[(&str, &str, &str)] = &[
    ("song", "localUri", "TEXT DEFAULT NULL"),
    ("song", "addedByLike", "INTEGER NOT NULL DEFAULT 0"),
    ("playlist", "description", "TEXT DEFAULT NULL"),
    ("imported_playlist", "remoteId", "TEXT"),
    ("imported_playlist", "remoteUrl", "TEXT"),
    ("imported_playlist", "lastSyncedAt", "INTEGER"),
];

// ---------------------------------------------------------------------------
// Database
// ---------------------------------------------------------------------------

/// Cached stream format (Room table `format`).
#[derive(Debug, Clone, PartialEq)]
pub struct FormatInfo {
    pub itag: Option<i64>,
    pub mime_type: String,
    pub codecs: String,
    pub bitrate: Option<i64>,
    pub sample_rate: Option<i64>,
    pub content_length: Option<i64>,
    pub loudness_db: Option<f64>,
    pub perceptual_loudness_db: Option<f64>,
    pub playback_url: Option<String>,
}

/// Handle around a `rusqlite::Connection` behind a `Mutex`: `Send + Sync`,
/// share it as `Arc<Db>`. All methods take `&self` and are short
/// transactions, so contention is negligible for a desktop app.
#[derive(Debug)]
pub struct Db {
    conn: Mutex<Connection>,
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Db>();
    assert_send_sync::<SettingsStore>();
};

fn epoch_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// (year, month 1–12) in UTC for `millis` (play-count buckets, Kotlin uses UTC).
fn year_month_utc(millis: i64) -> (i32, i32) {
    // Civil-from-days (Howard Hinnant).
    let days = millis.div_euclid(86_400_000);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    ((if m <= 2 { y + 1 } else { y }) as i32, m as i32)
}

const SONG_COLUMNS: &str = "s.id, s.title, s.duration, s.thumbnailUrl, s.albumId, s.albumName, s.explicit, s.year, \
     s.liked, s.likedDate, s.totalPlayTime, s.inLibrary, s.isLocal, s.localUri, s.isVideo, s.lyricsOffset, s.romanizeLyrics";

fn song_from_row(conn: &Connection, row: &Row<'_>) -> rusqlite::Result<Song> {
    let id: String = row.get("id")?;
    let duration_raw: i64 = row.get("duration")?;
    let is_local: bool = row.get("isLocal")?;
    let local_uri: Option<String> = row.get("localUri")?;
    let total_ms: i64 = row.get("totalPlayTime")?;
    let in_library: Option<i64> = row.get("inLibrary")?;

    let mut stmt = conn.prepare_cached(
        "SELECT a.id, a.name FROM song_artist_map m JOIN artist a ON a.id = m.artistId
         WHERE m.songId = ?1 ORDER BY m.position",
    )?;
    let artists = stmt
        .query_map(params![id], |r| Ok(ArtistRef { id: Some(r.get(0)?), name: r.get(1)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(Song {
        video_id: if is_local { None } else { Some(id.clone()) },
        id,
        title: row.get("title")?,
        artists,
        album_id: row.get("albumId")?,
        album_name: row.get("albumName")?,
        duration_secs: (duration_raw >= 0).then_some(duration_raw as f64),
        thumbnail_url: row.get("thumbnailUrl")?,
        explicit: row.get("explicit")?,
        is_video: row.get("isVideo")?,
        local_path: if is_local { local_uri.map(PathBuf::from) } else { None },
        liked: row.get("liked")?,
        liked_date: row.get("likedDate")?,
        in_library: in_library.is_some(),
        total_play_time: total_ms as f64 / 1000.0,
        lyrics_offset_ms: row.get("lyricsOffset")?,
        romanize_lyrics: row.get("romanizeLyrics")?,
        year: row.get("year")?,
    })
}

fn query_songs(conn: &Connection, sql: &str, params: impl rusqlite::Params) -> StorageResult<Vec<Song>> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(params)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(song_from_row(conn, row)?);
    }
    Ok(out)
}

fn song_exists(conn: &Connection, id: &str) -> StorageResult<bool> {
    Ok(conn.query_row("SELECT 1 FROM song WHERE id = ?1", params![id], |_| Ok(())).optional()?.is_some())
}

fn artist_id_of(a: &ArtistRef) -> String {
    a.id.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| local_artist_id(&a.name))
}

/// Upsert inside an open transaction. See [`Db::upsert_song`] for semantics.
fn upsert_song_in(conn: &Connection, song: &Song) -> StorageResult<()> {
    let now = epoch_millis();
    let duration: i64 = song.duration_secs.map(|d| d.round() as i64).unwrap_or(-1);
    let local_uri = song.local_path.as_ref().map(|p| p.to_string_lossy().into_owned());
    let liked_date = if song.liked { Some(song.liked_date.unwrap_or(now)) } else { None };
    let in_library = song.in_library.then_some(now);
    let total_ms = (song.total_play_time * 1000.0).round() as i64;

    conn.execute(
        "INSERT INTO song (id, title, duration, thumbnailUrl, albumId, albumName, explicit, year, liked, likedDate,
                           totalPlayTime, inLibrary, isLocal, localUri, isVideo, lyricsOffset, romanizeLyrics, addedByLike)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, 0)
         ON CONFLICT(id) DO UPDATE SET
            title = excluded.title,
            duration = CASE WHEN excluded.duration >= 0 THEN excluded.duration ELSE song.duration END,
            thumbnailUrl = COALESCE(excluded.thumbnailUrl, song.thumbnailUrl),
            albumId = COALESCE(excluded.albumId, song.albumId),
            albumName = COALESCE(excluded.albumName, song.albumName),
            explicit = excluded.explicit,
            year = COALESCE(excluded.year, song.year),
            liked = MAX(song.liked, excluded.liked),
            likedDate = COALESCE(song.likedDate, excluded.likedDate),
            totalPlayTime = MAX(song.totalPlayTime, excluded.totalPlayTime),
            inLibrary = COALESCE(song.inLibrary, excluded.inLibrary),
            isLocal = excluded.isLocal,
            localUri = COALESCE(excluded.localUri, song.localUri),
            isVideo = excluded.isVideo",
        params![
            song.id,
            song.title,
            duration,
            song.thumbnail_url,
            song.album_id,
            song.album_name,
            song.explicit,
            song.year,
            song.liked,
            liked_date,
            total_ms,
            in_library,
            song.local_path.is_some(),
            local_uri,
            song.is_video,
            song.lyrics_offset_ms,
            song.romanize_lyrics,
        ],
    )?;

    if !song.artists.is_empty() {
        conn.execute("DELETE FROM song_artist_map WHERE songId = ?1", params![song.id])?;
        for (pos, a) in song.artists.iter().enumerate() {
            let aid = artist_id_of(a);
            conn.execute(
                "INSERT INTO artist (id, name, lastUpdateTime, isLocal) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET name = CASE WHEN excluded.name != '' THEN excluded.name ELSE artist.name END",
                params![aid, a.name, now, a.id.is_none()],
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO song_artist_map (songId, artistId, position) VALUES (?1, ?2, ?3)",
                params![song.id, aid, pos as i64],
            )?;
        }
    }

    if let Some(album_id) = song.album_id.as_deref().filter(|a| !a.is_empty()) {
        conn.execute(
            "INSERT INTO album (id, title, songCount, duration, lastUpdateTime, isLocal) VALUES (?1, ?2, 0, 0, ?3, ?4)
             ON CONFLICT(id) DO NOTHING",
            params![album_id, song.album_name.as_deref().unwrap_or(""), now, song.local_path.is_some()],
        )?;
        conn.execute("DELETE FROM song_album_map WHERE songId = ?1 AND albumId != ?2", params![song.id, album_id])?;
        conn.execute(
            "INSERT OR IGNORE INTO song_album_map (songId, albumId, `index`) VALUES (?1, ?2, 0)",
            params![song.id, album_id],
        )?;
    }
    Ok(())
}

fn upsert_album_in(conn: &Connection, album: &Album) -> StorageResult<()> {
    conn.execute(
        "INSERT INTO album (id, playlistId, title, year, thumbnailUrl, songCount, duration, lastUpdateTime, description)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET
            playlistId = COALESCE(excluded.playlistId, album.playlistId),
            title = CASE WHEN excluded.title != '' THEN excluded.title ELSE album.title END,
            year = COALESCE(excluded.year, album.year),
            thumbnailUrl = COALESCE(excluded.thumbnailUrl, album.thumbnailUrl),
            songCount = CASE WHEN excluded.songCount > 0 THEN excluded.songCount ELSE album.songCount END,
            duration = CASE WHEN excluded.duration > 0 THEN excluded.duration ELSE album.duration END,
            lastUpdateTime = excluded.lastUpdateTime,
            description = COALESCE(excluded.description, album.description)",
        params![
            album.id,
            album.playlist_id,
            album.title,
            album.year,
            album.thumbnail_url,
            album.song_count.unwrap_or(0) as i64,
            album.duration_secs.map(|d| d.round() as i64).unwrap_or(0),
            epoch_millis(),
            album.description,
        ],
    )?;
    Ok(())
}

fn renumber_playlist(conn: &Connection, playlist_id: &str) -> StorageResult<()> {
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM playlist_song_map WHERE playlistId = ?1 ORDER BY position, id")?
        .query_map(params![playlist_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for (pos, id) in ids.iter().enumerate() {
        conn.execute("UPDATE playlist_song_map SET position = ?1 WHERE id = ?2", params![pos as i64, id])?;
    }
    Ok(())
}

fn touch_playlist(conn: &Connection, playlist_id: &str) -> StorageResult<()> {
    conn.execute("UPDATE playlist SET lastUpdateTime = ?1 WHERE id = ?2", params![epoch_millis(), playlist_id])?;
    Ok(())
}

impl Db {
    /// Open (or create) the database with the full v43 schema.
    pub fn open(path: &Path) -> StorageResult<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        Self::init(Connection::open(path)?)
    }

    /// In-memory database (tests, previews).
    pub fn open_in_memory() -> StorageResult<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> StorageResult<Self> {
        // journal_mode returns a row, so query it instead of executing.
        let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        conn.execute_batch(
            "PRAGMA busy_timeout = 60000;
             PRAGMA cache_size = -16000;
             PRAGMA wal_autocheckpoint = 1000;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        )?;
        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(StorageError::Other(format!(
                "database schema v{version} is newer than this build (v{SCHEMA_VERSION})"
            )));
        }
        conn.execute_batch(SCHEMA_V43)?;
        for (table, col, decl) in LATE_COLUMNS {
            let has: bool = conn
                .prepare(&format!("PRAGMA table_info(`{table}`)"))?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(Result::ok)
                .any(|c| c == *col);
            if !has {
                conn.execute_batch(&format!("ALTER TABLE `{table}` ADD COLUMN `{col}` {decl};"))?;
            }
        }
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Open at the default path (`db_path()`), creating dirs.
    pub fn open_default() -> StorageResult<Self> {
        ensure_dirs()?;
        Self::open(&db_path())
    }

    /// Lock the connection (a panic in another thread doesn't brick the DB).
    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Run `f` inside one transaction.
    fn tx<T>(&self, f: impl FnOnce(&Connection) -> StorageResult<T>) -> StorageResult<T> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    // -- songs ------------------------------------------------------------

    /// Insert or update a song (upsert by id) and its artist/album maps.
    ///
    /// Metadata (title, duration, art, album, explicit, year, isVideo, local
    /// path) is taken from `song`; unknown values (`None`, duration `None`)
    /// never erase known ones. User state is merged, never reset: `liked` and
    /// `in_library` can be set but not cleared here (use [`Db::set_liked`] /
    /// [`Db::set_in_library`]), play time only grows, and `lyrics_offset_ms` /
    /// `romanize_lyrics` are kept for existing rows (use their setters).
    /// Artist maps are replaced only when `song.artists` is non-empty.
    pub fn upsert_song(&self, song: &Song) -> StorageResult<()> {
        self.tx(|c| upsert_song_in(c, song))
    }

    /// Upsert many songs in one transaction.
    pub fn upsert_songs(&self, songs: &[Song]) -> StorageResult<()> {
        self.tx(|c| songs.iter().try_for_each(|s| upsert_song_in(c, s)))
    }

    pub fn get_song(&self, id: &str) -> StorageResult<Option<Song>> {
        let conn = self.conn();
        let mut songs = query_songs(&conn, &format!("SELECT {SONG_COLUMNS} FROM song s WHERE s.id = ?1"), params![id])?;
        Ok(songs.pop())
    }

    /// All songs in the library (`inLibrary IS NOT NULL`), newest first.
    pub fn all_songs(&self) -> StorageResult<Vec<Song>> {
        let conn = self.conn();
        query_songs(
            &conn,
            &format!("SELECT {SONG_COLUMNS} FROM song s WHERE s.inLibrary IS NOT NULL ORDER BY s.inLibrary DESC, s.rowid DESC"),
            [],
        )
    }

    /// Local-file songs (`isLocal = 1`), by title.
    pub fn local_songs(&self) -> StorageResult<Vec<Song>> {
        let conn = self.conn();
        query_songs(&conn, &format!("SELECT {SONG_COLUMNS} FROM song s WHERE s.isLocal = 1 ORDER BY s.title COLLATE NOCASE"), [])
    }

    /// Like/unlike (Kotlin `toggleLike`): liking stamps `likedDate` and adds
    /// the song to the library (`addedByLike = 1` when it wasn't there);
    /// unliking clears `likedDate` and keeps the library entry.
    pub fn set_liked(&self, song_id: &str, liked: bool) -> StorageResult<()> {
        let now = epoch_millis();
        let conn = self.conn();
        if liked {
            conn.execute(
                "UPDATE song SET liked = 1, likedDate = COALESCE(likedDate, ?1),
                    addedByLike = CASE WHEN inLibrary IS NULL THEN 1 ELSE addedByLike END,
                    inLibrary = COALESCE(inLibrary, ?1)
                 WHERE id = ?2",
                params![now, song_id],
            )?;
        } else {
            conn.execute("UPDATE song SET liked = 0, likedDate = NULL WHERE id = ?1", params![song_id])?;
        }
        Ok(())
    }

    /// All liked songs, most recent `likedDate` first.
    pub fn liked_songs(&self) -> StorageResult<Vec<Song>> {
        let conn = self.conn();
        query_songs(&conn, &format!("SELECT {SONG_COLUMNS} FROM song s WHERE s.liked = 1 ORDER BY s.likedDate DESC, s.rowid DESC"), [])
    }

    /// Add/remove from the library (Kotlin `toggleLibrary`): removing also
    /// unlikes, like Android does.
    pub fn set_in_library(&self, song_id: &str, in_library: bool) -> StorageResult<()> {
        let conn = self.conn();
        if in_library {
            conn.execute(
                "UPDATE song SET inLibrary = COALESCE(inLibrary, ?1), addedByLike = 0 WHERE id = ?2",
                params![epoch_millis(), song_id],
            )?;
        } else {
            conn.execute(
                "UPDATE song SET inLibrary = NULL, liked = 0, likedDate = NULL, addedByLike = 0 WHERE id = ?1",
                params![song_id],
            )?;
        }
        Ok(())
    }

    /// Per-song lyrics offset (ms, positive shows lines earlier).
    pub fn set_lyrics_offset(&self, song_id: &str, offset_ms: i32) -> StorageResult<()> {
        self.conn().execute("UPDATE song SET lyricsOffset = ?1 WHERE id = ?2", params![offset_ms, song_id])?;
        Ok(())
    }

    pub fn set_romanize_lyrics(&self, song_id: &str, romanize: bool) -> StorageResult<()> {
        self.conn().execute("UPDATE song SET romanizeLyrics = ?1 WHERE id = ?2", params![romanize, song_id])?;
        Ok(())
    }

    /// Save a local file as a library song (id from `ids::song_id_for_path`,
    /// tag metadata read by the caller; `local_path` must be set).
    pub fn add_local_song(&self, song: &Song) -> StorageResult<()> {
        if song.local_path.is_none() {
            return Err(StorageError::Other(format!("add_local_song: {} has no local_path", song.id)));
        }
        let mut s = song.clone();
        s.in_library = true;
        s.video_id = None;
        self.upsert_song(&s)
    }

    /// Delete a song row (cascades to maps, playlist entries and events).
    pub fn delete_song(&self, song_id: &str) -> StorageResult<()> {
        self.conn().execute("DELETE FROM song WHERE id = ?1", params![song_id])?;
        Ok(())
    }

    // -- play events / stats ----------------------------------------------

    /// Record a play: `event` row + `playCount` bump (UTC year/month) +
    /// `totalPlayTime` add. The song must have been upserted first.
    pub fn record_play(&self, song_id: &str, played_secs: f64) -> StorageResult<()> {
        let now = epoch_millis();
        let ms = (played_secs.max(0.0) * 1000.0).round() as i64;
        let (year, month) = year_month_utc(now);
        self.tx(|c| {
            if !song_exists(c, song_id)? {
                return Err(StorageError::Other(format!("record_play: unknown song {song_id}")));
            }
            c.execute("INSERT INTO event (songId, timestamp, playTime) VALUES (?1, ?2, ?3)", params![song_id, now, ms])?;
            c.execute("UPDATE song SET totalPlayTime = totalPlayTime + ?1 WHERE id = ?2", params![ms, song_id])?;
            c.execute(
                "INSERT INTO playCount (song, year, month, count) VALUES (?1, ?2, ?3, 1)
                 ON CONFLICT(song, year, month) DO UPDATE SET count = count + 1",
                params![song_id, year, month],
            )?;
            Ok(())
        })
    }

    /// Recently played (distinct songs, latest `event` first).
    pub fn recent_history(&self, limit: usize) -> StorageResult<Vec<Song>> {
        let conn = self.conn();
        query_songs(
            &conn,
            &format!(
                "SELECT {SONG_COLUMNS} FROM song s
                 JOIN (SELECT songId, MAX(timestamp) AS latest, MAX(id) AS last_id FROM event GROUP BY songId) e ON s.id = e.songId
                 ORDER BY e.latest DESC, e.last_id DESC LIMIT ?1"
            ),
            params![limit as i64],
        )
    }

    /// Most played by lifetime `playCount`, then `totalPlayTime`.
    pub fn most_played(&self, limit: usize) -> StorageResult<Vec<Song>> {
        let conn = self.conn();
        query_songs(
            &conn,
            &format!(
                "SELECT {SONG_COLUMNS} FROM song s
                 JOIN (SELECT song, SUM(count) AS total FROM playCount GROUP BY song) pc ON s.id = pc.song
                 ORDER BY pc.total DESC, s.totalPlayTime DESC LIMIT ?1"
            ),
            params![limit as i64],
        )
    }

    /// Lifetime play count of one song.
    pub fn play_count(&self, song_id: &str) -> StorageResult<u32> {
        let n: i64 = self.conn().query_row(
            "SELECT COALESCE(SUM(count), 0) FROM playCount WHERE song = ?1",
            params![song_id],
            |r| r.get(0),
        )?;
        Ok(n as u32)
    }

    // -- playlists ----------------------------------------------------------

    /// Create a playlist (`LP`+8 letters id), returns its id.
    pub fn create_playlist(&self, name: &str) -> StorageResult<String> {
        let conn = self.conn();
        let now = epoch_millis();
        loop {
            let id = mochy_core::ids::gen_playlist_id();
            let inserted = conn.execute(
                "INSERT OR IGNORE INTO playlist (id, name, createdAt, lastUpdateTime, isEditable, isLocal)
                 VALUES (?1, ?2, ?3, ?3, 1, 1)",
                params![id, name, now],
            )?;
            if inserted == 1 {
                return Ok(id);
            }
        }
    }

    /// Save/bookmark a remote playlist row (id as given, e.g. a YTM id).
    pub fn upsert_playlist(&self, p: &Playlist) -> StorageResult<()> {
        let now = epoch_millis();
        self.conn().execute(
            "INSERT INTO playlist (id, name, browseId, createdAt, lastUpdateTime, isEditable, remoteSongCount, thumbnailUrl, description)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, browseId = COALESCE(excluded.browseId, playlist.browseId),
                lastUpdateTime = excluded.lastUpdateTime, remoteSongCount = COALESCE(excluded.remoteSongCount, playlist.remoteSongCount),
                thumbnailUrl = COALESCE(excluded.thumbnailUrl, playlist.thumbnailUrl),
                description = COALESCE(excluded.description, playlist.description)",
            params![
                p.id,
                p.name,
                p.browse_id,
                if p.created_at > 0 { p.created_at } else { now },
                now,
                p.is_editable,
                (p.song_count > 0).then_some(p.song_count as i64),
                p.thumbnail_url,
                p.description,
            ],
        )?;
        Ok(())
    }

    /// All playlists (skips `LP_LIKED`/`LP_DOWNLOADED`), newest first.
    pub fn playlists(&self) -> StorageResult<Vec<Playlist>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "{PLAYLIST_SELECT} WHERE p.id NOT IN ('{LIKED_PLAYLIST_ID}', '{DOWNLOADED_PLAYLIST_ID}') ORDER BY p.createdAt DESC, p.rowid DESC"
        ))?;
        let list = stmt.query_map([], playlist_from_row)?.collect::<rusqlite::Result<_>>()?;
        Ok(list)
    }

    /// Including special playlists.
    pub fn playlist_by_id(&self, id: &str) -> StorageResult<Option<Playlist>> {
        let conn = self.conn();
        Ok(conn.query_row(&format!("{PLAYLIST_SELECT} WHERE p.id = ?1"), params![id], playlist_from_row).optional()?)
    }

    pub fn rename_playlist(&self, id: &str, name: &str) -> StorageResult<()> {
        self.conn().execute(
            "UPDATE playlist SET name = ?1, lastUpdateTime = ?2 WHERE id = ?3",
            params![name, epoch_millis(), id],
        )?;
        Ok(())
    }

    /// Delete a playlist and (cascade) its song entries; songs stay.
    pub fn delete_playlist(&self, id: &str) -> StorageResult<()> {
        self.conn().execute("DELETE FROM playlist WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Ordered songs of a playlist (`playlist_song_map` by position).
    pub fn playlist_songs(&self, playlist_id: &str) -> StorageResult<Vec<Song>> {
        let conn = self.conn();
        query_songs(
            &conn,
            &format!(
                "SELECT {SONG_COLUMNS} FROM playlist_song_map m JOIN song s ON s.id = m.songId
                 WHERE m.playlistId = ?1 ORDER BY m.position, m.id"
            ),
            params![playlist_id],
        )
    }

    /// Append; upserts the song when unknown, assigns `position` = max+1
    /// (0-based) and stores `setVideoId` when given.
    pub fn add_to_playlist(&self, playlist_id: &str, song: &Song, set_video_id: Option<&str>) -> StorageResult<()> {
        self.tx(|c| {
            if !song_exists(c, &song.id)? {
                upsert_song_in(c, song)?;
            }
            let max: i64 = c.query_row(
                "SELECT COALESCE(MAX(position), -1) FROM playlist_song_map WHERE playlistId = ?1",
                params![playlist_id],
                |r| r.get(0),
            )?;
            c.execute(
                "INSERT INTO playlist_song_map (playlistId, songId, position, setVideoId) VALUES (?1, ?2, ?3, ?4)",
                params![playlist_id, song.id, max + 1, set_video_id],
            )?;
            touch_playlist(c, playlist_id)
        })
    }

    /// Remove every entry of `song_id` from the playlist; positions are
    /// renumbered densely (the preview view relies on `position <= 3`).
    pub fn remove_from_playlist(&self, playlist_id: &str, song_id: &str) -> StorageResult<()> {
        self.tx(|c| {
            c.execute(
                "DELETE FROM playlist_song_map WHERE playlistId = ?1 AND songId = ?2",
                params![playlist_id, song_id],
            )?;
            renumber_playlist(c, playlist_id)?;
            touch_playlist(c, playlist_id)
        })
    }

    /// Move the entry at `from` to `to` (0-based positions).
    pub fn move_playlist_song(&self, playlist_id: &str, from: usize, to: usize) -> StorageResult<()> {
        self.tx(|c| {
            let mut ids: Vec<i64> = c
                .prepare("SELECT id FROM playlist_song_map WHERE playlistId = ?1 ORDER BY position, id")?
                .query_map(params![playlist_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            if from >= ids.len() || to >= ids.len() {
                return Err(StorageError::Other(format!("move_playlist_song: index out of range ({from}->{to}, len {})", ids.len())));
            }
            let id = ids.remove(from);
            ids.insert(to, id);
            for (pos, id) in ids.iter().enumerate() {
                c.execute("UPDATE playlist_song_map SET position = ?1 WHERE id = ?2", params![pos as i64, id])?;
            }
            touch_playlist(c, playlist_id)
        })
    }

    // -- albums / artists ---------------------------------------------------

    /// Insert/update an album; unknown fields never erase known ones.
    pub fn upsert_album(&self, album: &Album) -> StorageResult<()> {
        self.tx(|c| upsert_album_in(c, album))
    }

    /// Cache a whole album page: the album, its tracks in order
    /// (`song_album_map.index`) and the album artists (from the first track).
    pub fn save_album(&self, album: &Album, songs: &[Song]) -> StorageResult<()> {
        self.tx(|c| {
            upsert_album_in(c, album)?;
            for (i, s) in songs.iter().enumerate() {
                let mut s = s.clone();
                s.album_id = Some(album.id.clone());
                if s.album_name.is_none() {
                    s.album_name = Some(album.title.clone());
                }
                upsert_song_in(c, &s)?;
                c.execute(
                    "UPDATE song_album_map SET `index` = ?1 WHERE songId = ?2 AND albumId = ?3",
                    params![i as i64, s.id, album.id],
                )?;
            }
            if let Some(first) = songs.first() {
                for (order, a) in first.artists.iter().enumerate() {
                    c.execute(
                        "INSERT OR IGNORE INTO album_artist_map (albumId, artistId, `order`) VALUES (?1, ?2, ?3)",
                        params![album.id, artist_id_of(a), order as i64],
                    )?;
                }
            }
            Ok(())
        })
    }

    pub fn get_album(&self, id: &str) -> StorageResult<Option<Album>> {
        Ok(self.conn().query_row("SELECT * FROM album WHERE id = ?1", params![id], album_from_row).optional()?)
    }

    pub fn upsert_artist(&self, artist: &Artist) -> StorageResult<()> {
        self.conn().execute(
            "INSERT INTO artist (id, name, thumbnailUrl, channelId, lastUpdateTime) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name,
                thumbnailUrl = COALESCE(excluded.thumbnailUrl, artist.thumbnailUrl),
                channelId = COALESCE(excluded.channelId, artist.channelId),
                lastUpdateTime = excluded.lastUpdateTime",
            params![artist.id, artist.name, artist.thumbnail_url, artist.channel_id, epoch_millis()],
        )?;
        Ok(())
    }

    pub fn get_artist(&self, id: &str) -> StorageResult<Option<Artist>> {
        Ok(self.conn().query_row("SELECT * FROM artist WHERE id = ?1", params![id], artist_from_row).optional()?)
    }

    /// Albums present in the library (from `song_album_map` joins).
    pub fn library_albums(&self) -> StorageResult<Vec<Album>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT a.* FROM album a
             JOIN song_album_map sam ON a.id = sam.albumId
             JOIN song s ON s.id = sam.songId
             WHERE s.inLibrary IS NOT NULL
             ORDER BY a.title COLLATE NOCASE",
        )?;
        let v = stmt.query_map([], album_from_row)?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// Artists present in the library.
    pub fn library_artists(&self) -> StorageResult<Vec<Artist>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT a.* FROM artist a
             JOIN song_artist_map sam ON a.id = sam.artistId
             JOIN song s ON s.id = sam.songId
             WHERE s.inLibrary IS NOT NULL
             ORDER BY a.name COLLATE NOCASE",
        )?;
        let v = stmt.query_map([], artist_from_row)?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// Songs of one album in track order (`song_album_map.index`).
    pub fn album_songs(&self, album_id: &str) -> StorageResult<Vec<Song>> {
        let conn = self.conn();
        query_songs(
            &conn,
            &format!(
                "SELECT {SONG_COLUMNS} FROM song_album_map sam JOIN song s ON s.id = sam.songId
                 WHERE sam.albumId = ?1 ORDER BY sam.`index`, s.rowid"
            ),
            params![album_id],
        )
    }

    /// Songs of one artist (any library state), most played first.
    pub fn artist_songs(&self, artist_id: &str) -> StorageResult<Vec<Song>> {
        let conn = self.conn();
        query_songs(
            &conn,
            &format!(
                "SELECT {SONG_COLUMNS} FROM song_artist_map m JOIN song s ON s.id = m.songId
                 WHERE m.artistId = ?1 ORDER BY s.totalPlayTime DESC, s.title"
            ),
            params![artist_id],
        )
    }

    // -- format / lyrics / misc ----------------------------------------------

    /// Cache stream format info (Room table `format`, PK = song id). Room's
    /// NOT NULL columns get 0 for unknown values.
    #[allow(clippy::too_many_arguments)]
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
        self.conn().execute(
            "INSERT INTO format (id, itag, mimeType, codecs, bitrate, sampleRate, contentLength, loudnessDb)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET itag = excluded.itag, mimeType = excluded.mimeType, codecs = excluded.codecs,
                bitrate = excluded.bitrate, sampleRate = excluded.sampleRate, contentLength = excluded.contentLength,
                loudnessDb = excluded.loudnessDb",
            params![
                song_id,
                itag.unwrap_or(0),
                mime_type,
                codecs,
                bitrate.unwrap_or(0),
                sample_rate,
                content_length.unwrap_or(0),
                loudness_db
            ],
        )?;
        Ok(())
    }

    /// Cached format of a song.
    pub fn load_format(&self, song_id: &str) -> StorageResult<Option<FormatInfo>> {
        let nz = |v: i64| (v != 0).then_some(v);
        Ok(self
            .conn()
            .query_row(
                "SELECT itag, mimeType, codecs, bitrate, sampleRate, contentLength, loudnessDb, perceptualLoudnessDb, playbackUrl
                 FROM format WHERE id = ?1",
                params![song_id],
                |r| {
                    Ok(FormatInfo {
                        itag: nz(r.get(0)?),
                        mime_type: r.get(1)?,
                        codecs: r.get(2)?,
                        bitrate: nz(r.get(3)?),
                        sample_rate: r.get(4)?,
                        content_length: nz(r.get(5)?),
                        loudness_db: r.get(6)?,
                        perceptual_loudness_db: r.get(7)?,
                        playback_url: r.get(8)?,
                    })
                },
            )
            .optional()?)
    }

    /// Upsert lyrics for a video id (Room table `lyrics`); keeps any stored
    /// translation columns.
    pub fn save_lyrics(&self, video_id: &str, provider: &str, lyrics: &str) -> StorageResult<()> {
        self.conn().execute(
            "INSERT INTO lyrics (id, lyrics, provider) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET lyrics = excluded.lyrics, provider = excluded.provider",
            params![video_id, lyrics, provider],
        )?;
        Ok(())
    }

    /// Remember that no provider has lyrics for this track (Android's
    /// `LYRICS_NOT_FOUND` sentinel) so lookups aren't repeated.
    pub fn save_lyrics_not_found(&self, video_id: &str) -> StorageResult<()> {
        self.save_lyrics(video_id, "Unknown", LYRICS_NOT_FOUND)
    }

    /// True when the cache holds the not-found sentinel for this track.
    pub fn lyrics_not_found(&self, video_id: &str) -> StorageResult<bool> {
        let v: Option<String> = self
            .conn()
            .query_row("SELECT lyrics FROM lyrics WHERE id = ?1", params![video_id], |r| r.get(0))
            .optional()?;
        Ok(v.as_deref() == Some(LYRICS_NOT_FOUND))
    }

    /// (provider, lyrics) or None. Sentinel `LYRICS_NOT_FOUND` is returned as
    /// None for both missing and sentinel rows.
    pub fn load_lyrics(&self, video_id: &str) -> StorageResult<Option<(String, String)>> {
        let row: Option<(String, String)> = self
            .conn()
            .query_row("SELECT provider, lyrics FROM lyrics WHERE id = ?1", params![video_id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        Ok(row.filter(|(p, l)| l != LYRICS_NOT_FOUND && p != LYRICS_NOT_FOUND && !l.is_empty()))
    }

    /// Search history (unique `query`, newest first, trimmed to 100).
    pub fn add_search_history(&self, query: &str) -> StorageResult<()> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(());
        }
        self.tx(|c| {
            c.execute("DELETE FROM search_history WHERE query = ?1", params![q])?;
            c.execute("INSERT INTO search_history (query) VALUES (?1)", params![q])?;
            c.execute(
                "DELETE FROM search_history WHERE id NOT IN (SELECT id FROM search_history ORDER BY id DESC LIMIT ?1)",
                params![SEARCH_HISTORY_MAX],
            )?;
            Ok(())
        })
    }

    pub fn search_history(&self, limit: usize) -> StorageResult<Vec<String>> {
        self.search_history_prefix("", limit)
    }

    /// History entries starting with `prefix` (Kotlin `searchHistory(query)`).
    pub fn search_history_prefix(&self, prefix: &str, limit: usize) -> StorageResult<Vec<String>> {
        let conn = self.conn();
        let escaped = prefix.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        let mut stmt =
            conn.prepare("SELECT query FROM search_history WHERE query LIKE ?1 || '%' ESCAPE '\\' ORDER BY id DESC LIMIT ?2")?;
        let v = stmt.query_map(params![escaped, limit as i64], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    pub fn remove_search_history(&self, query: &str) -> StorageResult<()> {
        self.conn().execute("DELETE FROM search_history WHERE query = ?1", params![query])?;
        Ok(())
    }

    pub fn clear_search_history(&self) -> StorageResult<()> {
        self.conn().execute("DELETE FROM search_history", [])?;
        Ok(())
    }

    /// Persist a queue for `persistent_queue` restore.
    pub fn save_queue(&self, items: &[QueueItem], index: usize) -> StorageResult<()> {
        let json = serde_json::to_string(items)?;
        self.conn().execute(
            "INSERT INTO _queue (id, json, current_index) VALUES (0, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET json = excluded.json, current_index = excluded.current_index",
            params![json, index as i64],
        )?;
        Ok(())
    }

    /// Restored queue + index (None when absent). The index is clamped to
    /// the queue length.
    pub fn load_queue(&self) -> StorageResult<Option<(Vec<QueueItem>, usize)>> {
        let row: Option<(String, i64)> = self
            .conn()
            .query_row("SELECT json, current_index FROM _queue WHERE id = 0", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let Some((json, idx)) = row else { return Ok(None) };
        let items: Vec<QueueItem> = serde_json::from_str(&json)?;
        let idx = (idx.max(0) as usize).min(items.len().saturating_sub(1));
        Ok(Some((items, idx)))
    }

    pub fn clear_queue(&self) -> StorageResult<()> {
        self.conn().execute("DELETE FROM _queue", [])?;
        Ok(())
    }

    /// Escape hatch for read-only diagnostics/tests: run `f` with the raw
    /// connection (holding the lock).
    pub fn with_connection<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> StorageResult<T> {
        Ok(f(&self.conn())?)
    }
}

// ---------------------------------------------------------------------------
// Row helpers
// ---------------------------------------------------------------------------

const PLAYLIST_SELECT: &str = "SELECT p.*, (SELECT COUNT(*) FROM playlist_song_map m WHERE m.playlistId = p.id) AS localCount FROM playlist p";

fn playlist_from_row(row: &Row<'_>) -> rusqlite::Result<Playlist> {
    let remote: Option<i64> = row.get("remoteSongCount")?;
    let local: i64 = row.get("localCount")?;
    Ok(Playlist {
        id: row.get("id")?,
        name: row.get("name")?,
        browse_id: row.get("browseId")?,
        thumbnail_url: row.get("thumbnailUrl")?,
        song_count: if local > 0 { local as u32 } else { remote.unwrap_or(0) as u32 },
        description: row.get("description")?,
        is_editable: row.get("isEditable")?,
        created_at: row.get::<_, Option<i64>>("createdAt")?.unwrap_or(0),
    })
}

fn album_from_row(row: &Row<'_>) -> rusqlite::Result<Album> {
    let song_count: i64 = row.get("songCount")?;
    let duration: i64 = row.get("duration")?;
    Ok(Album {
        id: row.get("id")?,
        playlist_id: row.get("playlistId")?,
        title: row.get("title")?,
        year: row.get("year")?,
        thumbnail_url: row.get("thumbnailUrl")?,
        song_count: (song_count > 0).then_some(song_count as u32),
        duration_secs: (duration > 0).then_some(duration as f64),
        description: row.get("description")?,
    })
}

fn artist_from_row(row: &Row<'_>) -> rusqlite::Result<Artist> {
    Ok(Artist {
        id: row.get("id")?,
        name: row.get("name")?,
        thumbnail_url: row.get("thumbnailUrl")?,
        channel_id: row.get("channelId")?,
    })
}

// ---------------------------------------------------------------------------
// Settings store: AppSettings as pretty JSON at `settings_path()`.
// ---------------------------------------------------------------------------

/// What [`SettingsStore::load_report`] had to do to produce settings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoadReport {
    /// The file did not exist (defaults returned).
    pub missing: bool,
    /// The file was not valid JSON; it was moved to `backup`.
    pub corrupted: bool,
    /// Dotted paths of values that were rejected (wrong type / unknown enum
    /// value) and replaced by defaults.
    pub rejected: Vec<String>,
    /// Where the original file was copied before anything was dropped.
    pub backup: Option<PathBuf>,
}

pub struct SettingsStore {
    path: PathBuf,
}

/// Merge `loaded` onto `base` key by key: objects recurse, anything else
/// replaces. Keys unknown to `base` are dropped.
fn merge_known(base: &Value, loaded: &Value) -> Value {
    match (base, loaded) {
        (Value::Object(b), Value::Object(l)) => {
            let mut out = b.clone();
            for (k, bv) in b {
                if let Some(lv) = l.get(k) {
                    out.insert(k.clone(), merge_known(bv, lv));
                }
            }
            Value::Object(out)
        }
        (_, l) => l.clone(),
    }
}

/// Leaf paths of `loaded` that also exist in `base` (objects recurse when
/// both sides are objects).
fn leaf_paths(base: &Value, loaded: &Value, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    match (base, loaded) {
        (Value::Object(b), Value::Object(l)) => {
            for (k, bv) in b {
                if let Some(lv) = l.get(k) {
                    prefix.push(k.clone());
                    leaf_paths(bv, lv, prefix, out);
                    prefix.pop();
                }
            }
        }
        _ => out.push(prefix.clone()),
    }
}

fn get_path<'a>(v: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(v, |cur, k| cur.get(k))
}

fn set_path(v: &mut Value, path: &[String], new: Value) {
    let mut cur = v;
    for k in &path[..path.len() - 1] {
        cur = &mut cur[k.as_str()];
    }
    cur[path[path.len() - 1].as_str()] = new;
}

/// Deserialize settings leniently: every value that deserializes is kept,
/// every value that doesn't is replaced by its default (reported in
/// `rejected`). Missing values default via `#[serde(default)]`.
fn lenient_settings(loaded: &Value) -> (AppSettings, Vec<String>) {
    let default_val = serde_json::to_value(AppSettings::default()).expect("settings serialize");
    let merged = merge_known(&default_val, loaded);
    if let Ok(s) = serde_json::from_value::<AppSettings>(merged) {
        return (s, Vec::new());
    }
    let mut paths = Vec::new();
    leaf_paths(&default_val, loaded, &mut Vec::new(), &mut paths);
    let mut acc = default_val;
    let mut rejected = Vec::new();
    for path in paths {
        let Some(new) = get_path(loaded, &path) else { continue };
        let mut trial = acc.clone();
        set_path(&mut trial, &path, new.clone());
        if serde_json::from_value::<AppSettings>(trial.clone()).is_ok() {
            acc = trial;
        } else {
            rejected.push(path.join("."));
        }
    }
    let settings = serde_json::from_value(acc).unwrap_or_default();
    (settings, rejected)
}

impl SettingsStore {
    pub fn open(path: PathBuf) -> Self {
        Self { path }
    }

    /// Default path `settings_path()`.
    pub fn open_default() -> Self {
        Self { path: settings_path() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load settings; never fails on content: a missing file gives defaults,
    /// older/partial files fill in defaults, invalid values fall back to
    /// their defaults, and an unparseable file is backed up
    /// (`settings.json.corrupt-<millis>`) and replaced by defaults. Only I/O
    /// errors (e.g. permission denied) are returned as `Err`.
    pub fn load(&self) -> StorageResult<AppSettings> {
        self.load_report().map(|(s, _)| s)
    }

    /// [`SettingsStore::load`] plus a report of what was repaired.
    pub fn load_report(&self) -> StorageResult<(AppSettings, LoadReport)> {
        let mut report = LoadReport::default();
        let data = match std::fs::read(&self.path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                report.missing = true;
                return Ok((AppSettings::default(), report));
            }
            Err(e) => return Err(e.into()),
        };
        let text = String::from_utf8_lossy(&data);
        let text = text.trim_start_matches('\u{feff}');
        if text.trim().is_empty() {
            return Ok((AppSettings::default(), report));
        }
        let value: Value = match serde_json::from_str(text) {
            Ok(v @ Value::Object(_)) => v,
            _ => {
                report.corrupted = true;
                report.backup = Some(self.backup("corrupt")?);
                return Ok((AppSettings::default(), report));
            }
        };
        let (settings, rejected) = lenient_settings(&value);
        if !rejected.is_empty() {
            report.backup = Some(self.backup("bak")?);
        }
        report.rejected = rejected;
        Ok((settings, report))
    }

    fn backup(&self, tag: &str) -> StorageResult<PathBuf> {
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "settings.json".into());
        let dest = self.path.with_file_name(format!("{name}.{tag}-{}", epoch_millis()));
        std::fs::copy(&self.path, &dest)?;
        Ok(dest)
    }

    /// Atomic write: pretty JSON to a unique temp file next to the target,
    /// flushed to disk, then renamed over it (rename replaces on Windows too).
    pub fn save(&self, settings: &AppSettings) -> StorageResult<()> {
        use std::io::Write;
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let json = serde_json::to_string_pretty(settings)?;
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "settings.json".into());
        let tmp = self.path.with_file_name(format!(
            ".{name}.{}.{}.tmp",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| -> StorageResult<()> {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(json.as_bytes())?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, &self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn year_month_from_epoch() {
        assert_eq!(year_month_utc(0), (1970, 1));
        assert_eq!(year_month_utc(951_782_400_000), (2000, 2)); // 2000-02-29
        assert_eq!(year_month_utc(1_794_796_661_000), (2026, 11));
        assert_eq!(year_month_utc(1_704_067_199_000), (2023, 12)); // 2023-12-31 23:59:59
    }

    #[test]
    fn local_artist_ids_are_stable() {
        let a = local_artist_id("Sơn Tùng M-TP");
        assert_eq!(a, local_artist_id(" sơn tùng m-tp "));
        assert!(a.starts_with("LA") && a.len() == 10);
        assert_ne!(a, local_artist_id("Imagine Dragons"));
    }
}
