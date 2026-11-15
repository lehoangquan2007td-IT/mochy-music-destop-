//! NetEase Cloud Music (no account): `GET https://music.163.com/api/search/get?s=&type=1&limit=10`
//! → `GET https://music.163.com/api/song/lyric?id=&lv=1&yv=1`; YRC word timing preferred.

use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;

use super::direct::{
    TIMEOUT, TimedLine, TimedWord, TrackCandidate, best_match, has_lyrics, parse_lrc, to_lrc,
    without_credits,
};
use super::{MOCHY_UA, Query, get_ok, parse_json, url};
use crate::LyricsResult;

const SEARCH_LIMIT: u32 = 10;

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchResponse {
    result: Option<SearchResult>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchResult {
    songs: Vec<Song>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Song {
    id: i64,
    name: String,
    /// milliseconds
    duration: i64,
    artists: Vec<Artist>,
    alias: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Artist {
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LyricResponse {
    lrc: Option<Lyric>,
    yrc: Option<Lyric>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Lyric {
    lyric: Option<String>,
}

static YRC_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[(\d+),\d+](.*)$").expect("regex"));
static YRC_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\((\d+),(\d+),-?\d+\)").expect("regex"));

/// `[lineStart,lineLength](wordStart,wordLength,0)word…` in ms; a word runs until the next
/// marker so brackets inside words survive; untimed JSON credit lines are skipped.
pub(crate) fn parse_yrc(yrc: &str) -> Vec<TimedLine> {
    yrc.lines()
        .filter_map(|raw| {
            let c = YRC_LINE.captures(raw.trim())?;
            let start_ms: i64 = c[1].parse().ok()?;
            let body = c.get(2)?.as_str();
            let marks: Vec<regex::Captures> = YRC_WORD.captures_iter(body).collect();
            let words: Vec<TimedWord> = marks
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let whole = m.get(0).expect("whole");
                    let end = marks
                        .get(i + 1)
                        .map(|n| n.get(0).expect("whole").start())
                        .unwrap_or(body.len());
                    let s: i64 = m[1].parse().unwrap_or(0);
                    let len: i64 = m[2].parse().unwrap_or(0);
                    TimedWord {
                        text: body[whole.end()..end].to_string(),
                        start_ms: s,
                        end_ms: s + len,
                    }
                })
                .collect();
            let text = words
                .iter()
                .map(|w| w.text.as_str())
                .collect::<String>()
                .trim()
                .to_string();
            Some(TimedLine {
                start_ms,
                text,
                words,
            })
        })
        .collect()
}

pub(crate) async fn get_lyrics(http: &reqwest::Client, q: &Query) -> LyricsResult<Option<String>> {
    let search_url = url(
        "https://music.163.com/api/search/get",
        &[
            ("s", format!("{} {}", q.title, q.artist)),
            ("type", "1".into()),
            ("limit", SEARCH_LIMIT.to_string()),
        ],
    );
    let resp: SearchResponse = parse_json(&get_ok(http, &search_url, MOCHY_UA, TIMEOUT).await?)?;
    let cands: Vec<TrackCandidate<i64>> = resp
        .result
        .map(|r| r.songs)
        .unwrap_or_default()
        .into_iter()
        .map(|s| TrackCandidate {
            titles: std::iter::once(s.name).chain(s.alias).collect(),
            artists: s.artists.into_iter().map(|a| a.name).collect(),
            duration_sec: (s.duration / 1000) as i32,
            r#ref: s.id,
        })
        .collect();
    let Some(id) = best_match(&cands, &q.title, &q.artist, q.duration) else {
        return Ok(None);
    };
    let lyric_url = url(
        "https://music.163.com/api/song/lyric",
        &[
            ("id", id.to_string()),
            ("lv", "1".into()),
            ("yv", "1".into()),
        ],
    );
    let resp: LyricResponse = parse_json(&get_ok(http, &lyric_url, MOCHY_UA, TIMEOUT).await?)?;
    let lines = resp
        .yrc
        .and_then(|y| y.lyric)
        .map(|y| parse_yrc(&y))
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| {
            resp.lrc
                .and_then(|l| l.lyric)
                .map(|l| parse_lrc(&l))
                .unwrap_or_default()
        });
    let lines = without_credits(lines);
    Ok(has_lyrics(&lines).then(|| to_lrc(&lines)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_yrc() {
        let lines = parse_yrc(
            "{\"t\":0,\"c\":[{\"tx\":\"credits\"}]}\n[1000,2000](1000,500,0)Hel(1500,500,0)lo (2000,1000,0)(world)",
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].start_ms, 1000);
        assert_eq!(lines[0].text, "Hello (world)");
        assert_eq!(
            lines[0].words[2],
            TimedWord {
                text: "(world)".into(),
                start_ms: 2000,
                end_ms: 3000
            }
        );
    }
}
