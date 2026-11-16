//! Database side of imports (Android `ImportSteps` DB writes,
//! `PlaylistSourceRecord`, `ImportHistory`), against the Room-compatible
//! tables `import_session`, `import_session_song`, `imported_song`,
//! `imported_playlist` and `imported_playlist_track` that mochy-storage already
//! creates. All functions take the open [`Db`] (they run on the DB thread).
//! Dates are epoch milliseconds, like the rest of the schema.

use std::collections::{HashMap, HashSet};

use mochy_core::models::{Playlist, Song};
use mochy_storage::Db;
use rusqlite::{OptionalExtension, params};

use crate::matcher::{normalize_text, normalize_title};
use crate::matches::{track_key, youtube_key};
use crate::models::{ImportSource, ImportedTrack, PlaylistOrigin};

/// The source of a session that imported audio files from disk.
pub const DEVICE_SOURCE: &str = "device";

pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn log<T>(what: &str, r: Result<T, impl std::fmt::Display>) -> Option<T> {
    match r {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!("{what}: {e}");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

pub fn begin_session(db: &Db, source: ImportSource) -> Option<i64> {
    log(
        "begin import session",
        db.with_connection(|c| {
            c.execute("INSERT INTO import_session (source, startedAt) VALUES (?1, ?2)", params![source.key(), now_ms()])?;
            Ok(c.last_insert_rowid())
        }),
    )
}

pub fn session_exists(db: &Db, id: i64) -> bool {
    db.with_connection(|c| c.query_row("SELECT 1 FROM import_session WHERE id = ?1", params![id], |_| Ok(())).optional())
        .ok()
        .flatten()
        .is_some()
}

/// A session that changed nothing isn't worth keeping (or undoing).
pub fn delete_session_if_empty(db: &Db, id: i64) {
    log(
        "delete empty session",
        db.with_connection(|c| {
            c.execute(
                "DELETE FROM import_session WHERE id = ?1
                   AND NOT EXISTS (SELECT 1 FROM import_session_song WHERE sessionId = ?1)
                   AND NOT EXISTS (SELECT 1 FROM imported_playlist WHERE sessionId = ?1)",
                params![id],
            )
        }),
    );
}

// ---------------------------------------------------------------------------
// Songs
// ---------------------------------------------------------------------------

pub fn insert_songs(db: &Db, songs: &[Song]) {
    let mut seen = HashSet::new();
    for s in songs {
        if seen.insert(s.id.as_str()) {
            log("upsert imported song", db.upsert_song(s));
        }
    }
}

/// Files imported songs into the library and tags them with `source`; the
/// session records what it changed so an undo takes exactly that back out.
pub fn file_into_library(db: &Db, song_ids: &[String], source: ImportSource, session: Option<i64>, added_at: &HashMap<String, i64>) {
    let now = now_ms();
    let mut seen = HashSet::new();
    log(
        "file into library",
        db.with_connection(|c| {
            let tx = c.unchecked_transaction()?;
            for id in song_ids {
                if !seen.insert(id.as_str()) {
                    continue;
                }
                let row: Option<(Option<i64>, bool)> = tx
                    .query_row("SELECT inLibrary, addedByLike FROM song WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?;
                let Some((in_library, added_by_like)) = row else { continue };
                let adding = in_library.is_none();
                // imported on purpose, so its album shows even when a like put it in first
                if adding || added_by_like {
                    let date = in_library.or_else(|| added_at.get(id).copied()).unwrap_or(now);
                    tx.execute("UPDATE song SET inLibrary = ?1, addedByLike = 0 WHERE id = ?2", params![date, id])?;
                }
                tx.execute("INSERT OR REPLACE INTO imported_song (songId, source, importedAt) VALUES (?1, ?2, ?3)", params![id, source.key(), now])?;
                // the first step to touch a song records it; a later one finds it already in
                if let Some(s) = session {
                    tx.execute(
                        "INSERT OR IGNORE INTO import_session_song (sessionId, songId, addedToLibrary, liked) VALUES (?1, ?2, ?3, 0)",
                        params![s, id, adding],
                    )?;
                }
            }
            tx.commit()
        }),
    );
}

/// Likes the songs that aren't liked yet, dated when they were liked on the
/// source. Returns the ids newly liked.
pub fn like_imported(db: &Db, songs: &[(String, i64)], session: Option<i64>) -> Vec<String> {
    let mut liked = Vec::new();
    log(
        "like imported",
        db.with_connection(|c| {
            let tx = c.unchecked_transaction()?;
            for (id, date) in songs {
                let n = tx.execute("UPDATE song SET liked = 1, likedDate = ?1 WHERE id = ?2 AND liked = 0", params![date, id])?;
                if n > 0 {
                    if let Some(s) = session {
                        tx.execute("UPDATE import_session_song SET liked = 1 WHERE sessionId = ?1 AND songId = ?2", params![s, id])?;
                    }
                    liked.push(id.clone());
                }
            }
            tx.commit()
        }),
    );
    liked
}

// ---------------------------------------------------------------------------
// Playlists
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedPlaylistRow {
    pub playlist_id: String,
    pub source: String,
    pub session_id: Option<i64>,
    pub imported_at: i64,
    pub remote_id: Option<String>,
    pub remote_url: Option<String>,
    pub last_synced_at: Option<i64>,
}

pub fn imported_playlist(db: &Db, playlist_id: &str) -> Option<ImportedPlaylistRow> {
    db.with_connection(|c| {
        c.query_row(
            "SELECT playlistId, source, sessionId, importedAt, remoteId, remoteUrl, lastSyncedAt FROM imported_playlist WHERE playlistId = ?1",
            params![playlist_id],
            row_to_imported,
        )
        .optional()
    })
    .ok()
    .flatten()
}

fn row_to_imported(r: &rusqlite::Row) -> rusqlite::Result<ImportedPlaylistRow> {
    Ok(ImportedPlaylistRow {
        playlist_id: r.get(0)?,
        source: r.get(1)?,
        session_id: r.get(2)?,
        imported_at: r.get(3)?,
        remote_id: r.get(4)?,
        remote_url: r.get(5)?,
        last_synced_at: r.get(6)?,
    })
}

pub fn imported_playlists(db: &Db) -> Vec<ImportedPlaylistRow> {
    db.with_connection(|c| {
        let mut st = c.prepare(
            "SELECT i.playlistId, i.source, i.sessionId, i.importedAt, i.remoteId, i.remoteUrl, i.lastSyncedAt
             FROM imported_playlist i JOIN playlist p ON p.id = i.playlistId ORDER BY i.importedAt DESC",
        )?;
        st.query_map([], row_to_imported)?.collect()
    })
    .unwrap_or_default()
}

pub fn imported_track_count(db: &Db, playlist_id: &str) -> usize {
    db.with_connection(|c| c.query_row("SELECT COUNT(*) FROM imported_playlist_track WHERE playlistId = ?1", params![playlist_id], |r| r.get::<_, i64>(0)))
        .unwrap_or(0) as usize
}

/// The copy an earlier import made of this playlist: the same place on the
/// source, or else an editable one with its name; one tied to a different
/// remote playlist is never it.
pub fn existing_copy(db: &Db, source: ImportSource, name: &str, origin: Option<&PlaylistOrigin>) -> Option<Playlist> {
    let remote_id = origin.and_then(|o| o.remote_id.as_deref());
    if let Some(rid) = remote_id {
        let id: Option<String> = db
            .with_connection(|c| {
                c.query_row("SELECT playlistId FROM imported_playlist WHERE source = ?1 AND remoteId = ?2", params![source.key(), rid], |r| r.get(0))
                    .optional()
            })
            .ok()
            .flatten();
        if let Some(p) = id.and_then(|id| db.playlist_by_id(&id).ok().flatten()) {
            return Some(p);
        }
    }
    let mut candidates: Vec<Playlist> = db.playlists().unwrap_or_default().into_iter().filter(|p| p.name == name && p.is_editable).collect();
    // Kotlin reads them by name ascending; same names, so oldest first is stable enough.
    candidates.reverse();
    candidates.into_iter().find(|p| {
        let tied = imported_playlist(db, &p.id);
        remote_id.is_none() || tied.as_ref().is_none_or(|t| t.source != source.key() || t.remote_id.is_none())
    })
}

/// The tracks the playlist hasn't had from the service before, each once.
/// Older playlists only have their songs, so those are paired by title+artist.
pub fn new_tracks(db: &Db, playlist_id: &str, tracks: &[ImportedTrack]) -> Vec<ImportedTrack> {
    let mut known: HashSet<String> = db
        .with_connection(|c| {
            let mut st = c.prepare("SELECT trackKey FROM imported_playlist_track WHERE playlistId = ?1")?;
            st.query_map(params![playlist_id], |r| r.get(0))?.collect()
        })
        .unwrap_or_default();
    if known.is_empty() {
        for s in db.playlist_songs(playlist_id).unwrap_or_default() {
            let title = normalize_title(&s.title);
            known.insert(youtube_key(&s.id));
            for a in &s.artists {
                known.insert(format!("{title}|{}", normalize_text(&a.name)));
            }
        }
    }
    let mut seen = HashSet::new();
    tracks.iter().filter(|t| {
        let k = track_key(t);
        !known.contains(&k) && seen.insert(k)
    })
    .cloned()
    .collect()
}

/// Saves where the playlist lives on `source` and every song it has there now.
pub fn remember(db: &Db, source: ImportSource, playlist_id: &str, tracks: &[ImportedTrack], origin: Option<&PlaylistOrigin>) {
    // one made here, or from another service, that an import topped up by name: nothing to re-sync with
    if imported_playlist(db, playlist_id).is_none_or(|p| p.source != source.key()) {
        return;
    }
    let keys: HashSet<String> = tracks.iter().map(track_key).collect();
    log(
        "remember playlist source",
        db.with_connection(|c| {
            let tx = c.unchecked_transaction()?;
            tx.execute(
                "UPDATE imported_playlist SET remoteId = COALESCE(?2, remoteId), remoteUrl = COALESCE(?3, remoteUrl), lastSyncedAt = ?4 WHERE playlistId = ?1",
                params![playlist_id, origin.and_then(|o| o.remote_id.as_deref()), origin.and_then(|o| o.remote_url.as_deref()), now_ms()],
            )?;
            for k in &keys {
                tx.execute("INSERT OR IGNORE INTO imported_playlist_track (playlistId, trackKey) VALUES (?1, ?2)", params![playlist_id, k])?;
            }
            tx.commit()
        }),
    );
}

/// A new local playlist for an import, tied to its source for re-sync.
pub fn create_imported_playlist(
    db: &Db,
    name: &str,
    cover: Option<&str>,
    source: ImportSource,
    session: Option<i64>,
    origin: Option<&PlaylistOrigin>,
    songs: &[Song],
) -> Option<String> {
    let id = log("create playlist", db.create_playlist(name))?;
    let now = now_ms();
    log(
        "tie imported playlist",
        db.with_connection(|c| {
            c.execute("UPDATE playlist SET thumbnailUrl = COALESCE(?2, thumbnailUrl), bookmarkedAt = ?3 WHERE id = ?1", params![id, cover, now])?;
            c.execute(
                "INSERT OR REPLACE INTO imported_playlist (playlistId, source, sessionId, importedAt, remoteId, remoteUrl, lastSyncedAt)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
                params![id, source.key(), session, now, origin.and_then(|o| o.remote_id.as_deref()), origin.and_then(|o| o.remote_url.as_deref())],
            )
        }),
    );
    let mut seen = HashSet::new();
    for s in songs {
        if seen.insert(s.id.as_str()) {
            log("add to imported playlist", db.add_to_playlist(&id, s, None));
        }
    }
    Some(id)
}

pub fn set_cover_if_missing(db: &Db, playlist_id: &str, cover: &str) {
    log(
        "playlist cover",
        db.with_connection(|c| c.execute("UPDATE playlist SET thumbnailUrl = ?2 WHERE id = ?1 AND thumbnailUrl IS NULL", params![playlist_id, cover])),
    );
}

/// Adds songs to the end of the playlist, skipping any it has. Returns ids added.
pub fn append_new(db: &Db, playlist_id: &str, songs: &[Song]) -> Vec<String> {
    let mut already: HashSet<String> = db.playlist_songs(playlist_id).unwrap_or_default().into_iter().map(|s| s.id).collect();
    let mut added = Vec::new();
    for s in songs {
        if already.insert(s.id.clone()) && log("top up playlist", db.add_to_playlist(playlist_id, s, None)).is_some() {
            added.push(s.id.clone());
        }
    }
    added
}

// ---------------------------------------------------------------------------
// History / undo
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    pub id: i64,
    pub source: String,
    pub started_at: i64,
    pub songs: usize,
    pub playlists: usize,
}

/// Every import session, newest first (device-file sessions excluded).
pub fn sessions(db: &Db) -> Vec<SessionInfo> {
    db.with_connection(|c| {
        let mut st = c.prepare(
            "SELECT s.id, s.source, s.startedAt,
                (SELECT COUNT(*) FROM import_session_song x WHERE x.sessionId = s.id),
                (SELECT COUNT(*) FROM imported_playlist p WHERE p.sessionId = s.id)
             FROM import_session s WHERE s.source != ?1 ORDER BY s.startedAt DESC, s.id DESC",
        )?;
        st.query_map(params![DEVICE_SOURCE], |r| {
            Ok(SessionInfo { id: r.get(0)?, source: r.get(1)?, started_at: r.get(2)?, songs: r.get::<_, i64>(3)? as usize, playlists: r.get::<_, i64>(4)? as usize })
        })?
        .collect()
    })
    .unwrap_or_default()
}

/// How many songs each service brought in.
pub fn imported_counts(db: &Db) -> Vec<(String, usize)> {
    db.with_connection(|c| {
        let mut st = c.prepare("SELECT i.source, COUNT(*) FROM imported_song i JOIN song s ON s.id = i.songId GROUP BY i.source ORDER BY COUNT(*) DESC")?;
        st.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)))?.collect()
    })
    .unwrap_or_default()
}

/// Takes one import back out. It only reverses what it changed itself, so
/// songs already liked or in the library before stay that way. Returns how
/// many songs changed.
pub fn undo_session(db: &Db, session_id: i64) -> usize {
    let source: Option<String> =
        db.with_connection(|c| c.query_row("SELECT source FROM import_session WHERE id = ?1", params![session_id], |r| r.get(0)).optional()).ok().flatten();
    let Some(source) = source else { return 0 };
    let playlists: Vec<String> = db
        .with_connection(|c| {
            let mut st = c.prepare("SELECT playlistId FROM imported_playlist WHERE sessionId = ?1")?;
            st.query_map(params![session_id], |r| r.get(0))?.collect()
        })
        .unwrap_or_default();
    let mut removed = 0usize;
    log(
        "undo import",
        db.with_connection(|c| {
            let tx = c.unchecked_transaction()?;
            let rows: Vec<(String, bool, bool)> = {
                let mut st = tx.prepare("SELECT songId, addedToLibrary, liked FROM import_session_song WHERE sessionId = ?1")?;
                st.query_map(params![session_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?
            };
            for (song_id, added_to_library, row_liked) in rows {
                let song: Option<(bool, Option<i64>)> =
                    tx.query_row("SELECT liked, inLibrary FROM song WHERE id = ?1", params![song_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
                let Some((liked, in_library)) = song else { continue };
                let claimed_elsewhere: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM import_session_song WHERE songId = ?1 AND sessionId != ?2",
                    params![song_id, session_id],
                    |r| r.get(0),
                )?;
                let mut changed = false;
                if row_liked && liked {
                    tx.execute("UPDATE song SET liked = 0, likedDate = NULL WHERE id = ?1", params![song_id])?;
                    changed = true;
                }
                if added_to_library && claimed_elsewhere == 0 && in_library.is_some() {
                    tx.execute("UPDATE song SET inLibrary = NULL WHERE id = ?1", params![song_id])?;
                    changed = true;
                }
                if changed {
                    removed += 1;
                }
                let others_from_source: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM import_session_song x JOIN import_session s ON s.id = x.sessionId
                     WHERE x.songId = ?1 AND s.source = ?2 AND s.id != ?3",
                    params![song_id, source, session_id],
                    |r| r.get(0),
                )?;
                if others_from_source == 0 {
                    tx.execute("DELETE FROM imported_song WHERE songId = ?1 AND source = ?2", params![song_id, source])?;
                }
            }
            for p in &playlists {
                tx.execute("DELETE FROM playlist WHERE id = ?1", params![p])?;
                tx.execute("DELETE FROM imported_playlist WHERE playlistId = ?1", params![p])?;
                tx.execute("DELETE FROM imported_playlist_track WHERE playlistId = ?1", params![p])?;
            }
            tx.execute("DELETE FROM import_session_song WHERE sessionId = ?1", params![session_id])?;
            tx.execute("DELETE FROM import_session WHERE id = ?1", params![session_id])?;
            tx.commit()
        }),
    );
    removed
}

/// Undoes the newest import from a service. None when there is none.
pub fn undo_latest(db: &Db) -> Option<usize> {
    sessions(db).first().map(|s| undo_session(db, s.id))
}

/// Everything one service brought in.
pub fn delete_imports_from(db: &Db, source: &str) -> usize {
    sessions(db).into_iter().filter(|s| s.source == source).map(|s| undo_session(db, s.id)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mochy_core::models::ArtistRef;

    fn song(id: &str, title: &str, artist: &str) -> Song {
        let mut s = Song::remote(id, title);
        s.artists = vec![ArtistRef::new(artist)];
        s.duration_secs = Some(200.0);
        s
    }

    fn get(db: &Db, id: &str) -> Song {
        db.get_song(id).unwrap().unwrap()
    }

    #[test]
    fn import_then_undo_only_reverses_own_changes() {
        let db = Db::open_in_memory().unwrap();
        // already liked before the import
        let pre = song("pre00000001", "Before", "A");
        db.upsert_song(&pre).unwrap();
        db.set_liked(&pre.id, true).unwrap();

        let s1 = song("new00000001", "New", "B");
        let session = begin_session(&db, ImportSource::Spotify).unwrap();
        insert_songs(&db, &[pre.clone(), s1.clone()]);
        let ids = vec![pre.id.clone(), s1.id.clone()];
        file_into_library(&db, &ids, ImportSource::Spotify, Some(session), &HashMap::new());
        let liked = like_imported(&db, &[(pre.id.clone(), 1), (s1.id.clone(), 2)], Some(session));
        assert_eq!(liked, vec![s1.id.clone()]);
        assert!(get(&db, &s1.id).liked && get(&db, &s1.id).in_library);

        let origin = PlaylistOrigin { remote_id: Some("remote1".into()), remote_url: Some("https://open.spotify.com/playlist/remote1".into()) };
        let pid = create_imported_playlist(&db, "Road Trip", Some("https://x/cover.jpg"), ImportSource::Spotify, Some(session), Some(&origin), &[s1.clone()]).unwrap();
        let tracks = vec![ImportedTrack::new("New", "B", None)];
        remember(&db, ImportSource::Spotify, &pid, &tracks, Some(&origin));
        let row = imported_playlist(&db, &pid).unwrap();
        assert_eq!(row.remote_id.as_deref(), Some("remote1"));
        assert!(row.last_synced_at.is_some());
        assert_eq!(imported_track_count(&db, &pid), 1);
        assert_eq!(db.playlist_songs(&pid).unwrap().len(), 1);

        // importing again tops up the same copy, only with new tracks
        let found = existing_copy(&db, ImportSource::Spotify, "Road Trip", Some(&origin)).unwrap();
        assert_eq!(found.id, pid);
        let more = vec![ImportedTrack::new("New", "B", None), ImportedTrack::new("Second", "C", None)];
        let fresh = new_tracks(&db, &pid, &more);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].title, "Second");
        let s2 = song("new00000002", "Second", "C");
        assert_eq!(append_new(&db, &pid, &[s1.clone(), s2.clone()]), vec![s2.id.clone()]);

        let list = sessions(&db);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].playlists, 1);
        assert_eq!(list[0].songs, 2);
        assert_eq!(imported_counts(&db), vec![("spotify".to_string(), 2)]);

        let removed = undo_latest(&db).unwrap();
        assert_eq!(removed, 1, "only the new song changed");
        assert!(get(&db, &pre.id).liked, "liked before the import: stays liked");
        let n = get(&db, &s1.id);
        assert!(!n.liked && !n.in_library);
        assert!(db.playlist_by_id(&pid).unwrap().is_none());
        assert!(sessions(&db).is_empty());
        assert!(imported_counts(&db).is_empty());
    }

    #[test]
    fn new_tracks_without_record_pairs_by_title_and_artist() {
        let db = Db::open_in_memory().unwrap();
        let pid = db.create_playlist("Old").unwrap();
        db.add_to_playlist(&pid, &song("old00000001", "Song (feat. X)", "Band"), None).unwrap();
        let t = vec![ImportedTrack::new("Song", "Band, X", None), ImportedTrack::new("Other", "Band", None), ImportedTrack::new("Other", "Band", None)];
        let fresh = new_tracks(&db, &pid, &t);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].title, "Other");
        // remember() leaves playlists that aren't from this source alone
        remember(&db, ImportSource::Spotify, &pid, &t, None);
        assert_eq!(imported_track_count(&db, &pid), 0);
        // by name, when no remote id is known
        assert_eq!(existing_copy(&db, ImportSource::Other, "Old", None).unwrap().id, pid);
        assert!(existing_copy(&db, ImportSource::Other, "Nope", None).is_none());
    }

    #[test]
    fn empty_sessions_are_dropped() {
        let db = Db::open_in_memory().unwrap();
        let s = begin_session(&db, ImportSource::Deezer).unwrap();
        assert!(session_exists(&db, s));
        delete_session_if_empty(&db, s);
        assert!(!session_exists(&db, s));
    }
}
