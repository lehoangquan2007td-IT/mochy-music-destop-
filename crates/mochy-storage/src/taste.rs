//! Inputs of the taste profile (app `taste` module): skips (desktop table
//! `skip_event`, additive) and bulk reads of plays, songs, local playlists.
//!
//! A skip is the user leaving a track early (the app decides). Skips live in
//! their own table so the `event` table — and the Stats page built on it —
//! keeps Android's meaning ("a play of ≥ 5 s").

use std::collections::HashMap;

use mochy_core::models::Song;
use rusqlite::{Connection, params};

use crate::{
    DOWNLOADED_PLAYLIST_ID, Db, LIKED_PLAYLIST_ID, SONG_COLUMNS, StorageError, StorageResult, epoch_millis, query_songs, song_exists,
};

const DDL: &str = "CREATE TABLE IF NOT EXISTS `skip_event` (\
    `id` INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, \
    `songId` TEXT NOT NULL, \
    `timestamp` INTEGER NOT NULL, \
    `playedMs` INTEGER NOT NULL, \
    `durationMs` INTEGER, \
    FOREIGN KEY(`songId`) REFERENCES `song`(`id`) ON UPDATE NO ACTION ON DELETE CASCADE);
CREATE INDEX IF NOT EXISTS `index_skip_event_songId` ON `skip_event` (`songId`);
CREATE INDEX IF NOT EXISTS `index_skip_event_timestamp` ON `skip_event` (`timestamp`);";

/// Create the skip table (idempotent).
pub(crate) fn migrate(conn: &Connection) -> StorageResult<()> {
    conn.execute_batch(DDL)?;
    Ok(())
}

/// Same sanity bound as the stats queries (6 h).
const MAX_PLAY_MS: i64 = 21_600_000;

/// One play (`event` row) with the song's duration, for scoring.
#[derive(Debug, Clone, PartialEq)]
pub struct TastePlay {
    pub song_id: String,
    /// Epoch millis.
    pub timestamp: i64,
    pub play_ms: i64,
    /// Song duration when known.
    pub duration_ms: Option<i64>,
}

/// One skip (`skip_event` row).
#[derive(Debug, Clone, PartialEq)]
pub struct SkipRow {
    pub song_id: String,
    /// Epoch millis.
    pub timestamp: i64,
    pub played_ms: i64,
    pub duration_ms: Option<i64>,
}

impl Db {
    /// Record a skip now. The song row must exist (upsert it first).
    pub fn record_skip(&self, song_id: &str, played_secs: f64, duration_secs: Option<f64>) -> StorageResult<()> {
        self.record_skip_at(song_id, played_secs, duration_secs, epoch_millis())
    }

    /// Record a skip at a given time (tests, imports).
    pub fn record_skip_at(&self, song_id: &str, played_secs: f64, duration_secs: Option<f64>, timestamp_ms: i64) -> StorageResult<()> {
        let played = (played_secs.max(0.0) * 1000.0).round() as i64;
        let duration = duration_secs.filter(|d| *d > 0.0).map(|d| (d * 1000.0).round() as i64);
        let conn = self.conn();
        if !song_exists(&conn, song_id)? {
            return Err(StorageError::Other(format!("record_skip: unknown song {song_id}")));
        }
        conn.execute(
            "INSERT INTO skip_event (songId, timestamp, playedMs, durationMs) VALUES (?1, ?2, ?3, ?4)",
            params![song_id, timestamp_ms, played, duration],
        )?;
        Ok(())
    }

    /// Skips since `from` (epoch ms), oldest first.
    pub fn skips_since(&self, from: i64) -> StorageResult<Vec<SkipRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT k.songId, k.timestamp, k.playedMs, COALESCE(k.durationMs, CASE WHEN s.duration > 0 THEN s.duration * 1000 END)
             FROM skip_event k LEFT JOIN song s ON s.id = k.songId
             WHERE k.timestamp >= ?1 ORDER BY k.timestamp, k.id",
        )?;
        let rows = stmt
            .query_map(params![from], |r| {
                Ok(SkipRow { song_id: r.get(0)?, timestamp: r.get(1)?, played_ms: r.get(2)?, duration_ms: r.get(3)? })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Number of skips of one song (all time).
    pub fn skip_count(&self, song_id: &str) -> StorageResult<u32> {
        let n: i64 = self.conn().query_row("SELECT COUNT(*) FROM skip_event WHERE songId = ?1", params![song_id], |r| r.get(0))?;
        Ok(n as u32)
    }

    /// Plays since `from` (epoch ms) with the song duration, oldest first.
    /// Corrupt play times (outside 0..6 h) count as 0.
    pub fn taste_plays_since(&self, from: i64) -> StorageResult<Vec<TastePlay>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT e.songId, e.timestamp,
                    CASE WHEN e.playTime BETWEEN 0 AND {MAX_PLAY_MS} THEN CAST(e.playTime AS INTEGER) ELSE 0 END,
                    CASE WHEN s.duration > 0 THEN s.duration * 1000 END
             FROM event e JOIN song s ON s.id = e.songId
             WHERE e.timestamp >= ?1 ORDER BY e.timestamp, e.id"
        ))?;
        let rows = stmt
            .query_map(params![from], |r| {
                Ok(TastePlay { song_id: r.get(0)?, timestamp: r.get(1)?, play_ms: r.get(2)?, duration_ms: r.get(3)? })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// `(playlist id, song id)` of every user (local) playlist — `LP…` ids,
    /// not the liked / downloaded specials, not YouTube playlists.
    pub fn local_playlist_song_ids(&self) -> StorageResult<Vec<(String, String)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT m.playlistId, m.songId FROM playlist_song_map m JOIN playlist p ON p.id = m.playlistId
             WHERE p.id LIKE 'LP%' AND p.id NOT IN ('{LIKED_PLAYLIST_ID}', '{DOWNLOADED_PLAYLIST_ID}')
               AND (p.browseId IS NULL OR p.browseId NOT LIKE 'VL%')"
        ))?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Every song the profile can use: played or skipped since `from`, liked,
    /// or in a local playlist (with artists, like [`Db::get_song`]).
    pub fn taste_songs(&self, from: i64) -> StorageResult<Vec<Song>> {
        let conn = self.conn();
        query_songs(
            &conn,
            &format!(
                "SELECT {SONG_COLUMNS} FROM song s WHERE s.id IN (
                    SELECT songId FROM event WHERE timestamp >= ?1
                    UNION SELECT songId FROM skip_event WHERE timestamp >= ?1
                    UNION SELECT id FROM song WHERE liked = 1
                    UNION SELECT m.songId FROM playlist_song_map m WHERE m.playlistId LIKE 'LP%'
                        AND m.playlistId NOT IN ('{LIKED_PLAYLIST_ID}', '{DOWNLOADED_PLAYLIST_ID}'))"
            ),
            params![from],
        )
    }

    /// Artist id → thumbnail, for artists that have one.
    pub fn artist_thumbnails(&self) -> StorageResult<HashMap<String, String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id, thumbnailUrl FROM artist WHERE thumbnailUrl IS NOT NULL AND thumbnailUrl != ''")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Forget the skips of one song.
    pub fn delete_song_skips(&self, song_id: &str) -> StorageResult<()> {
        self.conn().execute("DELETE FROM skip_event WHERE songId = ?1", params![song_id])?;
        Ok(())
    }

    /// Forget every skip.
    pub fn clear_skips(&self) -> StorageResult<()> {
        self.conn().execute("DELETE FROM skip_event", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mochy_core::models::{ArtistRef, Playlist};

    fn song(id: &str, dur: f64) -> Song {
        let mut s = Song::remote(id, format!("T {id}"));
        s.duration_secs = Some(dur);
        s.artists = vec![ArtistRef { id: Some("UCx".into()), name: "X".into() }];
        s
    }

    #[test]
    fn skips_roundtrip_and_cascade() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.record_skip("nope", 1.0, None).is_err());
        db.upsert_song(&song("a", 200.0)).unwrap();
        db.record_skip_at("a", 3.2, None, 1_000).unwrap();
        db.record_skip_at("a", 0.0, Some(180.0), 2_000).unwrap();
        let rows = db.skips_since(0).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], SkipRow { song_id: "a".into(), timestamp: 1_000, played_ms: 3_200, duration_ms: Some(200_000) });
        assert_eq!(rows[1].duration_ms, Some(180_000));
        assert_eq!(db.skips_since(1_500).unwrap().len(), 1);
        assert_eq!(db.skip_count("a").unwrap(), 2);
        // Skips don't touch the play history.
        assert!(db.taste_plays_since(0).unwrap().is_empty());
        db.delete_song("a").unwrap();
        assert!(db.skips_since(0).unwrap().is_empty());
    }

    #[test]
    fn taste_inputs() {
        let db = Db::open_in_memory().unwrap();
        for id in ["p", "k", "l", "m", "old"] {
            db.upsert_song(&song(id, 100.0)).unwrap();
        }
        db.record_play_at("p", 90.0, 5_000).unwrap();
        db.record_play_at("old", 90.0, 10).unwrap();
        db.record_skip_at("k", 2.0, None, 5_000).unwrap();
        db.set_liked("l", true).unwrap();
        let pl = db.create_playlist("Mine").unwrap();
        db.add_to_playlist(&pl, &song("m", 100.0), None).unwrap();
        // A YouTube playlist doesn't count as local.
        db.upsert_playlist(&Playlist { id: "PLyt".into(), name: "YT".into(), ..Default::default() }).unwrap();
        db.add_to_playlist("PLyt", &song("old", 100.0), None).unwrap();

        let plays = db.taste_plays_since(1_000).unwrap();
        assert_eq!(plays, vec![TastePlay { song_id: "p".into(), timestamp: 5_000, play_ms: 90_000, duration_ms: Some(100_000) }]);
        assert_eq!(db.local_playlist_song_ids().unwrap(), vec![(pl, "m".to_string())]);
        let mut ids: Vec<String> = db.taste_songs(1_000).unwrap().into_iter().map(|s| s.id).collect();
        ids.sort();
        assert_eq!(ids, ["k", "l", "m", "p"]);
        db.clear_skips().unwrap();
        assert!(db.skips_since(0).unwrap().is_empty());
    }
}
