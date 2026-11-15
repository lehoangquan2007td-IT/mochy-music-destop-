//! Network providers. Each one returns the raw payload in the Mochy LRC dialect, exactly like
//! the Android `LyricsProvider.getLyrics` (`Ok(None)` = "no lyrics for this song").

use std::time::Duration;

use mochy_core::models::TrackMeta;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::header::USER_AGENT;
use serde::de::DeserializeOwned;

use crate::{LyricsError, LyricsResult, ProviderId};

pub(crate) mod betterlyrics;
pub(crate) mod clean;
pub(crate) mod deezer;
pub(crate) mod direct;
pub(crate) mod kugou;
pub(crate) mod lrclib;
pub(crate) mod lrcred;
pub(crate) mod netease;
pub(crate) mod paxsenix;
pub(crate) mod qqmusic;
pub(crate) mod simpmusic;
pub(crate) mod youlyplus;

/// "says who's asking so the server owner can reach us instead of blocking us"
pub(crate) const MOCHY_UA: &str = "Mochy (+https://github.com/realidkroo/Mochy-Music)";

/// What the Android app hands every provider (`LyricsProvider.getLyrics` arguments).
#[derive(Debug, Clone)]
pub(crate) struct Query {
    /// YouTube video id (`mediaMetadata.id`).
    pub video_id: Option<String>,
    pub title: String,
    /// All artists joined with ", " (Kotlin `joinToString { it.name }`).
    pub artist: String,
    /// Whole seconds, -1 when unknown.
    pub duration: i32,
    pub album: Option<String>,
}

impl Query {
    pub fn from_meta(meta: &TrackMeta) -> Query {
        Query {
            video_id: meta.video_id.clone().filter(|v| !v.is_empty()),
            title: meta.title.clone(),
            artist: meta.artists.join(", "),
            duration: meta
                .duration_secs
                .filter(|d| *d > 0.0)
                .map(|d| d.round() as i32)
                .unwrap_or(-1),
            album: meta.album.clone(),
        }
    }
}

/// Query-component encoding (RFC 3986 unreserved kept, space → `%20`).
const QUERY: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

pub(crate) fn enc(s: &str) -> String {
    utf8_percent_encode(s, QUERY).to_string()
}

/// `base?k=v&k=v` with percent-encoded values.
pub(crate) fn url(base: &str, params: &[(&str, String)]) -> String {
    if params.is_empty() {
        return base.to_string();
    }
    let q: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect();
    format!("{base}?{}", q.join("&"))
}

pub(crate) fn net(e: reqwest::Error) -> LyricsError {
    LyricsError::Network(e.to_string())
}

/// Drop `null` object members recursively so `#[serde(default)]` applies to them, like the
/// Kotlin clients' `coerceInputValues = true` / `explicitNulls = false`.
fn strip_nulls(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            map.retain(|_, x| !x.is_null());
            map.values_mut().for_each(strip_nulls);
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(strip_nulls),
        _ => {}
    }
}

/// Lenient JSON decode (see [`strip_nulls`]; a leading BOM is ignored).
pub(crate) fn parse_json<T: DeserializeOwned>(body: &str) -> LyricsResult<T> {
    let body = body.trim_start_matches('\u{FEFF}');
    let mut v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| LyricsError::Parse(e.to_string()))?;
    strip_nulls(&mut v);
    serde_json::from_value(v).map_err(|e| LyricsError::Parse(e.to_string()))
}

/// GET returning (status, body). Never fails on HTTP status.
pub(crate) async fn get(
    http: &reqwest::Client,
    url: &str,
    ua: &str,
    timeout: Duration,
) -> LyricsResult<(u16, String)> {
    let resp = http
        .get(url)
        .header(USER_AGENT, ua)
        .timeout(timeout)
        .send()
        .await
        .map_err(net)?;
    let status = resp.status().as_u16();
    let body = resp.text().await.map_err(net)?;
    Ok((status, body))
}

/// GET that treats non-2xx as an error (Ktor `expectSuccess = true`).
pub(crate) async fn get_ok(
    http: &reqwest::Client,
    url: &str,
    ua: &str,
    timeout: Duration,
) -> LyricsResult<String> {
    let (status, body) = get(http, url, ua, timeout).await?;
    if !(200..300).contains(&status) {
        return Err(LyricsError::Network(format!(
            "HTTP {status} from {}",
            url.split('?').next().unwrap_or(url)
        )));
    }
    Ok(body)
}

/// Dispatch to one provider. YouTube-based providers are not implemented here (they need
/// the InnerTube client) and return `Ok(None)`.
pub(crate) async fn fetch_raw(
    provider: ProviderId,
    q: &Query,
    http: &reqwest::Client,
) -> LyricsResult<Option<String>> {
    let raw = match provider {
        ProviderId::BetterLyrics => betterlyrics::get_lyrics(http, q).await?,
        ProviderId::YouLyPlus => youlyplus::get_lyrics(http, q).await?,
        ProviderId::Paxsenix => paxsenix::get_lyrics(http, q).await?,
        ProviderId::LrcRed => lrcred::get_lyrics(http, q).await?,
        ProviderId::SimpMusic => simpmusic::get_lyrics(http, q).await?,
        ProviderId::LrcLib => lrclib::get_lyrics(http, q).await?,
        ProviderId::Deezer => deezer::get_lyrics(http, q).await?,
        ProviderId::NetEase => netease::get_lyrics(http, q).await?,
        ProviderId::QQMusic => qqmusic::get_lyrics(http, q).await?,
        ProviderId::Kugou => kugou::get_lyrics(http, q).await?,
        ProviderId::YouTubeSubtitle | ProviderId::YouTubeMusic | ProviderId::YouTubeCaptions => {
            None
        }
    };
    Ok(raw.filter(|r| !r.trim().is_empty()))
}
