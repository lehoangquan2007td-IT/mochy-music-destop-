//! Taste scoring.
//!
//! FIRST CUT (placeholder so other modules have real data): top songs and
//! artists of the last 90 days from the stats queries, scored by play count
//! and listening time. To be replaced by the full engine (completion, skips,
//! likes, playlists, recency decay, dayparts, excluded playlists).

use std::collections::HashSet;

use mochy_core::models::Song;
use mochy_storage::{Db, StorageResult};

use super::{ArtistTaste, SongTaste, TasteProfile};

const DAY_MS: i64 = 86_400_000;

pub fn compute(db: &Db, _excluded_playlists: &HashSet<String>, now: i64) -> StorageResult<TasteProfile> {
    let from = now - 90 * DAY_MS;
    let liked: HashSet<String> = db.liked_songs()?.into_iter().map(|s| s.id).collect();
    let songs: Vec<SongTaste> = db
        .top_songs_between(from, now + 1, 200)?
        .into_iter()
        .map(|s| {
            let is_liked = liked.contains(&s.song.id);
            let score = s.plays as f32 + (s.ms as f32 / 60_000.0) * 0.25 + if is_liked { 3.0 } else { 0.0 };
            SongTaste { song: s.song, score, plays: s.plays, completed: 0, skips: 0, liked: is_liked, last_played: 0 }
        })
        .collect();
    let artists: Vec<ArtistTaste> = db
        .top_artists_between(from, now + 1, 100)?
        .into_iter()
        .map(|a| ArtistTaste {
            id: Some(a.artist.id.clone()),
            name: a.artist.name.clone(),
            thumbnail_url: a.artist.thumbnail_url.clone().or(a.fallback_thumbnail.clone()),
            score: a.ms as f32 / 60_000.0,
            short_term: 0.0,
            plays: a.plays,
            skips: 0,
            ms: a.ms,
        })
        .collect();
    let total_events = songs.iter().map(|s| s.plays as usize).sum();
    let mut p = TasteProfile { computed_at: now, total_events, artists, songs, ..Default::default() };
    p.songs.sort_by(|a, b| b.score.total_cmp(&a.score));
    p.rebuild_index();
    Ok(p)
}

pub fn rerank(profile: &TasteProfile, radio: Vec<Song>) -> Vec<Song> {
    if profile.is_empty() {
        return radio;
    }
    radio.into_iter().filter(|s| !profile.is_disliked(&s.id)).collect()
}
