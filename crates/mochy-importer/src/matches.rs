//! The YouTube song chosen for each imported track (Android
//! `ImportMatches.kt`): hand picks win in every later import and re-sync.
//! Kept in a small JSON file (`import_matches.json`) instead of a table.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use mochy_core::models::{ArtistRef, Song};
use serde::{Deserialize, Serialize};

use crate::matcher::{normalize_text, normalize_title, split_artists};
use crate::models::{ImportSource, ImportedTrack};

/// One track from another service and the YouTube song it became.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackMatch {
    pub key: String,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    /// An `ImportSource` key.
    pub source: String,
    pub song_id: String,
    /// Picked by hand: every later import uses this song without searching.
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub song_title: Option<String>,
    #[serde(default)]
    pub song_artists: Vec<String>,
    #[serde(default)]
    pub song_duration: Option<i64>,
    #[serde(default)]
    pub song_thumbnail: Option<String>,
}

impl TrackMatch {
    pub fn track(&self) -> ImportedTrack {
        ImportedTrack { duration_ms: self.duration_ms, ..ImportedTrack::new(self.title.clone(), self.artist.clone(), self.album.clone()) }
    }

    pub fn pinned_song(&self) -> Song {
        let mut s = Song::remote(self.song_id.clone(), self.song_title.clone().unwrap_or_else(|| self.title.clone()));
        let names = if self.song_artists.is_empty() { vec![self.artist.clone()] } else { self.song_artists.clone() };
        s.artists = names.into_iter().map(ArtistRef::new).collect();
        s.duration_secs = self.song_duration.map(|d| d as f64);
        s.thumbnail_url = Some(self.song_thumbnail.clone().unwrap_or_else(|| format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", self.song_id)));
        s
    }
}

/// What re-sync knows a service's song by: exact for YouTube songs, else the
/// cleaned title and first artist, which match whether read by link or account.
pub fn track_key(t: &ImportedTrack) -> String {
    if let Some(id) = t.video_id.as_ref().or(t.yt_item.as_ref().map(|s| &s.id)) {
        return youtube_key(id);
    }
    let first = split_artists(&t.artist).into_iter().next().unwrap_or_default();
    format!("{}|{}", normalize_title(&t.title), normalize_text(&first))
}

pub fn youtube_key(video_id: &str) -> String {
    format!("yt:{video_id}")
}

/// What a pick is saved under: the service's own id (survives a rename), else title and artist.
pub fn match_key(t: &ImportedTrack) -> String {
    t.source_id.clone().unwrap_or_else(|| track_key(t))
}

pub struct ImportMatches {
    file: Option<PathBuf>,
    state: Mutex<(Option<HashMap<String, TrackMatch>>, bool)>,
}

impl ImportMatches {
    pub fn open(file: PathBuf) -> Self {
        Self { file: Some(file), state: Mutex::new((None, false)) }
    }

    pub fn in_memory() -> Self {
        Self { file: None, state: Mutex::new((Some(HashMap::new()), false)) }
    }

    fn with<R>(&self, f: impl FnOnce(&mut HashMap<String, TrackMatch>, &mut bool) -> R) -> R {
        let mut g = self.state.lock().unwrap();
        if g.0.is_none() {
            g.0 = Some(self.load());
        }
        let (map, dirty) = &mut *g;
        f(map.as_mut().unwrap(), dirty)
    }

    /// The song the user picked for this track (older picks saved by title and artist too).
    pub fn pinned(&self, t: &ImportedTrack) -> Option<TrackMatch> {
        self.with(|m, _| {
            m.get(&match_key(t)).filter(|x| x.pinned).or_else(|| m.get(&track_key(t)).filter(|x| x.pinned)).cloned()
        })
    }

    /// Notes the song an import found for `t`. A hand pick is never replaced here.
    pub fn remember(&self, t: &ImportedTrack, source: ImportSource, song: &Song) {
        let key = match_key(t);
        if key.starts_with("yt:") {
            return;
        }
        self.with(|m, dirty| {
            if let Some(e) = m.get(&key)
                && (e.pinned || e.song_id == song.id)
            {
                return;
            }
            m.insert(
                key.clone(),
                TrackMatch {
                    key,
                    title: t.title.clone(),
                    artist: t.artist.clone(),
                    album: t.album.clone(),
                    duration_ms: t.duration_ms,
                    source: source.key().to_string(),
                    song_id: song.id.clone(),
                    pinned: false,
                    song_title: None,
                    song_artists: Vec::new(),
                    song_duration: None,
                    song_thumbnail: None,
                },
            );
            *dirty = true;
        });
    }

    /// The track `song_id` was imported as, a hand-picked one first.
    pub fn for_song(&self, song_id: &str) -> Option<TrackMatch> {
        self.with(|m, _| {
            let all: Vec<&TrackMatch> = m.values().filter(|x| x.song_id == song_id).collect();
            all.iter().find(|x| x.pinned).or(all.first()).map(|x| (*x).clone())
        })
    }

    /// From now on `track`'s match is `song`, in this import and every later one.
    pub fn pin_track(&self, t: &ImportedTrack, source: ImportSource, song: &Song) {
        let key = match_key(t);
        self.with(|m, dirty| {
            m.insert(
                key.clone(),
                TrackMatch {
                    key,
                    title: t.title.clone(),
                    artist: t.artist.clone(),
                    album: t.album.clone(),
                    duration_ms: t.duration_ms,
                    source: source.key().to_string(),
                    song_id: song.id.clone(),
                    pinned: true,
                    song_title: Some(song.title.clone()),
                    song_artists: song.artists.iter().map(|a| a.name.clone()).collect(),
                    song_duration: song.duration_secs.map(|d| d as i64),
                    song_thumbnail: song.thumbnail_url.clone(),
                },
            );
            *dirty = true;
        });
        self.flush();
    }

    /// Lets imports search for the track again.
    pub fn unpin(&self, key: &str) {
        self.with(|m, dirty| {
            if let Some(e) = m.get_mut(key)
                && e.pinned
            {
                e.pinned = false;
                *dirty = true;
            }
        });
        self.flush();
    }

    /// Writes what changed since the last write.
    pub fn flush(&self) {
        let Some(file) = &self.file else { return };
        let mut g = self.state.lock().unwrap();
        if !g.1 {
            return;
        }
        let Some(map) = &g.0 else { return };
        let list: Vec<&TrackMatch> = map.values().collect();
        let Ok(json) = serde_json::to_string(&list) else { return };
        let tmp = file.with_extension("json.tmp");
        match std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, file)) {
            Ok(()) => g.1 = false,
            Err(e) => tracing::warn!("couldn't save import matches: {e}"),
        }
    }

    fn load(&self) -> HashMap<String, TrackMatch> {
        let Some(file) = &self.file else { return HashMap::new() };
        let list: Vec<TrackMatch> = std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        list.into_iter().map(|m| (m.key.clone(), m)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Port of `PlaylistSourceRecordTest` (re-sync keys).
    #[test]
    fn same_song_read_by_link_or_account_keys_the_same() {
        // Spotify's embed page joins the artists into one subtitle; its account API lists them
        let by_link = ImportedTrack::new("Starboy (feat. Daft Punk)", "The Weeknd, Daft Punk", None);
        let by_account = ImportedTrack { duration_ms: Some(230_000), ..ImportedTrack::new("Starboy", "The Weeknd", Some("Starboy".into())) };
        assert_eq!(track_key(&by_link), track_key(&by_account));
        let apple = ImportedTrack::new("Café", "Artist & Friend", None);
        assert_eq!(track_key(&apple), track_key(&ImportedTrack::new("Cafe", "Artist", None)));
    }

    #[test]
    fn youtube_songs_key_by_their_id() {
        let song = ImportedTrack { video_id: Some("abc123".into()), ..ImportedTrack::new("Anything", "Anyone", None) };
        assert_eq!(track_key(&song), "yt:abc123");
    }

    #[test]
    fn different_songs_key_apart() {
        let original = ImportedTrack::new("Song", "Band", None);
        assert_ne!(track_key(&original), track_key(&ImportedTrack::new("Song (Remix)", "Band", None)));
        assert_ne!(track_key(&original), track_key(&ImportedTrack::new("Song", "Other Band", None)));
    }

    #[test]
    fn pins_win_and_persist() {
        let dir = std::env::temp_dir().join(format!("mochy-matches-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("import_matches.json");
        let _ = std::fs::remove_file(&file);
        let t = ImportedTrack { source_id: Some("spotify:track:1".into()), ..ImportedTrack::new("Song", "Band", None) };
        let found = Song::remote("found000001", "Song");
        let picked = Song::remote("picked00001", "Song (Album)");
        {
            let m = ImportMatches::open(file.clone());
            m.remember(&t, ImportSource::Spotify, &found);
            assert!(m.pinned(&t).is_none());
            m.pin_track(&t, ImportSource::Spotify, &picked);
            // a later search result never replaces a pick
            m.remember(&t, ImportSource::Spotify, &found);
            m.flush();
        }
        let m = ImportMatches::open(file.clone());
        let p = m.pinned(&t).unwrap();
        assert_eq!(p.song_id, "picked00001");
        assert_eq!(p.pinned_song().title, "Song (Album)");
        assert_eq!(m.for_song("picked00001").unwrap().key, "spotify:track:1");
        m.unpin("spotify:track:1");
        assert!(m.pinned(&t).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
