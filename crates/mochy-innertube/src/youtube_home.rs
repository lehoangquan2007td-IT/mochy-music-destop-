//! The youtube.com home feed (`FEwhat_to_watch`, `WEB` client) reduced to
//! what a music app wants from it: the **mixes** ("My Mix", "Mix – <artist>",
//! "Mix – <song>", playlist ids starting with `RD`) and, optionally, music
//! videos (from official artist / Topic / VEVO channels — a heuristic, since
//! the feed doesn't say which videos are music).
//!
//! Signed out, YouTube serves an empty home ("try searching"), so this is
//! useful with [`crate::YtmClient::set_auth`] only.
//!
//! Mixes are played through YouTube Music's `next` endpoint with their seed
//! video ([`MixSeed`]); [`crate::YtmClient::playlist`] does that for `RD…`
//! ids so the rest of the app can treat a mix as a playlist.

use std::time::Duration;

use mochy_core::models::{ArtistRef, Playlist, Song};
use serde_json::{Value, json};

use crate::{YtmClient, YtmResult, clients, parse};

pub const HOME_BROWSE_ID: &str = "FEwhat_to_watch";

/// How a mix starts: its first video and the watch params.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MixSeed {
    pub playlist_id: String,
    pub video_id: Option<String>,
    pub params: Option<String>,
    pub title: String,
    pub subtitle: Option<String>,
    pub thumbnail_url: Option<String>,
}

impl MixSeed {
    /// As a playlist card (`id` = `RD…`, `browse_id` = `VLRD…`).
    pub fn to_playlist(&self) -> Playlist {
        Playlist {
            id: self.playlist_id.clone(),
            name: self.title.clone(),
            browse_id: Some(format!("VL{}", self.playlist_id)),
            thumbnail_url: self.thumbnail_url.clone(),
            song_count: 0,
            description: self.subtitle.clone(),
            is_editable: false,
            created_at: 0,
        }
    }
}

/// One page (or several, merged) of the youtube.com home feed.
#[derive(Debug, Clone, Default)]
pub struct YoutubeHome {
    pub mixes: Vec<MixSeed>,
    /// Videos that look like music (artist badge, Topic or VEVO channel).
    pub music_videos: Vec<Song>,
    pub continuation: Option<String>,
}

/// A mix is an `RD…` playlist (radio). `RDCLAK…` are curated YouTube Music
/// playlists and `RDTMAK…` YTM supermixes: those browse like playlists.
pub fn is_mix_id(id: &str) -> bool {
    let id = id.strip_prefix("VL").unwrap_or(id);
    id.starts_with("RD") && !id.starts_with("RDCLAK") && !id.starts_with("RDTMAK")
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Visit every object in `v` depth-first; `f` returns `true` to skip the
/// object's children (it was consumed).
fn walk<'a>(v: &'a Value, f: &mut impl FnMut(&'a str, &'a Value) -> bool) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                if !f(k, child) {
                    walk(child, f);
                }
            }
        }
        Value::Array(a) => {
            for child in a {
                walk(child, f);
            }
        }
        _ => {}
    }
}

fn find_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    let mut out = None;
    walk(v, &mut |k, child| {
        if out.is_none() && k == key && child.is_string() {
            out = child.as_str();
        }
        out.is_some()
    });
    out
}

fn contains_str(v: &Value, needle: &str) -> bool {
    let mut hit = false;
    walk(v, &mut |_, child| {
        if !hit && child.as_str().is_some_and(|s| s == needle) {
            hit = true;
        }
        hit
    });
    hit
}

/// Largest image of a `sources` / `thumbnails` array.
fn best_image(arr: &Value) -> Option<String> {
    let list = arr.as_array()?;
    list.iter()
        .max_by_key(|t| t["width"].as_u64().unwrap_or(0))
        .and_then(|t| t["url"].as_str())
        .map(|u| if u.starts_with("//") { format!("https:{u}") } else { u.to_string() })
}

/// Duration text like "4:39" / "1:02:03" anywhere under `v`.
fn duration_under(v: &Value) -> Option<f64> {
    let mut out = None;
    walk(v, &mut |k, child| {
        if out.is_none()
            && (k == "text" || k == "simpleText" || k == "content")
            && let Some(s) = child.as_str()
            && s.contains(':')
            && s.chars().all(|c| c.is_ascii_digit() || c == ':')
        {
            out = parse::parse_duration(s);
        }
        out.is_some()
    });
    out
}

/// "Sơn Tùng M-TP - Topic" / "… - Chủ đề" → artist name, and whether it was a
/// Topic (auto-generated music) channel.
fn strip_topic(channel: &str) -> (String, bool) {
    for suffix in [" - Topic", " – Topic", " - Chủ đề", " – Chủ đề"] {
        if let Some(name) = channel.strip_suffix(suffix) {
            return (name.trim().to_string(), true);
        }
    }
    (channel.trim().to_string(), false)
}

fn looks_like_music_channel(channel: &str, artist_badge: bool) -> bool {
    let (_, topic) = strip_topic(channel);
    artist_badge || topic || channel.to_ascii_uppercase().ends_with("VEVO")
}

fn video_song(video_id: &str, title: &str, channel: &str, channel_id: Option<&str>, duration: Option<f64>, thumb: Option<String>) -> Song {
    let (artist, _) = strip_topic(channel);
    Song {
        id: video_id.to_string(),
        video_id: Some(video_id.to_string()),
        title: title.to_string(),
        artists: vec![ArtistRef { id: channel_id.map(str::to_string), name: artist }],
        duration_secs: duration,
        thumbnail_url: thumb.or_else(|| Some(format!("https://i.ytimg.com/vi/{video_id}/hqdefault.jpg"))),
        is_video: true,
        ..Default::default()
    }
}

/// `lockupViewModel` → mix seed (playlist lockups with an `RD` id).
fn lockup_mix(l: &Value) -> Option<MixSeed> {
    if l["contentType"].as_str() != Some("LOCKUP_CONTENT_TYPE_PLAYLIST") {
        return None;
    }
    let id = l["contentId"].as_str()?;
    if !is_mix_id(id) {
        return None;
    }
    let md = &l["metadata"]["lockupMetadataViewModel"];
    let title = md["title"]["content"].as_str().unwrap_or_default().trim().to_string();
    let subtitle = md["metadata"]["contentMetadataViewModel"]["metadataRows"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["metadataParts"].is_array()))
        .and_then(|r| r["metadataParts"][0]["text"]["content"].as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let watch = &l["rendererContext"]["commandContext"]["onTap"]["innertubeCommand"]["watchEndpoint"];
    let thumb = best_image(&l["contentImage"]["collectionThumbnailViewModel"]["primaryThumbnail"]["thumbnailViewModel"]["image"]["sources"])
        .or_else(|| best_image(&l["contentImage"]["thumbnailViewModel"]["image"]["sources"]));
    Some(MixSeed {
        playlist_id: id.to_string(),
        video_id: watch["videoId"].as_str().map(str::to_string),
        params: watch["params"].as_str().map(str::to_string),
        title: if title.is_empty() { "Mix".into() } else { title },
        subtitle,
        thumbnail_url: thumb,
    })
}

/// `lockupViewModel` (video) → song when it looks like music.
fn lockup_video(l: &Value) -> Option<Song> {
    if l["contentType"].as_str() != Some("LOCKUP_CONTENT_TYPE_VIDEO") {
        return None;
    }
    let id = l["contentId"].as_str()?;
    let md = &l["metadata"]["lockupMetadataViewModel"];
    let title = md["title"]["content"].as_str()?.trim();
    let first_row = md["metadata"]["contentMetadataViewModel"]["metadataRows"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["metadataParts"].is_array()))?;
    let channel_part = &first_row["metadataParts"][0];
    let channel = channel_part["text"]["content"].as_str()?.trim();
    let badge = contains_str(channel_part, "AUDIO_BADGE")
        || contains_str(channel_part, "OFFICIAL_ARTIST_BADGE")
        || contains_str(first_row, "AUDIO_BADGE");
    if !looks_like_music_channel(channel, badge) {
        return None;
    }
    let channel_id = find_str(channel_part, "browseId").or_else(|| find_str(&md["image"], "browseId"));
    let thumb = best_image(&l["contentImage"]["thumbnailViewModel"]["image"]["sources"]);
    Some(video_song(id, title, channel, channel_id, duration_under(&l["contentImage"]), thumb))
}

/// Classic `videoRenderer` → song when it looks like music.
fn video_renderer(v: &Value) -> Option<Song> {
    let id = v["videoId"].as_str()?;
    let title = parse::nav_text(&v["title"]);
    let owner = if v["ownerText"].is_object() { &v["ownerText"] } else { &v["longBylineText"] };
    let channel = parse::nav_text(owner);
    if title.is_empty() || channel.is_empty() {
        return None;
    }
    let badge = v["ownerBadges"].as_array().is_some_and(|b| {
        b.iter().any(|x| {
            let style = x["metadataBadgeRenderer"]["style"].as_str().unwrap_or_default();
            style == "BADGE_STYLE_TYPE_VERIFIED_ARTIST"
        })
    });
    if !looks_like_music_channel(&channel, badge) {
        return None;
    }
    let channel_id = owner["runs"][0]["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str();
    let duration = v["lengthText"]["simpleText"].as_str().and_then(parse::parse_duration).or_else(|| duration_under(&v["lengthText"]));
    Some(video_song(id, &title, &channel, channel_id, duration, best_image(&v["thumbnail"]["thumbnails"])))
}

/// Classic `radioRenderer` (older layout for mixes).
fn radio_renderer(r: &Value) -> Option<MixSeed> {
    let id = r["playlistId"].as_str()?;
    if !is_mix_id(id) {
        return None;
    }
    let watch = &r["navigationEndpoint"]["watchEndpoint"];
    Some(MixSeed {
        playlist_id: id.to_string(),
        video_id: watch["videoId"].as_str().map(str::to_string),
        params: watch["params"].as_str().map(str::to_string),
        title: parse::nav_text(&r["title"]),
        subtitle: Some(parse::nav_text(&r["longBylineText"])).filter(|s| !s.is_empty()),
        thumbnail_url: best_image(&r["thumbnail"]["thumbnails"]),
    })
}

/// Parse a home response (first page or `appendContinuationItemsAction`).
/// Shorts shelves are skipped; items keep feed order; duplicates dropped.
pub fn home_response(json: &Value) -> YoutubeHome {
    let mut out = YoutubeHome::default();
    walk(json, &mut |k, v| match k {
        "reelShelfRenderer" | "shortsLockupViewModel" | "reelItemRenderer" => true,
        "lockupViewModel" => {
            if let Some(m) = lockup_mix(v) {
                if !out.mixes.iter().any(|x| x.playlist_id == m.playlist_id) {
                    out.mixes.push(m);
                }
            } else if let Some(s) = lockup_video(v)
                && !out.music_videos.iter().any(|x| x.id == s.id)
            {
                out.music_videos.push(s);
            }
            true
        }
        "videoRenderer" => {
            if let Some(s) = video_renderer(v)
                && !out.music_videos.iter().any(|x| x.id == s.id)
            {
                out.music_videos.push(s);
            }
            true
        }
        "radioRenderer" | "compactRadioRenderer" => {
            if let Some(m) = radio_renderer(v)
                && !out.mixes.iter().any(|x| x.playlist_id == m.playlist_id)
            {
                out.mixes.push(m);
            }
            true
        }
        "continuationItemRenderer" => {
            if let Some(t) = v["continuationEndpoint"]["continuationCommand"]["token"].as_str() {
                out.continuation = Some(t.to_string());
            }
            true
        }
        _ => false,
    });
    out
}

// ---------------------------------------------------------------------------
// Client calls
// ---------------------------------------------------------------------------

impl YtmClient {
    async fn web_browse(&self, extra: Value) -> YtmResult<Value> {
        let visitor = self.visitor_data();
        let mut body = json!({ "context": self.context(&clients::WEB, Some(&visitor)) });
        if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                b.insert(k.clone(), v.clone());
            }
        }
        let resp = self.post_api(&clients::WEB, "browse", body, Some(&visitor), Duration::from_secs(20)).await?;
        self.note_logged_out(&resp);
        Ok(resp)
    }

    /// The youtube.com home feed: first page plus continuations until at
    /// least `min_mixes` mixes were found or `max_pages` pages were read.
    pub async fn youtube_home(&self, min_mixes: usize, max_pages: usize) -> YtmResult<YoutubeHome> {
        let first = self.web_browse(json!({ "browseId": HOME_BROWSE_ID })).await?;
        let mut home = home_response(&first);
        let mut pages = 1;
        while home.mixes.len() < min_mixes && pages < max_pages {
            let Some(token) = home.continuation.take() else { break };
            pages += 1;
            match self.web_browse(json!({ "continuation": token })).await {
                Ok(page) => {
                    let more = home_response(&page);
                    for m in more.mixes {
                        if !home.mixes.iter().any(|x| x.playlist_id == m.playlist_id) {
                            home.mixes.push(m);
                        }
                    }
                    for s in more.music_videos {
                        if !home.music_videos.iter().any(|x| x.id == s.id) {
                            home.music_videos.push(s);
                        }
                    }
                    home.continuation = more.continuation;
                }
                Err(e) => {
                    tracing_warn(&format!("youtube home continuation: {e}"));
                    break;
                }
            }
        }
        self.remember_mixes(&home.mixes);
        Ok(home)
    }

    /// Remember seeds so [`Self::playlist`] can start these mixes later.
    pub fn remember_mixes(&self, mixes: &[MixSeed]) {
        let mut seeds = self.inner.mix_seeds.lock().expect("mix seeds lock");
        for m in mixes {
            seeds.insert(m.playlist_id.clone(), m.clone());
        }
    }

    pub(crate) fn mix_seed(&self, playlist_id: &str) -> Option<MixSeed> {
        self.inner.mix_seeds.lock().expect("mix seeds lock").get(playlist_id).cloned()
    }

    /// Songs of a mix (`RD…`) through YouTube Music's `next` endpoint, with
    /// up to two radio continuations (≈ 50–75 songs).
    pub(crate) async fn mix_playlist(&self, playlist_id: &str) -> YtmResult<(Playlist, Vec<Song>)> {
        let seed = self.mix_seed(playlist_id);
        let video = seed.as_ref().and_then(|s| s.video_id.clone()).or_else(|| {
            // "RD" + an 11-char video id is that video's radio.
            let rest = &playlist_id[2..];
            (rest.len() == 11).then(|| rest.to_string())
        });
        let mut json = self.next_raw(video.as_deref(), Some(playlist_id), None).await?;
        let mut songs = parse::songs_from_next(&json);
        if songs.is_empty()
            && let Some(params) = seed.as_ref().and_then(|s| s.params.clone())
        {
            json = self.next_raw(video.as_deref(), Some(playlist_id), Some(&params)).await?;
            songs = parse::songs_from_next(&json);
        }
        for _ in 0..2 {
            let Some(token) = radio_continuation(&json) else { break };
            let Ok(page) = self.remix("next", json!({ "continuation": token, "isAudioOnly": true })).await else { break };
            let before = songs.len();
            for s in parse::songs_from_next(&page) {
                if !songs.iter().any(|x| x.id == s.id) {
                    songs.push(s);
                }
            }
            if songs.len() == before {
                break;
            }
            json = page;
        }
        if songs.is_empty() {
            return Err(crate::YtmError::Parse(format!("mix {playlist_id}: no songs")));
        }
        let panel_title = panel_title(&json);
        let mut playlist = seed.map(|s| s.to_playlist()).unwrap_or_else(|| Playlist {
            id: playlist_id.to_string(),
            name: panel_title.clone().unwrap_or_else(|| "Mix".into()),
            browse_id: Some(format!("VL{playlist_id}")),
            ..Default::default()
        });
        if playlist.name.is_empty() {
            playlist.name = panel_title.unwrap_or_else(|| "Mix".into());
        }
        if playlist.thumbnail_url.is_none() {
            playlist.thumbnail_url = songs.first().and_then(|s| s.thumbnail_url.clone());
        }
        playlist.song_count = songs.len() as u32;
        Ok((playlist, songs))
    }
}

/// Radio continuation token of a `next` panel (first page or continuation).
fn radio_continuation(json: &Value) -> Option<String> {
    let mut out = None;
    walk(json, &mut |k, v| {
        if out.is_none() && (k == "nextRadioContinuationData" || k == "nextContinuationData") {
            out = v["continuation"].as_str().map(str::to_string);
        }
        out.is_some()
    });
    out
}

fn panel_title(json: &Value) -> Option<String> {
    let mut out = None;
    walk(json, &mut |k, v| {
        if out.is_none() && k == "playlistPanelRenderer" {
            let t = v["title"].as_str().map(str::to_string).unwrap_or_else(|| parse::nav_text(&v["title"]));
            if !t.is_empty() {
                out = Some(t);
            }
        }
        out.is_some()
    });
    out
}

fn tracing_warn(msg: &str) {
    eprintln!("[mochy-innertube] {msg}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mix_lockup(id: &str, title: &str) -> Value {
        json!({ "lockupViewModel": {
            "contentId": id,
            "contentType": "LOCKUP_CONTENT_TYPE_PLAYLIST",
            "contentImage": { "collectionThumbnailViewModel": { "primaryThumbnail": { "thumbnailViewModel": { "image": { "sources": [
                { "url": "https://i.ytimg.com/vi/abc/hqdefault.jpg", "width": 480, "height": 270 }
            ] } } } } },
            "metadata": { "lockupMetadataViewModel": {
                "title": { "content": title },
                "metadata": { "contentMetadataViewModel": { "metadataRows": [
                    { "metadataParts": [{ "text": { "content": "Sơn Tùng M-TP, Bảo Anh và nhiều nghệ sĩ khác" } }, { "text": { "content": "Danh sách phát" } }] }
                ] } }
            } },
            "rendererContext": { "commandContext": { "onTap": { "innertubeCommand": { "watchEndpoint": {
                "videoId": "bTuqSWUYE6E", "playlistId": id, "params": "OALAAQE%3D"
            } } } } }
        } })
    }

    fn video_lockup(id: &str, channel: &str, badge: Option<&str>) -> Value {
        let mut part = json!({ "text": { "content": channel, "commandRuns": [{ "onTap": { "innertubeCommand": { "browseEndpoint": { "browseId": "UCchan" } } } }] } });
        if let Some(b) = badge {
            part["text"]["attachmentRuns"] = json!([{ "element": { "type": { "imageType": { "image": { "sources": [{ "clientResource": { "imageName": b } }] } } } } }]);
        }
        json!({ "lockupViewModel": {
            "contentId": id,
            "contentType": "LOCKUP_CONTENT_TYPE_VIDEO",
            "contentImage": { "thumbnailViewModel": {
                "image": { "sources": [{ "url": format!("https://i.ytimg.com/vi/{id}/hq720.jpg"), "width": 720 }] },
                "overlays": [{ "thumbnailBottomOverlayViewModel": { "badges": [{ "thumbnailBadgeViewModel": { "text": "4:39" } }] } }]
            } },
            "metadata": { "lockupMetadataViewModel": {
                "title": { "content": format!("Song {id}") },
                "metadata": { "contentMetadataViewModel": { "metadataRows": [
                    { "metadataParts": [part] },
                    { "metadataParts": [{ "text": { "content": "1,2 Tr lượt xem" } }] }
                ] } }
            } }
        } })
    }

    fn home_fixture() -> Value {
        json!({
            "contents": { "twoColumnBrowseResultsRenderer": { "tabs": [{ "tabRenderer": { "content": { "richGridRenderer": { "contents": [
                { "richItemRenderer": { "content": mix_lockup("RDbTuqSWUYE6E", "Danh sách kết hợp – Sơn Tùng M-TP") } },
                { "richItemRenderer": { "content": video_lockup("vid00000001", "Sơn Tùng M-TP Official", Some("AUDIO_BADGE")) } },
                { "richItemRenderer": { "content": video_lockup("vid00000002", "Random Vlogger", Some("CHECK_CIRCLE_FILLED")) } },
                { "richItemRenderer": { "content": video_lockup("vid00000003", "Hoàng Dũng - Chủ đề", None) } },
                { "richSectionRenderer": { "content": { "richShelfRenderer": { "contents": [
                    { "richItemRenderer": { "content": { "shortsLockupViewModel": { "entityId": "x" } } } }
                ] } } } },
                { "richItemRenderer": { "content": mix_lockup("RDMM", "My Mix") } },
                { "richItemRenderer": { "content": mix_lockup("RDCLAK5uy_curated", "Curated (not a mix)") } },
                { "richItemRenderer": { "content": mix_lockup("RDbTuqSWUYE6E", "duplicate") } },
                { "continuationItemRenderer": { "continuationEndpoint": { "continuationCommand": { "token": "NEXT" } } } }
            ] } } } }] } }
        })
    }

    #[test]
    fn parses_mixes_and_music_videos() {
        let home = home_response(&home_fixture());
        let ids: Vec<_> = home.mixes.iter().map(|m| m.playlist_id.as_str()).collect();
        assert_eq!(ids, ["RDbTuqSWUYE6E", "RDMM"]);
        let m = &home.mixes[0];
        assert_eq!(m.title, "Danh sách kết hợp – Sơn Tùng M-TP");
        assert_eq!(m.video_id.as_deref(), Some("bTuqSWUYE6E"));
        assert_eq!(m.params.as_deref(), Some("OALAAQE%3D"));
        assert!(m.subtitle.as_deref().unwrap().starts_with("Sơn Tùng"));
        assert_eq!(m.thumbnail_url.as_deref(), Some("https://i.ytimg.com/vi/abc/hqdefault.jpg"));
        let p = m.to_playlist();
        assert_eq!(p.browse_id.as_deref(), Some("VLRDbTuqSWUYE6E"));

        let vids: Vec<_> = home.music_videos.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(vids, ["vid00000001", "vid00000003"]);
        let topic = &home.music_videos[1];
        assert_eq!(topic.artists[0].name, "Hoàng Dũng");
        assert_eq!(home.music_videos[0].duration_secs, Some(279.0));
        assert_eq!(home.music_videos[0].artists[0].id.as_deref(), Some("UCchan"));
        assert_eq!(home.continuation.as_deref(), Some("NEXT"));
    }

    #[test]
    fn parses_continuation_and_classic_renderers() {
        let json = json!({ "onResponseReceivedActions": [{ "appendContinuationItemsAction": { "continuationItems": [
            { "richItemRenderer": { "content": { "videoRenderer": {
                "videoId": "classic0001",
                "title": { "runs": [{ "text": "Classic" }] },
                "ownerText": { "runs": [{ "text": "Artist VEVO", "navigationEndpoint": { "browseEndpoint": { "browseId": "UCvevo" } } }] },
                "lengthText": { "simpleText": "3:05" },
                "thumbnail": { "thumbnails": [{ "url": "//i.ytimg.com/vi/classic0001/hq.jpg", "width": 480 }] }
            } } } },
            { "richItemRenderer": { "content": { "radioRenderer": {
                "playlistId": "RDEMabc",
                "title": { "simpleText": "Mix – Artist" },
                "navigationEndpoint": { "watchEndpoint": { "videoId": "seed0000001", "playlistId": "RDEMabc" } }
            } } } }
        ] } }] });
        let home = home_response(&json);
        assert_eq!(home.music_videos.len(), 1);
        assert_eq!(home.music_videos[0].duration_secs, Some(185.0));
        assert_eq!(home.music_videos[0].thumbnail_url.as_deref(), Some("https://i.ytimg.com/vi/classic0001/hq.jpg"));
        assert_eq!(home.mixes.len(), 1);
        assert_eq!(home.mixes[0].title, "Mix – Artist");
        assert_eq!(home.mixes[0].video_id.as_deref(), Some("seed0000001"));
        assert!(home.continuation.is_none());
    }

    #[test]
    fn mix_ids() {
        assert!(is_mix_id("RDMM"));
        assert!(is_mix_id("VLRDbTuqSWUYE6E"));
        assert!(is_mix_id("RDEMxyz"));
        assert!(!is_mix_id("RDCLAK5uy_x"));
        assert!(!is_mix_id("RDTMAK5uy_x"));
        assert!(!is_mix_id("PLabc"));
    }

    #[test]
    fn real_search_lockup_parses() {
        // Trimmed from a live WEB search response (2026-11).
        let l = json!({
            "contentId": "RDbTuqSWUYE6E",
            "contentType": "LOCKUP_CONTENT_TYPE_PLAYLIST",
            "contentImage": { "collectionThumbnailViewModel": { "primaryThumbnail": { "thumbnailViewModel": {
                "image": { "sources": [{ "url": "https://i.ytimg.com/vi/bTuqSWUYE6E/hqdefault.jpg?sqp=x", "width": 480, "height": 270 }] },
                "overlays": [{ "thumbnailOverlayBadgeViewModel": { "thumbnailBadges": [{ "thumbnailBadgeViewModel": {
                    "icon": { "sources": [{ "clientResource": { "imageName": "MIX" } }] }, "text": "Tuyển tập"
                } }] } }]
            } } } },
            "metadata": { "lockupMetadataViewModel": {
                "title": { "content": "Danh sách kết hợp – MASHUP 6IN1 M-TP" },
                "metadata": { "contentMetadataViewModel": { "metadataRows": [
                    { "metadataParts": [{ "text": { "content": "Sơn Tùng M-TP, VietZ, Bảo Anh  và nhiều nghệ sĩ khác" } }, { "text": { "content": "Danh sách phát" } }] },
                    { "isSpacerRow": true }
                ] } }
            } },
            "rendererContext": { "commandContext": { "onTap": { "innertubeCommand": {
                "commandMetadata": { "webCommandMetadata": { "url": "/watch?v=bTuqSWUYE6E&list=RDbTuqSWUYE6E&start_radio=1" } },
                "watchEndpoint": { "videoId": "bTuqSWUYE6E", "playlistId": "RDbTuqSWUYE6E", "params": "OALAAQE%3D", "continuePlayback": true }
            } } } }
        });
        let m = lockup_mix(&l).unwrap();
        assert_eq!(m.video_id.as_deref(), Some("bTuqSWUYE6E"));
        assert_eq!(m.title, "Danh sách kết hợp – MASHUP 6IN1 M-TP");
    }

    #[test]
    fn radio_continuation_token() {
        let j = json!({ "contents": { "x": { "playlistPanelRenderer": {
            "title": "Mix – A",
            "continuations": [{ "nextRadioContinuationData": { "continuation": "RC" } }]
        } } } });
        assert_eq!(radio_continuation(&j).as_deref(), Some("RC"));
        assert_eq!(panel_title(&j).as_deref(), Some("Mix – A"));
    }
}
