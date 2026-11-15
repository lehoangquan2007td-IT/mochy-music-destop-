//! Local files: recursive folder scan (background thread), tag / duration
//! reading via lofty, ReplayGain lookup at play time.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mochy_core::models::{ArtistRef, Song};

use crate::services::{AppMsg, ScanMsg, Services};

pub const AUDIO_EXTS: &[&str] = &["mp3", "flac", "m4a", "aac", "mp4", "ogg", "oga", "opus", "wav", "aiff", "aif", "wv", "ape"];

pub fn is_audio_file(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Recursively collect audio files under `dir` (symlink loops are avoided by
/// not following directory symlinks).
pub fn collect_audio_files(dir: &Path, cancel: &AtomicBool, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        if ft.is_dir() {
            collect_audio_files(&p, cancel, out);
        } else if ft.is_file() && is_audio_file(&p) {
            out.push(p);
        }
    }
}

/// Build a library `Song` for a local file from its tags.
pub fn song_from_file(path: &Path) -> Song {
    use lofty::prelude::*;
    let file_stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut song = Song {
        id: mochy_core::ids::song_id_for_path(path),
        title: file_stem,
        local_path: Some(path.to_path_buf()),
        in_library: true,
        ..Default::default()
    };
    match lofty::read_from_path(path) {
        Ok(tagged) => {
            let d = tagged.properties().duration().as_secs_f64();
            if d > 0.0 {
                song.duration_secs = Some(d);
            }
            if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
                if let Some(t) = tag.title().filter(|t| !t.trim().is_empty()) {
                    song.title = t.trim().to_string();
                }
                if let Some(a) = tag.artist().filter(|a| !a.trim().is_empty()) {
                    song.artists = split_artists(&a);
                }
                if let Some(al) = tag.album().filter(|a| !a.trim().is_empty()) {
                    song.album_name = Some(al.trim().to_string());
                }
                song.year = tag.date().map(|d| d.year as i32);
            }
        }
        Err(e) => tracing::debug!(path = %path.display(), "tag read failed: {e}"),
    }
    song
}

fn split_artists(s: &str) -> Vec<ArtistRef> {
    s.split([';', '/', '\0'])
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(ArtistRef::new)
        .collect()
}

/// ReplayGain track gain (dB) and peak from the file's tags.
pub fn read_replay_gain(path: &Path) -> (Option<f32>, Option<f32>) {
    use lofty::prelude::*;
    let Ok(tagged) = lofty::read_from_path(path) else { return (None, None) };
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else { return (None, None) };
    let parse = |s: &str| {
        s.trim()
            .trim_end_matches("dB")
            .trim_end_matches("db")
            .trim()
            .parse::<f32>()
            .ok()
    };
    let gain = tag.get_string(ItemKey::ReplayGainTrackGain).and_then(parse);
    let peak = tag.get_string(ItemKey::ReplayGainTrackPeak).and_then(parse);
    (gain, peak)
}

/// Scan `dir` in a background thread, adding every audio file to the DB.
/// Progress / completion is reported over the bus.
pub fn spawn_scan(svc: Services, dir: PathBuf, cancel: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("mochy-scan".into())
        .spawn(move || {
            svc.bus.send(AppMsg::Scan(ScanMsg::Started(dir.clone())));
            tracing::info!(dir = %dir.display(), "scanning local folder");
            let mut files = Vec::new();
            collect_audio_files(&dir, &cancel, &mut files);
            let total = files.len();
            let mut added = 0usize;
            for (i, f) in files.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    svc.bus.send(AppMsg::Scan(ScanMsg::Cancelled));
                    return;
                }
                let song = song_from_file(f);
                svc.db.run(move |db| {
                    if let Err(e) = db.add_local_song(&song) {
                        tracing::warn!("add_local_song failed: {e}");
                    }
                });
                added += 1;
                if i % 25 == 0 || i + 1 == total {
                    svc.bus.send(AppMsg::Scan(ScanMsg::Progress { scanned: i + 1, added }));
                }
            }
            tracing::info!(added, "local scan done");
            svc.bus.send(AppMsg::Scan(ScanMsg::Done { added }));
        })
        .expect("spawn scan thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_extensions() {
        assert!(is_audio_file(Path::new("a/b/song.MP3")));
        assert!(is_audio_file(Path::new("x.flac")));
        assert!(!is_audio_file(Path::new("cover.jpg")));
        assert!(!is_audio_file(Path::new("noext")));
    }

    #[test]
    fn artist_splitting() {
        let a = split_artists("A; B / C");
        assert_eq!(a.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), vec!["A", "B", "C"]);
    }

    #[test]
    fn untagged_file_falls_back_to_file_name() {
        let s = song_from_file(Path::new("does/not/exist/My Track.mp3"));
        assert_eq!(s.title, "My Track");
        assert!(s.id.starts_with("LOC_"));
        assert!(s.is_local());
    }
}
