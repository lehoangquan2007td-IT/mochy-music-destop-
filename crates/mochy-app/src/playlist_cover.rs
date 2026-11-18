//! Custom cover pictures for the user's own (local) playlists.
//!
//! The picked picture is cropped to a square JPEG (same as song covers) and
//! copied to `<data>/covers/playlist-<id>-<stamp>.jpg`; the playlist's
//! `thumbnailUrl` then holds its `file://` link, which the image cache reads
//! from disk. Every change writes a new file name, so no stale texture is
//! shown, and the previous file is deleted.

use std::path::{Path, PathBuf};

use mochy_core::models::Playlist;
use mochy_storage::{DOWNLOADED_PLAYLIST_ID, Db, LIKED_PLAYLIST_ID};

use crate::services::{AppMsg, LibraryMsg, Services};
use crate::songedit::{file_url_path, path_to_file_url};
use crate::ui::i18n::tr;

pub fn covers_dir() -> PathBuf {
    mochy_storage::data_dir().join("covers")
}

/// Can the user pick a cover for `p`? Only their own local playlists, not
/// YouTube Music ones nor the automatic Liked / Downloaded lists.
pub fn can_edit(p: &Playlist) -> bool {
    crate::widgets::menus::is_local_playlist(p) && p.id != LIKED_PLAYLIST_ID && p.id != DOWNLOADED_PLAYLIST_ID
}

/// Is `p`'s cover a picture the user picked (vs. none / an imported link)?
pub fn has_custom(p: &Playlist) -> bool {
    p.thumbnail_url.as_deref().is_some_and(|u| u.starts_with("file://"))
}

fn file_prefix(id: &str) -> String {
    let safe: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    format!("playlist-{safe}-")
}

/// Copy `picked` (as a square JPEG) into the covers folder; returns its
/// `file://` link.
fn save_file(id: &str, picked: &Path) -> Result<String, String> {
    let bytes = std::fs::read(picked).map_err(|e| e.to_string())?;
    let cover = mochy_download::cover::prepare_cover(&bytes).ok_or_else(|| tr("Not a picture").to_string())?;
    let dir = covers_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let file = dir.join(format!("{}{stamp}.jpg", file_prefix(id)));
    std::fs::write(&file, &cover.jpeg).map_err(|e| e.to_string())?;
    Ok(path_to_file_url(&file))
}

/// Delete the playlist's cover files except the one `keep` links to.
pub fn delete_files(id: &str, keep: Option<&str>) {
    let keep = keep.and_then(file_url_path).and_then(|p| p.file_name().map(|n| n.to_os_string()));
    let prefix = file_prefix(id);
    let Ok(rd) = std::fs::read_dir(covers_dir()) else { return };
    for e in rd.flatten() {
        let name = e.file_name();
        if name.to_string_lossy().starts_with(&prefix) && Some(&name) != keep.as_ref() {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Store `cover` (or clear it), drop older files, refresh the library.
fn apply(svc: &Services, id: String, cover: Option<String>) {
    let bus = svc.bus.clone();
    svc.db.run(move |db: &Db| {
        if crate::db::log_err("set_playlist_cover", db.set_playlist_cover(&id, cover.as_deref())).is_some() {
            delete_files(&id, cover.as_deref());
        }
        if let Some(v) = crate::db::log_err("playlists", db.playlists()) {
            bus.send(AppMsg::Library(LibraryMsg::Playlists(v)));
        }
    });
}

/// Ask for a picture (native dialog, off the UI thread) and make it the
/// playlist's cover.
pub fn pick(svc: &Services, id: String) {
    let svc = svc.clone();
    std::thread::spawn(move || {
        let Some(path) = rfd::FileDialog::new()
            .set_title(tr("Choose a cover picture"))
            .add_filter(tr("Pictures"), &["png", "jpg", "jpeg", "webp"])
            .pick_file()
        else {
            return;
        };
        match save_file(&id, &path) {
            Ok(url) => {
                apply(&svc, id, Some(url));
                svc.bus.toast(tr("Playlist cover updated"));
            }
            Err(e) => svc.bus.toast(format!("{}: {e}", tr("Couldn't set the cover"))),
        }
    });
}

/// Go back to the automatic cover (first song's artwork).
pub fn remove(svc: &Services, id: String) {
    apply(svc, id, None);
    svc.bus.toast(tr("Custom cover removed"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_own_local_playlists_are_editable() {
        let p = |id: &str, browse: Option<&str>| Playlist { id: id.into(), browse_id: browse.map(Into::into), ..Default::default() };
        assert!(can_edit(&p("LPabcdefgh", None)));
        assert!(!can_edit(&p(LIKED_PLAYLIST_ID, None)));
        assert!(!can_edit(&p(DOWNLOADED_PLAYLIST_ID, None)));
        assert!(!can_edit(&p("PLyt123", Some("VLPLyt123"))));
    }

    #[test]
    fn custom_cover_is_a_file_link() {
        let mut p = Playlist { id: "LPabcdefgh".into(), ..Default::default() };
        assert!(!has_custom(&p));
        p.thumbnail_url = Some("https://img/x".into());
        assert!(!has_custom(&p));
        p.thumbnail_url = Some("file:///C:/covers/playlist-LPabcdefgh-1.jpg".into());
        assert!(has_custom(&p));
        assert_eq!(file_prefix("LP a/b"), "playlist-LP_a_b-");
    }
}
