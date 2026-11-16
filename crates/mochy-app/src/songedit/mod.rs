//! Song editing (port of Android `songedit/*`): hand edits of title,
//! artists, album, year and cover, kept in `song_edit` so a refetch can't
//! undo them and written into the song row so every list shows them; plus
//! importing a lyrics file for a song (stored in the `lyrics` table, which
//! the lyrics loader reads first).
//!
//! Like Android `SongEdits`, every edit is also kept in memory so queue
//! entries and the player can wear them without a database read.

pub mod ui;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, RwLock};

use mochy_core::models::{ArtistRef, Song};
use mochy_storage::{EditedArtist, SongEdit};

use crate::queue::Queue;
use crate::services::Services;

/// Provider name stored with imported lyrics (Android `USER_EDITED`).
pub const IMPORTED_PROVIDER: &str = "Imported";

static EDITS: LazyLock<RwLock<HashMap<String, SongEdit>>> = LazyLock::new(|| RwLock::new(HashMap::new()));
/// Songs whose edit / lyrics changed since the app last looked.
static CHANGED: LazyLock<Mutex<Vec<String>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn edits() -> std::sync::RwLockReadGuard<'static, HashMap<String, SongEdit>> {
    EDITS.read().unwrap_or_else(|e| e.into_inner())
}

/// Load every edit into memory (startup).
pub fn init(svc: &Services) {
    let rx = svc.db.query(|db| db.song_edits().unwrap_or_default());
    svc.spawn(async move {
        let list = rx.await.unwrap_or_default();
        let ids: Vec<String> = list.iter().map(|e| e.song_id.clone()).collect();
        {
            let mut map = EDITS.write().unwrap_or_else(|e| e.into_inner());
            for e in list {
                map.insert(e.song_id.clone(), e);
            }
        }
        if !ids.is_empty() {
            CHANGED.lock().unwrap_or_else(|e| e.into_inner()).extend(ids);
        }
    });
}

fn apply_edit(song: &mut Song, e: &SongEdit) {
    if let Some(t) = &e.title {
        song.title = t.clone();
    }
    if let Some(list) = &e.artists {
        song.artists = list.iter().map(|a| ArtistRef { id: (!a.id.is_empty()).then(|| a.id.clone()), name: a.name.clone() }).collect();
    }
    if let Some(a) = &e.album {
        song.album_name = Some(a.clone());
    }
    if let Some(y) = e.year {
        song.year = Some(y);
    }
    if let Some(c) = &e.cover {
        song.thumbnail_url = Some(c.clone());
    }
}

/// Lay the song's hand edit (if any) over it.
pub fn apply(song: &mut Song) {
    if let Some(e) = edits().get(&song.id) {
        apply_edit(song, e);
    }
}

/// Apply every edit to the queue's copies (cheap: the table is small).
pub fn apply_queue(queue: &mut Queue) {
    let map = edits();
    if map.is_empty() {
        return;
    }
    let ids: Vec<String> = queue.songs().filter(|s| map.contains_key(&s.id)).map(|s| s.id.clone()).collect();
    for id in ids {
        if let Some(e) = map.get(&id) {
            queue.for_each_song_mut(&id, |s| apply_edit(s, e));
        }
    }
}

pub fn mark_changed(song_id: &str) {
    CHANGED.lock().unwrap_or_else(|e| e.into_inner()).push(song_id.to_string());
}

/// Song ids edited since the last call (the app refreshes queue, lyrics and
/// library for them).
pub fn take_changed() -> Vec<String> {
    let mut v = std::mem::take(&mut *CHANGED.lock().unwrap_or_else(|e| e.into_inner()));
    v.sort();
    v.dedup();
    v
}

/// `file:///C:/x.jpg` / `file:///home/x.jpg` → a path.
pub fn file_url_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let rest = rest.replace("%20", " ");
    // Windows: "/C:/…" → "C:/…"
    let b = rest.as_bytes();
    let rest = if b.len() > 3 && b[0] == b'/' && b[2] == b':' { rest[1..].to_string() } else { rest };
    Some(PathBuf::from(rest))
}

pub fn path_to_file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/").replace(' ', "%20");
    if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") }
}

/// Pictures picked as a song's cover, copied into the app's own folder.
pub fn covers_dir() -> PathBuf {
    mochy_storage::data_dir().join("song_covers")
}

fn safe_id(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// Crop the picked picture to a square ≤1200 px JPEG in the covers folder
/// (Android `SongCoverFiles.save`); returns its `file://` URL.
pub fn save_cover_file(song_id: &str, picked: &Path) -> Result<String, String> {
    let bytes = std::fs::read(picked).map_err(|e| e.to_string())?;
    let cover = mochy_download::cover::prepare_cover(&bytes).ok_or_else(|| crate::ui::i18n::tr("Not a picture").to_string())?;
    let dir = covers_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let file = dir.join(format!("{}.{stamp}.jpg", safe_id(song_id)));
    std::fs::write(&file, &cover.jpeg).map_err(|e| e.to_string())?;
    Ok(path_to_file_url(&file))
}

/// Drop the song's older pictures, keeping the one its cover points at now.
fn delete_stale_covers(song_id: &str, keep: Option<&str>) {
    let keep = keep.and_then(file_url_path).and_then(|p| p.file_name().map(|n| n.to_os_string()));
    let prefix = format!("{}.", safe_id(song_id));
    let Ok(rd) = std::fs::read_dir(covers_dir()) else { return };
    for e in rd.flatten() {
        let name = e.file_name();
        if name.to_string_lossy().starts_with(&prefix) && Some(&name) != keep.as_ref() {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// What the edit dialog hands over when Save is pressed.
#[derive(Debug, Clone, Default)]
pub struct Draft {
    pub title: String,
    /// Comma-separated artist names.
    pub artists: String,
    pub album: String,
    pub year: String,
    /// New cover (URL or `file://`), `None` = unchanged.
    pub cover: Option<String>,
    /// Lyrics text to store, `None` = unchanged.
    pub lyrics: Option<String>,
}

pub fn split_artists(text: &str) -> Vec<String> {
    text.split([',', ';']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// The edit to store for `draft`: a field is only locked once it differs
/// from what the song had, or was locked already (Android
/// `SongEditSaver.save`). `original` is the song before this edit.
pub fn edit_for(original: &Song, before: Option<&SongEdit>, draft: &Draft) -> SongEdit {
    let title = Some(draft.title.trim()).filter(|t| !t.is_empty()).unwrap_or(&original.title).to_string();
    let album = draft.album.trim().to_string();
    let year = draft.year.trim().parse::<i32>().ok();
    let names = split_artists(&draft.artists);
    let old_names: Vec<String> = original.artists.iter().map(|a| a.name.clone()).collect();
    let artists_changed = !names.is_empty() && names != old_names;
    let artists = if artists_changed {
        Some(
            names
                .iter()
                .map(|n| {
                    // a name the song already had keeps its artist id (and page)
                    let id = original.artists.iter().find(|a| &a.name == n).and_then(|a| a.id.clone()).unwrap_or_default();
                    EditedArtist { id, name: n.clone(), thumbnail: None }
                })
                .collect(),
        )
    } else {
        before.and_then(|b| b.artists.clone())
    };
    SongEdit {
        song_id: original.id.clone(),
        title: (title != original.title || before.is_some_and(|b| b.title.is_some())).then_some(title),
        artists,
        album: (!album.is_empty() && (Some(album.as_str()) != original.album_name.as_deref() || before.is_some_and(|b| b.album.is_some())))
            .then_some(album),
        year: year.filter(|y| Some(*y) != original.year || before.is_some_and(|b| b.year.is_some())),
        cover: draft.cover.clone().or_else(|| before.and_then(|b| b.cover.clone())),
        video_id: before.and_then(|b| b.video_id.clone()),
        no_video: before.is_some_and(|b| b.no_video),
    }
}

/// Save an edit: memory overlay now, DB + downloaded file's tags in the
/// background, then the app refreshes queue / library / lyrics.
pub fn save(svc: &Services, original: &Song, draft: Draft) {
    let before = edits().get(&original.id).cloned();
    let edit = edit_for(original, before.as_ref(), &draft);
    EDITS.write().unwrap_or_else(|e| e.into_inner()).insert(edit.song_id.clone(), edit.clone());
    let mut edited = original.clone();
    apply_edit(&mut edited, &edit);
    let song = original.clone();
    let lyrics = draft.lyrics.clone();
    let cover = edit.cover.clone();
    let id = original.id.clone();
    svc.db.run(move |db| {
        crate::db::ensure_song(db, &song);
        crate::db::log_err("save_song_edit", db.save_song_edit(&edit));
        if let Some(text) = lyrics {
            let key = song.video_id.clone().unwrap_or_else(|| song.id.clone());
            crate::db::log_err("save_lyrics", db.save_lyrics(&key, IMPORTED_PROVIDER, &text));
        }
        if cover.as_deref().is_some_and(|c| c.starts_with("file://")) {
            delete_stale_covers(&id, cover.as_deref());
        }
        mark_changed(&id);
    });
    crate::downloads::retag(&edited);
    svc.bus.toast(crate::ui::i18n::tr("Song updated"));
}

/// Store an imported lyrics file for `song` (lyrics view reloads).
pub fn import_lyrics(svc: &Services, song: &Song, text: String) {
    let key = song.video_id.clone().unwrap_or_else(|| song.id.clone());
    let s = song.clone();
    svc.db.run(move |db| {
        crate::db::ensure_song(db, &s);
        crate::db::log_err("save_lyrics", db.save_lyrics(&key, IMPORTED_PROVIDER, &text));
        mark_changed(&s.id);
    });
    svc.bus.toast(crate::ui::i18n::tr("Lyrics imported"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song() -> Song {
        Song {
            artists: vec![ArtistRef::with_id("UC1", "Alpha"), ArtistRef::new("Beta")],
            album_name: Some("Album".into()),
            year: Some(2020),
            ..Song::remote("vid", "Title")
        }
    }

    #[test]
    fn only_changed_fields_are_locked() {
        let s = song();
        let d = Draft { title: "Title".into(), artists: "Alpha, Beta".into(), album: "Album".into(), year: "2020".into(), ..Default::default() };
        let e = edit_for(&s, None, &d);
        assert_eq!((e.title, e.artists, e.album, e.year, e.cover), (None, None, None, None, None));

        let d = Draft { title: " New ".into(), artists: "Alpha; Gamma".into(), album: "Other".into(), year: "1999".into(), cover: Some("https://c".into()), ..Default::default() };
        let e = edit_for(&s, None, &d);
        assert_eq!(e.title.as_deref(), Some("New"));
        let a = e.artists.unwrap();
        assert_eq!(a[0], EditedArtist { id: "UC1".into(), name: "Alpha".into(), thumbnail: None });
        assert_eq!(a[1].id, "");
        assert_eq!(e.album.as_deref(), Some("Other"));
        assert_eq!(e.year, Some(1999));
        assert_eq!(e.cover.as_deref(), Some("https://c"));
    }

    #[test]
    fn locked_fields_stay_locked_and_apply() {
        let mut s = song();
        let before = SongEdit { song_id: "vid".into(), title: Some("Mine".into()), cover: Some("file:///c.jpg".into()), ..Default::default() };
        apply_edit(&mut s, &before);
        assert_eq!(s.title, "Mine");
        assert_eq!(s.thumbnail_url.as_deref(), Some("file:///c.jpg"));
        let d = Draft { title: "Mine".into(), artists: "Alpha, Beta".into(), album: "Album".into(), year: "2020".into(), ..Default::default() };
        let e = edit_for(&s, Some(&before), &d);
        assert_eq!(e.title.as_deref(), Some("Mine"));
        assert_eq!(e.cover.as_deref(), Some("file:///c.jpg"));
    }

    #[test]
    fn file_urls() {
        assert_eq!(file_url_path("file:///C:/Users/a%20b/x.jpg"), Some(PathBuf::from("C:/Users/a b/x.jpg")));
        assert_eq!(file_url_path("file:///home/x.jpg"), Some(PathBuf::from("/home/x.jpg")));
        assert_eq!(file_url_path("https://x"), None);
        let url = path_to_file_url(Path::new(r"C:\Users\a b\x.jpg"));
        assert_eq!(url, "file:///C:/Users/a%20b/x.jpg");
        assert_eq!(file_url_path(&url), Some(PathBuf::from("C:/Users/a b/x.jpg")));
    }
}
