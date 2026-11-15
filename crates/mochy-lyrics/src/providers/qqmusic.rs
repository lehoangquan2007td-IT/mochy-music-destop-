//! QQ Music (no account): `POST https://u.y.qq.com/cgi-bin/musicu.fcg` with
//! `music.search.SearchCgiService/DoSearchForQQMusicDesktop`, then
//! `music.musichallSong.PlayLyricInfo/GetPlayLyricInfo` (base64 line-timed LRC).

use base64::Engine;
use reqwest::header::{CONTENT_TYPE, USER_AGENT};
use serde::Deserialize;
use serde_json::{Value, json};

use super::direct::{
    TIMEOUT, TrackCandidate, best_match, has_lyrics, parse_lrc, to_lrc, without_credits,
};
use super::{MOCHY_UA, Query, net, parse_json};
use crate::{LyricsError, LyricsResult};

const ENDPOINT: &str = "https://u.y.qq.com/cgi-bin/musicu.fcg";
const SEARCH_LIMIT: u32 = 10;

#[derive(Deserialize, Clone, Default)]
#[serde(default)]
struct Song {
    id: i64,
    mid: String,
    name: String,
    /// seconds
    interval: i32,
    singer: Vec<Singer>,
}

#[derive(Deserialize, Clone, Default)]
#[serde(default)]
struct Singer {
    name: String,
}

async fn musicu(
    http: &reqwest::Client,
    module: &str,
    method: &str,
    param: Value,
) -> LyricsResult<Value> {
    let body = json!({ "req": { "module": module, "method": method, "param": param } });
    let resp = http
        .post(ENDPOINT)
        .header(USER_AGENT, MOCHY_UA)
        .header(CONTENT_TYPE, "application/json")
        .timeout(TIMEOUT)
        .body(body.to_string())
        .send()
        .await
        .map_err(net)?;
    let status = resp.status();
    let text = resp.text().await.map_err(net)?;
    if !status.is_success() {
        return Err(LyricsError::Network(format!("HTTP {status} from QQ Music")));
    }
    parse_json(&text)
}

pub(crate) async fn get_lyrics(http: &reqwest::Client, q: &Query) -> LyricsResult<Option<String>> {
    let search = musicu(
        http,
        "music.search.SearchCgiService",
        "DoSearchForQQMusicDesktop",
        json!({ "query": format!("{} {}", q.title, q.artist), "num_per_page": SEARCH_LIMIT, "page_num": 1, "search_type": 0 }),
    )
    .await?;
    let songs: Vec<Song> = search
        .pointer("/req/data/body/song/list")
        .cloned()
        .map(|v| serde_json::from_value(v).unwrap_or_default())
        .unwrap_or_default();
    let cands: Vec<TrackCandidate<Song>> = songs
        .into_iter()
        .map(|s| TrackCandidate {
            titles: vec![s.name.clone()],
            artists: s.singer.iter().map(|x| x.name.clone()).collect(),
            duration_sec: s.interval,
            r#ref: s,
        })
        .collect();
    let Some(song) = best_match(&cands, &q.title, &q.artist, q.duration) else {
        return Ok(None);
    };
    let lyric = musicu(
        http,
        "music.musichallSong.PlayLyricInfo",
        "GetPlayLyricInfo",
        json!({ "songMID": song.mid, "songID": song.id }),
    )
    .await?;
    let encoded = lyric
        .pointer("/req/data/lyric")
        .and_then(Value::as_str)
        .unwrap_or("");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|e| LyricsError::Parse(format!("QQ lyric base64: {e}")))?;
    let lrc = String::from_utf8_lossy(&bytes);
    let lines = without_credits(parse_lrc(&lrc));
    Ok(has_lyrics(&lines).then(|| to_lrc(&lines)))
}
