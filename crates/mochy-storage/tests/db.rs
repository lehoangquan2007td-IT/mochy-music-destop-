//! `Db` tests against a real on-disk database in a temp dir.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use mochy_core::models::{Album, Artist, ArtistRef, Playlist, QueueItem, Song};
use mochy_storage::{Db, LIKED_PLAYLIST_ID, SCHEMA_VERSION};
use serde_json::Value;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("mochy-storage-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn db(&self) -> Db {
        Db::open(&self.0.join("mochy.db")).expect("open db")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn song(id: &str, title: &str, artist: &str) -> Song {
    Song {
        artists: vec![ArtistRef::with_id(format!("UC_{artist}"), artist)],
        duration_secs: Some(200.0),
        thumbnail_url: Some(format!("https://img/{id}")),
        ..Song::remote(id, title)
    }
}

// --- schema -------------------------------------------------------------------

/// Expected v43 columns: Room's exported v34 schema plus migrations 35→43
/// (`MusicDatabase.kt`).
/// (column name, affinity, notNull)
type Column = (String, String, bool);

fn expected_schema() -> (BTreeMap<String, Vec<Column>>, BTreeSet<String>) {
    let path = format!("{}/tests/fixtures/room_schema_v34.json", env!("CARGO_MANIFEST_DIR"));
    let j: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut tables = BTreeMap::new();
    for e in j["database"]["entities"].as_array().unwrap() {
        let cols = e["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["columnName"].as_str().unwrap().to_string(),
                    f["affinity"].as_str().unwrap().to_string(),
                    f["notNull"].as_bool().unwrap_or(false),
                )
            })
            .collect::<Vec<_>>();
        tables.insert(e["tableName"].as_str().unwrap().to_string(), cols);
    }
    let views: BTreeSet<String> =
        j["database"]["views"].as_array().unwrap().iter().map(|v| v["viewName"].as_str().unwrap().to_string()).collect();

    let c = |n: &str, t: &str, nn: bool| (n.to_string(), t.to_string(), nn);
    // 35→36 playlist_event; 36→37 song.localUri; 37→38 playlist.description
    tables.insert("playlist_event".into(), vec![c("id", "INTEGER", true), c("playlistId", "TEXT", true), c("timestamp", "INTEGER", true)]);
    tables.get_mut("song").unwrap().push(c("localUri", "TEXT", false));
    tables.get_mut("playlist").unwrap().push(c("description", "TEXT", false));
    // 38→39 imported_song; 39→40 import_session(+_song), imported_playlist; 40→41 remote cols + track table
    tables.insert("imported_song".into(), vec![c("songId", "TEXT", true), c("source", "TEXT", true), c("importedAt", "INTEGER", true)]);
    tables.insert("import_session".into(), vec![c("id", "INTEGER", true), c("source", "TEXT", true), c("startedAt", "INTEGER", true)]);
    tables.insert(
        "import_session_song".into(),
        vec![c("sessionId", "INTEGER", true), c("songId", "TEXT", true), c("addedToLibrary", "INTEGER", true), c("liked", "INTEGER", true)],
    );
    tables.insert(
        "imported_playlist".into(),
        vec![
            c("playlistId", "TEXT", true),
            c("source", "TEXT", true),
            c("sessionId", "INTEGER", false),
            c("importedAt", "INTEGER", true),
            c("remoteId", "TEXT", false),
            c("remoteUrl", "TEXT", false),
            c("lastSyncedAt", "INTEGER", false),
        ],
    );
    tables.insert("imported_playlist_track".into(), vec![c("playlistId", "TEXT", true), c("trackKey", "TEXT", true)]);
    // 41→42 song.addedByLike; 42→43 song_edit
    tables.get_mut("song").unwrap().push(c("addedByLike", "INTEGER", true));
    tables.insert(
        "song_edit".into(),
        vec![
            c("songId", "TEXT", true),
            c("title", "TEXT", false),
            c("artists", "TEXT", false),
            c("album", "TEXT", false),
            c("year", "INTEGER", false),
            c("cover", "TEXT", false),
            c("videoId", "TEXT", false),
            c("noVideo", "INTEGER", true),
        ],
    );
    (tables, views)
}

#[test]
fn schema_matches_room_v43() {
    let dir = TempDir::new("schema");
    let db = dir.db();
    let (tables, views) = expected_schema();
    assert_eq!(tables.len(), 24, "Room v43 has 24 entity tables");

    let version: i32 = db.with_connection(|c| c.query_row("PRAGMA user_version", [], |r| r.get(0))).unwrap();
    assert_eq!(version, 43);
    assert_eq!(SCHEMA_VERSION, 43);
    let mode: String = db.with_connection(|c| c.query_row("PRAGMA journal_mode", [], |r| r.get(0))).unwrap();
    assert_eq!(mode.to_lowercase(), "wal");
    let fk: i64 = db.with_connection(|c| c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))).unwrap();
    assert_eq!(fk, 1);

    for (table, cols) in &tables {
        let actual: Vec<Column> = db
            .with_connection(|c| {
                c.prepare(&format!("PRAGMA table_info(`{table}`)"))?
                    .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)? != 0)))?
                    .collect()
            })
            .unwrap();
        assert!(!actual.is_empty(), "missing table {table}");
        let want: BTreeSet<_> = cols.iter().collect();
        let got: BTreeSet<_> = actual.iter().collect();
        assert_eq!(want, got, "columns of {table} (name, type, notNull)");
    }
    let actual_views: BTreeSet<String> = db
        .with_connection(|c| c.prepare("SELECT name FROM sqlite_master WHERE type = 'view'")?.query_map([], |r| r.get(0))?.collect())
        .unwrap();
    assert_eq!(actual_views, views);
    // Reopening an existing database is idempotent.
    drop(db);
    let db = dir.db();
    assert!(db.get_song("nope").unwrap().is_none());
}

#[test]
fn db_is_shareable_across_threads() {
    let dir = TempDir::new("threads");
    let db = Arc::new(dir.db());
    let handles: Vec<_> = (0..4)
        .map(|t| {
            let db = Arc::clone(&db);
            std::thread::spawn(move || {
                for i in 0..10 {
                    db.upsert_song(&song(&format!("v{t}_{i}"), "T", "A")).unwrap();
                    db.set_in_library(&format!("v{t}_{i}"), true).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(db.all_songs().unwrap().len(), 40);
}

// --- songs ----------------------------------------------------------------------

#[test]
fn songs_upsert_get_roundtrip() {
    let dir = TempDir::new("songs");
    let db = dir.db();
    let mut s = song("Kx7B-XvmFtE", "Believer", "Imagine Dragons");
    s.artists.push(ArtistRef::new("Kaskade"));
    s.album_id = Some("MPREb_q16Gzaa1WK8".into());
    s.album_name = Some("Evolve".into());
    s.explicit = true;
    s.year = Some(2017);
    s.romanize_lyrics = true;
    db.upsert_song(&s).unwrap();

    let got = db.get_song("Kx7B-XvmFtE").unwrap().unwrap();
    assert_eq!(got.title, "Believer");
    assert_eq!(got.video_id.as_deref(), Some("Kx7B-XvmFtE"));
    assert_eq!(got.artists.len(), 2);
    assert_eq!(got.artists[0], ArtistRef::with_id("UC_Imagine Dragons", "Imagine Dragons"));
    assert_eq!(got.artists[1].name, "Kaskade");
    assert!(got.artists[1].id.as_deref().unwrap().starts_with("LA"), "name-only artist gets an LA id");
    assert_eq!(got.album_name.as_deref(), Some("Evolve"));
    assert_eq!(got.duration_secs, Some(200.0));
    assert!(got.explicit && got.romanize_lyrics && !got.liked && !got.in_library);
    assert_eq!(got.year, Some(2017));
    assert!(db.get_song("missing").unwrap().is_none());

    // Unknown metadata doesn't erase known values; title updates.
    let sparse = Song { duration_secs: None, thumbnail_url: None, artists: vec![], ..Song::remote("Kx7B-XvmFtE", "Believer (Remastered)") };
    db.upsert_song(&sparse).unwrap();
    let got = db.get_song("Kx7B-XvmFtE").unwrap().unwrap();
    assert_eq!(got.title, "Believer (Remastered)");
    assert_eq!(got.duration_secs, Some(200.0));
    assert!(got.thumbnail_url.is_some());
    assert_eq!(got.artists.len(), 2);
    assert_eq!(got.album_id.as_deref(), Some("MPREb_q16Gzaa1WK8"));
}

#[test]
fn upsert_does_not_cascade_delete_or_reset_state() {
    let dir = TempDir::new("cascade");
    let db = dir.db();
    let s = song("a1", "Song", "Artist");
    db.upsert_song(&s).unwrap();
    let pl = db.create_playlist("Mix").unwrap();
    db.add_to_playlist(&pl, &s, None).unwrap();
    db.record_play("a1", 30.0).unwrap();
    db.set_liked("a1", true).unwrap();
    db.set_lyrics_offset("a1", 250).unwrap();

    // Re-upserting a fresh copy (e.g. from a search result) keeps everything.
    db.upsert_song(&song("a1", "Song", "Artist")).unwrap();
    assert_eq!(db.playlist_songs(&pl).unwrap().len(), 1, "playlist entry survived");
    assert_eq!(db.recent_history(10).unwrap().len(), 1, "history survived");
    let got = db.get_song("a1").unwrap().unwrap();
    assert!(got.liked && got.in_library);
    assert_eq!(got.lyrics_offset_ms, 250);
    assert!((got.total_play_time - 30.0).abs() < 1e-9);
    // Re-upserting an artist/album must not drop the song's maps either.
    db.upsert_artist(&Artist { id: "UC_Artist".into(), name: "Artist!".into(), ..Default::default() }).unwrap();
    assert_eq!(db.get_song("a1").unwrap().unwrap().artists[0].name, "Artist!");
}

#[test]
fn liked_and_library() {
    let dir = TempDir::new("liked");
    let db = dir.db();
    for (id, t) in [("s1", "One"), ("s2", "Two"), ("s3", "Three")] {
        db.upsert_song(&song(id, t, "A")).unwrap();
    }
    assert!(db.all_songs().unwrap().is_empty());
    db.set_liked("s1", true).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    db.set_liked("s2", true).unwrap();
    let liked: Vec<String> = db.liked_songs().unwrap().into_iter().map(|s| s.id).collect();
    assert_eq!(liked, ["s2", "s1"], "most recent like first");
    let s1 = db.get_song("s1").unwrap().unwrap();
    assert!(s1.liked && s1.liked_date.is_some() && s1.in_library, "liking adds to library");
    let added_by_like: i64 = db.with_connection(|c| c.query_row("SELECT addedByLike FROM song WHERE id='s1'", [], |r| r.get(0))).unwrap();
    assert_eq!(added_by_like, 1);

    db.set_liked("s1", false).unwrap();
    let s1 = db.get_song("s1").unwrap().unwrap();
    assert!(!s1.liked && s1.liked_date.is_none() && s1.in_library, "unlike keeps library entry");

    db.set_in_library("s3", true).unwrap();
    let lib: BTreeSet<String> = db.all_songs().unwrap().into_iter().map(|s| s.id).collect();
    assert_eq!(lib, BTreeSet::from(["s1".into(), "s2".into(), "s3".into()]));
    let in_lib: Option<i64> = db.with_connection(|c| c.query_row("SELECT inLibrary FROM song WHERE id='s3'", [], |r| r.get(0))).unwrap();
    assert!(in_lib.unwrap() > 1_600_000_000_000, "inLibrary is an epoch-millis timestamp like Room");

    db.set_in_library("s2", false).unwrap();
    let s2 = db.get_song("s2").unwrap().unwrap();
    assert!(!s2.in_library && !s2.liked, "removing from library unlikes (Kotlin toggleLibrary)");
    assert_eq!(db.liked_songs().unwrap().len(), 0);

    db.set_romanize_lyrics("s3", false).unwrap();
    assert!(!db.get_song("s3").unwrap().unwrap().romanize_lyrics);
}

#[test]
fn local_songs() {
    let dir = TempDir::new("local");
    let db = dir.db();
    let path = PathBuf::from(r"C:\Music\Sơn Tùng - Lạc Trôi.flac");
    let id = mochy_core::ids::song_id_for_path(&path);
    let s = Song {
        id: id.clone(),
        video_id: None,
        title: "Lạc Trôi".into(),
        artists: vec![ArtistRef::new("Sơn Tùng M-TP")],
        local_path: Some(path.clone()),
        duration_secs: Some(233.0),
        ..Default::default()
    };
    db.add_local_song(&s).unwrap();
    let got = db.get_song(&id).unwrap().unwrap();
    assert!(got.is_local() && got.in_library && got.video_id.is_none());
    assert_eq!(got.local_path.as_deref(), Some(path.as_path()));
    assert_eq!(db.local_songs().unwrap().len(), 1);
    assert_eq!(db.all_songs().unwrap().len(), 1);
    assert!(db.add_local_song(&song("remote", "x", "y")).is_err(), "remote song rejected");
}

// --- history ----------------------------------------------------------------------

#[test]
fn history_and_most_played() {
    let dir = TempDir::new("history");
    let db = dir.db();
    for id in ["h1", "h2", "h3"] {
        db.upsert_song(&song(id, id, "A")).unwrap();
    }
    db.record_play("h1", 10.0).unwrap();
    db.record_play("h2", 20.0).unwrap();
    db.record_play("h1", 15.5).unwrap();
    db.record_play("h3", 5.0).unwrap();
    db.record_play("h1", 1.0).unwrap();
    db.record_play("h2", 1.0).unwrap();

    let recent: Vec<String> = db.recent_history(10).unwrap().into_iter().map(|s| s.id).collect();
    assert_eq!(recent, ["h2", "h1", "h3"], "distinct, latest first");
    assert_eq!(db.recent_history(2).unwrap().len(), 2);

    let most: Vec<String> = db.most_played(10).unwrap().into_iter().map(|s| s.id).collect();
    assert_eq!(most, ["h1", "h2", "h3"]);
    assert_eq!(db.play_count("h1").unwrap(), 3);
    let h1 = db.get_song("h1").unwrap().unwrap();
    assert!((h1.total_play_time - 26.5).abs() < 1e-9, "{}", h1.total_play_time);
    let ms: i64 = db.with_connection(|c| c.query_row("SELECT totalPlayTime FROM song WHERE id='h1'", [], |r| r.get(0))).unwrap();
    assert_eq!(ms, 26_500, "stored in milliseconds like Room");
    let events: i64 = db.with_connection(|c| c.query_row("SELECT COUNT(*) FROM event", [], |r| r.get(0))).unwrap();
    assert_eq!(events, 6);
    assert!(db.record_play("unknown", 1.0).is_err());
}

// --- playlists ----------------------------------------------------------------------

#[test]
fn playlists_crud_ordering_remove() {
    let dir = TempDir::new("playlists");
    let db = dir.db();
    let id = db.create_playlist("Road trip").unwrap();
    assert!(id.starts_with("LP") && id.len() == 10, "{id}");
    let other = db.create_playlist("Chill").unwrap();
    assert_eq!(db.playlists().unwrap().len(), 2);

    // Songs are upserted on demand.
    for i in 0..5 {
        db.add_to_playlist(&id, &song(&format!("p{i}"), &format!("Track {i}"), "A"), (i == 0).then_some("SETVID0")).unwrap();
    }
    db.add_to_playlist(&id, &song("p1", "Track 1", "A"), None).unwrap(); // duplicate entry allowed
    let order = |db: &Db| db.playlist_songs(&id).unwrap().into_iter().map(|s| s.id).collect::<Vec<_>>();
    assert_eq!(order(&db), ["p0", "p1", "p2", "p3", "p4", "p1"]);
    let set_vid: Option<String> = db
        .with_connection(|c| c.query_row("SELECT setVideoId FROM playlist_song_map WHERE songId='p0'", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(set_vid.as_deref(), Some("SETVID0"));
    assert_eq!(db.playlist_by_id(&id).unwrap().unwrap().song_count, 6);

    db.move_playlist_song(&id, 4, 0).unwrap();
    assert_eq!(order(&db), ["p4", "p0", "p1", "p2", "p3", "p1"]);
    assert!(db.move_playlist_song(&id, 9, 0).is_err());

    db.remove_from_playlist(&id, "p1").unwrap();
    assert_eq!(order(&db), ["p4", "p0", "p2", "p3"]);
    let positions: Vec<i64> = db
        .with_connection(|c| {
            c.prepare("SELECT position FROM playlist_song_map WHERE playlistId=?1 ORDER BY position")?
                .query_map([&id], |r| r.get(0))?
                .collect()
        })
        .unwrap();
    assert_eq!(positions, [0, 1, 2, 3], "dense positions after removal");
    let preview: i64 = db
        .with_connection(|c| c.query_row("SELECT COUNT(*) FROM playlist_song_map_preview WHERE playlistId=?1", [&id], |r| r.get(0)))
        .unwrap();
    assert_eq!(preview, 4);

    db.rename_playlist(&id, "Road trip 2").unwrap();
    assert_eq!(db.playlist_by_id(&id).unwrap().unwrap().name, "Road trip 2");

    db.delete_playlist(&id).unwrap();
    assert!(db.playlist_by_id(&id).unwrap().is_none());
    let maps: i64 = db.with_connection(|c| c.query_row("SELECT COUNT(*) FROM playlist_song_map", [], |r| r.get(0))).unwrap();
    assert_eq!(maps, 0, "entries cascade-deleted");
    assert!(db.get_song("p0").unwrap().is_some(), "songs survive playlist deletion");
    assert_eq!(db.playlists().unwrap().iter().map(|p| p.id.clone()).collect::<Vec<_>>(), [other]);

    // Special playlists are hidden from the list but reachable by id.
    db.upsert_playlist(&Playlist { id: LIKED_PLAYLIST_ID.into(), name: "Liked".into(), ..Default::default() }).unwrap();
    assert_eq!(db.playlists().unwrap().len(), 1);
    assert!(db.playlist_by_id(LIKED_PLAYLIST_ID).unwrap().is_some());
}

#[test]
fn playlist_cover_set_clear_and_kept_by_upsert() {
    let dir = TempDir::new("playlist-cover");
    let db = dir.db();
    let id = db.create_playlist("Mine").unwrap();
    let cover = |db: &Db| db.playlist_by_id(&id).unwrap().unwrap().thumbnail_url;
    assert_eq!(cover(&db), None);

    db.set_playlist_cover(&id, Some("file:///C:/data/covers/playlist-a.jpg")).unwrap();
    assert_eq!(cover(&db).as_deref(), Some("file:///C:/data/covers/playlist-a.jpg"));
    assert_eq!(db.playlists().unwrap()[0].thumbnail_url.as_deref(), Some("file:///C:/data/covers/playlist-a.jpg"));

    // A sync upsert carrying a remote thumbnail keeps the user's picture.
    let row = db.playlist_by_id(&id).unwrap().unwrap();
    db.upsert_playlist(&Playlist { thumbnail_url: Some("https://img/remote".into()), ..row }).unwrap();
    assert_eq!(cover(&db).as_deref(), Some("file:///C:/data/covers/playlist-a.jpg"));

    db.set_playlist_cover(&id, None).unwrap();
    assert_eq!(cover(&db), None);

    // Without a custom picture the remote thumbnail still updates.
    let row = db.playlist_by_id(&id).unwrap().unwrap();
    db.upsert_playlist(&Playlist { thumbnail_url: Some("https://img/remote".into()), ..row }).unwrap();
    assert_eq!(cover(&db).as_deref(), Some("https://img/remote"));
}

// --- albums / artists ---------------------------------------------------------------

#[test]
fn albums_and_artists() {
    let dir = TempDir::new("albums");
    let db = dir.db();
    let album = Album {
        id: "MPREb_q16Gzaa1WK8".into(),
        playlist_id: Some("OLAK5uy_n7O88V1oc".into()),
        title: "Evolve".into(),
        year: Some(2017),
        thumbnail_url: Some("https://img/evolve".into()),
        song_count: Some(3),
        duration_secs: Some(600.0),
        description: Some("Third album".into()),
    };
    let tracks: Vec<Song> = ["Next To Me", "I Don't Know Why", "Whatever It Takes"]
        .iter()
        .enumerate()
        .map(|(i, t)| song(&format!("t{i}"), t, "Imagine Dragons"))
        .collect();
    db.save_album(&album, &[tracks[2].clone(), tracks[0].clone(), tracks[1].clone()]).unwrap();
    assert_eq!(db.get_album(&album.id).unwrap().unwrap(), album);
    let order: Vec<String> = db.album_songs(&album.id).unwrap().into_iter().map(|s| s.id).collect();
    assert_eq!(order, ["t2", "t0", "t1"], "track order from song_album_map.index");
    assert!(db.album_songs(&album.id).unwrap().iter().all(|s| s.album_name.as_deref() == Some("Evolve")));
    let aam: i64 = db.with_connection(|c| c.query_row("SELECT COUNT(*) FROM album_artist_map", [], |r| r.get(0))).unwrap();
    assert_eq!(aam, 1);

    // Library views only include albums/artists with library songs.
    assert!(db.library_albums().unwrap().is_empty());
    assert!(db.library_artists().unwrap().is_empty());
    db.set_in_library("t0", true).unwrap();
    assert_eq!(db.library_albums().unwrap().len(), 1);
    let artists = db.library_artists().unwrap();
    assert_eq!(artists.len(), 1);
    assert_eq!(artists[0].name, "Imagine Dragons");
    assert_eq!(db.artist_songs(&artists[0].id).unwrap().len(), 3);

    // Partial album upsert keeps known fields.
    db.upsert_album(&Album { id: album.id.clone(), title: "Evolve".into(), ..Default::default() }).unwrap();
    let a = db.get_album(&album.id).unwrap().unwrap();
    assert_eq!((a.year, a.song_count, a.description.as_deref()), (Some(2017), Some(3), Some("Third album")));

    let artist = Artist {
        id: "UC0aXrjVxG5pZr99v77wZdPQ".into(),
        name: "Imagine Dragons".into(),
        thumbnail_url: Some("https://img/id".into()),
        channel_id: Some("UCT9zcQNlyht7fRlcjmflRSA".into()),
    };
    db.upsert_artist(&artist).unwrap();
    assert_eq!(db.get_artist(&artist.id).unwrap().unwrap(), artist);
    db.upsert_artist(&Artist { id: artist.id.clone(), name: "Imagine Dragons".into(), ..Default::default() }).unwrap();
    assert_eq!(db.get_artist(&artist.id).unwrap().unwrap().thumbnail_url, artist.thumbnail_url);
}

// --- formats / lyrics / search / queue ------------------------------------------------

#[test]
fn formats() {
    let dir = TempDir::new("formats");
    let db = dir.db();
    db.save_format("Kx7B-XvmFtE", Some(140), "audio/mp4", "mp4a.40.2", Some(129_573), Some(44_100), Some(3_309_731), Some(7.46))
        .unwrap();
    let f = db.load_format("Kx7B-XvmFtE").unwrap().unwrap();
    assert_eq!(f.itag, Some(140));
    assert_eq!(f.mime_type, "audio/mp4");
    assert_eq!(f.codecs, "mp4a.40.2");
    assert_eq!((f.bitrate, f.sample_rate, f.content_length), (Some(129_573), Some(44_100), Some(3_309_731)));
    assert_eq!(f.loudness_db, Some(7.46));
    db.save_format("Kx7B-XvmFtE", Some(251), "audio/webm", "opus", None, None, None, None).unwrap();
    let f = db.load_format("Kx7B-XvmFtE").unwrap().unwrap();
    assert_eq!((f.itag, f.bitrate, f.content_length, f.loudness_db), (Some(251), None, None, None));
    assert!(db.load_format("none").unwrap().is_none());
}

#[test]
fn lyrics_cache() {
    let dir = TempDir::new("lyrics");
    let db = dir.db();
    assert!(db.load_lyrics("v1").unwrap().is_none());
    db.save_lyrics("v1", "LrcLib", "[00:01.00] hello").unwrap();
    assert_eq!(db.load_lyrics("v1").unwrap(), Some(("LrcLib".into(), "[00:01.00] hello".into())));
    // Translation columns survive a lyrics update.
    db.with_connection(|c| c.execute("UPDATE lyrics SET translatedLyrics = 'xin chào' WHERE id = 'v1'", [])).unwrap();
    db.save_lyrics("v1", "BetterLyrics", "[00:01.00] hi").unwrap();
    assert_eq!(db.load_lyrics("v1").unwrap().unwrap().0, "BetterLyrics");
    let tr: String = db.with_connection(|c| c.query_row("SELECT translatedLyrics FROM lyrics WHERE id='v1'", [], |r| r.get(0))).unwrap();
    assert_eq!(tr, "xin chào");

    db.save_lyrics_not_found("v2").unwrap();
    assert!(db.load_lyrics("v2").unwrap().is_none());
    assert!(db.lyrics_not_found("v2").unwrap());
    assert!(!db.lyrics_not_found("v1").unwrap());
    db.save_lyrics("v3", "LYRICS_NOT_FOUND", "").unwrap();
    assert!(db.load_lyrics("v3").unwrap().is_none());
}

#[test]
fn search_history_dedupe_and_limit() {
    let dir = TempDir::new("search");
    let db = dir.db();
    db.add_search_history("believer").unwrap();
    db.add_search_history("son tung").unwrap();
    db.add_search_history("believer").unwrap();
    db.add_search_history("   ").unwrap();
    assert_eq!(db.search_history(10).unwrap(), ["believer", "son tung"]);
    for i in 0..120 {
        db.add_search_history(&format!("q{i}")).unwrap();
    }
    let all = db.search_history(1000).unwrap();
    assert_eq!(all.len(), 100, "trimmed to 100");
    assert_eq!(all[0], "q119");
    assert_eq!(db.search_history(3).unwrap(), ["q119", "q118", "q117"]);
    // Only q20..q119 survive the trim, so "q11" matches q110..q119.
    assert_eq!(db.search_history_prefix("q11", 100).unwrap().len(), 10);
    assert!(db.search_history_prefix("q1", 100).unwrap().iter().all(|q| q.starts_with("q1")));
    db.add_search_history("100%_real").unwrap();
    assert_eq!(db.search_history_prefix("100%", 10).unwrap(), ["100%_real"]);
    db.remove_search_history("q119").unwrap();
    assert_eq!(db.search_history(1).unwrap(), ["100%_real"]);
    db.clear_search_history().unwrap();
    assert!(db.search_history(10).unwrap().is_empty());
}

#[test]
fn queue_roundtrip() {
    let dir = TempDir::new("queue");
    let db = dir.db();
    assert!(db.load_queue().unwrap().is_none());
    let items: Vec<QueueItem> = (0..3)
        .map(|i| QueueItem {
            video_id: format!("v{i}"),
            playlist_id: Some("RDAMVMKx7B-XvmFtE".into()),
            set_video_id: (i == 1).then(|| "SV1".into()),
            title: format!("Bài hát {i}"),
            artists: vec![ArtistRef::with_id("UC1", "Sơn Tùng M-TP")],
            duration_secs: Some(180.5),
            thumbnail_url: Some("https://img".into()),
        })
        .collect();
    db.save_queue(&items, 2).unwrap();
    assert_eq!(db.load_queue().unwrap(), Some((items.clone(), 2)));
    db.save_queue(&items[..1], 0).unwrap();
    assert_eq!(db.load_queue().unwrap(), Some((items[..1].to_vec(), 0)));
    db.save_queue(&items, 99).unwrap();
    assert_eq!(db.load_queue().unwrap().unwrap().1, 2, "index clamped");

    // Survives reopening.
    drop(db);
    let db = dir.db();
    assert_eq!(db.load_queue().unwrap().unwrap().0.len(), 3);
    db.clear_queue().unwrap();
    assert!(db.load_queue().unwrap().is_none());
}

#[test]
fn delete_song_cascades() {
    let dir = TempDir::new("delete");
    let db = dir.db();
    db.upsert_song(&song("d1", "x", "A")).unwrap();
    let pl = db.create_playlist("P").unwrap();
    db.add_to_playlist(&pl, &song("d1", "x", "A"), None).unwrap();
    db.record_play("d1", 3.0).unwrap();
    db.delete_song("d1").unwrap();
    assert!(db.get_song("d1").unwrap().is_none());
    assert!(db.playlist_songs(&pl).unwrap().is_empty());
    assert!(db.recent_history(5).unwrap().is_empty());
}

#[test]
fn in_memory_db_works() {
    let db = Db::open_in_memory().unwrap();
    db.upsert_song(&song("m1", "Mem", "A")).unwrap();
    assert!(db.get_song("m1").unwrap().is_some());
}
