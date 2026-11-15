//! LRCLIB: `GET https://lrclib.net/api/search?track_name=&artist_name=&album_name=` with the
//! Android fallback chain (title only, `q=artist title`, `q=title`, raw title+artist), then
//! `bestMatchingForRelaxed` (synced first, ±5 s) or, for unknown duration, a fuzzy name match.

use std::time::Duration;

use serde::Deserialize;

use super::clean::{clean_artist, clean_title};
use super::{MOCHY_UA, Query, get_ok, parse_json, url};
use crate::LyricsResult;

const BASE: &str = "https://lrclib.net";
/// Android sets no timeout here; the 35 s provider cap applies. Keep each request bounded.
const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Track {
    pub track_name: String,
    pub artist_name: String,
    pub duration: f64,
    pub plain_lyrics: Option<String>,
    pub synced_lyrics: Option<String>,
}

impl Track {
    fn has_lyrics(&self) -> bool {
        self.synced_lyrics.is_some() || self.plain_lyrics.is_some()
    }
    fn diff(&self, duration: i32) -> i64 {
        ((self.duration as i64) - duration as i64).abs()
    }
}

async fn query_with(http: &reqwest::Client, params: Vec<(&str, String)>) -> Vec<Track> {
    let u = url(&format!("{BASE}/api/search"), &params);
    match get_ok(http, &u, MOCHY_UA, TIMEOUT).await {
        Ok(body) => parse_json::<Vec<Track>>(&body)
            .unwrap_or_default()
            .into_iter()
            .filter(Track::has_lyrics)
            .collect(),
        Err(_) => Vec::new(),
    }
}

async fn query_lyrics(
    http: &reqwest::Client,
    artist: &str,
    title: &str,
    album: Option<&str>,
) -> Vec<Track> {
    let ct = clean_title(title);
    let ca = clean_artist(artist);
    let mut p = vec![("track_name", ct.clone()), ("artist_name", ca.clone())];
    if let Some(a) = album {
        p.push(("album_name", a.to_string()));
    }
    let attempts: Vec<Vec<(&str, String)>> = vec![
        p,
        vec![("track_name", ct.clone())],
        vec![("q", format!("{ca} {ct}"))],
        vec![("q", ct.clone())],
    ];
    for params in attempts {
        let r = query_with(http, params).await;
        if !r.is_empty() {
            return r;
        }
    }
    if ct != title.trim() {
        return query_with(
            http,
            vec![
                ("track_name", title.trim().to_string()),
                ("artist_name", artist.trim().to_string()),
            ],
        )
        .await;
    }
    Vec::new()
}

/// `bestMatchingForRelaxed`.
pub(crate) fn best_matching_relaxed(tracks: &[Track], duration: i32) -> Option<&Track> {
    if tracks.is_empty() {
        return None;
    }
    if duration == -1 {
        return tracks
            .iter()
            .find(|t| t.synced_lyrics.is_some())
            .or(tracks.first());
    }
    let synced = tracks
        .iter()
        .filter(|t| t.synced_lyrics.is_some())
        .min_by_key(|t| t.diff(duration))
        .filter(|t| t.diff(duration) <= 5);
    synced.or_else(|| {
        tracks
            .iter()
            .min_by_key(|t| t.diff(duration))
            .filter(|t| t.diff(duration) <= 5)
    })
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        prev = cur;
    }
    prev[b.len()]
}

fn similarity(s1: &str, s2: &str) -> f64 {
    if s1 == s2 {
        return 1.0;
    }
    if s1.is_empty() || s2.is_empty() {
        return 0.0;
    }
    let contains: f64 = if s1.contains(s2) || s2.contains(s1) {
        0.8
    } else {
        0.0
    };
    let (a, b): (Vec<char>, Vec<char>) = (s1.chars().collect(), s2.chars().collect());
    let max_len = a.len().max(b.len()) as f64;
    contains.max(1.0 - levenshtein(&a, &b) as f64 / max_len)
}

/// `findBestMatch` (unknown duration): average title/artist similarity > 0.6.
pub(crate) fn find_best_match<'a>(
    tracks: &'a [Track],
    title: &str,
    artist: &str,
) -> Option<&'a Track> {
    let (t, a) = (title.trim().to_lowercase(), artist.trim().to_lowercase());
    let sim = |tr: &Track| {
        (similarity(&t, &tr.track_name.trim().to_lowercase())
            + similarity(&a, &tr.artist_name.trim().to_lowercase()))
            / 2.0
    };
    let mut best: Option<(&Track, f64)> = None;
    for tr in tracks {
        let score = sim(tr) + if tr.synced_lyrics.is_some() { 0.1 } else { 0.0 };
        if best.is_none_or(|(_, s)| score > s) {
            best = Some((tr, score));
        }
    }
    best.map(|(tr, _)| tr).filter(|tr| sim(tr) > 0.6)
}

pub(crate) async fn get_lyrics(http: &reqwest::Client, q: &Query) -> LyricsResult<Option<String>> {
    let tracks = query_lyrics(http, &q.artist, &q.title, q.album.as_deref()).await;
    let best = if q.duration == -1 {
        find_best_match(&tracks, &clean_title(&q.title), &clean_artist(&q.artist))
    } else {
        best_matching_relaxed(&tracks, q.duration)
    };
    Ok(best.and_then(|t| t.synced_lyrics.clone().or_else(|| t.plain_lyrics.clone())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tr(name: &str, d: f64, synced: bool) -> Track {
        Track {
            track_name: name.into(),
            artist_name: "A".into(),
            duration: d,
            plain_lyrics: Some("plain".into()),
            synced_lyrics: synced.then(|| "[00:01.00]x".to_string()),
        }
    }

    #[test]
    fn relaxed_prefers_synced_within_5s() {
        let t = vec![
            tr("a", 200.0, false),
            tr("b", 204.0, true),
            tr("c", 230.0, true),
        ];
        assert_eq!(best_matching_relaxed(&t, 200).unwrap().track_name, "b");
        assert_eq!(
            best_matching_relaxed(&t, 210).map(|t| t.track_name.as_str()),
            None
        );
        let t = vec![tr("a", 200.0, false), tr("c", 230.0, true)];
        assert_eq!(best_matching_relaxed(&t, 201).unwrap().track_name, "a");
        assert_eq!(best_matching_relaxed(&t, -1).unwrap().track_name, "c");
    }

    #[test]
    fn fuzzy_match_without_duration() {
        let t = vec![
            tr("Totally Different", 1.0, true),
            tr("Blinding Lights", 1.0, false),
        ];
        assert_eq!(
            find_best_match(&t, "blinding lights", "a")
                .unwrap()
                .track_name,
            "Blinding Lights"
        );
        assert!(find_best_match(&t, "zzzz", "qqqq").is_none());
    }
}
