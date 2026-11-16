//! Downloads table (`_download`) and song edits (`song_edit`).

use std::path::PathBuf;

use mochy_core::models::{ArtistRef, Song};
use mochy_storage::{Db, DownloadRow, EditedArtist, KIND_CACHE, KIND_DOWNLOAD, SCHEMA_VERSION, SongEdit};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("mochy-offline-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn path(&self) -> PathBuf {
        self.0.join("mochy.db")
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
        album_name: Some("Original album".into()),
        thumbnail_url: Some(format!("https://img/{id}")),
        duration_secs: Some(180.0),
        ..Song::remote(id, title)
    }
}

#[test]
fn migration_adds_download_table_to_existing_db() {
    let dir = TempDir::new("migrate");
    {
        let db = Db::open(&dir.path()).unwrap();
        // An older desktop build: no _download table yet.
        db.with_connection(|c| c.execute_batch("DROP TABLE _download;")).unwrap();
        db.upsert_song(&song("v1", "Kept", "A")).unwrap();
    }
    let db = Db::open(&dir.path()).unwrap();
    let (has, version): (bool, i32) = db
        .with_connection(|c| {
            let has = c
                .query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name='_download'", [], |_| Ok(()))
                .is_ok();
            let v: i32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
            Ok((has, v))
        })
        .unwrap();
    assert!(has);
    assert_eq!(version, SCHEMA_VERSION);
    assert_eq!(db.get_song("v1").unwrap().unwrap().title, "Kept");
    // Re-opening is idempotent.
    drop(db);
    let db = Db::open(&dir.path()).unwrap();
    assert!(db.downloads().unwrap().is_empty());
}

#[test]
fn download_rows_roundtrip_and_flags() {
    let db = Db::open_in_memory().unwrap();
    db.upsert_song(&song("v1", "One", "A")).unwrap();
    let mut row = DownloadRow {
        song_id: "v1".into(),
        kind: KIND_DOWNLOAD,
        state: "queued".into(),
        song_json: "{\"id\":\"v1\"}".into(),
        created_at: 10,
        updated_at: 10,
        ..Default::default()
    };
    db.upsert_download(&row).unwrap();
    row.state = "done".into();
    row.file_path = Some("C:\\Music\\A - One.opus".into());
    row.bytes = 1234;
    row.total_bytes = 1234;
    row.updated_at = 20;
    db.upsert_download(&row).unwrap();
    db.upsert_download(&DownloadRow { song_id: "v2".into(), kind: KIND_CACHE, state: "done".into(), created_at: 11, ..Default::default() })
        .unwrap();
    let rows = db.downloads().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], row);
    assert_eq!(rows[1].kind, KIND_CACHE);

    db.touch_download("v1", 99).unwrap();
    db.touch_download("v1", 50).unwrap();
    assert_eq!(db.downloads().unwrap()[0].last_used_at, 99);

    assert!(!db.song_is_downloaded("v1").unwrap());
    db.set_song_downloaded("v1", true).unwrap();
    assert!(db.song_is_downloaded("v1").unwrap());
    let date: Option<i64> = db.with_connection(|c| c.query_row("SELECT dateDownload FROM song WHERE id='v1'", [], |r| r.get(0))).unwrap();
    assert!(date.is_some());
    db.set_song_downloaded("v1", false).unwrap();
    assert!(!db.song_is_downloaded("v1").unwrap());

    db.delete_download("v1").unwrap();
    assert_eq!(db.downloads().unwrap().len(), 1);
}

#[test]
fn song_edit_survives_refetch() {
    let db = Db::open_in_memory().unwrap();
    db.upsert_song(&song("v1", "YouTube title", "Orig")).unwrap();
    let edit = SongEdit {
        song_id: "v1".into(),
        title: Some("My title".into()),
        artists: Some(vec![
            EditedArtist { id: "UC_New".into(), name: "New artist".into(), thumbnail: None },
            EditedArtist { id: String::new(), name: "Guest".into(), thumbnail: None },
        ]),
        album: Some("My album".into()),
        year: Some(1999),
        cover: Some("file:///C:/covers/v1.jpg".into()),
        ..Default::default()
    };
    db.save_song_edit(&edit).unwrap();
    let check = |db: &Db| {
        let s = db.get_song("v1").unwrap().unwrap();
        assert_eq!(s.title, "My title");
        assert_eq!(s.album_name.as_deref(), Some("My album"));
        assert_eq!(s.year, Some(1999));
        assert_eq!(s.thumbnail_url.as_deref(), Some("file:///C:/covers/v1.jpg"));
        let names: Vec<&str> = s.artists.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["New artist", "Guest"]);
    };
    check(&db);
    // A refresh from YouTube upserts the original metadata again.
    db.upsert_song(&song("v1", "YouTube title", "Orig")).unwrap();
    check(&db);

    let stored = db.song_edit("v1").unwrap().unwrap();
    assert_eq!(stored, edit);
    assert_eq!(db.song_edits().unwrap().len(), 1);
    // Android stores the artists as a JSON list of {id, name, thumbnail}.
    let raw: String = db.with_connection(|c| c.query_row("SELECT artists FROM song_edit", [], |r| r.get(0))).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v[0]["id"], "UC_New");
    assert_eq!(v[1]["name"], "Guest");

    db.delete_song_edit("v1").unwrap();
    db.upsert_song(&song("v1", "YouTube title", "Orig")).unwrap();
    assert_eq!(db.get_song("v1").unwrap().unwrap().title, "YouTube title");
}

#[test]
fn broken_artist_json_reads_as_no_edit() {
    let db = Db::open_in_memory().unwrap();
    db.upsert_song(&song("v1", "T", "Orig")).unwrap();
    db.with_connection(|c| c.execute("INSERT INTO song_edit (songId, artists, noVideo) VALUES ('v1', 'not json', 0)", [])).unwrap();
    assert_eq!(db.song_edit("v1").unwrap().unwrap().artists, None);
    db.upsert_song(&song("v1", "T", "Orig")).unwrap();
    assert_eq!(db.get_song("v1").unwrap().unwrap().artists[0].name, "Orig");
}
