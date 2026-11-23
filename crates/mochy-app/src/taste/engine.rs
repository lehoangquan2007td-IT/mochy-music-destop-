//! Taste scoring (`compute`) and radio reranking (`rerank`).
//!
//! Song score, from the last two years of history:
//!
//! ```text
//! score = Σ plays  w(play) · ½^(age / 30 d)          w = 1 completed, 0.2 + 0.5 · fraction otherwise
//!       − Σ skips  0.75 · ½^(age / 21 d)              (recent skips weigh more)
//!       + 2 if liked + 0.75 per local playlist (max 1.5)
//! ```
//!
//! A play is *completed* at ≥ 80 % of the duration or within 15 s of the
//! end. Artists sum their songs' scores (featured artists at half weight),
//! merged by lowercased name. `short_term` is the same sum over the last 28
//! days (likes of that period included, playlists not). Dayparts use the
//! play/skip part only, bucketed by local hour. Songs of an excluded
//! playlist don't count at all unless liked.

use std::collections::{HashMap, HashSet, VecDeque};

use mochy_core::models::Song;
use mochy_storage::{Db, SkipRow, StorageResult, TastePlay};

use super::{ArtistTaste, Daypart, SongTaste, TasteProfile};

const DAY_MS: i64 = 86_400_000;
/// History older than this is ignored (its weight is < 2⁻²⁴ anyway).
const WINDOW_DAYS: i64 = 730;
const HALF_LIFE_DAYS: f64 = 30.0;
const SKIP_HALF_LIFE_DAYS: f64 = 21.0;
const SHORT_TERM_DAYS: i64 = 28;
/// Weight of a play that stopped right away / the extra for listening it all.
const PARTIAL_BASE: f32 = 0.2;
const PARTIAL_SPAN: f32 = 0.5;
/// Without a known duration a play counts as complete after this long.
const UNKNOWN_FULL_MS: i64 = 150_000;
const SKIP_WEIGHT: f32 = 0.75;
const LIKED_BONUS: f32 = 2.0;
const PLAYLIST_BONUS: f32 = 0.75;
const PLAYLIST_BONUS_MAX: f32 = 1.5;
/// Share of a song's score credited to its 2nd, 3rd… artists.
const FEATURED_SHARE: f32 = 0.5;
const DAYPART_TOP: usize = 20;

/// Was a play complete, and how much it counts (0.2..=1).
fn play_weight(play_ms: i64, duration_ms: Option<i64>) -> (bool, f32) {
    let play_ms = play_ms.max(0);
    let (completed, frac) = match duration_ms.filter(|d| *d > 0) {
        Some(d) => (play_ms * 5 >= d * 4 || play_ms >= d - 15_000, play_ms as f32 / d as f32),
        None => (play_ms >= UNKNOWN_FULL_MS, play_ms as f32 / UNKNOWN_FULL_MS as f32),
    };
    if completed { (true, 1.0) } else { (false, PARTIAL_BASE + PARTIAL_SPAN * frac.clamp(0.0, 1.0)) }
}

fn decay(now: i64, ts: i64, half_life_days: f64) -> f32 {
    let age_days = (now - ts).max(0) as f64 / DAY_MS as f64;
    0.5f64.powf(age_days / half_life_days) as f32
}

fn daypart_of(ts: i64, utc_offset_secs: i64) -> Daypart {
    let local = ts.div_euclid(1000) + utc_offset_secs;
    Daypart::from_hour((local.rem_euclid(86_400) / 3600) as u32)
}

fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Per-song history totals.
#[derive(Default)]
struct SongAcc {
    score: f32,
    short: f32,
    plays: u32,
    completed: u32,
    skips: u32,
    ms: i64,
    last_played: i64,
    events: usize,
    dp_score: [f32; 4],
    dp_plays: [u32; 4],
    dp_skips: [u32; 4],
    dp_ms: [i64; 4],
}

#[derive(Default)]
struct ArtistAcc {
    t: ArtistTaste,
    /// Ids merged into this name (for the thumbnail lookup).
    ids: Vec<String>,
    best_song: f32,
    song_thumb: Option<String>,
    dp_score: [f32; 4],
    dp_plays: [u32; 4],
    dp_skips: [u32; 4],
    dp_ms: [i64; 4],
}

/// Everything `compute` reads from the database.
struct Input {
    plays: Vec<TastePlay>,
    skips: Vec<SkipRow>,
    songs: Vec<Song>,
    /// `(playlist id, song id)` of local playlists.
    playlist_songs: Vec<(String, String)>,
    artist_thumbs: HashMap<String, String>,
    utc_offset_secs: i64,
}

pub fn compute(db: &Db, excluded_playlists: &HashSet<String>, now: i64) -> StorageResult<TasteProfile> {
    let from = now - WINDOW_DAYS * DAY_MS;
    let input = Input {
        plays: db.taste_plays_since(from)?,
        skips: db.skips_since(from)?,
        songs: db.taste_songs(from)?,
        playlist_songs: db.local_playlist_song_ids()?,
        artist_thumbs: db.artist_thumbnails()?,
        utc_offset_secs: db.local_utc_offset_secs().unwrap_or(0),
    };
    Ok(build(input, excluded_playlists, now))
}

fn build(input: Input, excluded: &HashSet<String>, now: i64) -> TasteProfile {
    let short_from = now - SHORT_TERM_DAYS * DAY_MS;
    let songs: HashMap<&str, &Song> = input.songs.iter().map(|s| (s.id.as_str(), s)).collect();

    // Playlists: songs of excluded ones are dropped (unless liked), the
    // others get a bonus per playlist.
    let mut in_playlists: HashMap<&str, HashSet<&str>> = HashMap::new();
    let mut dropped: HashSet<&str> = HashSet::new();
    for (pl, id) in &input.playlist_songs {
        if excluded.contains(pl) {
            if songs.get(id.as_str()).is_none_or(|s| !s.liked) {
                dropped.insert(id.as_str());
            }
        } else {
            in_playlists.entry(id.as_str()).or_default().insert(pl.as_str());
        }
    }

    let mut acc: HashMap<&str, SongAcc> = HashMap::new();
    for p in &input.plays {
        if dropped.contains(p.song_id.as_str()) || !songs.contains_key(p.song_id.as_str()) {
            continue;
        }
        let (completed, w) = play_weight(p.play_ms, p.duration_ms);
        let v = w * decay(now, p.timestamp, HALF_LIFE_DAYS);
        let dp = daypart_of(p.timestamp, input.utc_offset_secs).index();
        let a = acc.entry(p.song_id.as_str()).or_default();
        a.score += v;
        if p.timestamp >= short_from {
            a.short += v;
        }
        a.plays += 1;
        a.completed += completed as u32;
        a.ms += p.play_ms.max(0);
        a.last_played = a.last_played.max(p.timestamp);
        a.events += 1;
        a.dp_score[dp] += v;
        a.dp_plays[dp] += 1;
        a.dp_ms[dp] += p.play_ms.max(0);
    }
    for k in &input.skips {
        if dropped.contains(k.song_id.as_str()) || !songs.contains_key(k.song_id.as_str()) {
            continue;
        }
        let v = SKIP_WEIGHT * decay(now, k.timestamp, SKIP_HALF_LIFE_DAYS);
        let dp = daypart_of(k.timestamp, input.utc_offset_secs).index();
        let a = acc.entry(k.song_id.as_str()).or_default();
        a.score -= v;
        if k.timestamp >= short_from {
            a.short -= v;
        }
        a.skips += 1;
        a.events += 1;
        a.dp_score[dp] -= v;
        a.dp_skips[dp] += 1;
    }

    let mut total_events = 0usize;
    let mut disliked = HashSet::new();
    // (song taste, short-term score, history totals)
    let mut scored: Vec<(SongTaste, f32, SongAcc)> = Vec::new();
    for s in &input.songs {
        if dropped.contains(s.id.as_str()) {
            continue;
        }
        let a = acc.remove(s.id.as_str()).unwrap_or_default();
        let playlists = in_playlists.get(s.id.as_str()).map_or(0, |p| p.len());
        if a.events == 0 && !s.liked && playlists == 0 {
            continue;
        }
        total_events += a.events;
        let mut score = a.score + (PLAYLIST_BONUS * playlists as f32).min(PLAYLIST_BONUS_MAX);
        let mut short = a.short;
        if s.liked {
            score += LIKED_BONUS;
            if s.liked_date.is_some_and(|d| d >= short_from) {
                short += LIKED_BONUS;
            }
        }
        if !s.liked && a.skips >= 2 && a.skips > 2 * a.completed {
            disliked.insert(s.id.clone());
        }
        let t = SongTaste {
            song: s.clone(),
            score,
            plays: a.plays,
            completed: a.completed,
            skips: a.skips,
            liked: s.liked,
            last_played: a.last_played,
        };
        scored.push((t, short, a));
    }
    scored.sort_by(|a, b| {
        b.0.score.total_cmp(&a.0.score).then(b.0.last_played.cmp(&a.0.last_played)).then_with(|| a.0.song.id.cmp(&b.0.song.id))
    });

    // Artists, merged by name; songs are visited best first.
    let mut artists: Vec<ArtistAcc> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (t, short, a) in &scored {
        let mut seen = HashSet::new();
        for (pos, ar) in t.song.artists.iter().enumerate() {
            let key = name_key(&ar.name);
            if key.is_empty() || !seen.insert(key.clone()) {
                continue;
            }
            let share = if pos == 0 { 1.0 } else { FEATURED_SHARE };
            let i = *index.entry(key).or_insert_with(|| {
                artists.push(ArtistAcc {
                    t: ArtistTaste { name: ar.name.trim().to_string(), ..Default::default() },
                    best_song: f32::NEG_INFINITY,
                    ..Default::default()
                });
                artists.len() - 1
            });
            let m = &mut artists[i];
            if let Some(id) = ar.id.as_deref().filter(|id| !id.is_empty()) {
                if m.t.id.as_deref().is_none_or(|cur| !cur.starts_with("UC") && id.starts_with("UC")) {
                    m.t.id = Some(id.to_string());
                }
                if !m.ids.iter().any(|x| x == id) {
                    m.ids.push(id.to_string());
                }
            }
            m.t.score += share * t.score;
            m.t.short_term += share * short;
            m.t.plays += a.plays;
            m.t.skips += a.skips;
            m.t.ms += a.ms;
            for d in 0..4 {
                m.dp_score[d] += share * a.dp_score[d];
                m.dp_plays[d] += a.dp_plays[d];
                m.dp_skips[d] += a.dp_skips[d];
                m.dp_ms[d] += a.dp_ms[d];
            }
            if t.song.thumbnail_url.is_some() && t.score > m.best_song {
                m.best_song = t.score;
                m.song_thumb = t.song.thumbnail_url.clone();
            }
        }
    }
    for m in &mut artists {
        // The navigable id's picture first, then any merged id's, then the top song's art.
        let own = m.t.id.as_ref().and_then(|id| input.artist_thumbs.get(id));
        m.t.thumbnail_url = own.or_else(|| m.ids.iter().find_map(|id| input.artist_thumbs.get(id))).cloned().or_else(|| m.song_thumb.clone());
    }
    let by_score = |a: &ArtistTaste, b: &ArtistTaste| {
        b.score.total_cmp(&a.score).then(b.plays.cmp(&a.plays)).then_with(|| a.name.cmp(&b.name))
    };
    let mut by_daypart: [Vec<ArtistTaste>; 4] = Default::default();
    for (d, list) in by_daypart.iter_mut().enumerate() {
        *list = artists
            .iter()
            .filter(|m| m.dp_score[d] > 0.0)
            .map(|m| ArtistTaste {
                score: m.dp_score[d],
                plays: m.dp_plays[d],
                skips: m.dp_skips[d],
                ms: m.dp_ms[d],
                ..m.t.clone()
            })
            .collect();
        list.sort_by(by_score);
        list.truncate(DAYPART_TOP);
    }
    let mut artists: Vec<ArtistTaste> = artists.into_iter().map(|m| m.t).collect();
    artists.sort_by(by_score);

    let mut p = TasteProfile {
        computed_at: now,
        total_events,
        artists,
        songs: scored.into_iter().map(|(t, _, _)| t).collect(),
        disliked,
        by_daypart,
        ..Default::default()
    };
    p.rebuild_index();
    p
}

// ---------------------------------------------------------------------------
// Radio
// ---------------------------------------------------------------------------

/// Share of the list a perfect-taste song can climb / a just-heard one sinks.
const BOOST_SHARE: f32 = 0.25;
const DEMOTE_SHARE: f32 = 0.33;
/// Songs played less than this long ago sink.
const RECENT_MS: i64 = DAY_MS;
/// At most this many songs of one artist in a row.
const MAX_RUN: usize = 2;

pub fn rerank(profile: &TasteProfile, radio: Vec<Song>) -> Vec<Song> {
    rerank_at(profile, radio, profile.computed_at)
}

/// [`rerank`] with an explicit "now" (epoch ms) for the recency demotion.
fn rerank_at(profile: &TasteProfile, radio: Vec<Song>, now: i64) -> Vec<Song> {
    if profile.is_empty() {
        return radio;
    }
    let kept: Vec<Song> = radio.into_iter().filter(|s| !profile.is_disliked(&s.id)).collect();
    let n = kept.len();
    if n < 2 {
        return kept;
    }
    let top_short = profile.artists.iter().map(|a| a.short_term).fold(0.0f32, f32::max);
    let boost_span = (n as f32 * BOOST_SHARE).max(3.0);
    let demote_span = (n as f32 * DEMOTE_SHARE).max(5.0);
    let mut keyed: Vec<(f32, usize, Song)> = kept
        .into_iter()
        .enumerate()
        .map(|(i, s)| {
            // How much the user likes the song's artists, 0..=1.
            let affinity = s
                .artists
                .iter()
                .enumerate()
                .map(|(k, a)| {
                    let long = profile.artist_score(&a.name).sqrt();
                    let short = match profile.artist_index.get(&name_key(&a.name)) {
                        Some(&j) if top_short > 0.0 => (profile.artists[j].short_term / top_short).clamp(0.0, 1.0).sqrt(),
                        _ => 0.0,
                    };
                    let share = if k == 0 { 1.0 } else { FEATURED_SHARE };
                    share * (0.6 * long + 0.4 * short)
                })
                .fold(0.0f32, f32::max);
            let boost = (affinity + 0.25 * profile.song_score(&s.id)).min(1.0);
            let mut key = i as f32 - boost_span * boost;
            if let Some(&j) = profile.song_index.get(&s.id) {
                let t = &profile.songs[j];
                let age = now - t.last_played;
                if t.last_played > 0 && age < RECENT_MS {
                    let fresh = 1.0 - age.max(0) as f32 / RECENT_MS as f32;
                    key += demote_span * (0.5 + 0.5 * fresh);
                }
                if t.score < 0.0 {
                    key += demote_span * 0.5;
                }
            }
            (key, i, s)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    space_artists(keyed.into_iter().map(|(_, _, s)| s).collect())
}

/// Reorder so no more than [`MAX_RUN`] songs of one (primary) artist play in
/// a row, moving the least disruptive later song up when needed.
fn space_artists(list: Vec<Song>) -> Vec<Song> {
    let key = |s: &Song| name_key(s.artist_name());
    let mut pending: VecDeque<(String, Song)> = list.into_iter().map(|s| (key(&s), s)).collect();
    let mut out: Vec<Song> = Vec::with_capacity(pending.len());
    let mut keys: Vec<String> = Vec::with_capacity(pending.len());
    while !pending.is_empty() {
        let blocked = (keys.len() >= MAX_RUN)
            .then(|| &keys[keys.len() - MAX_RUN..])
            .filter(|run| !run[0].is_empty() && run.iter().all(|k| *k == run[0]))
            .map(|run| run[0].clone());
        let idx = blocked.and_then(|b| pending.iter().position(|(k, _)| *k != b)).unwrap_or(0);
        let Some((k, s)) = pending.remove(idx) else { break };
        keys.push(k);
        out.push(s);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mochy_core::models::ArtistRef;

    const NOW: i64 = 1_790_000_000_000;

    fn song(id: &str, artist: &str, dur: f64) -> Song {
        let mut s = Song::remote(id, format!("Song {id}"));
        s.duration_secs = Some(dur);
        s.thumbnail_url = Some(format!("https://img/{id}"));
        s.artists = vec![ArtistRef { id: Some(format!("UC{artist}")), name: artist.into() }];
        s
    }

    fn db_with(songs: &[Song]) -> Db {
        let db = Db::open_in_memory().unwrap();
        db.upsert_songs(songs).unwrap();
        db
    }

    fn ago(days: f64) -> i64 {
        NOW - (days * DAY_MS as f64) as i64
    }

    #[test]
    fn completion_rules() {
        assert_eq!(play_weight(160_000, Some(200_000)), (true, 1.0)); // 80 %
        assert_eq!(play_weight(186_000, Some(200_000)).0, true); // within 15 s
        let (c, w) = play_weight(50_000, Some(200_000));
        assert!(!c && (w - 0.325).abs() < 1e-4);
        assert!(play_weight(150_000, None).0);
        assert!(!play_weight(60_000, None).0);
    }

    #[test]
    fn empty_history_is_empty_profile() {
        let db = Db::open_in_memory().unwrap();
        let p = compute(&db, &HashSet::new(), NOW).unwrap();
        assert!(p.is_empty());
        assert!(p.artists.is_empty() && p.songs.is_empty());
    }

    #[test]
    fn plays_completion_recency_likes_and_skips() {
        let songs = [song("full", "A", 200.0), song("part", "B", 200.0), song("old", "C", 200.0), song("liked", "D", 200.0), song("skip", "E", 200.0)];
        let db = db_with(&songs);
        for d in [1.0, 2.0, 3.0] {
            db.record_play_at("full", 190.0, ago(d)).unwrap();
            db.record_play_at("part", 30.0, ago(d)).unwrap();
            db.record_play_at("old", 190.0, ago(d + 200.0)).unwrap();
        }
        db.set_liked("liked", true).unwrap();
        db.record_play_at("skip", 190.0, ago(10.0)).unwrap();
        db.record_skip_at("skip", 4.0, None, ago(1.0)).unwrap();
        db.record_skip_at("skip", 2.0, None, ago(0.5)).unwrap();

        let p = compute(&db, &HashSet::new(), NOW).unwrap();
        let get = |id: &str| &p.songs[p.song_index[id]];
        assert!(get("full").score > get("part").score);
        assert!(get("part").score > get("old").score);
        assert_eq!(get("full").completed, 3);
        assert_eq!(get("part").completed, 0);
        assert_eq!(get("full").plays, 3);
        assert_eq!(get("full").last_played, ago(1.0));
        assert!(get("liked").liked && get("liked").score >= LIKED_BONUS);
        assert_eq!(get("skip").skips, 2);
        assert!(get("skip").score < get("full").score / 2.0);
        // 2 skips vs 1 completed play: not "clearly" disliked.
        assert!(!p.is_disliked("skip"));
        assert_eq!(p.songs[0].song.id, "full");
        assert_eq!(p.total_events, 3 * 3 + 1 + 2);
        // Artists follow their songs; A is on top and has full stats.
        assert_eq!(p.artists[0].name, "A");
        assert_eq!(p.artists[0].id.as_deref(), Some("UCA"));
        assert_eq!(p.artists[0].plays, 3);
        assert_eq!(p.artists[0].ms, 570_000);
        assert_eq!(p.artists[0].thumbnail_url.as_deref(), Some("https://img/full"));
        assert!((p.artist_score("a") - 1.0).abs() < 1e-6);
        // "old" is outside the last 28 days.
        let c = &p.artists[p.artist_index["c"]];
        assert!(c.score > 0.0 && c.short_term == 0.0);
        assert!(p.artists[p.artist_index["a"]].short_term > 0.0);
    }

    #[test]
    fn disliked_needs_repeated_skips() {
        let songs = [song("x", "X", 200.0), song("y", "Y", 200.0), song("z", "Z", 200.0)];
        let db = db_with(&songs);
        for d in [1.0, 2.0, 3.0] {
            db.record_skip_at("x", 3.0, None, ago(d)).unwrap();
            db.record_skip_at("z", 3.0, None, ago(d)).unwrap();
        }
        db.record_skip_at("y", 3.0, None, ago(1.0)).unwrap();
        db.set_liked("z", true).unwrap();
        let p = compute(&db, &HashSet::new(), NOW).unwrap();
        assert!(p.is_disliked("x"));
        assert!(!p.is_disliked("y")); // a single skip
        assert!(!p.is_disliked("z")); // liked wins
        assert!(p.songs[p.song_index["x"]].score < 0.0);
        assert_eq!(p.song_score("x"), 0.0);
    }

    #[test]
    fn playlists_bonus_and_exclusion() {
        let songs = [song("a", "A", 200.0), song("b", "B", 200.0), song("c", "C", 200.0), song("d", "D", 200.0)];
        let db = db_with(&songs);
        let keep = db.create_playlist("Keep").unwrap();
        let sleep = db.create_playlist("Sleep sounds").unwrap();
        db.add_to_playlist(&keep, &songs[0], None).unwrap();
        db.add_to_playlist(&sleep, &songs[1], None).unwrap();
        db.add_to_playlist(&sleep, &songs[2], None).unwrap();
        db.set_liked("c", true).unwrap();
        for _ in 0..5 {
            db.record_play_at("b", 190.0, ago(1.0)).unwrap();
        }
        db.record_play_at("d", 190.0, ago(1.0)).unwrap();

        let p = compute(&db, &HashSet::new(), NOW).unwrap();
        assert!((p.songs[p.song_index["a"]].score - PLAYLIST_BONUS).abs() < 1e-6);
        assert_eq!(p.artists[0].name, "B");

        let excluded: HashSet<String> = [sleep.clone()].into();
        let p = compute(&db, &excluded, NOW).unwrap();
        assert!(!p.song_index.contains_key("b"));
        assert!(!p.artist_index.contains_key("b"));
        // Liked songs count even in an excluded playlist (but get no playlist bonus).
        assert!((p.songs[p.song_index["c"]].score - LIKED_BONUS).abs() < 1e-6);
        assert_eq!(p.total_events, 1);
    }

    #[test]
    fn artists_merge_by_name_prefer_youtube_id() {
        let mut local = song("l", "Sơn Tùng M-TP", 200.0);
        local.artists[0].id = None; // local tag → LA… id
        let mut yt = song("y", "sơn tùng m-tp ", 200.0);
        yt.artists[0].id = Some("UCsontung".into());
        let mut feat = song("f", "Other", 200.0);
        feat.artists.push(ArtistRef { id: Some("UCsontung".into()), name: "Sơn Tùng M-TP".into() });
        let db = db_with(&[local, yt, feat]);
        db.record_play_at("l", 190.0, ago(1.0)).unwrap();
        db.record_play_at("y", 190.0, ago(1.0)).unwrap();
        db.record_play_at("f", 190.0, ago(1.0)).unwrap();
        let p = compute(&db, &HashSet::new(), NOW).unwrap();
        let st = &p.artists[p.artist_index["sơn tùng m-tp"]];
        assert_eq!(st.id.as_deref(), Some("UCsontung"));
        assert_eq!(st.plays, 3);
        assert_eq!(p.artists[0].name, st.name);
        let other = &p.artists[p.artist_index["other"]];
        assert!(st.score > other.score * 2.0);
    }

    #[test]
    fn dayparts_use_local_time() {
        let songs = [song("m", "Morning Guy", 200.0), song("n", "Night Owl", 200.0)];
        let db = db_with(&songs);
        let off = db.local_utc_offset_secs().unwrap();
        // Local midnight of a day a week ago.
        let day = (ago(7.0) / 1000 + off).div_euclid(86_400) * 86_400;
        let at = |h: i64| (day + h * 3600 - off) * 1000;
        db.record_play_at("m", 190.0, at(8)).unwrap();
        db.record_play_at("m", 190.0, at(9)).unwrap();
        db.record_play_at("n", 190.0, at(23)).unwrap();
        db.record_play_at("n", 190.0, at(2)).unwrap();
        db.record_play_at("n", 190.0, at(14)).unwrap();
        let p = compute(&db, &HashSet::new(), NOW).unwrap();
        let names = |d: Daypart| p.by_daypart[d.index()].iter().map(|a| a.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(Daypart::Morning), ["Morning Guy"]);
        assert_eq!(names(Daypart::Night), ["Night Owl"]);
        assert_eq!(names(Daypart::Afternoon), ["Night Owl"]);
        assert!(names(Daypart::Evening).is_empty());
        assert_eq!(p.by_daypart[Daypart::Night.index()][0].plays, 2);
    }

    // ---- rerank ----

    fn profile(artists: &[(&str, f32, f32)], songs: Vec<SongTaste>) -> TasteProfile {
        let mut p = TasteProfile {
            computed_at: NOW,
            total_events: 10,
            artists: artists
                .iter()
                .map(|(n, s, st)| ArtistTaste { name: n.to_string(), score: *s, short_term: *st, ..Default::default() })
                .collect(),
            songs,
            ..Default::default()
        };
        p.rebuild_index();
        p
    }

    fn taste(s: Song, score: f32, last_played: i64) -> SongTaste {
        SongTaste { song: s, score, plays: 1, completed: 1, skips: 0, liked: false, last_played }
    }

    fn ids(v: &[Song]) -> Vec<&str> {
        v.iter().map(|s| s.id.as_str()).collect()
    }

    #[test]
    fn rerank_is_noop_without_profile() {
        let radio: Vec<Song> = (0..5).map(|i| song(&format!("r{i}"), &format!("A{i}"), 200.0)).collect();
        let out = rerank(&TasteProfile::default(), radio.clone());
        assert_eq!(out, radio);
    }

    #[test]
    fn rerank_boosts_liked_artists_and_drops_disliked() {
        let radio: Vec<Song> = (0..12).map(|i| song(&format!("r{i}"), &format!("Art{i}"), 200.0)).collect();
        let mut p = profile(&[("Art9", 10.0, 5.0), ("Art1", 1.0, 0.0)], vec![]);
        p.disliked.insert("r3".into());
        let out = rerank(&p, radio.clone());
        assert_eq!(out.len(), 11);
        assert!(!ids(&out).contains(&"r3"));
        let pos = |id: &str| ids(&out).iter().position(|x| *x == id).unwrap();
        assert!(pos("r9") < 9 - 2, "favourite artist moves up: {:?}", ids(&out));
        // Unknown artists keep YouTube's relative order.
        let unknown: Vec<&str> = ids(&out).into_iter().filter(|x| !["r1", "r9"].contains(x)).collect();
        assert_eq!(unknown, ["r0", "r2", "r4", "r5", "r6", "r7", "r8", "r10", "r11"]);
        // Deterministic.
        assert_eq!(rerank(&p, radio.clone()), out);
    }

    #[test]
    fn rerank_demotes_just_played_songs() {
        let radio: Vec<Song> = (0..12).map(|i| song(&format!("r{i}"), &format!("Art{i}"), 200.0)).collect();
        let p = profile(&[("Zed", 1.0, 1.0)], vec![taste(radio[0].clone(), 0.0, NOW - 3_600_000), taste(radio[1].clone(), 0.0, NOW - 5 * DAY_MS)]);
        let out = rerank(&p, radio);
        let pos = |id: &str| ids(&out).iter().position(|x| *x == id).unwrap();
        assert!(pos("r0") >= 4, "{:?}", ids(&out));
        assert_eq!(pos("r1"), 0);
    }

    #[test]
    fn rerank_spaces_same_artist() {
        let mut radio: Vec<Song> = (0..4).map(|i| song(&format!("a{i}"), "Same", 200.0)).collect();
        radio.extend((0..3).map(|i| song(&format!("o{i}"), &format!("Other{i}"), 200.0)));
        let p = profile(&[("Same", 10.0, 10.0)], vec![]);
        let out = rerank(&p, radio);
        let artists: Vec<&str> = out.iter().map(|s| s.artist_name()).collect();
        for w in artists.windows(3) {
            assert!(!(w[0] == w[1] && w[1] == w[2]), "{artists:?}");
        }
        assert_eq!(out.len(), 7);
        assert_eq!(ids(&out)[..2], ["a0", "a1"]);
        // Nothing else to interleave: the run is kept.
        let only: Vec<Song> = (0..3).map(|i| song(&format!("s{i}"), "Same", 200.0)).collect();
        assert_eq!(space_artists(only).len(), 3);
    }
}
