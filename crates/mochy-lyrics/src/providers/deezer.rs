//! Deezer with an anonymous login: `GET https://api.deezer.com/search?q=&limit=10`, token from
//! `GET https://auth.deezer.com/login/anonymous?jo=p&rto=c` (JWT, reused until 30 s before its
//! `exp`, 5 min fallback), lyrics via GraphQL `POST https://pipe.deezer.com/api`.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::direct::{
    TIMEOUT, TimedLine, TimedWord, TrackCandidate, best_match, has_lyrics, to_lrc,
};
use super::{MOCHY_UA, Query, get_ok, net, parse_json, url};
use crate::{LyricsError, LyricsResult};

const SEARCH_LIMIT: u32 = 10;
const FALLBACK_TOKEN_LIFETIME_MS: i64 = 5 * 60_000;
const TOKEN_RENEW_MARGIN_MS: i64 = 30_000;
const NO_SPACE_BEFORE: &str = ",.!?;:)]}…";

/// (jwt, expires_at_ms)
static TOKEN: Mutex<Option<(String, i64)>> = Mutex::const_new(None);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchResponse {
    data: Vec<Track>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Track {
    id: i64,
    title: String,
    title_short: String,
    duration: i32,
    artist: Option<Artist>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Artist {
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Lyrics {
    text: Option<String>,
    synchronized_lines: Option<Vec<Line>>,
    synchronized_word_by_word_lines: Option<Vec<WordLine>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Line {
    milliseconds: i64,
    line: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WordLine {
    start: i64,
    words: Vec<Word>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Word {
    start: i64,
    end: i64,
    word: String,
}

fn is_unspaced(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x2FDF | 0x3005 | 0x3007 | 0x3021..=0x3029 | 0x3038..=0x303B | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x3FFFF // Han
        | 0x3041..=0x309F // Hiragana
        | 0x30A0..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9D // Katakana
        | 0x0E00..=0x0E7F) // Thai
}

/// Bare words → line text: spaces between words except before punctuation and between
/// characters of scripts written without spaces.
pub(crate) fn join_words(words: &[&str]) -> String {
    let mut out = String::new();
    for w in words.iter().map(|w| w.trim()).filter(|w| !w.is_empty()) {
        let first = w.chars().next().unwrap_or(' ');
        let attached = NO_SPACE_BEFORE.contains(first)
            || out
                .chars()
                .last()
                .is_some_and(|l| is_unspaced(l) && is_unspaced(first));
        if !out.is_empty() && !attached {
            out.push(' ');
        }
        out.push_str(w);
    }
    out
}

fn expiry_of(jwt: &str) -> Option<i64> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    Some(v.get("exp")?.as_i64()? * 1000)
}

async fn access_token(http: &reqwest::Client) -> LyricsResult<String> {
    let mut guard = TOKEN.lock().await;
    if let Some((jwt, exp)) = guard.as_ref()
        && now_ms() < *exp
    {
        return Ok(jwt.clone());
    }
    let u = url(
        "https://auth.deezer.com/login/anonymous",
        &[("jo", "p".into()), ("rto", "c".into())],
    );
    let v: Value = parse_json(&get_ok(http, &u, MOCHY_UA, TIMEOUT).await?)?;
    let jwt = v
        .get("jwt")
        .and_then(Value::as_str)
        .ok_or_else(|| LyricsError::Parse("Deezer anonymous login: no jwt".into()))?
        .to_string();
    let expires = expiry_of(&jwt).unwrap_or_else(|| now_ms() + FALLBACK_TOKEN_LIFETIME_MS);
    *guard = Some((jwt.clone(), expires - TOKEN_RENEW_MARGIN_MS));
    Ok(jwt)
}

async fn fetch_lyrics(http: &reqwest::Client, track_id: i64) -> LyricsResult<Option<Lyrics>> {
    let query = format!(
        "query {{ track(trackId: \"{track_id}\") {{ lyrics {{ text synchronizedLines {{ milliseconds line }} synchronizedWordByWordLines {{ start words {{ start end word }} }} }} }} }}"
    );
    let result: LyricsResult<Option<Lyrics>> = async {
        let token = access_token(http).await?;
        let resp = http
            .post("https://pipe.deezer.com/api")
            .header(USER_AGENT, MOCHY_UA)
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header(CONTENT_TYPE, "application/json")
            .timeout(TIMEOUT)
            .body(json!({ "query": query }).to_string())
            .send()
            .await
            .map_err(net)?;
        let status = resp.status();
        let body = resp.text().await.map_err(net)?;
        if !status.is_success() {
            return Err(LyricsError::Network(format!(
                "HTTP {status} from Deezer pipe"
            )));
        }
        let v: Value = parse_json(&body)?;
        Ok(v.pointer("/data/track/lyrics")
            .filter(|l| !l.is_null())
            .map(|l| serde_json::from_value(l.clone()).unwrap_or_default()))
    }
    .await;
    if result.is_err() {
        // most likely the token went stale early: forget it so the next song logs in again
        *TOKEN.lock().await = None;
    }
    result
}

pub(crate) async fn get_lyrics(http: &reqwest::Client, q: &Query) -> LyricsResult<Option<String>> {
    let u = url(
        "https://api.deezer.com/search",
        &[
            ("q", format!("{} {}", q.title, q.artist)),
            ("limit", SEARCH_LIMIT.to_string()),
        ],
    );
    let resp: SearchResponse = parse_json(&get_ok(http, &u, MOCHY_UA, TIMEOUT).await?)?;
    let cands: Vec<TrackCandidate<i64>> = resp
        .data
        .into_iter()
        .map(|t| TrackCandidate {
            titles: vec![t.title, t.title_short],
            artists: t.artist.map(|a| a.name).into_iter().collect(),
            duration_sec: t.duration,
            r#ref: t.id,
        })
        .collect();
    let Some(id) = best_match(&cands, &q.title, &q.artist, q.duration) else {
        return Ok(None);
    };
    let Some(lyrics) = fetch_lyrics(http, id).await? else {
        return Ok(None);
    };
    let mut lines: Vec<TimedLine> = lyrics
        .synchronized_word_by_word_lines
        .unwrap_or_default()
        .into_iter()
        .map(|l| TimedLine {
            start_ms: l.start,
            text: join_words(&l.words.iter().map(|w| w.word.as_str()).collect::<Vec<_>>()),
            words: l
                .words
                .iter()
                .map(|w| TimedWord {
                    text: w.word.clone(),
                    start_ms: w.start,
                    end_ms: w.end,
                })
                .collect(),
        })
        .collect();
    if lines.is_empty() {
        lines = lyrics
            .synchronized_lines
            .unwrap_or_default()
            .into_iter()
            .map(|l| TimedLine {
                start_ms: l.milliseconds,
                text: l.line.trim().to_string(),
                words: Vec::new(),
            })
            .collect();
    }
    if has_lyrics(&lines) {
        return Ok(Some(to_lrc(&lines)));
    }
    Ok(lyrics
        .text
        .filter(|t| !t.trim().is_empty())
        .map(|t| t.replace("\r\n", "\n").trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_words_with_cjk_rules() {
        assert_eq!(join_words(&["Hello", ",", "world", "!"]), "Hello, world!");
        assert_eq!(join_words(&["我", "爱", "你", "baby"]), "我爱你 baby");
    }
}
