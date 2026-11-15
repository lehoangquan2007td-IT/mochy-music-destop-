//! BetterLyrics ("Apple Music"): `GET https://lyrics-api.boidu.dev/getLyrics?s=&a=&d=&al=`
//! → `{"ttml": "..."}` → TTML parser → LRC.

use std::time::Duration;

use serde::Deserialize;

use super::{MOCHY_UA, Query, get, parse_json, url};
use crate::{LyricsResult, lines_to_lrc, parse_ttml};

const BASE: &str = "https://lyrics-api.boidu.dev";
/// Ktor: connect 10 s + request 15 s.
const TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Deserialize)]
struct TtmlResponse {
    ttml: Option<String>,
}

pub(crate) async fn get_lyrics(http: &reqwest::Client, q: &Query) -> LyricsResult<Option<String>> {
    let mut params = vec![("s", q.title.clone()), ("a", q.artist.clone())];
    if q.duration > 0 {
        params.push(("d", q.duration.to_string()));
    }
    if let Some(album) = q.album.as_ref().filter(|a| !a.trim().is_empty()) {
        params.push(("al", album.clone()));
    }
    let (status, body) = get(
        http,
        &url(&format!("{BASE}/getLyrics"), &params),
        MOCHY_UA,
        TIMEOUT,
    )
    .await?;
    if status != 200 {
        return Ok(None);
    }
    let Some(ttml) = parse_json::<TtmlResponse>(&body)?
        .ttml
        .filter(|t| !t.trim().is_empty())
    else {
        return Ok(None);
    };
    let lines = parse_ttml(&ttml)?;
    Ok((!lines.is_empty()).then(|| lines_to_lrc(&lines)))
}
