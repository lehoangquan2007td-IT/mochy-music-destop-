//! lrc.red: `GET https://lrc.red/search.json?q=` (title+artist, then title alone) →
//! `GET https://lrc.red/s/{isrc}.ttml` (Apple-style TTML).

use serde::Deserialize;

use super::direct::{TIMEOUT, TrackCandidate, best_match};
use super::{MOCHY_UA, Query, get_ok, parse_json, url};
use crate::{LyricsResult, lines_to_lrc, parse_ttml};

pub(crate) const BASE_URL: &str = "https://lrc.red";

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchResponse {
    hits: Vec<Hit>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Hit {
    isrc: String,
    title: String,
    artist: String,
    /// seconds, with a fraction
    duration: f64,
}

async fn search(
    http: &reqwest::Client,
    query: String,
) -> LyricsResult<Vec<TrackCandidate<String>>> {
    let u = url(&format!("{BASE_URL}/search.json"), &[("q", query)]);
    let resp: SearchResponse = parse_json(&get_ok(http, &u, MOCHY_UA, TIMEOUT).await?)?;
    Ok(resp
        .hits
        .into_iter()
        .filter(|h| !h.isrc.is_empty())
        .map(|h| TrackCandidate {
            titles: vec![h.title],
            artists: vec![h.artist],
            duration_sec: h.duration.round() as i32,
            r#ref: h.isrc,
        })
        .collect())
}

pub(crate) async fn get_lyrics(http: &reqwest::Client, q: &Query) -> LyricsResult<Option<String>> {
    let first = search(http, format!("{} {}", q.title, q.artist)).await?;
    let isrc = match best_match(&first, &q.title, &q.artist, q.duration) {
        Some(i) => Some(i),
        // a long joined artist list can knock the song out of the results
        None => best_match(
            &search(http, q.title.clone()).await?,
            &q.title,
            &q.artist,
            q.duration,
        ),
    };
    let Some(isrc) = isrc else { return Ok(None) };
    let ttml = get_ok(
        http,
        &format!("{BASE_URL}/s/{}.ttml", super::enc(&isrc)),
        MOCHY_UA,
        TIMEOUT,
    )
    .await?;
    let lines = parse_ttml(&ttml)?;
    Ok((!lines.is_empty()).then(|| lines_to_lrc(&lines)))
}
