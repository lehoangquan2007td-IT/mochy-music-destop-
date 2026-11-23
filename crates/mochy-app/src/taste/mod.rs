//! Taste profile ("Gu của bạn"): what the user likes, computed locally from
//! the listening history (plays, completion, skips, likes, playlists).
//!
//! Shared contract — other modules only use what is `pub` here:
//! - [`current`] / [`version`]: the latest published profile (cheap, any thread).
//! - [`refresh`]: recompute in the background (DB thread) and publish.
//! - [`TasteProfile`]'s query helpers (`artist_score`, `song_score`,
//!   `is_disliked`, `seeds`).
//! - [`rerank`]: order radio results by taste.
//!
//! `engine.rs` holds the scoring (`compute`).

mod engine;
pub mod mixes;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use mochy_core::models::Song;

pub use engine::compute;

/// Parts of the day, local time: 5–11, 12–16, 17–21, 22–4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Daypart {
    Morning,
    Afternoon,
    Evening,
    Night,
}

impl Daypart {
    pub const ALL: [Daypart; 4] = [Daypart::Morning, Daypart::Afternoon, Daypart::Evening, Daypart::Night];

    pub fn from_hour(h: u32) -> Self {
        match h {
            5..=11 => Daypart::Morning,
            12..=16 => Daypart::Afternoon,
            17..=21 => Daypart::Evening,
            _ => Daypart::Night,
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

/// One artist in the profile (artists under several ids are merged by name).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArtistTaste {
    /// Navigable id (YouTube `UC…` preferred) when known.
    pub id: Option<String>,
    pub name: String,
    pub thumbnail_url: Option<String>,
    /// Overall score (higher = liked more); not normalised.
    pub score: f32,
    /// Score from the last ~4 weeks only (current taste).
    pub short_term: f32,
    pub plays: u32,
    pub skips: u32,
    pub ms: i64,
}

/// One song in the profile.
#[derive(Debug, Clone, PartialEq)]
pub struct SongTaste {
    pub song: Song,
    pub score: f32,
    pub plays: u32,
    /// Plays that reached (almost) the end.
    pub completed: u32,
    pub skips: u32,
    pub liked: bool,
    /// Epoch millis of the last play (0 = never).
    pub last_played: i64,
}

#[derive(Debug, Clone, Default)]
pub struct TasteProfile {
    /// Epoch millis when computed.
    pub computed_at: i64,
    /// Events the profile is based on (0 = no history: callers should hide
    /// taste UI / skip reranking).
    pub total_events: usize,
    /// Sorted by `score`, best first.
    pub artists: Vec<ArtistTaste>,
    /// Sorted by `score`, best first.
    pub songs: Vec<SongTaste>,
    /// Song ids the user keeps skipping (don't recommend them).
    pub disliked: HashSet<String>,
    /// Top artists per [`Daypart`] (index with `Daypart::index`), best first.
    pub by_daypart: [Vec<ArtistTaste>; 4],
    /// Lowercased artist name → index in `artists`.
    pub artist_index: HashMap<String, usize>,
    /// Song id → index in `songs`.
    pub song_index: HashMap<String, usize>,
}

impl TasteProfile {
    pub fn is_empty(&self) -> bool {
        self.total_events == 0 && self.songs.is_empty()
    }

    /// Fill `artist_index` / `song_index` from the sorted lists.
    pub fn rebuild_index(&mut self) {
        self.artist_index = self.artists.iter().enumerate().map(|(i, a)| (a.name.trim().to_lowercase(), i)).collect();
        self.song_index = self.songs.iter().enumerate().map(|(i, s)| (s.song.id.clone(), i)).collect();
    }

    /// 0..=1, relative to the top artist (0 = unknown).
    pub fn artist_score(&self, name: &str) -> f32 {
        let top = self.artists.first().map(|a| a.score).unwrap_or(0.0);
        if top <= 0.0 {
            return 0.0;
        }
        self.artist_index
            .get(&name.trim().to_lowercase())
            .map(|&i| (self.artists[i].score / top).clamp(0.0, 1.0))
            .unwrap_or(0.0)
    }

    /// 0..=1, relative to the top song (0 = unknown / negative).
    pub fn song_score(&self, id: &str) -> f32 {
        let top = self.songs.first().map(|s| s.score).unwrap_or(0.0);
        if top <= 0.0 {
            return 0.0;
        }
        self.song_index.get(id).map(|&i| (self.songs[i].score / top).clamp(0.0, 1.0)).unwrap_or(0.0)
    }

    pub fn is_disliked(&self, id: &str) -> bool {
        self.disliked.contains(id)
    }

    /// Best `n` songs to seed mixes/radio with: high score, at most one per
    /// artist, never disliked.
    pub fn seeds(&self, n: usize) -> Vec<Song> {
        let mut seen_artists = HashSet::new();
        let mut out = Vec::new();
        for s in &self.songs {
            if out.len() >= n {
                break;
            }
            if s.score <= 0.0 || self.disliked.contains(&s.song.id) || s.song.video_id.is_none() {
                continue;
            }
            let artist = s.song.artists.first().map(|a| a.name.trim().to_lowercase()).unwrap_or_default();
            if !artist.is_empty() && !seen_artists.insert(artist) {
                continue;
            }
            out.push(s.song.clone());
        }
        out
    }
}

/// Order radio results by taste. Keeps YouTube's order as the base (it is
/// already relevant to the seed) and nudges: liked artists up, disliked songs
/// out. No-op for an empty profile.
pub fn rerank(profile: &TasteProfile, radio: Vec<Song>) -> Vec<Song> {
    engine::rerank(profile, radio)
}

static CURRENT: RwLock<Option<Arc<TasteProfile>>> = RwLock::new(None);
static VERSION: AtomicU64 = AtomicU64::new(0);

/// The latest published profile, if one was computed yet.
pub fn current() -> Option<Arc<TasteProfile>> {
    CURRENT.read().ok().and_then(|g| g.clone())
}

/// Bumped on every publish; compare to notice a new profile.
pub fn version() -> u64 {
    VERSION.load(Ordering::Acquire)
}

pub fn publish(p: TasteProfile) {
    if let Ok(mut g) = CURRENT.write() {
        *g = Some(Arc::new(p));
    }
    VERSION.fetch_add(1, Ordering::AcqRel);
}

/// Recompute on the DB thread and publish. `excluded_playlists`: local
/// playlist ids the user excluded from their taste.
pub fn refresh(svc: &crate::services::Services, excluded_playlists: Vec<String>) {
    let bus = svc.bus.clone();
    svc.db.run(move |db| {
        let excluded: HashSet<String> = excluded_playlists.into_iter().collect();
        match compute(db, &excluded, crate::browse::now_ms()) {
            Ok(p) => {
                tracing::info!(events = p.total_events, artists = p.artists.len(), songs = p.songs.len(), "taste profile computed");
                publish(p);
                bus.request_repaint();
            }
            Err(e) => tracing::warn!("taste profile: {e}"),
        }
    });
}
