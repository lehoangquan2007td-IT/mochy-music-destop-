//! YouLyPlus (LyricsPlus): `GET {base}/v2/lyrics/get?title=&artist=&duration=&album=&id=&isrc=`
//! raced across the mirrors; first non-empty answer wins (last working mirror goes first).
//! syncedLyrics > `lyrics[]` (converted to rich `<mm:ss.mmm>` syllable LRC) > plainLyrics.

use std::sync::Mutex;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use serde::Deserialize;

use super::{MOCHY_UA, Query, get_ok, parse_json, url};
use crate::LyricsResult;
use crate::script::unspaced_boundary;

/// "the workers.dev, atomix and both vercel mirrors were dead as of october 2026"
const BASE_SERVERS: [&str; 2] = [
    "https://lyricsplus.prjktla.my.id",
    "https://lyricsplus.binimum.org",
];
/// Ktor: connect 3 s, request 8 s.
const TIMEOUT: Duration = Duration::from_secs(8);

static LAST_WORKING: Mutex<Option<&'static str>> = Mutex::new(None);

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct LyricsResponse {
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
    lyrics: Option<Vec<LyricsItem>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct LyricsItem {
    pub text: Option<String>,
    pub time: Option<f64>,
    pub syllabus: Option<Vec<Syllable>>,
    pub element: Option<LyricsElement>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct LyricsElement {
    pub singer: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Syllable {
    pub text: Option<String>,
    pub time: Option<f64>,
    pub is_background: Option<bool>,
}

fn servers() -> Vec<&'static str> {
    let last = *LAST_WORKING.lock().unwrap_or_else(|e| e.into_inner());
    match last {
        Some(l) => std::iter::once(l)
            .chain(BASE_SERVERS.iter().copied().filter(|s| *s != l))
            .collect(),
        None => BASE_SERVERS.to_vec(),
    }
}

fn format_time(ms: i64, syllable: bool) -> String {
    let ms = ms.max(0);
    let (open, close) = if syllable { ('<', '>') } else { ('[', ']') };
    format!(
        "{open}{:02}:{:02}.{:03}{close}",
        ms / 1000 / 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

/// `List<LyricsItem>.convertToLrc()`.
pub(crate) fn convert_to_lrc(items: &[LyricsItem]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let ends_ws = |t: &Option<String>| {
        t.as_deref()
            .and_then(|t| t.chars().last())
            .is_some_and(char::is_whitespace)
    };
    let lines: Vec<String> = items
        .iter()
        .map(|item| {
            let stamp = format_time(item.time.unwrap_or(0.0) as i64, false);
            let is_bg = item
                .syllabus
                .as_ref()
                .is_some_and(|s| s.iter().any(|x| x.is_background == Some(true)));
            let singer = item
                .element
                .as_ref()
                .and_then(|e| e.singer.as_deref())
                .filter(|s| !s.trim().is_empty());
            let marker = if is_bg {
                "{bg}".to_string()
            } else if let Some(s) = singer {
                format!("{{agent:{s}}}")
            } else {
                String::new()
            };
            match item.syllabus.as_ref().filter(|s| !s.is_empty()) {
                Some(syllabus) => {
                    let mut sb = stamp + &marker;
                    let spaced_by_api = syllabus[..syllabus.len() - 1]
                        .iter()
                        .any(|s| ends_ws(&s.text));
                    for (i, syl) in syllabus.iter().enumerate() {
                        sb.push_str(&format_time(syl.time.unwrap_or(0.0) as i64, true));
                        sb.push_str(syl.text.as_deref().unwrap_or(""));
                        // deviation from Android: no space between two CJK/Thai syllables
                        // (Android rendered "沈 む よ う に")
                        let next = syllabus
                            .get(i + 1)
                            .and_then(|n| n.text.as_deref())
                            .unwrap_or("");
                        let cjk = unspaced_boundary(syl.text.as_deref().unwrap_or(""), next);
                        if !spaced_by_api && i < syllabus.len() - 1 && !ends_ws(&syl.text) && !cjk {
                            sb.push(' ');
                        }
                    }
                    sb.trim().to_string()
                }
                None => format!("{stamp}{marker}{}", item.text.as_deref().unwrap_or("")),
            }
        })
        .collect();
    Some(lines.join("\n"))
}

async fn fetch_from_server(http: &reqwest::Client, base: &str, q: &Query) -> Option<String> {
    let mut params = vec![
        ("title", q.title.clone()),
        ("artist", q.artist.clone()),
        ("duration", q.duration.to_string()),
    ];
    if let Some(a) = &q.album {
        params.push(("album", a.clone()));
    }
    if let Some(id) = &q.video_id {
        params.push(("id", id.clone()));
    }
    let body = get_ok(
        http,
        &url(&format!("{base}/v2/lyrics/get"), &params),
        MOCHY_UA,
        TIMEOUT,
    )
    .await
    .ok()?;
    let resp: LyricsResponse = parse_json(&body).ok()?;
    resp.synced_lyrics
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            resp.lyrics
                .as_deref()
                .and_then(convert_to_lrc)
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(|| resp.plain_lyrics.filter(|s| !s.trim().is_empty()))
}

pub(crate) async fn get_lyrics(http: &reqwest::Client, q: &Query) -> LyricsResult<Option<String>> {
    let mut jobs: FuturesUnordered<_> = servers()
        .into_iter()
        .map(|server| async move { (server, fetch_from_server(http, server, q).await) })
        .collect();
    while let Some((server, lrc)) = jobs.next().await {
        if let Some(lrc) = lrc {
            *LAST_WORKING.lock().unwrap_or_else(|e| e.into_inner()) = Some(server);
            return Ok(Some(lrc));
        }
    }
    Ok(None)
}
