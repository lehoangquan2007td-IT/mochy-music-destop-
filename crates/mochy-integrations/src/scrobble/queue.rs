//! Plays waiting to be sent, kept in a JSON file so they outlast the app
//! closing or a week offline (Android `ScrobbleQueue`). Each play remembers
//! which services still need it, so one service being down doesn't hold back
//! the other.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::audioscrobbler::ScrobbleTrack;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ScrobbleSite {
    #[serde(rename = "LASTFM")]
    LastFm,
    #[serde(rename = "LIBREFM")]
    LibreFm,
}

impl ScrobbleSite {
    pub const ALL: [ScrobbleSite; 2] = [ScrobbleSite::LastFm, ScrobbleSite::LibreFm];

    pub fn label(self) -> &'static str {
        match self {
            ScrobbleSite::LastFm => "Last.fm",
            ScrobbleSite::LibreFm => "Libre.fm",
        }
    }

    pub fn profile_url(self, username: &str) -> String {
        match self {
            ScrobbleSite::LastFm => format!("https://www.last.fm/user/{username}"),
            ScrobbleSite::LibreFm => format!("https://libre.fm/user/{username}"),
        }
    }

    pub fn sign_up_url(self) -> &'static str {
        match self {
            ScrobbleSite::LastFm => "https://www.last.fm/join",
            ScrobbleSite::LibreFm => "https://libre.fm/register.php",
        }
    }

    /// Where the user creates their own API key/secret.
    pub fn api_account_url(self) -> &'static str {
        match self {
            ScrobbleSite::LastFm => "https://www.last.fm/api/account/create",
            ScrobbleSite::LibreFm => "https://libre.fm/",
        }
    }
}

/// A play waiting to be sent, and the services that still need it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingScrobble {
    pub track: ScrobbleTrack,
    /// When it started playing, unix seconds.
    pub started_at: i64,
    pub sites: BTreeSet<ScrobbleSite>,
}

pub const MAX_PLAYS: usize = 5000;
/// Last.fm turns away scrobbles more than two weeks old.
pub const MAX_AGE_SECONDS: i64 = 14 * 24 * 60 * 60;

pub struct ScrobbleQueue {
    file: Option<PathBuf>,
    plays: Vec<PendingScrobble>,
}

impl ScrobbleQueue {
    /// Load from `file` (a missing or cut-short file starts empty).
    pub fn open(file: PathBuf) -> Self {
        let plays = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| match serde_json::from_str::<Vec<PendingScrobble>>(&t) {
                Ok(p) => Some(p),
                Err(e) => {
                    tracing::warn!("couldn't read the scrobble queue, starting it again: {e}");
                    None
                }
            })
            .unwrap_or_default();
        Self { file: Some(file), plays }
    }

    /// In memory only (tests).
    pub fn in_memory() -> Self {
        Self { file: None, plays: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.plays.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plays.is_empty()
    }

    pub fn add(&mut self, play: PendingScrobble) {
        self.plays.push(play);
        // weeks offline with music on all day stays well under this
        if self.plays.len() > MAX_PLAYS {
            let extra = self.plays.len() - MAX_PLAYS;
            self.plays.drain(..extra);
        }
        self.save();
    }

    /// The oldest plays `site` still needs, up to `max`. Plays too old for
    /// any service are dropped.
    pub fn next_for(&mut self, site: ScrobbleSite, max: usize, now_secs: i64) -> Vec<PendingScrobble> {
        let before = self.plays.len();
        let oldest = now_secs - MAX_AGE_SECONDS;
        self.plays.retain(|p| p.started_at >= oldest);
        if self.plays.len() != before {
            self.save();
        }
        self.plays.iter().filter(|p| p.sites.contains(&site)).take(max).cloned().collect()
    }

    /// `sent` reached `site`. A play no other service waits for is done with.
    pub fn mark_sent(&mut self, site: ScrobbleSite, sent: &[PendingScrobble]) {
        // by play rather than by object: the other service may have been marked in between
        let done: HashSet<(i64, &ScrobbleTrack)> = sent.iter().map(|p| (p.started_at, &p.track)).collect();
        let mut changed = false;
        for p in &mut self.plays {
            if done.contains(&(p.started_at, &p.track)) && p.sites.remove(&site) {
                changed = true;
            }
        }
        self.plays.retain(|p| !p.sites.is_empty());
        if changed {
            self.save();
        }
    }

    /// Signed out of `site`: nothing waiting for it can be sent any more.
    pub fn forget(&mut self, site: ScrobbleSite) {
        let before = self.plays.len();
        let mut changed = false;
        for p in &mut self.plays {
            changed |= p.sites.remove(&site);
        }
        self.plays.retain(|p| !p.sites.is_empty());
        if changed || self.plays.len() != before {
            self.save();
        }
    }

    fn save(&self) {
        let Some(file) = &self.file else { return };
        let Ok(json) = serde_json::to_string(&self.plays) else { return };
        // write-then-rename so a crash mid-write never leaves half a file
        let tmp = file.with_extension("json.tmp");
        let r = std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, file));
        if let Err(e) = r {
            tracing::warn!("couldn't save the scrobble queue: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(track: &str, at: i64, sites: &[ScrobbleSite]) -> PendingScrobble {
        PendingScrobble {
            track: ScrobbleTrack { artist: "A".into(), track: track.into(), album: None, duration_seconds: Some(200) },
            started_at: at,
            sites: sites.iter().copied().collect(),
        }
    }

    #[test]
    fn per_site_bookkeeping() {
        let mut q = ScrobbleQueue::in_memory();
        let both = [ScrobbleSite::LastFm, ScrobbleSite::LibreFm];
        q.add(play("1", 1000, &both));
        q.add(play("2", 1001, &[ScrobbleSite::LastFm]));
        let now = 2000;
        let lf = q.next_for(ScrobbleSite::LastFm, 50, now);
        assert_eq!(lf.len(), 2);
        q.mark_sent(ScrobbleSite::LastFm, &lf);
        // "1" still waits for Libre.fm, "2" is done.
        assert_eq!(q.len(), 1);
        assert_eq!(q.next_for(ScrobbleSite::LastFm, 50, now).len(), 0);
        let libre = q.next_for(ScrobbleSite::LibreFm, 50, now);
        assert_eq!(libre.len(), 1);
        q.forget(ScrobbleSite::LibreFm);
        assert!(q.is_empty());
    }

    #[test]
    fn old_plays_and_cap() {
        let mut q = ScrobbleQueue::in_memory();
        q.add(play("old", 0, &[ScrobbleSite::LastFm]));
        q.add(play("new", MAX_AGE_SECONDS + 10, &[ScrobbleSite::LastFm]));
        let batch = q.next_for(ScrobbleSite::LastFm, 50, MAX_AGE_SECONDS + 20);
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].track.track, "new");
        for i in 0..(MAX_PLAYS + 5) {
            q.add(play("x", i as i64 + 1_000_000_000, &[ScrobbleSite::LastFm]));
        }
        assert_eq!(q.len(), MAX_PLAYS);
        assert_eq!(q.next_for(ScrobbleSite::LastFm, 2, 1_000_000_100).len(), 2);
    }

    #[test]
    fn persists_to_file() {
        let dir = std::env::temp_dir().join(format!("mochy-scrobble-q-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("scrobble_queue.json");
        let _ = std::fs::remove_file(&file);
        {
            let mut q = ScrobbleQueue::open(file.clone());
            q.add(play("1", 5, &[ScrobbleSite::LastFm]));
        }
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"startedAt\":5"));
        assert!(text.contains("\"LASTFM\""));
        let q = ScrobbleQueue::open(file.clone());
        assert_eq!(q.len(), 1);
        std::fs::write(&file, "[{\"broken").unwrap();
        assert!(ScrobbleQueue::open(file.clone()).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
