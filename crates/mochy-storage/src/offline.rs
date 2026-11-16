//! Downloads (desktop table `_download`) and hand edits of songs (Room table
//! `song_edit`, Android `SongEdit` / `SongEditSaver`).
//!
//! `_download` is desktop-only (like `_queue`): Android keeps its downloads
//! in ExoPlayer's download index, the desktop keeps plain files plus one row
//! per song here. The Room-compatible `song.isDownloaded` / `dateDownload`
//! columns are kept in step. Additive migration only (`CREATE TABLE IF NOT
//! EXISTS`).

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{Db, StorageResult, epoch_millis, local_artist_id};

const DDL: &str = "CREATE TABLE IF NOT EXISTS `_download` (\
    `songId` TEXT NOT NULL, \
    `kind` INTEGER NOT NULL DEFAULT 0, \
    `state` TEXT NOT NULL, \
    `songJson` TEXT NOT NULL DEFAULT '{}', \
    `filePath` TEXT, \
    `bytes` INTEGER NOT NULL DEFAULT 0, \
    `totalBytes` INTEGER NOT NULL DEFAULT 0, \
    `error` TEXT, \
    `createdAt` INTEGER NOT NULL DEFAULT 0, \
    `updatedAt` INTEGER NOT NULL DEFAULT 0, \
    `lastUsedAt` INTEGER NOT NULL DEFAULT 0, \
    PRIMARY KEY(`songId`));";

/// Create the desktop download table (idempotent).
pub(crate) fn migrate(conn: &Connection) -> StorageResult<()> {
    conn.execute_batch(DDL)?;
    Ok(())
}

/// `kind` column: a user download, or a song kept by the stream cache.
pub const KIND_DOWNLOAD: i64 = 0;
pub const KIND_CACHE: i64 = 1;

/// One `_download` row. `state` is the app's status name
/// (`queued`/`running`/`done`/`failed`/`cancelled`); `song_json` the song
/// as the app knew it (serde JSON of `mochy_core::models::Song`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DownloadRow {
    pub song_id: String,
    pub kind: i64,
    pub state: String,
    pub song_json: String,
    pub file_path: Option<String>,
    pub bytes: i64,
    pub total_bytes: i64,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_used_at: i64,
}

/// An artist picked in the edit sheet (Android `EditedArtist`, stored as a
/// JSON list in `song_edit.artists`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditedArtist {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub thumbnail: Option<String>,
}

/// What someone changed on a song by hand (Room `song_edit`); `None` fields
/// follow YouTube, set ones survive every refetch.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SongEdit {
    pub song_id: String,
    pub title: Option<String>,
    pub artists: Option<Vec<EditedArtist>>,
    pub album: Option<String>,
    pub year: Option<i32>,
    /// A cover link, or a `file://` picture the user picked.
    pub cover: Option<String>,
    pub video_id: Option<String>,
    pub no_video: bool,
}

fn decode_artists(text: Option<String>) -> Option<Vec<EditedArtist>> {
    // a broken row reads as no edit rather than taking the song's artists away
    text.and_then(|t| serde_json::from_str::<Vec<EditedArtist>>(&t).ok()).filter(|v| !v.is_empty())
}

fn edit_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SongEdit> {
    Ok(SongEdit {
        song_id: r.get(0)?,
        title: r.get(1)?,
        artists: decode_artists(r.get(2)?),
        album: r.get(3)?,
        year: r.get(4)?,
        cover: r.get(5)?,
        video_id: r.get(6)?,
        no_video: r.get(7)?,
    })
}

const EDIT_COLUMNS: &str = "songId, title, artists, album, year, cover, videoId, noVideo";

fn load_edit(conn: &Connection, song_id: &str) -> StorageResult<Option<SongEdit>> {
    Ok(conn
        .query_row(&format!("SELECT {EDIT_COLUMNS} FROM song_edit WHERE songId = ?1"), params![song_id], edit_from_row)
        .optional()?)
}

/// Lay an edit over the song row (title, album, year, cover, artists).
fn apply_edit_in(conn: &Connection, e: &SongEdit) -> StorageResult<()> {
    conn.execute(
        "UPDATE song SET title = COALESCE(?2, title), albumName = COALESCE(?3, albumName),
                         year = COALESCE(?4, year), thumbnailUrl = COALESCE(?5, thumbnailUrl)
         WHERE id = ?1",
        params![e.song_id, e.title, e.album, e.year, e.cover],
    )?;
    if let Some(artists) = &e.artists {
        let exists: bool = conn.query_row("SELECT 1 FROM song WHERE id = ?1", params![e.song_id], |_| Ok(())).optional()?.is_some();
        if exists {
            conn.execute("DELETE FROM song_artist_map WHERE songId = ?1", params![e.song_id])?;
            let now = epoch_millis();
            for (pos, a) in artists.iter().enumerate() {
                let id = if a.id.is_empty() { local_artist_id(&a.name) } else { a.id.clone() };
                // an artist already in the library keeps its own photo and details
                conn.execute(
                    "INSERT INTO artist (id, name, thumbnailUrl, channelId, lastUpdateTime, isLocal) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(id) DO NOTHING",
                    params![id, a.name, a.thumbnail, id.starts_with("UC").then_some(id.clone()), now, !id.starts_with("UC")],
                )?;
                conn.execute(
                    "INSERT OR IGNORE INTO song_artist_map (songId, artistId, position) VALUES (?1, ?2, ?3)",
                    params![e.song_id, id, pos as i64],
                )?;
            }
        }
    }
    Ok(())
}

/// Re-apply a stored edit after the song row was refreshed from YouTube
/// (called at the end of every song upsert), so a refetch can't undo it.
pub(crate) fn reapply_song_edit_in(conn: &Connection, song_id: &str) -> StorageResult<()> {
    if let Some(e) = load_edit(conn, song_id)? {
        apply_edit_in(conn, &e)?;
    }
    Ok(())
}

fn download_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DownloadRow> {
    Ok(DownloadRow {
        song_id: r.get(0)?,
        kind: r.get(1)?,
        state: r.get(2)?,
        song_json: r.get(3)?,
        file_path: r.get(4)?,
        bytes: r.get(5)?,
        total_bytes: r.get(6)?,
        error: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
        last_used_at: r.get(10)?,
    })
}

impl Db {
    // -- downloads ----------------------------------------------------------

    pub fn upsert_download(&self, row: &DownloadRow) -> StorageResult<()> {
        self.conn().execute(
            "INSERT INTO _download (songId, kind, state, songJson, filePath, bytes, totalBytes, error, createdAt, updatedAt, lastUsedAt)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(songId) DO UPDATE SET kind = excluded.kind, state = excluded.state, songJson = excluded.songJson,
                filePath = excluded.filePath, bytes = excluded.bytes, totalBytes = excluded.totalBytes, error = excluded.error,
                updatedAt = excluded.updatedAt, lastUsedAt = MAX(_download.lastUsedAt, excluded.lastUsedAt)",
            params![
                row.song_id,
                row.kind,
                row.state,
                row.song_json,
                row.file_path,
                row.bytes,
                row.total_bytes,
                row.error,
                row.created_at,
                row.updated_at,
                row.last_used_at
            ],
        )?;
        Ok(())
    }

    /// Every download row, oldest first.
    pub fn downloads(&self) -> StorageResult<Vec<DownloadRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT songId, kind, state, songJson, filePath, bytes, totalBytes, error, createdAt, updatedAt, lastUsedAt
             FROM _download ORDER BY createdAt, songId",
        )?;
        let rows = stmt.query_map([], download_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn delete_download(&self, song_id: &str) -> StorageResult<()> {
        self.conn().execute("DELETE FROM _download WHERE songId = ?1", params![song_id])?;
        Ok(())
    }

    /// Played from disk just now (stream-cache LRU).
    pub fn touch_download(&self, song_id: &str, at: i64) -> StorageResult<()> {
        self.conn().execute("UPDATE _download SET lastUsedAt = MAX(lastUsedAt, ?2) WHERE songId = ?1", params![song_id, at])?;
        Ok(())
    }

    /// Room `song.isDownloaded` / `dateDownload` (Android
    /// `updateDownloadedInfo`).
    pub fn set_song_downloaded(&self, song_id: &str, downloaded: bool) -> StorageResult<()> {
        let at = downloaded.then(epoch_millis);
        self.conn().execute(
            "UPDATE song SET isDownloaded = ?2, dateDownload = ?3 WHERE id = ?1",
            params![song_id, downloaded, at],
        )?;
        Ok(())
    }

    pub fn song_is_downloaded(&self, song_id: &str) -> StorageResult<bool> {
        let v: Option<bool> =
            self.conn().query_row("SELECT isDownloaded FROM song WHERE id = ?1", params![song_id], |r| r.get(0)).optional()?;
        Ok(v.unwrap_or(false))
    }

    // -- song edits -----------------------------------------------------------

    pub fn song_edit(&self, song_id: &str) -> StorageResult<Option<SongEdit>> {
        load_edit(&self.conn(), song_id)
    }

    /// The whole (small) table, for the in-memory overlay.
    pub fn song_edits(&self) -> StorageResult<Vec<SongEdit>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!("SELECT {EDIT_COLUMNS} FROM song_edit"))?;
        let rows = stmt.query_map([], edit_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Store an edit and write it into the song row itself, so every list
    /// shows it (Android `SongEditSaver.save`). The song row should exist.
    pub fn save_song_edit(&self, edit: &SongEdit) -> StorageResult<()> {
        self.tx(|c| {
            let artists = edit.artists.as_ref().map(serde_json::to_string).transpose()?;
            c.execute(
                "INSERT INTO song_edit (songId, title, artists, album, year, cover, videoId, noVideo)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(songId) DO UPDATE SET title = excluded.title, artists = excluded.artists, album = excluded.album,
                    year = excluded.year, cover = excluded.cover, videoId = excluded.videoId, noVideo = excluded.noVideo",
                params![edit.song_id, edit.title, artists, edit.album, edit.year, edit.cover, edit.video_id, edit.no_video],
            )?;
            apply_edit_in(c, edit)
        })
    }

    /// Forget an edit (the song row keeps its current values until the next
    /// refresh from YouTube).
    pub fn delete_song_edit(&self, song_id: &str) -> StorageResult<()> {
        self.conn().execute("DELETE FROM song_edit WHERE songId = ?1", params![song_id])?;
        Ok(())
    }
}
