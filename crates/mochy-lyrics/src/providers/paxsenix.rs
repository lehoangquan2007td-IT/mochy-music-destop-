//! Paxsenix (Apple Music mirror): `GET https://lyrics.paxsenix.org/apple-music/search?q=`,
//! scored (duration ±2 s = +100 …), then `GET /apple-music/lyrics?id=` for the top 5 in
//! parallel; the first word-timed answer wins, else the first plain one.

use std::sync::LazyLock;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

use super::clean::{clean_artist, clean_title_paxsenix};
use super::{Query, get_ok, parse_json, url};
use crate::{LyricsResult, lines_to_lrc, parse_ttml};

const BASE: &str = "https://lyrics.paxsenix.org";
/// Ktor: connect 4 s, request 5 s.
const TIMEOUT: Duration = Duration::from_secs(5);

fn user_agent() -> String {
    format!("Mochy/{}", env!("CARGO_PKG_VERSION"))
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SearchResult {
    pub id: Value,
    pub song_name: Option<String>,
    pub track_name: Option<String>,
    pub artist_name: Option<String>,
    /// milliseconds
    pub duration: Option<f64>,
}

impl SearchResult {
    fn id_str(&self) -> Option<String> {
        match &self.id {
            Value::String(s) if !s.is_empty() => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    }
    fn display_name(&self) -> &str {
        self.track_name
            .as_deref()
            .or(self.song_name.as_deref())
            .unwrap_or("")
    }
    fn display_artist(&self) -> &str {
        self.artist_name.as_deref().unwrap_or("")
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LyricText {
    text: String,
    timestamp: f64,
    endtime: f64,
    part: bool,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct LyricsContent {
    timestamp: f64,
    text: Vec<LyricText>,
    background: bool,
    opposite_turn: bool,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct LyricsResponse {
    #[serde(rename = "type")]
    kind: Option<String>,
    content: Vec<LyricsContent>,
    elrc: Option<String>,
    elrc_multi_person: Option<String>,
    ttml_content: Option<String>,
    plain: Option<String>,
}

static CLEANUP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*\(.*?\)|\s*\[.*?\]").expect("regex"));
static WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("regex"));

/// `scoreAndFilterResults`: best first, score > 0 only, at most 10.
pub(crate) fn score_results(
    results: Vec<SearchResult>,
    title: &str,
    artist: &str,
    duration: i32,
) -> Vec<(SearchResult, f64)> {
    let duration_ms = duration as f64 * 1000.0;
    let cleaned_title = CLEANUP
        .replace_all(title, "")
        .to_lowercase()
        .trim()
        .to_string();
    let cleaned_artist = clean_artist(artist).to_lowercase();
    let target_mixed = title.to_lowercase().contains("mixed");
    let target_remix = title.to_lowercase().contains("remix");
    let mut scored: Vec<(SearchResult, f64)> = results
        .into_iter()
        .map(|r| {
            let mut score = 0.0;
            if let Some(d) = r.duration {
                let diff = (d - duration_ms).abs();
                score += if diff <= 2000.0 {
                    100.0
                } else if diff <= 5000.0 {
                    50.0
                } else if diff <= 10000.0 {
                    10.0
                } else {
                    -50.0
                };
            }
            let name = r.display_name().to_string();
            let rt = CLEANUP
                .replace_all(&name, "")
                .to_lowercase()
                .trim()
                .to_string();
            if rt == cleaned_title {
                score += 80.0;
            } else if rt.contains(&cleaned_title) || cleaned_title.contains(&rt) {
                score += 40.0;
            }
            let lname = name.to_lowercase();
            if lname.contains("mixed") && !target_mixed {
                score -= 60.0;
            }
            if lname.contains("remix") && !target_remix {
                score -= 40.0;
            }
            let ra = r.display_artist().to_lowercase();
            if ra.contains(&cleaned_artist) {
                score += 50.0;
            } else if cleaned_artist
                .split_whitespace()
                .filter(|w| w.chars().count() > 2)
                .any(|w| ra.contains(w))
            {
                score += 25.0;
            }
            (r, score)
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .filter(|(_, s)| *s > 0.0)
        .take(10)
        .collect()
}

async fn search(http: &reqwest::Client, query: &str) -> Vec<SearchResult> {
    let u = url(
        &format!("{BASE}/apple-music/search"),
        &[("q", query.to_string())],
    );
    match get_ok(http, &u, &user_agent(), TIMEOUT).await {
        Ok(body) => parse_json::<Vec<SearchResult>>(&body).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// A line's text from its timed pieces (`joinedLineText`).
fn joined_line_text(pieces: &[LyricText]) -> String {
    if pieces.is_empty() {
        return String::new();
    }
    let ends_ws = |t: &str| t.chars().last().is_some_and(char::is_whitespace);
    let spaced_by_api = pieces[..pieces.len() - 1].iter().any(|p| ends_ws(&p.text));
    let mut s = String::new();
    for (i, p) in pieces.iter().enumerate() {
        s.push_str(&p.text);
        if !spaced_by_api && i < pieces.len() - 1 && !p.part && !ends_ws(&p.text) {
            s.push(' ');
        }
    }
    WS.replace_all(&s, " ").trim().to_string()
}

fn centi_stamp(ms: i64) -> String {
    let ms = ms.max(0);
    format!(
        "[{:02}:{:02}.{:02}]",
        ms / 1000 / 60,
        ms / 1000 % 60,
        ms % 1000 / 10
    )
}

fn convert_response(resp: LyricsResponse) -> Option<String> {
    if let Some(ttml) = resp
        .ttml_content
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        && let Ok(lines) = parse_ttml(ttml)
    {
        let lrc = lines_to_lrc(&lines);
        if !lrc.is_empty() {
            return Some(lrc);
        }
    }
    for alt in [resp.elrc_multi_person, resp.elrc, resp.plain]
        .into_iter()
        .flatten()
    {
        if !alt.trim().is_empty() {
            return Some(alt);
        }
    }
    if resp.content.is_empty() {
        return None;
    }
    if resp.kind.as_deref() != Some("Syllable") {
        // "line" lyrics still carry a start time per line
        let timed = resp.content.iter().any(|l| l.timestamp > 0.0);
        let lines: Vec<String> = resp
            .content
            .iter()
            .filter_map(|l| {
                let text = joined_line_text(&l.text);
                if text.trim().is_empty() {
                    None
                } else if timed {
                    Some(format!("{}{text}", centi_stamp(l.timestamp as i64)))
                } else {
                    Some(text)
                }
            })
            .collect();
        return Some(lines.join("\n"));
    }
    let mut out = String::new();
    for line in &resp.content {
        let marker = if line.background {
            "{bg}"
        } else if line.opposite_turn {
            "{agent:v2}"
        } else {
            "{agent:v1}"
        };
        let text = joined_line_text(&line.text);
        if text.trim().is_empty() {
            continue;
        }
        out.push_str(&format!(
            "{}{marker}{text}\n",
            centi_stamp(line.timestamp as i64)
        ));
        if !line.text.is_empty() {
            let words: Vec<String> = line
                .text
                .iter()
                .map(|w| format!("{}:{}:{}", w.text, w.timestamp / 1000.0, w.endtime / 1000.0))
                .collect();
            out.push_str(&format!("<{}>\n", words.join("|")));
        }
    }
    Some(out)
}

/// (lrc, has word timings) — `fetchLyricsForTrackWithType`.
async fn fetch_track(http: &reqwest::Client, id: String) -> (String, bool) {
    let u = url(&format!("{BASE}/apple-music/lyrics"), &[("id", id)]);
    let Ok(body) = get_ok(http, &u, &user_agent(), TIMEOUT).await else {
        return (String::new(), false);
    };
    let Ok(resp) = parse_json::<LyricsResponse>(&body) else {
        return (String::new(), false);
    };
    match convert_response(resp) {
        Some(lrc) => {
            let words = lrc.contains('<') && lrc.contains('>');
            (lrc, words)
        }
        None => (String::new(), false),
    }
}

pub(crate) async fn get_lyrics(http: &reqwest::Client, q: &Query) -> LyricsResult<Option<String>> {
    let ct = clean_title_paxsenix(&q.title);
    let ca = clean_artist(&q.artist);
    let mut queries = vec![format!("{ct} {ca}"), ct.clone()];
    if let Some(album) = q.album.as_ref().filter(|a| !a.trim().is_empty()) {
        queries.push(format!("{ct} {ca} {album}"));
    }
    let mut all = Vec::new();
    for query in &queries {
        if all.is_empty() {
            let results = search(http, query).await;
            if !results.is_empty() {
                all = score_results(results, &q.title, &q.artist, q.duration);
            }
        }
    }
    let mut jobs: FuturesUnordered<_> = all
        .into_iter()
        .take(5)
        .filter_map(|(r, _)| r.id_str())
        .map(|id| fetch_track(http, id))
        .collect();
    let mut plain = None;
    while let Some((lrc, words)) = jobs.next().await {
        if lrc.is_empty() {
            continue;
        }
        if words {
            return Ok(Some(lrc));
        }
        plain.get_or_insert(lrc);
    }
    Ok(plain)
}
