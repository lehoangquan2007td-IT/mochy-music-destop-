//! The three YouTube sources (Android `YouTubeLyricsProvider`, `YouTubeSubtitleLyricsProvider`,
//! `YouTubeCaptionsLyricsProvider` + innertube `Captions.kt`):
//!
//! - **YouTubeMusic**: the "Lyrics" tab of the `next` response, browsed (plain text).
//! - **YouTubeSubtitle**: `get_transcript` as the WEB client — the video's transcript panel,
//!   one `[mm:ss.mmm]` line per cue group.
//! - **YouTubeCaptions**: the video's caption track (auto-generated, or an upload in the same
//!   language), asked as clients whose caption links work as-is, downloaded as `fmt=json3`.
//!
//! All three need the video id; local files have none.

use std::sync::OnceLock;
use std::time::Duration;

use base64::Engine as _;
use mochy_innertube::{YtmClient, clients};
use serde_json::{Value, json};

use super::Query;
use crate::{LyricsError, LyricsResult};

const TIMEOUT: Duration = Duration::from_secs(15);

/// A client for callers that didn't hand one over (tests, the example): no visitor data yet,
/// it adopts the one YouTube assigns.
fn fallback_client() -> Option<&'static YtmClient> {
    static CLIENT: OnceLock<Option<YtmClient>> = OnceLock::new();
    CLIENT.get_or_init(|| YtmClient::new_offline().ok()).as_ref()
}

fn client(q: &Query) -> LyricsResult<&YtmClient> {
    match &q.ytm {
        Some(c) => Ok(c),
        None => fallback_client().ok_or_else(|| LyricsError::Other("no InnerTube client".into())),
    }
}

fn yt_err(e: mochy_innertube::YtmError) -> LyricsError {
    LyricsError::Network(e.to_string())
}

/// YouTube Music's own lyrics (unsynced).
pub(crate) async fn music_lyrics(q: &Query) -> LyricsResult<Option<String>> {
    let Some(id) = q.video_id.as_deref() else { return Ok(None) };
    let text = client(q)?.lyrics(id).await.map_err(yt_err)?;
    Ok(text.filter(|t| !t.trim().is_empty()))
}

// ---- transcript ------------------------------------------------------------------------------

/// `get_transcript` params: protobuf field 1 = video id (`"\n" + len + id`, base64).
pub(crate) fn transcript_params(video_id: &str) -> String {
    let mut raw = vec![b'\n', video_id.len() as u8];
    raw.extend_from_slice(video_id.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(raw)
}

fn lrc_stamp(ms: u64) -> String {
    format!("[{:02}:{:02}.{:03}]", ms / 60_000, ms / 1000 % 60, ms % 1000)
}

fn as_u64(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

fn simple_text(v: &Value) -> String {
    if let Some(s) = v["simpleText"].as_str() {
        return s.to_string();
    }
    v["runs"]
        .as_array()
        .map(|runs| runs.iter().filter_map(|r| r["text"].as_str()).collect::<String>())
        .unwrap_or_default()
}

/// Transcript response → LRC (Android `YouTube.transcript`). `None` without cue groups.
pub(crate) fn transcript_to_lrc(json: &Value) -> Option<String> {
    let groups = json["actions"][0]["updateEngagementPanelAction"]["content"]["transcriptRenderer"]["body"]
        ["transcriptBodyRenderer"]["cueGroups"]
        .as_array()?;
    let lines: Vec<String> = groups
        .iter()
        .filter_map(|g| {
            let cue = &g["transcriptCueGroupRenderer"]["cues"][0]["transcriptCueRenderer"];
            let time = as_u64(&cue["startOffsetMs"])?;
            let text = simple_text(&cue["cue"]);
            let text = text.trim_matches('♪').trim_matches(' ');
            Some(format!("{}{}", lrc_stamp(time), text))
        })
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

pub(crate) async fn transcript(q: &Query) -> LyricsResult<Option<String>> {
    let Some(id) = q.video_id.as_deref() else { return Ok(None) };
    let ytm = client(q)?;
    let (hl, gl) = ytm.locale_pair();
    let c = &clients::WEB;
    let body = json!({
        "context": { "client": { "clientName": c.name, "clientVersion": c.version, "hl": hl, "gl": gl } },
        "params": transcript_params(id),
    });
    let resp = ytm
        .http
        .post("https://music.youtube.com/youtubei/v1/get_transcript?prettyPrint=false")
        .header("Content-Type", "application/json")
        .header("User-Agent", c.user_agent)
        .header("X-YouTube-Client-Name", c.id)
        .header("X-YouTube-Client-Version", c.version)
        .header("Origin", "https://music.youtube.com")
        .timeout(TIMEOUT)
        .json(&body)
        .send()
        .await
        .map_err(super::net)?;
    if !resp.status().is_success() {
        // 400 "Precondition check failed": no transcript for this video, or (as measured on
        // 2026-11-17 for every video) YouTube refusing unauthenticated transcript requests.
        return Ok(None);
    }
    let json: Value = resp.json().await.map_err(super::net)?;
    Ok(transcript_to_lrc(&json))
}

// ---- captions --------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CaptionTrack {
    pub base_url: String,
    pub language: Option<String>,
    pub auto_generated: bool,
}

pub(crate) fn caption_tracks(player: &Value) -> Vec<CaptionTrack> {
    player["captions"]["playerCaptionsTracklistRenderer"]["captionTracks"]
        .as_array()
        .map(|tracks| {
            tracks
                .iter()
                .filter_map(|t| {
                    Some(CaptionTrack {
                        base_url: t["baseUrl"].as_str()?.to_string(),
                        language: t["languageCode"].as_str().map(str::to_string),
                        auto_generated: t["kind"].as_str() == Some("asr"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The auto-generated track, or an uploaded one in its language (the auto track says which
/// language is sung; uploads are as often translations). With no auto track, nothing.
pub(crate) fn pick_caption_track(tracks: &[CaptionTrack]) -> Option<&CaptionTrack> {
    let auto = tracks.iter().find(|t| t.auto_generated)?;
    let lang = auto.language.as_deref().map(|l| l.split('-').next().unwrap_or(l));
    tracks
        .iter()
        .find(|t| !t.auto_generated && t.language.as_deref().map(|l| l.split('-').next().unwrap_or(l)) == lang)
        .or(Some(auto))
}

/// Sound labels like "[Music]" and note signs aren't words.
fn clean_caption(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0;
    for c in text.chars() {
        match c {
            '[' => {
                depth += 1;
                out.push(' ');
            }
            ']' if depth > 0 => depth -= 1,
            _ if depth > 0 => {}
            '♪' => out.push(' '),
            _ => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Timed-text JSON (`fmt=json3`) → LRC, one line per caption; empty events drop out and a
/// rolling caption repeating its line is skipped (Android `timedTextToLrc`).
pub(crate) fn timed_text_to_lrc(body: &str) -> String {
    let Ok(json) = serde_json::from_str::<Value>(body) else { return String::new() };
    let Some(events) = json["events"].as_array() else { return String::new() };
    let mut lines = Vec::new();
    let mut previous: Option<String> = None;
    for e in events {
        let Some(start) = as_u64(&e["tStartMs"]) else { continue };
        let Some(segs) = e["segs"].as_array() else { continue };
        let raw: String = segs.iter().filter_map(|s| s["utf8"].as_str()).collect();
        let text = clean_caption(&raw);
        if text.is_empty() || previous.as_deref() == Some(text.as_str()) {
            continue;
        }
        lines.push(format!("{}{}", lrc_stamp(start), text));
        previous = Some(text);
    }
    lines.join("\n")
}

/// `baseUrl` with `fmt=json3`, replacing the `fmt` it may already carry (Ktor
/// `url.parameters["fmt"] = "json3"`).
pub(crate) fn with_fmt_json3(base: &str) -> String {
    let (path, query) = base.split_once('?').unwrap_or((base, ""));
    let mut params: Vec<&str> = query.split('&').filter(|p| !p.is_empty() && !p.starts_with("fmt=")).collect();
    params.push("fmt=json3");
    format!("{path}?{}", params.join("&"))
}

/// Timed-text XML (`format="3"`, `<p t= d=>`; also the old `<text start= dur=>`) → LRC, for
/// servers that ignore `fmt=json3`.
pub(crate) fn srv3_to_lrc(body: &str) -> String {
    let Ok(root) = crate::xml::parse(body) else { return String::new() };
    let mut ps = Vec::new();
    root.descendants("p", &mut ps);
    let mut lines = Vec::new();
    let mut previous: Option<String> = None;
    let mut push = |start: u64, raw: String| {
        let text = clean_caption(&raw);
        if text.is_empty() || previous.as_deref() == Some(text.as_str()) {
            return;
        }
        lines.push(format!("{}{}", lrc_stamp(start), text));
        previous = Some(text);
    };
    if ps.is_empty() {
        let mut texts = Vec::new();
        root.descendants("text", &mut texts);
        for t in texts {
            if let Some(start) = t.attr("start").and_then(|s| s.parse::<f64>().ok()) {
                push((start * 1000.0).round() as u64, t.text_content());
            }
        }
    } else {
        for p in ps {
            if let Some(start) = p.attr("t").and_then(|s| s.parse::<u64>().ok()) {
                push(start, p.text_content());
            }
        }
    }
    lines.join("
")
}

async fn captions_as(ytm: &YtmClient, video_id: &str, client: &clients::ClientSpec) -> LyricsResult<Option<String>> {
    let player = ytm.player(client, video_id).await.map_err(yt_err)?;
    let tracks = caption_tracks(&player);
    let Some(track) = pick_caption_track(&tracks) else { return Ok(None) };
    let url = with_fmt_json3(&track.base_url);
    let resp = ytm
        .http
        .get(&url)
        .header("User-Agent", client.user_agent)
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(super::net)?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let body = resp.text().await.map_err(super::net)?;
    let lrc = if body.trim_start().starts_with('<') { srv3_to_lrc(&body) } else { timed_text_to_lrc(&body) };
    Ok((!lrc.trim().is_empty()).then_some(lrc))
}

/// The video's captions as line-synced lyrics, from the first client whose tracks download.
pub(crate) async fn captions(q: &Query) -> LyricsResult<Option<String>> {
    let Some(id) = q.video_id.as_deref() else { return Ok(None) };
    let ytm = client(q)?;
    for c in [&clients::ANDROID_VR_NO_AUTH, &clients::WEB_REMIX] {
        if let Ok(Some(lrc)) = captions_as(ytm, id, c).await {
            return Ok(Some(lrc));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_params_encode_video_id() {
        // "\n\x0b" + id, base64 — same bytes as Android's `"\n${11.toChar()}$videoId"`
        let p = transcript_params("dQw4w9WgXcQ");
        let raw = base64::engine::general_purpose::STANDARD.decode(p).unwrap();
        assert_eq!(&raw[..2], b"\n\x0b");
        assert_eq!(&raw[2..], b"dQw4w9WgXcQ");
    }

    #[test]
    fn transcript_fixture_to_lrc() {
        let json = json!({"actions": [{"updateEngagementPanelAction": {"content": {"transcriptRenderer": {"body": {
            "transcriptBodyRenderer": {"cueGroups": [
                {"transcriptCueGroupRenderer": {"cues": [{"transcriptCueRenderer": {"cue": {"simpleText": "♪ Never gonna give you up ♪"}, "startOffsetMs": "18800", "durationMs": "2000"}}]}},
                {"transcriptCueGroupRenderer": {"cues": [{"transcriptCueRenderer": {"cue": {"runs": [{"text": "Never gonna "}, {"text": "let you down"}]}, "startOffsetMs": 61005}}]}}
            ]}}}}}}]});
        assert_eq!(
            transcript_to_lrc(&json).unwrap(),
            "[00:18.800]Never gonna give you up\n[01:01.005]Never gonna let you down"
        );
        assert!(transcript_to_lrc(&json!({})).is_none());
    }

    #[test]
    fn caption_track_choice() {
        let player = json!({"captions": {"playerCaptionsTracklistRenderer": {"captionTracks": [
            {"baseUrl": "https://x/en", "languageCode": "en"},
            {"baseUrl": "https://x/ja", "languageCode": "ja"},
            {"baseUrl": "https://x/ja-asr", "languageCode": "ja", "kind": "asr"}
        ]}}});
        let tracks = caption_tracks(&player);
        assert_eq!(tracks.len(), 3);
        // uploaded track in the sung (auto) language wins over the translation and the asr one
        assert_eq!(pick_caption_track(&tracks).unwrap().base_url, "https://x/ja");
        // without an auto track nothing is picked
        assert!(pick_caption_track(&tracks[..2]).is_none());
        // only auto
        assert_eq!(pick_caption_track(&tracks[2..]).unwrap().base_url, "https://x/ja-asr");
    }

    #[test]
    fn fmt_param_is_replaced() {
        assert_eq!(
            with_fmt_json3("https://www.youtube.com/api/timedtext?v=x&fmt=srv3&lang=en"),
            "https://www.youtube.com/api/timedtext?v=x&lang=en&fmt=json3"
        );
        assert_eq!(with_fmt_json3("https://x/t"), "https://x/t?fmt=json3");
    }

    #[test]
    fn srv3_fixture() {
        let body = r#"<?xml version="1.0" encoding="utf-8" ?><timedtext format="3">
<body>
<p t="1360" d="1680">[♪♪♪]</p>
<p t="18640" d="3240">♪ We&#39;re no strangers to love ♪</p>
<p t="22640" d="4320"><s>You</s><s t="200"> know</s></p>
</body></timedtext>"#;
        assert_eq!(srv3_to_lrc(body), "[00:18.640]We're no strangers to love
[00:22.640]You know");
        let old = r#"<transcript><text start="1.5" dur="2">hi &amp; bye</text></transcript>"#;
        assert_eq!(srv3_to_lrc(old), "[00:01.500]hi & bye");
    }

    #[test]
    fn timed_text_fixture() {
        let body = r#"{"events":[
            {"tStartMs":0,"dDurationMs":1000,"segs":[{"utf8":"[Music]"}]},
            {"tStartMs":1500,"dDurationMs":2000,"segs":[{"utf8":"hello"},{"utf8":" world","tOffsetMs":400}]},
            {"tStartMs":3000,"segs":[{"utf8":"\n"}]},
            {"tStartMs":3500,"dDurationMs":2000,"segs":[{"utf8":"hello world"}]},
            {"tStartMs":65010,"dDurationMs":2000,"segs":[{"utf8":"♪ next  line ♪"}]}
        ]}"#;
        assert_eq!(timed_text_to_lrc(body), "[00:01.500]hello world\n[01:05.010]next line");
        assert_eq!(timed_text_to_lrc("not json"), "");
    }
}
