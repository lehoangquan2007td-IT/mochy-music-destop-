//! KuGou: `GET https://mobileservice.kugou.com/api/v3/search/song?version=9108&plat=0&pagesize=8&showtype=0&keyword=`
//! → `GET https://lyrics.kugou.com/search?ver=1&man=yes&client=pc&hash=` (per song within 8 s)
//! or `…&duration=<ms>&keyword=` → `GET https://lyrics.kugou.com/download?fmt=lrc&charset=utf8&client=pc&ver=1&id=&accesskey=`
//! (base64 `content`), then credit lines stripped (`normalize`).

use std::sync::LazyLock;
use std::time::Duration;

use base64::Engine;
use regex::Regex;
use serde::Deserialize;

use super::{Query, enc, get_ok, parse_json, url};
use crate::{LyricsError, LyricsResult};

const PAGE_SIZE: u32 = 8;
const HEAD_CUT_LIMIT: isize = 30;
const DURATION_TOLERANCE: i32 = 8;
const TIMEOUT: Duration = Duration::from_secs(15);
/// Android uses Ktor's default agent for KuGou.
const UA: &str = "Ktor client";

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchSongResponse {
    data: SongData,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SongData {
    info: Vec<SongInfo>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SongInfo {
    duration: i32,
    hash: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchLyricsResponse {
    candidates: Vec<Candidate>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Candidate {
    id: serde_json::Value,
    accesskey: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DownloadResponse {
    content: String,
}

static PAREN: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"\(.*\)",
        r"（.*）",
        r"「.*」",
        r"『.*』",
        r"<.*>",
        r"《.*》",
        r"〈.*〉",
        r"＜.*＞",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("regex"))
    .collect()
});
static ACCEPTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[(\d\d):(\d\d)\.(\d{2,3})\].*$").expect("regex"));
static BANNED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^.+].+[:：].+$").expect("regex"));

struct Keyword {
    title: String,
    artist: String,
    album: Option<String>,
}

impl Keyword {
    fn new(title: &str, artist: &str, album: Option<&str>) -> Keyword {
        let mut t = title.to_string();
        for r in PAREN.iter() {
            t = r.replace_all(&t, "").into_owned();
        }
        let mut a = artist
            .replace(", ", "、")
            .replace(" & ", "、")
            .replace('.', "")
            .replace('和', "、");
        for r in PAREN.iter().take(2) {
            a = r.replace_all(&a, "").into_owned();
        }
        Keyword {
            title: t,
            artist: a,
            album: album.map(str::to_string),
        }
    }

    fn search_query(&self) -> String {
        let mut s = format!("{} - {}", self.title, self.artist);
        if let Some(album) = self.album.as_ref().filter(|a| !a.trim().is_empty()) {
            s.push(' ');
            s.push_str(album);
        }
        s
    }
}

/// `String.normalize()`: keep timed lines, cut credit lines near the head / tail.
pub(crate) fn normalize(lrc: &str) -> String {
    let lines: Vec<&str> = lrc.lines().filter(|l| ACCEPTED.is_match(l)).collect();
    let last = lines.len() as isize - 1;
    let mut head_cut = 0usize;
    let mut i = HEAD_CUT_LIMIT.min(last);
    while i >= 0 {
        if BANNED.is_match(lines[i as usize]) {
            head_cut = i as usize + 1;
            break;
        }
        i -= 1;
    }
    let mut tail_cut = 0usize;
    let mut i = (lines.len() as isize - HEAD_CUT_LIMIT).min(last);
    while i >= 0 {
        if BANNED.is_match(lines[(last - i) as usize]) {
            tail_cut = i as usize + 1;
            break;
        }
        i -= 1;
    }
    let filtered = &lines[head_cut.min(lines.len())..];
    let keep = filtered.len().saturating_sub(tail_cut);
    filtered[..keep].join("\n")
}

async fn get_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    u: &str,
) -> LyricsResult<T> {
    parse_json(&get_ok(http, u, UA, TIMEOUT).await?)
}

pub(crate) async fn get_lyrics(http: &reqwest::Client, q: &Query) -> LyricsResult<Option<String>> {
    let kw = Keyword::new(&q.title, &q.artist, q.album.as_deref());
    let keyword = enc(&kw.search_query());
    let songs_url = format!(
        "{}&keyword={keyword}",
        url(
            "https://mobileservice.kugou.com/api/v3/search/song",
            &[
                ("version", "9108".into()),
                ("plat", "0".into()),
                ("pagesize", PAGE_SIZE.to_string()),
                ("showtype", "0".into())
            ],
        )
    );
    let songs: SearchSongResponse = get_json(http, &songs_url).await?;
    let mut candidate: Option<Candidate> = None;
    for song in songs.data.info {
        if q.duration == -1 || (song.duration - q.duration).abs() <= DURATION_TOLERANCE {
            let u = url(
                "https://lyrics.kugou.com/search",
                &[
                    ("ver", "1".into()),
                    ("man", "yes".into()),
                    ("client", "pc".into()),
                    ("hash", song.hash),
                ],
            );
            let r: SearchLyricsResponse = get_json(http, &u).await?;
            if let Some(c) = r.candidates.into_iter().next() {
                candidate = Some(c);
                break;
            }
        }
    }
    if candidate.is_none() {
        let mut params = vec![
            ("ver", "1".to_string()),
            ("man", "yes".into()),
            ("client", "pc".into()),
        ];
        if q.duration != -1 {
            params.push(("duration", (q.duration as i64 * 1000).to_string()));
        }
        let u = format!(
            "{}&keyword={keyword}",
            url("https://lyrics.kugou.com/search", &params)
        );
        let r: SearchLyricsResponse = get_json(http, &u).await?;
        candidate = r.candidates.into_iter().next();
    }
    let Some(c) = candidate else { return Ok(None) };
    let id = match &c.id {
        serde_json::Value::String(s) => s.clone(),
        v => v.to_string(),
    };
    let u = url(
        "https://lyrics.kugou.com/download",
        &[
            ("fmt", "lrc".into()),
            ("charset", "utf8".into()),
            ("client", "pc".into()),
            ("ver", "1".into()),
            ("id", id),
            ("accesskey", c.accesskey),
        ],
    );
    let dl: DownloadResponse = get_json(http, &u).await?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(dl.content.trim())
        .map_err(|e| LyricsError::Parse(format!("KuGou base64: {e}")))?;
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim_start_matches('\u{FEFF}');
    let out = normalize(text);
    Ok((!out.trim().is_empty()).then_some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_normalization() {
        let k = Keyword::new("晴天（深情版） (Live)", "Jay Chou & Someone, Other", None);
        assert_eq!(k.title, "晴天 ");
        assert_eq!(k.artist, "Jay Chou、Someone、Other");
    }

    #[test]
    fn normalize_cuts_head_credits() {
        let src = "[ti:x]\n[00:00.00]Song - Artist\n[00:01.00]作词：A\n[00:02.00]作曲：B\n[00:05.00]line one\n[00:06.00]line two";
        assert_eq!(normalize(src), "[00:05.00]line one\n[00:06.00]line two");
    }
}
