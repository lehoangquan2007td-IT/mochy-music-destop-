//! SimpMusic: `GET https://api-lyrics.simpmusic.org/v1/{videoId}` (fallback
//! `https://musicfy-yt-music-server.onrender.com/v1/`), duration filter ±10 s,
//! richSyncLyrics > syncedLyrics > plainLyrics.

use std::time::Duration;

use reqwest::header::{ACCEPT, CONTENT_TYPE, USER_AGENT};
use serde::Deserialize;

use super::{Query, net, parse_json};
use crate::LyricsResult;

const BASE_URL: &str = "https://api-lyrics.simpmusic.org/v1/";
const FALLBACK_URL: &str = "https://musicfy-yt-music-server.onrender.com/v1/";
/// Ktor: connect 10 s + request 15 s.
const TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct LyricsData {
    #[serde(rename = "durationSeconds")]
    pub duration: Option<f64>,
    pub synced_lyrics: Option<String>,
    #[serde(rename = "plainLyric")]
    pub plain_lyrics: Option<String>,
    pub rich_sync_lyrics: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ApiResponse {
    #[serde(rename = "type")]
    kind: Option<String>,
    data: Vec<LyricsData>,
}

/// `None` = request failed / non-200 (try the fallback); `Some(vec)` = answered.
async fn attempt(http: &reqwest::Client, base: &str, video_id: &str) -> Option<Vec<LyricsData>> {
    let resp = http
        .get(format!("{base}{}", super::enc(video_id)))
        .header(ACCEPT, "application/json")
        .header(USER_AGENT, "SimpMusicLyrics/1.0")
        .header(CONTENT_TYPE, "application/json")
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(net)
        .ok()?;
    if resp.status().as_u16() != 200 {
        return None;
    }
    let body = resp.text().await.ok()?;
    let api: ApiResponse = parse_json(&body).ok()?;
    Some(if api.kind.as_deref() == Some("success") {
        api.data
    } else {
        Vec::new()
    })
}

fn dur(t: &LyricsData) -> i64 {
    t.duration.unwrap_or(0.0) as i64
}

/// Pick the track per Android `SimpMusicLyrics.getLyrics`.
pub(crate) fn pick(tracks: Vec<LyricsData>, duration: i32) -> Option<String> {
    let d = duration as i64;
    let valid: Vec<LyricsData> = if duration > 0 {
        tracks
            .into_iter()
            .filter(|t| (dur(t) - d).abs() <= 10)
            .collect()
    } else {
        tracks
    };
    let best = if duration > 0 && valid.len() > 1 {
        valid.iter().min_by_key(|t| (dur(t) - d).abs())
    } else {
        valid.first()
    }?;
    let nonblank = |s: &Option<String>| s.clone().filter(|s| !s.trim().is_empty());
    nonblank(&best.rich_sync_lyrics)
        .or_else(|| nonblank(&best.synced_lyrics))
        .or_else(|| nonblank(&best.plain_lyrics))
}

pub(crate) async fn get_lyrics(http: &reqwest::Client, q: &Query) -> LyricsResult<Option<String>> {
    let Some(video_id) = q.video_id.as_deref() else {
        return Ok(None);
    };
    let tracks = match attempt(http, BASE_URL, video_id).await {
        Some(t) => t,
        None => attempt(http, FALLBACK_URL, video_id)
            .await
            .unwrap_or_default(),
    };
    Ok(pick(tracks, q.duration))
}
