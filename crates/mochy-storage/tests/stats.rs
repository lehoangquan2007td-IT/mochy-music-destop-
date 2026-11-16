//! Listening stats, history and saved-library queries on a temp database.

use std::path::PathBuf;

use mochy_core::models::{Album, Artist, ArtistRef, Playlist, ShelfItem, Song};
use mochy_storage::{Db, StatsPeriod};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("mochy-stats-{tag}-{}-{n}", std::process::id()));
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

fn song(id: &str, title: &str, artists: &[(&str, &str)], album: Option<(&str, &str)>) -> Song {
    Song {
        artists: artists.iter().map(|(id, name)| ArtistRef::with_id(*id, *name)).collect(),
        duration_secs: Some(200.0),
        thumbnail_url: Some(format!("https://img/{id}")),
        album_id: album.map(|a| a.0.to_string()),
        album_name: album.map(|a| a.1.to_string()),
        ..Song::remote(id, title)
    }
}

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;
/// 2026-06-01 00:00:00 UTC (a Monday).
const T0: i64 = 1_780_272_000_000;

fn seeded(db: &Db) {
    let a = song("a", "Alpha", &[("UC_x", "Xavier")], Some(("MPRE_1", "First Album")));
    let b = song("b", "Beta", &[("UC_x", "Xavier"), ("UC_y", "Yuna")], Some(("MPRE_1", "First Album")));
    let c = song("c", "Gamma", &[("UC_y", "Yuna")], Some(("MPRE_2", "Second")));
    // Same artist name under a local id: merged with UC_y by name.
    let d = Song { artists: vec![ArtistRef::new("yuna ")], ..song("d", "Delta", &[], None) };
    for s in [&a, &b, &c, &d] {
        db.upsert_song(s).unwrap();
    }
    // a: 3 plays × 100 s; b: 2 × 200 s; c: 1 × 600 s; d: 1 × 50 s.
    db.record_play_at("a", 100.0, T0 + HOUR).unwrap();
    db.record_play_at("a", 100.0, T0 + 2 * HOUR).unwrap();
    db.record_play_at("a", 100.0, T0 + DAY + HOUR).unwrap();
    db.record_play_at("b", 200.0, T0 + 3 * HOUR).unwrap();
    db.record_play_at("b", 200.0, T0 + 2 * DAY).unwrap();
    db.record_play_at("c", 600.0, T0 + 3 * DAY).unwrap();
    db.record_play_at("d", 50.0, T0 + 4 * DAY).unwrap();
    // Outside the range used below.
    db.record_play_at("c", 600.0, T0 + 40 * DAY).unwrap();
}

#[test]
fn summary_and_top_lists() {
    let t = TempDir::new("top");
    let db = t.db();
    seeded(&db);
    let (from, to) = (T0, T0 + 30 * DAY);

    let s = db.stats_summary(from, to).unwrap();
    assert_eq!(s.total_ms, (300 + 400 + 600 + 50) * 1000);
    assert_eq!(s.plays, 7);
    assert_eq!(s.songs, 4);
    assert_eq!(s.artists, 2, "Yuna/yuna merged by name");
    assert_eq!(s.albums, 2);

    let songs = db.top_songs_between(from, to, 10).unwrap();
    let order: Vec<&str> = songs.iter().map(|s| s.song.id.as_str()).collect();
    assert_eq!(order, ["a", "b", "c", "d"], "plays first, then time");
    assert_eq!((songs[0].plays, songs[0].ms), (3, 300_000));
    assert_eq!(db.top_songs_between(from, to, 2).unwrap().len(), 2);

    let artists = db.top_artists_between(from, to, 10).unwrap();
    // Yuna: b 400 + c 600 + d 50 = 1050 s; Xavier: a 300 + b 400 = 700 s.
    assert_eq!(artists.len(), 2);
    assert_eq!(artists[0].artist.name, "Yuna");
    assert_eq!(artists[0].artist.id, "UC_y");
    assert_eq!(artists[0].ms, 1_050_000);
    assert_eq!(artists[0].plays, 4);
    assert_eq!(artists[1].artist.name, "Xavier");
    assert_eq!(artists[1].ms, 700_000);
    assert!(artists[0].fallback_thumbnail.is_some());

    let albums = db.top_albums_between(from, to, 10).unwrap();
    assert_eq!(albums.len(), 2);
    assert_eq!(albums[0].album.id, "MPRE_1");
    assert_eq!(albums[0].album.title, "First Album");
    assert_eq!(albums[0].ms, 700_000);
    assert_eq!(albums[0].plays, 5);
    assert_eq!(albums[0].artist, "Xavier");
    assert_eq!(albums[1].album.id, "MPRE_2");

    // All-time includes the late play.
    let all = db.stats_summary(0, i64::MAX).unwrap();
    assert_eq!(all.plays, 8);
    assert_eq!(db.first_event_time().unwrap(), Some(T0 + HOUR));
}

#[test]
fn buckets_cover_every_play() {
    let t = TempDir::new("buckets");
    let db = t.db();
    seeded(&db);
    let (from, to) = (T0, T0 + 30 * DAY);
    let b = db.listening_buckets(from, to).unwrap();
    let total = db.stats_summary(from, to).unwrap().total_ms;
    assert_eq!(b.by_hour.iter().sum::<i64>(), total);
    assert_eq!(b.by_weekday.iter().sum::<i64>(), total);
    assert_eq!(b.by_day.iter().map(|d| d.1).sum::<i64>(), total);
    // Plays at 01:00, 02:00, 03:00 UTC (+ 01:00 next day) hit three distinct local hours.
    assert!(b.by_hour.iter().filter(|&&ms| ms > 0).count() >= 3);
    assert!(b.by_day.len() >= 4 && b.by_day.len() <= 6, "days: {:?}", b.by_day);
    assert!(b.by_day.windows(2).all(|w| w[0].0 < w[1].0), "ascending days");
    assert_eq!(db.local_day(T0 + HOUR).unwrap().len(), 10);
    // Plays on 4 consecutive days (+1) land on at least 4 weekdays.
    assert!(b.by_weekday.iter().filter(|&&ms| ms > 0).count() >= 4);
}

#[test]
fn period_bounds_are_contiguous_and_ordered() {
    let t = TempDir::new("bounds");
    let db = t.db();
    let now = T0 + 10 * DAY + 5 * HOUR;
    for p in [StatsPeriod::Week, StatsPeriod::Month, StatsPeriod::Year] {
        let (from, to) = db.stats_period_bounds(p, 0, now).unwrap();
        assert!(from <= now && now < to, "{p:?}: {from} {now} {to}");
        let (pfrom, pto) = db.stats_period_bounds(p, 1, now).unwrap();
        assert_eq!(pto, from, "{p:?}: previous period ends where this one starts");
        assert!(pfrom < pto);
    }
    let (wf, wt) = db.stats_period_bounds(StatsPeriod::Week, 0, now).unwrap();
    // 7 days ± a DST hour.
    assert!((wt - wf - 7 * DAY).abs() <= HOUR);
    let (af, at) = db.stats_period_bounds(StatsPeriod::All, 0, now).unwrap();
    assert_eq!((af, at), (0, now + 1));
}

#[test]
fn history_events_and_removal() {
    let t = TempDir::new("history");
    let db = t.db();
    seeded(&db);
    // "a" was played 3 times, last at T0 + 1 day + 1 h.
    let ff: Vec<String> = db.forgotten_favorites(T0 + 10 * DAY, 10).unwrap().into_iter().map(|s| s.id).collect();
    assert_eq!(ff, ["a"]);
    assert!(db.forgotten_favorites(T0, 10).unwrap().is_empty());
    assert!(db.local_utc_offset_secs().unwrap().abs() <= 14 * 3600);

    let h = db.history_events(100).unwrap();
    assert_eq!(h.len(), 8);
    assert_eq!(h[0].song.id, "c", "newest first");
    assert!(h.windows(2).all(|w| w[0].timestamp >= w[1].timestamp));
    let first_a = h.iter().find(|e| e.song.id == "a").unwrap().event_id;
    db.delete_event(first_a).unwrap();
    assert_eq!(db.history_events(100).unwrap().len(), 7);
    db.delete_song_history("a").unwrap();
    let h = db.history_events(100).unwrap();
    assert!(h.iter().all(|e| e.song.id != "a"));
    assert_eq!(db.history_events(2).unwrap().len(), 2);
    db.clear_history().unwrap();
    assert!(db.history_events(10).unwrap().is_empty());
    // The songs themselves stay.
    assert!(db.get_song("a").unwrap().is_some());
}

#[test]
fn saved_albums_artists_playlists() {
    let t = TempDir::new("saved");
    let db = t.db();
    let album = Album { id: "MPRE_s".into(), title: "Saved Album".into(), year: Some(2024), thumbnail_url: Some("https://img/al".into()), ..Default::default() };
    db.set_album_saved(&album, &[ArtistRef::with_id("UC_z", "Zed")], true).unwrap();
    let artist = Artist { id: "UC_q".into(), name: "Quinn".into(), ..Default::default() };
    db.set_artist_saved(&artist, true).unwrap();
    let pl = Playlist { id: "PLremote".into(), name: "Remote mix".into(), browse_id: Some("VLPLremote".into()), song_count: 12, ..Default::default() };
    db.set_playlist_saved(&pl, true).unwrap();

    let ids = db.saved_ids().unwrap();
    assert!(ids.albums.contains("MPRE_s"));
    assert!(ids.artists.contains("UC_q"));
    assert!(ids.playlists.contains("PLremote") && ids.playlists.contains("VLPLremote"));

    let albums = db.library_album_entries().unwrap();
    let a = albums.iter().find(|a| a.album.id == "MPRE_s").expect("saved album listed");
    assert_eq!(a.artists, "Zed");
    assert!(a.saved_at.is_some());

    // A library song brings its album and artist in, unsaved.
    let mut s = song("l", "Lib song", &[("UC_w", "Wren")], Some(("MPRE_l", "Lib Album")));
    s.in_library = true;
    db.upsert_song(&s).unwrap();
    let albums = db.library_album_entries().unwrap();
    let l = albums.iter().find(|a| a.album.id == "MPRE_l").expect("library album");
    assert_eq!((l.library_songs, l.saved_at, l.artists.as_str()), (1, None, "Wren"));
    let artists = db.library_artist_entries().unwrap();
    assert!(artists.iter().any(|a| a.artist.id == "UC_q" && a.saved_at.is_some()));
    let w = artists.iter().find(|a| a.artist.id == "UC_w").expect("library artist");
    assert_eq!(w.song_count, 1);
    assert_eq!(w.artist.thumbnail_url.as_deref(), Some("https://img/l"), "falls back to song art");

    let recent = db.recently_added(10).unwrap();
    assert!(recent.windows(2).all(|w| w[0].at >= w[1].at));
    assert!(recent.iter().any(|r| matches!(&r.item, ShelfItem::Song(s) if s.id == "l")));
    assert!(recent.iter().any(|r| matches!(&r.item, ShelfItem::Album(a) if a.id == "MPRE_s")));
    assert!(recent.iter().any(|r| matches!(&r.item, ShelfItem::Playlist(p) if p.id == "PLremote")));

    // Unsave.
    db.set_album_saved(&album, &[], false).unwrap();
    db.set_artist_saved(&artist, false).unwrap();
    db.set_playlist_saved(&pl, false).unwrap();
    let ids = db.saved_ids().unwrap();
    assert!(ids.albums.is_empty() && ids.artists.is_empty() && ids.playlists.is_empty());
    assert!(db.playlists().unwrap().iter().all(|p| p.id != "PLremote"), "remote row removed");
    assert!(db.library_album_entries().unwrap().iter().all(|a| a.album.id != "MPRE_s"));
}

#[test]
fn real_typed_play_times_do_not_break_stats() {
    // Rows written by other builds can carry REAL play times / totals.
    let t = TempDir::new("real");
    let db = t.db();
    db.upsert_song(&song("r", "Real", &[("UC_r", "Ray")], Some(("MPRE_r", "R")))).unwrap();
    db.with_connection(|c| {
        c.execute("INSERT INTO event (songId, timestamp, playTime) VALUES ('r', ?1, 1500.5)", [T0])?;
        // A corrupt, saturated value is ignored instead of overflowing the sums.
        c.execute("INSERT INTO event (songId, timestamp, playTime) VALUES ('r', ?1, 9.223372036854776e18)", [T0 + 1])?;
        c.execute("UPDATE song SET totalPlayTime = 1500.5, inLibrary = 1 WHERE id = 'r'", [])
    })
    .unwrap();
    let (from, to) = (T0 - DAY, T0 + DAY);
    assert_eq!(db.stats_summary(from, to).unwrap().total_ms, 1500);
    assert_eq!(db.top_songs_between(from, to, 5).unwrap()[0].ms, 1500);
    assert_eq!(db.top_artists_between(from, to, 5).unwrap()[0].ms, 1500);
    assert_eq!(db.top_albums_between(from, to, 5).unwrap()[0].ms, 1500);
    assert_eq!(db.listening_buckets(from, to).unwrap().by_hour.iter().sum::<i64>(), 1500);
    let h = db.history_events(5).unwrap();
    assert_eq!((h[0].play_time_ms, h[1].play_time_ms), (0, 1500), "corrupt play counts as 0");
    assert!(!db.library_album_entries().unwrap().is_empty());
    assert!(!db.library_artist_entries().unwrap().is_empty());
}

#[test]
fn legacy_real_columns_are_converted_only_once() {
    // Old desktop builds created `event.playTime` / `song.totalPlayTime` as REAL
    // seconds. The ms conversion must run once, not on every open.
    let t = TempDir::new("legacy");
    let path = t.0.join("mochy.db");
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE `song` (`id` TEXT NOT NULL, `title` TEXT NOT NULL, `duration` INTEGER NOT NULL, `thumbnailUrl` TEXT, `albumId` TEXT, `albumName` TEXT, `explicit` INTEGER NOT NULL DEFAULT 0, `year` INTEGER, `date` INTEGER, `dateModified` INTEGER, `liked` INTEGER NOT NULL, `likedDate` INTEGER, `totalPlayTime` REAL NOT NULL DEFAULT 0.0, `inLibrary` INTEGER, `dateDownload` INTEGER, `isLocal` INTEGER NOT NULL DEFAULT false, `libraryAddToken` TEXT, `libraryRemoveToken` TEXT, `lyricsOffset` INTEGER NOT NULL DEFAULT 0, `romanizeLyrics` INTEGER NOT NULL DEFAULT true, `isDownloaded` INTEGER NOT NULL DEFAULT 0, `isUploaded` INTEGER NOT NULL DEFAULT false, `isVideo` INTEGER NOT NULL DEFAULT false, `localUri` TEXT DEFAULT NULL, `addedByLike` INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(`id`));
             CREATE TABLE `event` (`id` INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, `songId` TEXT NOT NULL, `timestamp` INTEGER NOT NULL, `playTime` REAL NOT NULL DEFAULT 0.0);
             INSERT INTO song (id, title, duration, liked, totalPlayTime) VALUES ('x', 'X', 200, 0, 30.5);
             INSERT INTO event (songId, timestamp, playTime) VALUES ('x', 1780272000000, 30.5);",
        )
        .unwrap();
    }
    for _ in 0..3 {
        let db = Db::open(&path).unwrap();
        assert_eq!(db.stats_summary(0, i64::MAX).unwrap().total_ms, 30_500);
        assert_eq!(db.get_song("x").unwrap().unwrap().total_play_time, 30.5);
    }
}
