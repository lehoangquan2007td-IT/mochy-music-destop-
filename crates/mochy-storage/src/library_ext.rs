//! Library parity and listening-stats queries (additive: the v43 schema is
//! unchanged — saved albums/artists/playlists use Room's `bookmarkedAt`
//! columns, stats read the `event` table like Android's `RecapRepository`).
//!
//! Event timestamps are true epoch milliseconds (desktop `record_play`);
//! hour/weekday/day buckets are computed in the machine's local time by
//! SQLite (`'localtime'` modifier), so DST is handled per event.

use std::collections::{HashMap, HashSet};

use mochy_core::models::{Album, Artist, ArtistRef, Playlist, ShelfItem, Song};
use rusqlite::{OptionalExtension, params};

use crate::{
    Db, LIKED_PLAYLIST_ID, DOWNLOADED_PLAYLIST_ID, PLAYLIST_SELECT, SONG_COLUMNS, StorageError, StorageResult, album_from_row,
    artist_from_row, epoch_millis, playlist_from_row, query_songs, song_exists, upsert_album_in, year_month_utc,
};

/// One play of the listening history (an `event` row).
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub event_id: i64,
    /// Epoch millis.
    pub timestamp: i64,
    pub play_time_ms: i64,
    pub song: Song,
}

/// An album of the library: saved (bookmarked) or holding library songs.
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumEntry {
    pub album: Album,
    /// Album artists ("A, B"), from `album_artist_map` or the first track.
    pub artists: String,
    /// `bookmarkedAt` (epoch ms) when saved.
    pub saved_at: Option<i64>,
    /// When it entered the library (bookmark or earliest library song).
    pub added_at: i64,
    /// Number of library songs of this album.
    pub library_songs: u32,
    /// Listening time over all plays (ms).
    pub play_time_ms: i64,
}

/// An artist of the library: saved (bookmarked) or with library songs.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtistEntry {
    pub artist: Artist,
    pub saved_at: Option<i64>,
    pub added_at: i64,
    /// Library songs by this artist.
    pub song_count: u32,
    pub play_time_ms: i64,
}

/// Ids the user saved, for "Save / Remove from library" menus.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SavedIds {
    pub albums: HashSet<String>,
    pub artists: HashSet<String>,
    /// Remote playlists: playlist ids *and* their browse ids.
    pub playlists: HashSet<String>,
}

/// One entry of "Recently added".
#[derive(Debug, Clone)]
pub struct RecentItem {
    /// Epoch millis the item was added/saved.
    pub at: i64,
    pub item: ShelfItem,
}

/// Period selector of the Stats page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatsPeriod {
    /// Monday-based calendar week.
    Week,
    Month,
    Year,
    All,
}

/// Totals of one period.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatsSummary {
    pub total_ms: i64,
    pub plays: u32,
    pub songs: u32,
    pub artists: u32,
    pub albums: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatSong {
    pub song: Song,
    pub plays: u32,
    pub ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatArtist {
    pub artist: Artist,
    pub plays: u32,
    pub ms: i64,
    /// Artwork of their most played song (artists rarely have thumbnails cached).
    pub fallback_thumbnail: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatAlbum {
    pub album: Album,
    pub artist: String,
    pub plays: u32,
    pub ms: i64,
}

/// Listening time per local hour / weekday / day.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListeningBuckets {
    /// ms per hour of the day, 0..23.
    pub by_hour: [i64; 24],
    /// ms per weekday, Monday first.
    pub by_weekday: [i64; 7],
    /// `(YYYY-MM-DD local, ms)` for each day that has plays, ascending.
    pub by_day: Vec<(String, i64)>,
}

/// SQL expression: an epoch-ms column as SQLite local time.
fn local(col: &str) -> String {
    format!("{col} / 1000, 'unixepoch', 'localtime'")
}

impl Db {
    // -- history ------------------------------------------------------------

    /// Record a play at a given time (imports, tests). Same bookkeeping as
    /// [`Db::record_play`].
    pub fn record_play_at(&self, song_id: &str, played_secs: f64, timestamp_ms: i64) -> StorageResult<()> {
        let ms = (played_secs.max(0.0) * 1000.0).round() as i64;
        let (year, month) = year_month_utc(timestamp_ms);
        self.tx(|c| {
            if !song_exists(c, song_id)? {
                return Err(StorageError::Other(format!("record_play_at: unknown song {song_id}")));
            }
            c.execute("INSERT INTO event (songId, timestamp, playTime) VALUES (?1, ?2, ?3)", params![song_id, timestamp_ms, ms])?;
            c.execute("UPDATE song SET totalPlayTime = totalPlayTime + ?1 WHERE id = ?2", params![ms, song_id])?;
            c.execute(
                "INSERT INTO playCount (song, year, month, count) VALUES (?1, ?2, ?3, 1)
                 ON CONFLICT(song, year, month) DO UPDATE SET count = count + 1",
                params![song_id, year, month],
            )?;
            Ok(())
        })
    }

    /// The newest `limit` plays (one entry per event), newest first.
    pub fn history_events(&self, limit: usize) -> StorageResult<Vec<HistoryEntry>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id, songId, timestamp, playTime FROM event ORDER BY timestamp DESC, id DESC LIMIT ?1")?;
        let rows: Vec<(i64, String, i64, i64)> = stmt
            .query_map(params![limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        let mut cache: HashMap<String, Song> = HashMap::new();
        let mut out = Vec::with_capacity(rows.len());
        for (event_id, song_id, timestamp, play_time_ms) in rows {
            let song = match cache.get(&song_id) {
                Some(s) => s.clone(),
                None => {
                    let Some(s) =
                        query_songs(&conn, &format!("SELECT {SONG_COLUMNS} FROM song s WHERE s.id = ?1"), params![song_id])?.pop()
                    else {
                        continue;
                    };
                    cache.insert(song_id.clone(), s.clone());
                    s
                }
            };
            out.push(HistoryEntry { event_id, timestamp, play_time_ms, song });
        }
        Ok(out)
    }

    /// Remove one play from the history ("Remove from history").
    pub fn delete_event(&self, event_id: i64) -> StorageResult<()> {
        self.conn().execute("DELETE FROM event WHERE id = ?1", params![event_id])?;
        Ok(())
    }

    /// Remove every play of a song from the history.
    pub fn delete_song_history(&self, song_id: &str) -> StorageResult<()> {
        self.conn().execute("DELETE FROM event WHERE songId = ?1", params![song_id])?;
        Ok(())
    }

    /// Clear the whole listening history (Kotlin `clearListenHistory`).
    pub fn clear_history(&self) -> StorageResult<()> {
        self.conn().execute("DELETE FROM event", [])?;
        Ok(())
    }

    // -- saved albums / artists / playlists ----------------------------------

    /// Save (bookmark) or unsave an album. Saving stores the album row and
    /// its artists so the library can list it offline.
    pub fn set_album_saved(&self, album: &Album, artists: &[ArtistRef], saved: bool) -> StorageResult<()> {
        let now = epoch_millis();
        self.tx(|c| {
            upsert_album_in(c, album)?;
            if saved {
                c.execute("UPDATE album SET bookmarkedAt = COALESCE(bookmarkedAt, ?1) WHERE id = ?2", params![now, album.id])?;
                for (order, a) in artists.iter().enumerate() {
                    let Some(aid) = a.id.clone().filter(|s| !s.is_empty()) else { continue };
                    c.execute(
                        "INSERT INTO artist (id, name, lastUpdateTime) VALUES (?1, ?2, ?3)
                         ON CONFLICT(id) DO UPDATE SET name = CASE WHEN excluded.name != '' THEN excluded.name ELSE artist.name END",
                        params![aid, a.name, now],
                    )?;
                    c.execute(
                        "INSERT OR IGNORE INTO album_artist_map (albumId, artistId, `order`) VALUES (?1, ?2, ?3)",
                        params![album.id, aid, order as i64],
                    )?;
                }
            } else {
                c.execute("UPDATE album SET bookmarkedAt = NULL WHERE id = ?1", params![album.id])?;
            }
            Ok(())
        })
    }

    /// Save (bookmark / "subscribe") or unsave an artist.
    pub fn set_artist_saved(&self, artist: &Artist, saved: bool) -> StorageResult<()> {
        self.upsert_artist(artist)?;
        let at = saved.then(epoch_millis);
        self.conn().execute(
            "UPDATE artist SET bookmarkedAt = CASE WHEN ?1 IS NULL THEN NULL ELSE COALESCE(bookmarkedAt, ?1) END WHERE id = ?2",
            params![at, artist.id],
        )?;
        Ok(())
    }

    /// Save a remote (YouTube Music) playlist to the library, or remove it.
    /// Unsaving deletes the row unless it holds local songs.
    pub fn set_playlist_saved(&self, p: &Playlist, saved: bool) -> StorageResult<()> {
        if saved {
            let mut row = p.clone();
            row.is_editable = false;
            self.upsert_playlist(&row)?;
            self.conn().execute(
                "UPDATE playlist SET bookmarkedAt = COALESCE(bookmarkedAt, ?1), isLocal = 0 WHERE id = ?2",
                params![epoch_millis(), p.id],
            )?;
        } else {
            let conn = self.conn();
            let local: i64 =
                conn.query_row("SELECT COUNT(*) FROM playlist_song_map WHERE playlistId = ?1", params![p.id], |r| r.get(0))?;
            if local == 0 {
                conn.execute("DELETE FROM playlist WHERE id = ?1 AND isEditable = 0", params![p.id])?;
            }
            conn.execute("UPDATE playlist SET bookmarkedAt = NULL WHERE id = ?1", params![p.id])?;
        }
        Ok(())
    }

    /// Ids of every saved album / artist / remote playlist.
    pub fn saved_ids(&self) -> StorageResult<SavedIds> {
        let conn = self.conn();
        let col = |sql: &str| -> StorageResult<HashSet<String>> {
            let mut stmt = conn.prepare(sql)?;
            let v = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?;
            Ok(v)
        };
        let albums = col("SELECT id FROM album WHERE bookmarkedAt IS NOT NULL")?;
        let artists = col("SELECT id FROM artist WHERE bookmarkedAt IS NOT NULL")?;
        let mut playlists = col("SELECT id FROM playlist WHERE bookmarkedAt IS NOT NULL")?;
        playlists.extend(col("SELECT browseId FROM playlist WHERE bookmarkedAt IS NOT NULL AND browseId IS NOT NULL")?);
        Ok(SavedIds { albums, artists, playlists })
    }

    /// Library albums: saved ones plus albums of library songs.
    pub fn library_album_entries(&self) -> StorageResult<Vec<AlbumEntry>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT a.*,
                (SELECT COUNT(*) FROM song_album_map m JOIN song s ON s.id = m.songId
                  WHERE m.albumId = a.id AND s.inLibrary IS NOT NULL) AS libSongs,
                (SELECT MIN(s.inLibrary) FROM song_album_map m JOIN song s ON s.id = m.songId
                  WHERE m.albumId = a.id AND s.inLibrary IS NOT NULL) AS firstAdded,
                (SELECT COALESCE(SUM(s.totalPlayTime), 0) FROM song_album_map m JOIN song s ON s.id = m.songId
                  WHERE m.albumId = a.id) AS playMs,
                (SELECT group_concat(name, ', ') FROM (SELECT ar.name FROM album_artist_map aam JOIN artist ar ON ar.id = aam.artistId
                  WHERE aam.albumId = a.id ORDER BY aam.`order`)) AS albumArtists,
                (SELECT ar.name FROM song_album_map m JOIN song_artist_map sam ON sam.songId = m.songId
                  JOIN artist ar ON ar.id = sam.artistId WHERE m.albumId = a.id ORDER BY m.`index`, sam.position LIMIT 1) AS songArtist,
                (SELECT s.thumbnailUrl FROM song_album_map m JOIN song s ON s.id = m.songId
                  WHERE m.albumId = a.id AND s.thumbnailUrl IS NOT NULL LIMIT 1) AS songThumb
             FROM album a
             WHERE a.bookmarkedAt IS NOT NULL OR libSongs > 0",
        )?;
        let v = stmt
            .query_map([], |r| {
                let mut album = album_from_row(r)?;
                if album.thumbnail_url.is_none() {
                    album.thumbnail_url = r.get("songThumb")?;
                }
                let saved_at: Option<i64> = r.get("bookmarkedAt")?;
                let first: Option<i64> = r.get("firstAdded")?;
                let artists: Option<String> = r.get("albumArtists")?;
                let song_artist: Option<String> = r.get("songArtist")?;
                Ok(AlbumEntry {
                    album,
                    artists: artists.filter(|s| !s.is_empty()).or(song_artist).unwrap_or_default(),
                    saved_at,
                    added_at: saved_at.or(first).unwrap_or(0),
                    library_songs: r.get::<_, i64>("libSongs")? as u32,
                    play_time_ms: r.get("playMs")?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// Library artists: saved ones plus artists of library songs.
    pub fn library_artist_entries(&self) -> StorageResult<Vec<ArtistEntry>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT a.*,
                (SELECT COUNT(*) FROM song_artist_map m JOIN song s ON s.id = m.songId
                  WHERE m.artistId = a.id AND s.inLibrary IS NOT NULL) AS libSongs,
                (SELECT MIN(s.inLibrary) FROM song_artist_map m JOIN song s ON s.id = m.songId
                  WHERE m.artistId = a.id AND s.inLibrary IS NOT NULL) AS firstAdded,
                (SELECT COALESCE(SUM(s.totalPlayTime), 0) FROM song_artist_map m JOIN song s ON s.id = m.songId
                  WHERE m.artistId = a.id) AS playMs,
                (SELECT s.thumbnailUrl FROM song_artist_map m JOIN song s ON s.id = m.songId
                  WHERE m.artistId = a.id AND s.thumbnailUrl IS NOT NULL ORDER BY s.totalPlayTime DESC LIMIT 1) AS songThumb
             FROM artist a
             WHERE (a.bookmarkedAt IS NOT NULL OR libSongs > 0) AND a.name != ''",
        )?;
        let v = stmt
            .query_map([], |r| {
                let mut artist = artist_from_row(r)?;
                if artist.thumbnail_url.is_none() {
                    artist.thumbnail_url = r.get("songThumb")?;
                }
                let saved_at: Option<i64> = r.get("bookmarkedAt")?;
                let first: Option<i64> = r.get("firstAdded")?;
                Ok(ArtistEntry {
                    artist,
                    saved_at,
                    added_at: saved_at.or(first).unwrap_or(0),
                    song_count: r.get::<_, i64>("libSongs")? as u32,
                    play_time_ms: r.get("playMs")?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// Everything added to the library recently (songs by `inLibrary`, saved
    /// albums/artists/playlists by `bookmarkedAt`, local playlists by
    /// `createdAt`), newest first.
    pub fn recently_added(&self, limit: usize) -> StorageResult<Vec<RecentItem>> {
        let conn = self.conn();
        let mut out: Vec<RecentItem> = Vec::new();
        let songs = query_songs(
            &conn,
            &format!("SELECT {SONG_COLUMNS} FROM song s WHERE s.inLibrary IS NOT NULL ORDER BY s.inLibrary DESC LIMIT ?1"),
            params![limit as i64],
        )?;
        let mut stmt = conn.prepare("SELECT id, inLibrary FROM song WHERE inLibrary IS NOT NULL ORDER BY inLibrary DESC LIMIT ?1")?;
        let added: HashMap<String, i64> =
            stmt.query_map(params![limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        for s in songs {
            let at = added.get(&s.id).copied().unwrap_or(0);
            out.push(RecentItem { at, item: ShelfItem::Song(s) });
        }
        let mut stmt = conn.prepare("SELECT * FROM album WHERE bookmarkedAt IS NOT NULL ORDER BY bookmarkedAt DESC LIMIT ?1")?;
        for r in stmt.query_map(params![limit as i64], |r| Ok((r.get::<_, i64>("bookmarkedAt")?, album_from_row(r)?)))? {
            let (at, a) = r?;
            out.push(RecentItem { at, item: ShelfItem::Album(a) });
        }
        let mut stmt = conn.prepare("SELECT * FROM artist WHERE bookmarkedAt IS NOT NULL ORDER BY bookmarkedAt DESC LIMIT ?1")?;
        for r in stmt.query_map(params![limit as i64], |r| Ok((r.get::<_, i64>("bookmarkedAt")?, artist_from_row(r)?)))? {
            let (at, a) = r?;
            out.push(RecentItem { at, item: ShelfItem::Artist(a) });
        }
        let mut stmt = conn.prepare(&format!(
            "{PLAYLIST_SELECT} WHERE p.id NOT IN ('{LIKED_PLAYLIST_ID}', '{DOWNLOADED_PLAYLIST_ID}')
             ORDER BY COALESCE(p.bookmarkedAt, p.createdAt) DESC LIMIT ?1"
        ))?;
        for r in stmt.query_map(params![limit as i64], |r| {
            let at: Option<i64> = r.get("bookmarkedAt")?;
            let created: Option<i64> = r.get("createdAt")?;
            Ok((at.or(created).unwrap_or(0), playlist_from_row(r)?))
        })? {
            let (at, p) = r?;
            out.push(RecentItem { at, item: ShelfItem::Playlist(p) });
        }
        out.sort_by(|a, b| b.at.cmp(&a.at));
        out.truncate(limit);
        Ok(out)
    }

    // -- stats ----------------------------------------------------------------

    /// `[from, to)` bounds (epoch ms) of `period` shifted back by `offset`
    /// periods, in local time, relative to `now_ms`. `All` → `(0, now + 1)`.
    pub fn stats_period_bounds(&self, period: StatsPeriod, offset: u32, now_ms: i64) -> StorageResult<(i64, i64)> {
        let (start_mods, step) = match period {
            StatsPeriod::All => return Ok((0, now_ms + 1)),
            // 'weekday 0' moves forward to Sunday (or stays), -6 days = Monday.
            StatsPeriod::Week => ("'start of day', 'weekday 0', '-6 days'", "days"),
            StatsPeriod::Month => ("'start of month'", "months"),
            StatsPeriod::Year => ("'start of year'", "years"),
        };
        let (back, len) = match period {
            StatsPeriod::Week => (7 * offset as i64, 7),
            _ => (offset as i64, 1),
        };
        let sql = format!(
            "SELECT CAST(strftime('%s', ?1 / 1000, 'unixepoch', 'localtime', {start_mods}, '-{back} {step}', 'utc') AS INTEGER) * 1000,
                    CAST(strftime('%s', ?1 / 1000, 'unixepoch', 'localtime', {start_mods}, '-{back} {step}', '+{len} {step}', 'utc') AS INTEGER) * 1000"
        );
        Ok(self.conn().query_row(&sql, params![now_ms], |r| Ok((r.get(0)?, r.get(1)?)))?)
    }

    /// Totals of `[from, to)`.
    pub fn stats_summary(&self, from: i64, to: i64) -> StorageResult<StatsSummary> {
        let conn = self.conn();
        let (total_ms, plays, songs): (i64, i64, i64) = conn.query_row(
            "SELECT COALESCE(SUM(playTime), 0), COUNT(*), COUNT(DISTINCT songId) FROM event WHERE timestamp >= ?1 AND timestamp < ?2",
            params![from, to],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let artists: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT lower(trim(a.name))) FROM event e JOIN song_artist_map m ON m.songId = e.songId
             JOIN artist a ON a.id = m.artistId WHERE e.timestamp >= ?1 AND e.timestamp < ?2 AND trim(a.name) != ''",
            params![from, to],
            |r| r.get(0),
        )?;
        let albums: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT s.albumId) FROM event e JOIN song s ON s.id = e.songId
             WHERE e.timestamp >= ?1 AND e.timestamp < ?2 AND s.albumId IS NOT NULL AND s.albumId != ''",
            params![from, to],
            |r| r.get(0),
        )?;
        Ok(StatsSummary { total_ms, plays: plays as u32, songs: songs as u32, artists: artists as u32, albums: albums as u32 })
    }

    /// Most played songs of `[from, to)`: by plays, then listening time
    /// (Android `RecapRepository` order).
    pub fn top_songs_between(&self, from: i64, to: i64, limit: usize) -> StorageResult<Vec<StatSong>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT songId, COUNT(*) AS plays, COALESCE(SUM(playTime), 0) AS ms FROM event
             WHERE timestamp >= ?1 AND timestamp < ?2 GROUP BY songId ORDER BY plays DESC, ms DESC LIMIT ?3",
        )?;
        let rows: Vec<(String, i64, i64)> =
            stmt.query_map(params![from, to, limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        let mut out = Vec::with_capacity(rows.len());
        for (id, plays, ms) in rows {
            if let Some(song) = query_songs(&conn, &format!("SELECT {SONG_COLUMNS} FROM song s WHERE s.id = ?1"), params![id])?.pop() {
                out.push(StatSong { song, plays: plays as u32, ms });
            }
        }
        Ok(out)
    }

    /// Most listened artists of `[from, to)` by time. Artists that appear
    /// under several ids (channel, topic channel, local tag) are merged by
    /// name like Android does.
    pub fn top_artists_between(&self, from: i64, to: i64, limit: usize) -> StorageResult<Vec<StatArtist>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT a.*, COUNT(e.id) AS plays, COALESCE(SUM(e.playTime), 0) AS ms,
                (SELECT s.thumbnailUrl FROM event e2 JOIN song_artist_map m2 ON m2.songId = e2.songId JOIN song s ON s.id = e2.songId
                  WHERE m2.artistId = a.id AND e2.timestamp >= ?1 AND e2.timestamp < ?2 AND s.thumbnailUrl IS NOT NULL
                  GROUP BY e2.songId ORDER BY SUM(e2.playTime) DESC LIMIT 1) AS topThumb
             FROM event e JOIN song_artist_map m ON m.songId = e.songId JOIN artist a ON a.id = m.artistId
             WHERE e.timestamp >= ?1 AND e.timestamp < ?2 AND trim(a.name) != ''
             GROUP BY a.id ORDER BY ms DESC",
        )?;
        let rows: Vec<StatArtist> = stmt
            .query_map(params![from, to], |r| {
                Ok(StatArtist {
                    artist: artist_from_row(r)?,
                    plays: r.get::<_, i64>("plays")? as u32,
                    ms: r.get("ms")?,
                    fallback_thumbnail: r.get("topThumb")?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut merged: Vec<StatArtist> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for a in rows {
            let key = a.artist.name.trim().to_lowercase();
            match index.get(&key) {
                Some(&i) => {
                    let m = &mut merged[i];
                    m.plays += a.plays;
                    m.ms += a.ms;
                    if m.artist.thumbnail_url.is_none() {
                        m.artist.thumbnail_url = a.artist.thumbnail_url;
                    }
                    // Prefer a YouTube id (navigable) over a local one.
                    if !m.artist.id.starts_with("UC") && a.artist.id.starts_with("UC") {
                        m.artist.id = a.artist.id;
                    }
                }
                None => {
                    index.insert(key, merged.len());
                    merged.push(a);
                }
            }
        }
        merged.sort_by(|a, b| b.ms.cmp(&a.ms).then(b.plays.cmp(&a.plays)));
        merged.truncate(limit);
        Ok(merged)
    }

    /// Most listened albums of `[from, to)` by time.
    pub fn top_albums_between(&self, from: i64, to: i64, limit: usize) -> StorageResult<Vec<StatAlbum>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT s.albumId, COUNT(e.id) AS plays, COALESCE(SUM(e.playTime), 0) AS ms,
                    MAX(s.albumName) AS songAlbumName, MAX(s.thumbnailUrl) AS songThumb, MIN(s.id) AS anySong
             FROM event e JOIN song s ON s.id = e.songId
             WHERE e.timestamp >= ?1 AND e.timestamp < ?2 AND s.albumId IS NOT NULL AND s.albumId != ''
             GROUP BY s.albumId ORDER BY ms DESC, plays DESC LIMIT ?3",
        )?;
        let rows: Vec<(String, i64, i64, Option<String>, Option<String>, String)> = stmt
            .query_map(params![from, to, limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        let mut out = Vec::with_capacity(rows.len());
        for (album_id, plays, ms, song_album, song_thumb, any_song) in rows {
            let mut album = conn
                .query_row("SELECT * FROM album WHERE id = ?1", params![album_id], album_from_row)
                .optional()?
                .unwrap_or_else(|| Album { id: album_id.clone(), ..Default::default() });
            if album.title.is_empty() {
                album.title = song_album.unwrap_or_default();
            }
            if album.thumbnail_url.is_none() {
                album.thumbnail_url = song_thumb;
            }
            let artist: String = conn
                .query_row(
                    "SELECT a.name FROM song_artist_map m JOIN artist a ON a.id = m.artistId WHERE m.songId = ?1 ORDER BY m.position LIMIT 1",
                    params![any_song],
                    |r| r.get(0),
                )
                .optional()?
                .unwrap_or_default();
            out.push(StatAlbum { album, artist, plays: plays as u32, ms });
        }
        Ok(out)
    }

    /// Listening time of `[from, to)` per local hour, weekday and day.
    pub fn listening_buckets(&self, from: i64, to: i64) -> StorageResult<ListeningBuckets> {
        let conn = self.conn();
        let mut b = ListeningBuckets::default();
        let ts = local("timestamp");
        let mut stmt = conn.prepare(&format!(
            "SELECT CAST(strftime('%H', {ts}) AS INTEGER), CAST(strftime('%w', {ts}) AS INTEGER), SUM(playTime)
             FROM event WHERE timestamp >= ?1 AND timestamp < ?2 GROUP BY 1, 2"
        ))?;
        for r in stmt.query_map(params![from, to], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))? {
            let (h, w, ms) = r?;
            if (0..24).contains(&h) {
                b.by_hour[h as usize] += ms;
            }
            if (0..7).contains(&w) {
                // %w: 0 = Sunday → Monday-first index.
                b.by_weekday[((w + 6) % 7) as usize] += ms;
            }
        }
        let mut stmt = conn.prepare(&format!(
            "SELECT strftime('%Y-%m-%d', {ts}) AS d, SUM(playTime) FROM event
             WHERE timestamp >= ?1 AND timestamp < ?2 GROUP BY d ORDER BY d"
        ))?;
        b.by_day = stmt.query_map(params![from, to], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok(b)
    }

    /// Local calendar day (`YYYY-MM-DD`) of an epoch-ms instant, per SQLite.
    pub fn local_day(&self, ms: i64) -> StorageResult<String> {
        Ok(self.conn().query_row(&format!("SELECT strftime('%Y-%m-%d', {})", local("?1")), params![ms], |r| r.get(0))?)
    }

    /// Current offset of local time from UTC, in seconds (history grouping).
    pub fn local_utc_offset_secs(&self) -> StorageResult<i64> {
        Ok(self.conn().query_row(
            "SELECT CAST(strftime('%s', 'now', 'localtime') AS INTEGER) - CAST(strftime('%s', 'now') AS INTEGER)",
            [],
            |r| r.get(0),
        )?)
    }

    /// "Forgotten favorites" (Android home): songs played at least three
    /// times overall but not since `before_ms`, most played first.
    pub fn forgotten_favorites(&self, before_ms: i64, limit: usize) -> StorageResult<Vec<Song>> {
        let conn = self.conn();
        query_songs(
            &conn,
            &format!(
                "SELECT {SONG_COLUMNS} FROM song s
                 JOIN (SELECT songId, COUNT(*) AS plays, MAX(timestamp) AS last FROM event GROUP BY songId) e ON e.songId = s.id
                 WHERE e.plays >= 3 AND e.last < ?1
                 ORDER BY e.plays DESC, s.totalPlayTime DESC LIMIT ?2"
            ),
            params![before_ms, limit as i64],
        )
    }

    /// Time of the first recorded play.
    pub fn first_event_time(&self) -> StorageResult<Option<i64>> {
        Ok(self.conn().query_row("SELECT MIN(timestamp) FROM event", [], |r| r.get(0))?)
    }
}
