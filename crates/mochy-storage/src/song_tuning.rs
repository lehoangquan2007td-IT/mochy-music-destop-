//! Per-song speed / pitch (Android `SongTuning`, SharedPreferences
//! `song_tuning`: `speed,pitch` per media id). Desktop-only table, created
//! on first use (`CREATE TABLE IF NOT EXISTS`, additive).

use std::collections::HashMap;

use rusqlite::params;

use crate::{Db, StorageResult};

const TABLE: &str = "CREATE TABLE IF NOT EXISTS `_song_tuning` (`songId` TEXT NOT NULL, `speed` REAL NOT NULL, `semitones` REAL NOT NULL, PRIMARY KEY(`songId`));";

/// One song's own speed (factor) and pitch (semitones).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SongTuningRow {
    pub speed: f32,
    pub semitones: f32,
}

impl Db {
    /// Every saved tuning, by song id.
    pub fn song_tunings(&self) -> StorageResult<HashMap<String, SongTuningRow>> {
        let conn = self.conn();
        conn.execute_batch(TABLE)?;
        let mut stmt = conn.prepare("SELECT songId, speed, semitones FROM _song_tuning")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, SongTuningRow { speed: r.get::<_, f64>(1)? as f32, semitones: r.get::<_, f64>(2)? as f32 }))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    pub fn set_song_tuning(&self, song_id: &str, t: SongTuningRow) -> StorageResult<()> {
        let conn = self.conn();
        conn.execute_batch(TABLE)?;
        conn.execute(
            "INSERT INTO _song_tuning (songId, speed, semitones) VALUES (?1, ?2, ?3)
             ON CONFLICT(songId) DO UPDATE SET speed = excluded.speed, semitones = excluded.semitones",
            params![song_id, t.speed as f64, t.semitones as f64],
        )?;
        Ok(())
    }

    /// Back to the Audio Lab values for this song.
    pub fn clear_song_tuning(&self, song_id: &str) -> StorageResult<()> {
        let conn = self.conn();
        conn.execute_batch(TABLE)?;
        conn.execute("DELETE FROM _song_tuning WHERE songId = ?1", params![song_id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn song_tuning_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.song_tunings().unwrap().is_empty());
        db.set_song_tuning("a", SongTuningRow { speed: 1.25, semitones: -2.0 }).unwrap();
        db.set_song_tuning("b", SongTuningRow { speed: 0.8, semitones: 0.0 }).unwrap();
        db.set_song_tuning("a", SongTuningRow { speed: 1.5, semitones: 1.0 }).unwrap();
        let all = db.song_tunings().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all["a"], SongTuningRow { speed: 1.5, semitones: 1.0 });
        db.clear_song_tuning("a").unwrap();
        let all = db.song_tunings().unwrap();
        assert_eq!(all.len(), 1);
        assert!(all.contains_key("b"));
    }
}
