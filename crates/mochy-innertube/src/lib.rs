//! mochy-innertube — YouTube Music InnerTube client (spec §2).
//!
//! Contract for mochy-app: async methods on [`YtmClient`], returning
//! `mochy-core` model types. All network access is async (tokio).
//!
//! Stream acquisition order (spec §2.4): InnerTubeX backend (skip in MVP),
//! then the client walk **VISIONOS first** (no PoToken) → WEB_REMIX →
//! TVHTML5_SAMSUNG / WEB_EMBEDDED_PLAYER / TVHTML5_DOWNGRADED, then NewPipe
//! decipher fallback. Every candidate URL MUST be validated with a ranged
//! GET at the last byte (probe byte 1_048_576 for untokened URLs).
//!
//! MVP codec preference: **mp4a (AAC, e.g. itag 140) over opus** — the audio
//! engine cannot decode opus in MVP (symphonia lacks it). Rank AAC first and
//! mark others clearly in `StreamResult::format`.

use std::sync::RwLock;
use std::time::Duration;

use mochy_core::models::{Album, Artist, Playlist, QueueItem, SearchResults, Shelf, Song, TrackMeta};
use mochy_core::settings::AudioQuality;
use serde_json::{json, Value};
use thiserror::Error;

pub const BASE_URL: &str = "https://music.youtube.com/youtubei/v1";
pub const ORIGIN: &str = "https://music.youtube.com";

/// Client ids/versions (spec §2.2).
pub mod clients {
    pub const WEB_ID: &str = "1";
    pub const WEB_VERSION: &str = "2.20260708.00.00";
    pub const WEB_REMIX_ID: &str = "67";
    pub const WEB_REMIX_VERSION: &str = "1.20260707.12.00";
    pub const VISIONOS_ID: &str = "101";
    pub const VISIONOS_VERSION: &str = "1.04";
    pub const WEB_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:140.0) Gecko/20100101 Firefox/140.0";
    /// Untokened URLs serve only ~786 KB; probe here (spec §2.6).
    pub const UNTOKENED_WINDOW_PROBE_BYTE: u64 = 1_048_576;
    /// Give-up time for a backend attempt (spec §2.4).
    pub const BACKEND_TIMEOUT_MS: u64 = 12_000;
    /// Queue cap (spec §2.3).
    pub const MAX_GET_QUEUE_SIZE: usize = 1000;
}

/// Search filter params (exact, URL-encoded protobufs — spec §2.3).
pub mod search_filters {
    pub const SONG: &str = "EgWKAQIIAWoKEAkQBRAKEAMQBA%3D%3D";
    pub const VIDEO: &str = "EgWKAQIQAWoKEAkQChAFEAMQBA%3D%3D";
    pub const ALBUM: &str = "EgWKAQIYAWoKEAkQChAFEAMQBA%3D%3D";
    pub const ARTIST: &str = "EgWKAQIgAWoKEAkQChAFEAMQBA%3D%3D";
    pub const FEATURED_PLAYLIST: &str = "EgeKAQQoADgBagwQDhAKEAMQBRAJEAQ%3D";
    pub const COMMUNITY_PLAYLIST: &str = "EgeKAQQoAEABagoQAxAEEAoQCRAF";
}

/// Resolved audio format of a stream URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    AacM4a,
    OpusWebm,
    Mp3,
    Flac,
    Unknown,
}

impl AudioFormat {
    /// True when mochy-audio (symphonia/rodio) can decode it in MVP.
    pub fn decodable(self) -> bool {
        matches!(self, AudioFormat::AacM4a | AudioFormat::Mp3 | AudioFormat::Flac)
    }
}

/// A validated, playable stream (spec §2.5/§2.6).
#[derive(Debug, Clone)]
pub struct StreamResult {
    pub url: String,
    pub format: AudioFormat,
    pub itag: Option<u32>,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate_hz: Option<u32>,
    pub content_length: Option<u64>,
    /// Loudness from player response (`loudnessDb`), used for normalization.
    pub loudness_db: Option<f32>,
    /// Headers that must accompany the media request.
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Error)]
pub enum YtmError {
    #[error("network: {0}")]
    Network(String),
    #[error("parse: {0}")]
    Parse(String),
    #[error("no playable stream for {0}")]
    NoStream(String),
    #[error("premium-only track: {0}")]
    PremiumOnly(String),
    #[error("{0}")]
    Other(String),
}

pub type YtmResult<T> = Result<T, YtmError>;

/// InnerTube client. `Clone` is cheap and shares state.
#[derive(Debug)]
pub struct YtmClient {
    pub http: reqwest::Client,
    visitor_data: RwLock<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

impl YtmClient {
    /// Build the WEB_REMIX context JSON body with the current visitor data.
    fn web_remix_context(&self) -> Value {
        let vd = self.visitor_data.read().unwrap().clone();
        json!({
            "context": {
                "client": {
                    "clientName": "WEB_REMIX",
                    "clientVersion": clients::WEB_REMIX_VERSION,
                    "visitorData": vd,
                    "hl": "en",
                    "gl": "US"
                }
            }
        })
    }

    /// Build a VISIONOS context body (for player requests).
    fn visionos_context(&self, visionos_visitor: &str) -> Value {
        json!({
            "context": {
                "client": {
                    "clientName": "IOS",
                    "clientVersion": clients::VISIONOS_VERSION,
                    "visitorData": visionos_visitor,
                    "clientScreen": "WATCH"
                }
            }
        })
    }

    /// POST to an InnerTube endpoint, return the JSON body.
    async fn post_innertube(&self, endpoint: &str, body: Value) -> YtmResult<Value> {
        let url = format!("{}/{}?prettyPrint=false", BASE_URL, endpoint);
        let resp = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .header("X-YouTube-Client-Name", clients::WEB_REMIX_ID)
            .header("X-YouTube-Client-Version", clients::WEB_REMIX_VERSION)
            .header("X-Origin", ORIGIN)
            .json(&body)
            .timeout(Duration::from_millis(clients::BACKEND_TIMEOUT_MS))
            .send()
            .await
            .map_err(|e| YtmError::Network(e.to_string()))?;

        let json: Value = resp.json().await.map_err(|e| YtmError::Network(e.to_string()))?;
        Ok(json)
    }

    /// Mint visitor data from sw.js_data or visitor_id endpoint.
    async fn mint_visitor(http: &reqwest::Client) -> YtmResult<String> {
        // Try sw.js_data first
        if let Ok(resp) = http
            .get("https://music.youtube.com/sw.js_data")
            .timeout(Duration::from_secs(10))
            .send()
            .await
        {
            if let Ok(text) = resp.text().await {
                // Strip first 5 chars (")]}'\n")
                let trimmed = if text.len() > 5 { &text[5..] } else { &text };
                // Try to find visitorData matching ^Cg[ts]
                if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                    if let Some(vd) = Self::extract_visitor_data(&parsed) {
                        if vd.starts_with("Cgt") || vd.starts_with("Cgs") {
                            return Ok(vd);
                        }
                    }
                }
                // Also try raw regex-style matching on the text
                for part in trimmed.split('"') {
                    let p = part.trim();
                    if (p.starts_with("Cgt") || p.starts_with("Cgs")) && p.len() > 10 {
                        return Ok(p.to_string());
                    }
                }
            }
        }

        // Fallback: POST to visitor_id endpoint
        let body = json!({
            "context": {
                "client": {
                    "clientName": "WEB_REMIX",
                    "clientVersion": clients::WEB_REMIX_VERSION
                }
            }
        });
        let resp = http
            .post(format!("{}/visitor_id", BASE_URL))
            .header("Content-Type", "application/json")
            .json(&body)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| YtmError::Network(format!("visitor_id: {e}")))?;

        let json: Value = resp.json().await.map_err(|e| YtmError::Network(e.to_string()))?;
        json["visitorData"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| YtmError::Parse("no visitorData in visitor_id response".into()))
    }

    /// Recursively search a JSON value for a string field named "visitorData".
    fn extract_visitor_data(val: &Value) -> Option<String> {
        match val {
            Value::Object(map) => {
                if let Some(Value::String(s)) = map.get("visitorData") {
                    return Some(s.clone());
                }
                for v in map.values() {
                    if let Some(found) = Self::extract_visitor_data(v) {
                        return Some(found);
                    }
                }
                None
            }
            Value::Array(arr) => {
                for v in arr {
                    if let Some(found) = Self::extract_visitor_data(v) {
                        return Some(found);
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Validate a stream URL with a ranged GET.
    /// For tokened URLs: probe last byte. For untokened: probe at UNTOKENED_WINDOW_PROBE_BYTE.
    async fn validate_stream_url(&self, url: &str, content_length: Option<u64>) -> bool {
        let is_untokened = !url.contains("&pot=") && !url.contains("?pot=");
        let probe_byte = if is_untokened {
            clients::UNTOKENED_WINDOW_PROBE_BYTE
        } else {
            content_length.unwrap_or(1).saturating_sub(1)
        };

        let range_header = format!("bytes={}-{}", probe_byte, probe_byte);
        match self
            .http
            .get(url)
            .header("Range", range_header)
            .timeout(Duration::from_secs(8))
            .send()
            .await
        {
            Ok(resp) => resp.status().is_success() || resp.status().as_u16() == 206,
            Err(_) => false,
        }
    }
}

impl YtmClient {
    /// Build a client, fetching/minting visitor data (spec §2.1).
    pub async fn new() -> YtmResult<Self> {
        use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE, USER_AGENT};

        let mut default_headers = HeaderMap::new();
        default_headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        default_headers.insert(USER_AGENT, HeaderValue::from_static(clients::WEB_UA));

        let http = reqwest::Client::builder()
            .default_headers(default_headers)
            .gzip(true)
            .deflate(true)
            .build()
            .map_err(|e| YtmError::Network(format!("reqwest client build: {e}")))?;

        let visitor = Self::mint_visitor(&http).await?;

        Ok(Self {
            http,
            visitor_data: RwLock::new(visitor),
        })
    }

    /// Search across all filters at once (songs/videos/albums/artists/playlists).
    pub async fn search_all(&self, query: &str) -> YtmResult<SearchResults> {
        let mut body = self.web_remix_context();
        body["query"] = Value::String(query.to_string());
        let json = self.post_innertube("search", body).await?;
        Ok(parse::search_response(&json))
    }

    /// Search with a single filter; returns songs for SONG/VIDEO filters.
    pub async fn search_songs(&self, query: &str, filter_params: &str) -> YtmResult<Vec<Song>> {
        let mut body = self.web_remix_context();
        body["query"] = Value::String(query.to_string());
        body["params"] = Value::String(filter_params.to_string());
        let json = self.post_innertube("search", body).await?;
        let results = parse::search_response(&json);
        // For SONG/VIDEO filters the songs or videos list is what we want
        let mut songs = results.songs;
        songs.extend(results.videos);
        Ok(songs)
    }

    /// `music/get_search_suggestions`.
    pub async fn suggestions(&self, query: &str) -> YtmResult<Vec<String>> {
        let mut body = self.web_remix_context();
        body["input"] = Value::String(query.to_string());
        let json = self.post_innertube("music/get_search_suggestions", body).await?;

        let mut suggestions = Vec::new();
        // Walk contents[].searchSuggestionsSectionRenderer.contents[].searchSuggestionRenderer.suggestion.runs[].text
        if let Some(contents) = json["contents"].as_array() {
            for section in contents {
                if let Some(items) = section["searchSuggestionsSectionRenderer"]["contents"].as_array() {
                    for item in items {
                        let runs = &item["searchSuggestionRenderer"]["suggestion"]["runs"];
                        if let Some(runs) = runs.as_array() {
                            let text: String = runs.iter()
                                .filter_map(|r| r["text"].as_str())
                                .collect::<Vec<_>>()
                                .join("");
                            if !text.is_empty() {
                                suggestions.push(text);
                            }
                        }
                        // Also try navigationSuggestionRenderer
                        let runs2 = &item["navigationSuggestionRenderer"]["suggestion"]["runs"];
                        if let Some(runs) = runs2.as_array() {
                            let text: String = runs.iter()
                                .filter_map(|r| r["text"].as_str())
                                .collect::<Vec<_>>()
                                .join("");
                            if !text.is_empty() {
                                suggestions.push(text);
                            }
                        }
                    }
                }
            }
        }
        Ok(suggestions)
    }

    /// Browse an album (`MPRE…` id).
    pub async fn album(&self, browse_id: &str) -> YtmResult<(Album, Vec<Song>)> {
        let mut body = self.web_remix_context();
        body["browseId"] = Value::String(browse_id.to_string());
        let json = self.post_innertube("browse", body).await?;
        Ok(parse::album_response(&json))
    }

    /// Browse an artist (`LA…` id): top songs + albums (name in `Shelf`s).
    pub async fn artist(&self, browse_id: &str) -> YtmResult<(Artist, Vec<Shelf>)> {
        let mut body = self.web_remix_context();
        body["browseId"] = Value::String(browse_id.to_string());
        let json = self.post_innertube("browse", body).await?;
        Ok(parse::artist_response(&json))
    }

    /// Browse a playlist (`VL…` community or `PL…` local).
    pub async fn playlist(&self, browse_id: &str) -> YtmResult<(Playlist, Vec<Song>)> {
        let mut body = self.web_remix_context();
        // Playlists need VL prefix for browse if they start with PL
        let bid = if browse_id.starts_with("PL") {
            format!("VL{}", browse_id)
        } else {
            browse_id.to_string()
        };
        body["browseId"] = Value::String(bid);
        let json = self.post_innertube("browse", body).await?;
        Ok(parse::playlist_response(&json))
    }

    /// Home shelfs (spec §7.2 sources map to sections).
    pub async fn home(&self) -> YtmResult<Vec<Shelf>> {
        let mut body = self.web_remix_context();
        body["browseId"] = Value::String("FEmusic_home".to_string());
        let json = self.post_innertube("browse", body).await?;

        let mut shelves = Vec::new();
        // Walk tabs[0].tabRenderer.content.sectionListRenderer.contents[]
        let sections = &json["contents"]["singleColumnBrowseResultsRenderer"]["tabs"];
        if let Some(tabs) = sections.as_array() {
            if let Some(tab) = tabs.first() {
                let contents = &tab["tabRenderer"]["content"]["sectionListRenderer"]["contents"];
                if let Some(sections) = contents.as_array() {
                    for section in sections {
                        // musicCarouselShelfRenderer
                        if let Some(shelf) = section.get("musicCarouselShelfRenderer") {
                            let title = parse::nav_text(&shelf["header"]["musicCarouselShelfBasicHeaderRenderer"]["title"]);
                            let mut items = Vec::new();
                            if let Some(contents) = shelf["contents"].as_array() {
                                for item in contents {
                                    if let Some(two_row) = item.get("musicTwoRowItemRenderer") {
                                        if let Some(shelf_item) = parse::two_row_to_shelf_item(two_row) {
                                            items.push(shelf_item);
                                        }
                                    }
                                    if let Some(list_item) = item.get("musicResponsiveListItemRenderer") {
                                        if let Some(song) = parse::list_item_to_song(&Value::Object(
                                            std::iter::once(("musicResponsiveListItemRenderer".to_string(), list_item.clone())).collect(),
                                        )) {
                                            items.push(mochy_core::models::ShelfItem::Song(song));
                                        }
                                    }
                                }
                            }
                            if !title.is_empty() || !items.is_empty() {
                                shelves.push(Shelf { title, items });
                            }
                        }
                        // musicShelfRenderer (list sections)
                        if let Some(shelf) = section.get("musicShelfRenderer") {
                            let title = parse::nav_text(&shelf["title"]);
                            let mut items = Vec::new();
                            if let Some(contents) = shelf["contents"].as_array() {
                                for item in contents {
                                    if let Some(song) = parse::list_item_to_song(item) {
                                        items.push(mochy_core::models::ShelfItem::Song(song));
                                    }
                                }
                            }
                            if !title.is_empty() || !items.is_empty() {
                                shelves.push(Shelf { title, items });
                            }
                        }
                    }
                }
            }
        }
        Ok(shelves)
    }

    /// Related songs for a video (from `next`).
    pub async fn related(&self, video_id: &str) -> YtmResult<Vec<Song>> {
        let mut body = self.web_remix_context();
        body["videoId"] = Value::String(video_id.to_string());
        body["isAudioOnly"] = Value::Bool(true);
        let json = self.post_innertube("next", body).await?;

        let mut songs = Vec::new();
        // Walk contents.singleColumnMusicWatchNextResultsRenderer.tabbedRenderer.
        //   watchNextTabbedResultsRenderer.tabs[].tabRenderer.content.musicQueueRenderer.content.
        //   playlistPanelRenderer.contents[].playlistPanelVideoRenderer → related tracks
        // Also try the related tab
        let tabs = &json["contents"]["singleColumnMusicWatchNextResultsRenderer"]["tabbedRenderer"]
            ["watchNextTabbedResultsRenderer"]["tabs"];
        if let Some(tabs) = tabs.as_array() {
            // Related is usually tab[2] (Up next, Lyrics, Related)
            for tab in tabs {
                let section = &tab["tabRenderer"]["content"]["musicQueueRenderer"]["content"]
                    ["playlistPanelRenderer"]["contents"];
                if let Some(items) = section.as_array() {
                    for item in items {
                        if let Some(renderer) = item.get("playlistPanelVideoRenderer") {
                            if let Some(song) = parse::panel_video_to_song(renderer) {
                                songs.push(song);
                            }
                        }
                    }
                }
                // Also try sectionListRenderer for the related tab
                let sections = &tab["tabRenderer"]["content"]["sectionListRenderer"]["contents"];
                if let Some(secs) = sections.as_array() {
                    for sec in secs {
                        if let Some(shelf) = sec.get("musicShelfRenderer") {
                            if let Some(contents) = shelf["contents"].as_array() {
                                for item in contents {
                                    if let Some(song) = parse::list_item_to_song(item) {
                                        songs.push(song);
                                    }
                                }
                            }
                        }
                        if let Some(shelf) = sec.get("musicCarouselShelfRenderer") {
                            if let Some(contents) = shelf["contents"].as_array() {
                                for item in contents {
                                    if let Some(song) = parse::list_item_to_song(item) {
                                        songs.push(song);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(songs)
    }

    /// Autoplay/radio queue for a seed video (capped at 1000).
    pub async fn get_queue(&self, video_id: &str) -> YtmResult<Vec<QueueItem>> {
        let mut body = self.web_remix_context();
        body["videoId"] = Value::String(video_id.to_string());
        body["isAudioOnly"] = Value::Bool(true);
        body["enablePersistentPlaylistPanel"] = Value::Bool(true);
        let json = self.post_innertube("next", body).await?;

        let mut items = parse::queue_from_next(&json);
        items.truncate(clients::MAX_GET_QUEUE_SIZE);
        Ok(items)
    }

    /// Resolve a validated, decodable stream for a video (client walk,
    /// validation probe, AAC-first ranking — spec §2.4–§2.6).
    pub async fn resolve_stream(
        &self,
        video_id: &str,
        _quality: AudioQuality,
    ) -> YtmResult<StreamResult> {
        // --- 1. VISIONOS first (no PoToken, own visitor, apiOrigin www.youtube.com) ---
        let visionos_visitor = Self::mint_visitor(&self.http)
            .await
            .unwrap_or_else(|_| String::new());

        let mut visionos_body = self.visionos_context(&visionos_visitor);
        visionos_body["videoId"] = Value::String(video_id.to_string());
        visionos_body["contentCheckOk"] = Value::Bool(true);
        visionos_body["racyCheckOk"] = Value::Bool(true);

        let visionos_url = format!(
            "https://www.youtube.com/youtubei/v1/player?prettyPrint=false"
        );

        if let Ok(resp) = self
            .http
            .post(&visionos_url)
            .header("Content-Type", "application/json")
            .header("X-YouTube-Client-Name", clients::VISIONOS_ID)
            .header("X-YouTube-Client-Version", clients::VISIONOS_VERSION)
            .json(&visionos_body)
            .timeout(Duration::from_millis(clients::BACKEND_TIMEOUT_MS))
            .send()
            .await
        {
            if let Ok(player_json) = resp.json::<Value>().await {
                // Check for premium-only
                if let Some(status) = player_json["playabilityStatus"]["status"].as_str() {
                    if status == "UNPLAYABLE" {
                        if let Some(reason) = player_json["playabilityStatus"]["reason"].as_str() {
                            if reason.contains("Premium") {
                                return Err(YtmError::PremiumOnly(video_id.to_string()));
                            }
                        }
                    }
                }

                if let Some(mut stream) = parse::pick_audio_format(&player_json) {
                    // Validate the URL
                    if self.validate_stream_url(&stream.url, stream.content_length).await {
                        // Add required headers
                        stream.headers.push((
                            "User-Agent".to_string(),
                            clients::WEB_UA.to_string(),
                        ));
                        return Ok(stream);
                    }
                }
            }
        }

        // --- 2. WEB_REMIX fallback ---
        let mut remix_body = self.web_remix_context();
        remix_body["videoId"] = Value::String(video_id.to_string());
        remix_body["contentCheckOk"] = Value::Bool(true);
        remix_body["racyCheckOk"] = Value::Bool(true);

        let player_json = self.post_innertube("player", remix_body).await?;

        // Check for premium-only
        if let Some(status) = player_json["playabilityStatus"]["status"].as_str() {
            if status == "UNPLAYABLE" {
                if let Some(reason) = player_json["playabilityStatus"]["reason"].as_str() {
                    if reason.contains("Premium") {
                        return Err(YtmError::PremiumOnly(video_id.to_string()));
                    }
                }
            }
        }

        if let Some(mut stream) = parse::pick_audio_format(&player_json) {
            if self.validate_stream_url(&stream.url, stream.content_length).await {
                stream.headers.push((
                    "User-Agent".to_string(),
                    clients::WEB_UA.to_string(),
                ));
                return Ok(stream);
            }
        }

        Err(YtmError::NoStream(video_id.to_string()))
    }

    /// Track metadata for lyrics matching (title/artists/album/duration).
    pub async fn track_meta(&self, video_id: &str) -> YtmResult<TrackMeta> {
        let mut body = self.web_remix_context();
        body["videoId"] = Value::String(video_id.to_string());
        body["contentCheckOk"] = Value::Bool(true);
        body["racyCheckOk"] = Value::Bool(true);

        let json = self.post_innertube("player", body).await?;
        let details = &json["videoDetails"];

        let title = details["title"].as_str().unwrap_or("").to_string();
        let author = details["author"].as_str().unwrap_or("").to_string();

        let artists: Vec<String> = if author.is_empty() {
            Vec::new()
        } else {
            // The author field may contain multiple artists separated by " - Topic"
            // or comma-separated. Clean up " - Topic" suffix.
            let cleaned = author.trim_end_matches(" - Topic");
            cleaned
                .split(", ")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };

        let duration_secs = details["lengthSeconds"]
            .as_str()
            .and_then(|s| s.parse::<f64>().ok());

        let album = json["microformat"]["microformatDataRenderer"]["pageOwnerDetails"]["name"]
            .as_str()
            .map(|s| s.to_string());

        Ok(TrackMeta {
            title,
            artists,
            album,
            duration_secs,
            video_id: Some(video_id.to_string()),
        })
    }
}

// ---------------------------------------------------------------------------
// Pure parsing helpers — public so tests can pin response shapes without network.
// ---------------------------------------------------------------------------

pub mod parse {
    use mochy_core::models::{
        Album, Artist, ArtistRef, Playlist, QueueItem, SearchResults, Shelf, ShelfItem, Song,
    };
    use serde_json::Value;

    /// Extract text from a runs array or simpleText.
    pub fn nav_text(val: &Value) -> String {
        if let Some(s) = val["simpleText"].as_str() {
            return s.to_string();
        }
        if let Some(runs) = val["runs"].as_array() {
            return runs
                .iter()
                .filter_map(|r| r["text"].as_str())
                .collect::<Vec<_>>()
                .join("");
        }
        String::new()
    }

    /// Extract a thumbnail URL from a thumbnails array, preferring the largest.
    fn thumbnail_url(thumbnails: &Value) -> Option<String> {
        thumbnails["thumbnails"]
            .as_array()
            .and_then(|arr| arr.last())
            .and_then(|t| t["url"].as_str())
            .map(|s| {
                if s.starts_with("//") {
                    format!("https:{s}")
                } else {
                    s.to_string()
                }
            })
    }

    /// Parse a duration string like "3:45" or "1:02:30" into seconds.
    fn parse_duration(text: &str) -> Option<f64> {
        let parts: Vec<&str> = text.trim().split(':').collect();
        match parts.len() {
            2 => {
                let mins: f64 = parts[0].parse().ok()?;
                let secs: f64 = parts[1].parse().ok()?;
                Some(mins * 60.0 + secs)
            }
            3 => {
                let hours: f64 = parts[0].parse().ok()?;
                let mins: f64 = parts[1].parse().ok()?;
                let secs: f64 = parts[2].parse().ok()?;
                Some(hours * 3600.0 + mins * 60.0 + secs)
            }
            _ => None,
        }
    }

    /// Extract videoId from a navigationEndpoint.
    fn nav_video_id(nav: &Value) -> Option<String> {
        nav["watchEndpoint"]["videoId"]
            .as_str()
            .or_else(|| nav["navigationEndpoint"]["watchEndpoint"]["videoId"].as_str())
            .map(|s| s.to_string())
    }

    /// Extract browseId from a navigationEndpoint.
    fn nav_browse_id(nav: &Value) -> Option<String> {
        nav["browseEndpoint"]["browseId"]
            .as_str()
            .or_else(|| nav["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str())
            .map(|s| s.to_string())
    }

    /// Extract tracks from a list of `musicResponsiveListItemRenderer`s.
    pub fn list_item_to_song(r: &Value) -> Option<Song> {
        let renderer = r
            .get("musicResponsiveListItemRenderer")
            .unwrap_or(r);

        // Extract videoId from overlay or flexColumns endpoint
        let video_id = renderer["overlay"]["musicItemThumbnailOverlayRenderer"]["content"]
            ["musicPlayButtonRenderer"]["playNavigationEndpoint"]["watchEndpoint"]["videoId"]
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| {
                // Try from flexColumns
                let cols = renderer["flexColumns"].as_array()?;
                for col in cols {
                    let runs = col["musicResponsiveListItemFlexColumnRenderer"]["text"]["runs"]
                        .as_array()?;
                    for run in runs {
                        if let Some(vid) = run["navigationEndpoint"]["watchEndpoint"]["videoId"].as_str() {
                            return Some(vid.to_string());
                        }
                    }
                }
                None
            })
            .or_else(|| {
                renderer["navigationEndpoint"]["watchEndpoint"]["videoId"]
                    .as_str()
                    .map(|s| s.to_string())
            });

        let video_id = video_id?;

        let columns = renderer["flexColumns"].as_array();

        // Column 0 = title
        let title = columns
            .as_ref()
            .and_then(|cols| cols.first())
            .map(|col| {
                nav_text(
                    &col["musicResponsiveListItemFlexColumnRenderer"]["text"],
                )
            })
            .unwrap_or_default();

        if title.is_empty() {
            return None;
        }

        // Column 1 = artist(s) · album · duration (or artist · duration)
        let mut artists = Vec::new();
        let mut album_name = None;
        let mut album_id = None;
        let mut duration = None;

        if let Some(cols) = columns.as_ref() {
            if let Some(col1) = cols.get(1) {
                let runs = &col1["musicResponsiveListItemFlexColumnRenderer"]["text"]["runs"];
                if let Some(runs) = runs.as_array() {
                    for run in runs {
                        let text = run["text"].as_str().unwrap_or("");
                        // Skip separators
                        if text == " • " || text == " & " || text == ", " || text == " · " {
                            continue;
                        }
                        // Check if this run has a browse endpoint
                        if let Some(browse_id) = run["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str() {
                            if browse_id.starts_with("UC") || browse_id.starts_with("LA") {
                                // Artist
                                artists.push(ArtistRef::with_id(browse_id, text));
                            } else if browse_id.starts_with("MPRE") {
                                // Album
                                album_name = Some(text.to_string());
                                album_id = Some(browse_id.to_string());
                            } else {
                                artists.push(ArtistRef::with_id(browse_id, text));
                            }
                        } else if parse_duration(text).is_some() {
                            duration = parse_duration(text);
                        } else if !text.is_empty() && text != "Song" && text != "Video" {
                            // Likely an artist without a browse id
                            artists.push(ArtistRef::new(text));
                        }
                    }
                }
            }
        }

        let thumbnail = thumbnail_url(&renderer["thumbnail"]["musicThumbnailRenderer"]["thumbnail"]);
        let explicit = renderer["badges"].as_array().map_or(false, |badges| {
            badges.iter().any(|b| {
                b["musicInlineBadgeRenderer"]["icon"]["iconType"].as_str() == Some("MUSIC_EXPLICIT_BADGE")
            })
        });

        Some(Song {
            id: video_id.clone(),
            video_id: Some(video_id),
            title,
            artists,
            album_id,
            album_name,
            duration_secs: duration,
            thumbnail_url: thumbnail,
            explicit,
            ..Default::default()
        })
    }

    /// Parse a `musicTwoRowItemRenderer` into a ShelfItem.
    pub fn two_row_to_shelf_item(r: &Value) -> Option<ShelfItem> {
        let title = nav_text(&r["title"]);
        let subtitle = nav_text(&r["subtitle"]);
        let thumb = thumbnail_url(&r["thumbnailRenderer"]["musicThumbnailRenderer"]["thumbnail"]);

        // Determine type from the browse endpoint
        let nav = &r["navigationEndpoint"];

        if let Some(vid) = nav["watchEndpoint"]["videoId"].as_str() {
            // Song/video
            return Some(ShelfItem::Song(Song {
                id: vid.to_string(),
                video_id: Some(vid.to_string()),
                title,
                artists: vec![ArtistRef::new(subtitle)],
                thumbnail_url: thumb,
                ..Default::default()
            }));
        }

        if let Some(browse_id) = nav["browseEndpoint"]["browseId"].as_str() {
            if browse_id.starts_with("MPRE") {
                return Some(ShelfItem::Album(Album {
                    id: browse_id.to_string(),
                    title,
                    thumbnail_url: thumb,
                    ..Default::default()
                }));
            }
            if browse_id.starts_with("UC") || browse_id.starts_with("LA") {
                return Some(ShelfItem::Artist(Artist {
                    id: browse_id.to_string(),
                    name: title,
                    thumbnail_url: thumb,
                    channel_id: Some(browse_id.to_string()),
                }));
            }
            if browse_id.starts_with("VL") || browse_id.starts_with("PL") || browse_id.starts_with("RDCLAK") {
                return Some(ShelfItem::Playlist(Playlist {
                    id: browse_id.to_string(),
                    name: title,
                    thumbnail_url: thumb,
                    ..Default::default()
                }));
            }
            // Fallback: treat as playlist
            return Some(ShelfItem::Playlist(Playlist {
                id: browse_id.to_string(),
                name: title,
                thumbnail_url: thumb,
                ..Default::default()
            }));
        }

        None
    }

    /// Convert a playlistPanelVideoRenderer to a Song.
    pub fn panel_video_to_song(r: &Value) -> Option<Song> {
        let video_id = r["navigationEndpoint"]["watchEndpoint"]["videoId"]
            .as_str()
            .or_else(|| r["videoId"].as_str())?
            .to_string();

        let title = nav_text(&r["title"]);
        if title.is_empty() {
            return None;
        }

        let mut artists = Vec::new();
        if let Some(runs) = r["longBylineText"]["runs"].as_array()
            .or_else(|| r["shortBylineText"]["runs"].as_array())
        {
            for run in runs {
                let text = run["text"].as_str().unwrap_or("");
                if text == " • " || text == " & " || text == ", " || text == " · " {
                    continue;
                }
                if let Some(browse_id) = run["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str() {
                    artists.push(ArtistRef::with_id(browse_id, text));
                } else if !text.is_empty() {
                    artists.push(ArtistRef::new(text));
                }
            }
        }

        let duration = r["lengthText"]["runs"]
            .as_array()
            .and_then(|runs| runs.first())
            .and_then(|run| run["text"].as_str())
            .and_then(parse_duration)
            .or_else(|| {
                r["lengthText"]["simpleText"]
                    .as_str()
                    .and_then(parse_duration)
            });

        let thumbnail = thumbnail_url(&r["thumbnail"]);

        Some(Song {
            id: video_id.clone(),
            video_id: Some(video_id),
            title,
            artists,
            duration_secs: duration,
            thumbnail_url: thumbnail,
            ..Default::default()
        })
    }

    /// Parse a search response filtered by one filter params value.
    pub fn search_response(json: &Value) -> SearchResults {
        let mut results = SearchResults::default();

        // Try tabbedSearchResultsRenderer first
        let tab_contents = &json["contents"]["tabbedSearchResultsRenderer"]["tabs"];
        let section_list = if let Some(tabs) = tab_contents.as_array() {
            tabs.first()
                .and_then(|t| t["tabRenderer"]["content"]["sectionListRenderer"]["contents"].as_array())
        } else {
            // Try direct sectionListRenderer
            json["contents"]["sectionListRenderer"]["contents"].as_array()
        };

        if let Some(sections) = section_list {
            for section in sections {
                // musicShelfRenderer
                if let Some(shelf) = section.get("musicShelfRenderer") {
                    let shelf_title = nav_text(&shelf["title"]).to_lowercase();
                    if let Some(contents) = shelf["contents"].as_array() {
                        for item in contents {
                            if shelf_title.contains("artist") {
                                // Parse as artist
                                if let Some(artist) = list_item_to_artist(item) {
                                    results.artists.push(artist);
                                }
                            } else if shelf_title.contains("album") {
                                if let Some(album) = list_item_to_album(item) {
                                    results.albums.push(album);
                                }
                            } else if shelf_title.contains("playlist") || shelf_title.contains("community") || shelf_title.contains("featured") {
                                if let Some(playlist) = list_item_to_playlist(item) {
                                    results.playlists.push(playlist);
                                }
                            } else if shelf_title.contains("video") {
                                if let Some(song) = list_item_to_song(item) {
                                    results.videos.push(song);
                                }
                            } else {
                                // Default: songs (or top result)
                                if let Some(song) = list_item_to_song(item) {
                                    results.songs.push(song);
                                }
                            }
                        }
                    }
                }
                // musicCardShelfRenderer (top result)
                if let Some(card) = section.get("musicCardShelfRenderer") {
                    let video_id = card["onTap"]["watchEndpoint"]["videoId"]
                        .as_str()
                        .map(|s| s.to_string());
                    if let Some(vid) = video_id {
                        let title = nav_text(&card["title"]);
                        let subtitle = nav_text(&card["subtitle"]);
                        results.songs.push(Song {
                            id: vid.clone(),
                            video_id: Some(vid),
                            title,
                            artists: vec![ArtistRef::new(subtitle)],
                            ..Default::default()
                        });
                    }
                }
            }
        }

        results
    }

    /// Parse a list item as an Artist.
    fn list_item_to_artist(r: &Value) -> Option<Artist> {
        let renderer = r
            .get("musicResponsiveListItemRenderer")
            .unwrap_or(r);

        let browse_id = nav_browse_id(&renderer["navigationEndpoint"])?;
        let columns = renderer["flexColumns"].as_array()?;
        let name = columns.first().map(|col| {
            nav_text(&col["musicResponsiveListItemFlexColumnRenderer"]["text"])
        }).unwrap_or_default();

        if name.is_empty() {
            return None;
        }

        let thumbnail = thumbnail_url(&renderer["thumbnail"]["musicThumbnailRenderer"]["thumbnail"]);

        Some(Artist {
            id: browse_id.clone(),
            name,
            thumbnail_url: thumbnail,
            channel_id: Some(browse_id),
        })
    }

    /// Parse a list item as an Album.
    fn list_item_to_album(r: &Value) -> Option<Album> {
        let renderer = r
            .get("musicResponsiveListItemRenderer")
            .unwrap_or(r);

        let browse_id = nav_browse_id(&renderer["navigationEndpoint"])?;
        let columns = renderer["flexColumns"].as_array()?;
        let title = columns.first().map(|col| {
            nav_text(&col["musicResponsiveListItemFlexColumnRenderer"]["text"])
        }).unwrap_or_default();

        if title.is_empty() {
            return None;
        }

        let thumbnail = thumbnail_url(&renderer["thumbnail"]["musicThumbnailRenderer"]["thumbnail"]);

        // Try to extract year from second column
        let year = columns.get(1).and_then(|col| {
            let text = nav_text(&col["musicResponsiveListItemFlexColumnRenderer"]["text"]);
            // Look for a 4-digit year
            text.split(|c: char| !c.is_ascii_digit())
                .find(|s| s.len() == 4)
                .and_then(|s| s.parse::<i32>().ok())
        });

        Some(Album {
            id: browse_id,
            title,
            thumbnail_url: thumbnail,
            year,
            ..Default::default()
        })
    }

    /// Parse a list item as a Playlist.
    fn list_item_to_playlist(r: &Value) -> Option<Playlist> {
        let renderer = r
            .get("musicResponsiveListItemRenderer")
            .unwrap_or(r);

        let browse_id = nav_browse_id(&renderer["navigationEndpoint"])?;
        let columns = renderer["flexColumns"].as_array()?;
        let name = columns.first().map(|col| {
            nav_text(&col["musicResponsiveListItemFlexColumnRenderer"]["text"])
        }).unwrap_or_default();

        if name.is_empty() {
            return None;
        }

        let thumbnail = thumbnail_url(&renderer["thumbnail"]["musicThumbnailRenderer"]["thumbnail"]);

        Some(Playlist {
            id: browse_id,
            name,
            thumbnail_url: thumbnail,
            ..Default::default()
        })
    }

    /// Parse an album browse response.
    pub fn album_response(json: &Value) -> (Album, Vec<Song>) {
        let header = &json["header"]["musicImmersiveHeaderRenderer"];
        let header_alt = &json["header"]["musicDetailHeaderRenderer"];
        let header_v2 = if header["title"].is_null() { header_alt } else { header };

        let title = nav_text(&header_v2["title"]);
        let thumbnail = thumbnail_url(
            &header_v2["thumbnail"]["musicThumbnailRenderer"]["thumbnail"],
        )
        .or_else(|| {
            thumbnail_url(&json["background"]["musicThumbnailRenderer"]["thumbnail"])
        });

        // Extract artists from straplineTextOne
        let mut year = None;
        let description = header_v2["description"]["musicDescriptionShelfRenderer"]["description"]
            .as_str()
            .or_else(|| json["header"]["musicDetailHeaderRenderer"]["description"]["runs"].as_array()
                .and_then(|runs| runs.first())
                .and_then(|r| r["text"].as_str()))
            .map(|s| s.to_string());

        // Try to extract year from subtitle
        if let Some(runs) = header_v2["subtitle"]["runs"].as_array() {
            for run in runs {
                if let Some(text) = run["text"].as_str() {
                    if text.len() == 4 {
                        if let Ok(y) = text.parse::<i32>() {
                            year = Some(y);
                        }
                    }
                }
            }
        }

        // Browse id from the response
        let browse_id = json["responseContext"]["serviceTrackingParams"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|p| p["params"].as_array())
            .and_then(|params| {
                params.iter().find_map(|p| {
                    if p["key"].as_str() == Some("browse_id") {
                        p["value"].as_str().map(|s| s.to_string())
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_default();

        let album = Album {
            id: browse_id,
            title,
            thumbnail_url: thumbnail,
            year,
            description,
            ..Default::default()
        };

        // Songs from musicShelfRenderer contents
        let mut songs = Vec::new();
        let tab_contents = &json["contents"]["singleColumnBrowseResultsRenderer"]["tabs"];
        if let Some(tabs) = tab_contents.as_array() {
            if let Some(tab) = tabs.first() {
                let sections = &tab["tabRenderer"]["content"]["sectionListRenderer"]["contents"];
                if let Some(sections) = sections.as_array() {
                    for section in sections {
                        if let Some(shelf) = section.get("musicShelfRenderer") {
                            if let Some(contents) = shelf["contents"].as_array() {
                                for (idx, item) in contents.iter().enumerate() {
                                    if let Some(mut song) = list_item_to_song(item) {
                                        song.album_id = Some(album.id.clone());
                                        song.album_name = Some(album.title.clone());
                                        // Set track index if not present
                                        let _ = idx;
                                        songs.push(song);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        (album, songs)
    }

    /// Parse an artist browse response.
    pub fn artist_response(json: &Value) -> (Artist, Vec<Shelf>) {
        let header = &json["header"]["musicImmersiveHeaderRenderer"];
        let header_alt = &json["header"]["musicVisualHeaderRenderer"];
        let header_v2 = if header["title"].is_null() { header_alt } else { header };

        let name = nav_text(&header_v2["title"]);
        let thumbnail = thumbnail_url(
            &header_v2["thumbnail"]["musicThumbnailRenderer"]["thumbnail"],
        );
        let channel_id = header_v2["subscriptionButton"]["subscribeButtonRenderer"]["channelId"]
            .as_str()
            .map(|s| s.to_string());

        let browse_id = json["responseContext"]["serviceTrackingParams"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|p| p["params"].as_array())
            .and_then(|params| {
                params.iter().find_map(|p| {
                    if p["key"].as_str() == Some("browse_id") {
                        p["value"].as_str().map(|s| s.to_string())
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_default();

        let artist = Artist {
            id: browse_id,
            name,
            thumbnail_url: thumbnail,
            channel_id,
        };

        // Shelves from sectionListRenderer
        let mut shelves = Vec::new();
        let tab_contents = &json["contents"]["singleColumnBrowseResultsRenderer"]["tabs"];
        if let Some(tabs) = tab_contents.as_array() {
            if let Some(tab) = tabs.first() {
                let sections = &tab["tabRenderer"]["content"]["sectionListRenderer"]["contents"];
                if let Some(sections) = sections.as_array() {
                    for section in sections {
                        // musicShelfRenderer (e.g. Songs, Albums)
                        if let Some(shelf) = section.get("musicShelfRenderer") {
                            let shelf_title = nav_text(&shelf["title"]);
                            let mut items = Vec::new();
                            if let Some(contents) = shelf["contents"].as_array() {
                                for item in contents {
                                    if let Some(song) = list_item_to_song(item) {
                                        items.push(ShelfItem::Song(song));
                                    }
                                }
                            }
                            shelves.push(Shelf {
                                title: shelf_title,
                                items,
                            });
                        }
                        // musicCarouselShelfRenderer
                        if let Some(shelf) = section.get("musicCarouselShelfRenderer") {
                            let shelf_title = nav_text(
                                &shelf["header"]["musicCarouselShelfBasicHeaderRenderer"]["title"],
                            );
                            let mut items = Vec::new();
                            if let Some(contents) = shelf["contents"].as_array() {
                                for item in contents {
                                    if let Some(two_row) = item.get("musicTwoRowItemRenderer") {
                                        if let Some(shelf_item) = two_row_to_shelf_item(two_row) {
                                            items.push(shelf_item);
                                        }
                                    }
                                    if let Some(_list_item) =
                                        item.get("musicResponsiveListItemRenderer")
                                    {
                                        if let Some(song) = list_item_to_song(item) {
                                            items.push(ShelfItem::Song(song));
                                        }
                                    }
                                }
                            }
                            shelves.push(Shelf {
                                title: shelf_title,
                                items,
                            });
                        }
                    }
                }
            }
        }

        (artist, shelves)
    }

    /// Parse a playlist browse response.
    pub fn playlist_response(json: &Value) -> (Playlist, Vec<Song>) {
        let header = &json["header"]["musicDetailHeaderRenderer"];
        let header_alt = &json["header"]["musicEditablePlaylistDetailHeaderRenderer"];
        let header_v2 = if header["title"].is_null() {
            &header_alt["header"]["musicDetailHeaderRenderer"]
        } else {
            header
        };
        // Also try immersive header
        let header_v3 = &json["header"]["musicImmersiveHeaderRenderer"];
        let use_header = if header_v2["title"].is_null() { header_v3 } else { header_v2 };

        let name = nav_text(&use_header["title"]);
        let thumbnail = thumbnail_url(
            &use_header["thumbnail"]["musicThumbnailRenderer"]["thumbnail"],
        )
        .or_else(|| {
            thumbnail_url(
                &use_header["thumbnail"]["croppedSquareThumbnailRenderer"]["thumbnail"],
            )
        });

        let description = use_header["description"]["runs"]
            .as_array()
            .and_then(|runs| runs.first())
            .and_then(|r| r["text"].as_str())
            .or_else(|| use_header["description"].as_str())
            .map(|s| s.to_string());

        // Song count from subtitle
        let mut song_count = 0u32;
        if let Some(runs) = use_header["subtitle"]["runs"].as_array() {
            for run in runs {
                if let Some(text) = run["text"].as_str() {
                    // Try to find a number followed by "songs" or "tracks"
                    let trimmed = text.replace(',', "");
                    if let Ok(n) = trimmed.trim().parse::<u32>() {
                        song_count = n;
                    }
                }
            }
        }

        let browse_id = json["responseContext"]["serviceTrackingParams"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|p| p["params"].as_array())
            .and_then(|params| {
                params.iter().find_map(|p| {
                    if p["key"].as_str() == Some("browse_id") {
                        p["value"].as_str().map(|s| s.to_string())
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_default();

        let playlist = Playlist {
            id: browse_id,
            name,
            thumbnail_url: thumbnail,
            song_count,
            description,
            ..Default::default()
        };

        // Songs
        let mut songs = Vec::new();
        let tab_contents = &json["contents"]["singleColumnBrowseResultsRenderer"]["tabs"];
        if let Some(tabs) = tab_contents.as_array() {
            if let Some(tab) = tabs.first() {
                let sections = &tab["tabRenderer"]["content"]["sectionListRenderer"]["contents"];
                if let Some(sections) = sections.as_array() {
                    for section in sections {
                        if let Some(shelf) = section.get("musicShelfRenderer") {
                            if let Some(contents) = shelf["contents"].as_array() {
                                for item in contents {
                                    if let Some(song) = list_item_to_song(item) {
                                        songs.push(song);
                                    }
                                }
                            }
                        }
                        // Also try musicPlaylistShelfRenderer
                        if let Some(shelf) = section.get("musicPlaylistShelfRenderer") {
                            if let Some(contents) = shelf["contents"].as_array() {
                                for item in contents {
                                    if let Some(song) = list_item_to_song(item) {
                                        songs.push(song);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        (playlist, songs)
    }

    /// Parse `next` response into the radio/queue continuation list.
    pub fn queue_from_next(json: &Value) -> Vec<QueueItem> {
        let mut items = Vec::new();

        // Walk contents.singleColumnMusicWatchNextResultsRenderer.tabbedRenderer.
        //   watchNextTabbedResultsRenderer.tabs[0].tabRenderer.content.musicQueueRenderer.content.
        //   playlistPanelRenderer.contents[]
        let tabs = &json["contents"]["singleColumnMusicWatchNextResultsRenderer"]["tabbedRenderer"]
            ["watchNextTabbedResultsRenderer"]["tabs"];

        if let Some(tabs) = tabs.as_array() {
            if let Some(tab) = tabs.first() {
                let panel = &tab["tabRenderer"]["content"]["musicQueueRenderer"]["content"]
                    ["playlistPanelRenderer"]["contents"];
                if let Some(contents) = panel.as_array() {
                    for item in contents {
                        if let Some(renderer) = item.get("playlistPanelVideoRenderer") {
                            if let Some(qi) = panel_video_to_queue_item(renderer) {
                                items.push(qi);
                            }
                        }
                    }
                }
                // Also try continuations in the autoplay panel
                let autoplay = &tab["tabRenderer"]["content"]["musicQueueRenderer"]["content"]
                    ["playlistPanelRenderer"]["continuations"];
                if autoplay.is_array() {
                    // Initial contents are already above; continuations would need
                    // additional network calls which we don't do here.
                }
            }
        }

        items
    }

    /// Convert a playlistPanelVideoRenderer to a QueueItem.
    fn panel_video_to_queue_item(r: &Value) -> Option<QueueItem> {
        let video_id = r["navigationEndpoint"]["watchEndpoint"]["videoId"]
            .as_str()
            .or_else(|| r["videoId"].as_str())?
            .to_string();

        let title = nav_text(&r["title"]);

        let playlist_id = r["navigationEndpoint"]["watchEndpoint"]["playlistId"]
            .as_str()
            .map(|s| s.to_string());

        let set_video_id = r["playlistSetVideoId"]
            .as_str()
            .map(|s| s.to_string());

        let mut artists = Vec::new();
        if let Some(runs) = r["longBylineText"]["runs"]
            .as_array()
            .or_else(|| r["shortBylineText"]["runs"].as_array())
        {
            for run in runs {
                let text = run["text"].as_str().unwrap_or("");
                if text == " • " || text == " & " || text == ", " || text == " · " {
                    continue;
                }
                if let Some(browse_id) =
                    run["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str()
                {
                    artists.push(ArtistRef::with_id(browse_id, text));
                } else if !text.is_empty() {
                    artists.push(ArtistRef::new(text));
                }
            }
        }

        let duration = r["lengthText"]["runs"]
            .as_array()
            .and_then(|runs| runs.first())
            .and_then(|run| run["text"].as_str())
            .and_then(parse_duration)
            .or_else(|| {
                r["lengthText"]["simpleText"]
                    .as_str()
                    .and_then(parse_duration)
            });

        let thumbnail = thumbnail_url(&r["thumbnail"]);

        Some(QueueItem {
            video_id,
            playlist_id,
            set_video_id,
            title,
            artists,
            duration_secs: duration,
            thumbnail_url: thumbnail,
        })
    }

    /// Pick the best playable `adaptiveFormats` entry per MVP ranking:
    /// AAC-first (decodable), then bitrate/sample rate (spec §2.5 inverted).
    pub fn pick_audio_format(player_json: &Value) -> Option<crate::StreamResult> {
        let formats = player_json["streamingData"]["adaptiveFormats"].as_array()?;

        let loudness_db = player_json["playerConfig"]["audioConfig"]["loudnessDb"]
            .as_f64()
            .or_else(|| {
                player_json["playerConfig"]["audioConfig"]["perceptualLoudnessDb"].as_f64()
            })
            .map(|v| v as f32);

        // Collect audio-only formats
        let mut candidates: Vec<(crate::AudioFormat, u32, u32, u32, String, Option<u64>)> = Vec::new();

        for fmt in formats {
            let mime = fmt["mimeType"].as_str().unwrap_or("");
            // Filter audio-only
            if !mime.starts_with("audio/") {
                continue;
            }

            let url = fmt["url"]
                .as_str()
                .or_else(|| fmt["signatureCipher"].as_str())
                .unwrap_or("");
            if url.is_empty() {
                // signatureCipher URLs need deobfuscation — skip in MVP
                if fmt["signatureCipher"].is_string() {
                    continue;
                }
                continue;
            }

            let itag = fmt["itag"].as_u64().unwrap_or(0) as u32;
            let bitrate = fmt["averageBitrate"]
                .as_u64()
                .or_else(|| fmt["bitrate"].as_u64())
                .unwrap_or(0) as u32;
            let sample_rate = fmt["audioSampleRate"]
                .as_str()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            let content_length = fmt["contentLength"]
                .as_str()
                .and_then(|s| s.parse::<u64>().ok());

            let audio_format = if mime.contains("mp4a") || mime.contains("mp4") {
                crate::AudioFormat::AacM4a
            } else if mime.contains("opus") {
                crate::AudioFormat::OpusWebm
            } else if mime.contains("mp3") || mime.contains("mpeg") {
                crate::AudioFormat::Mp3
            } else if mime.contains("flac") {
                crate::AudioFormat::Flac
            } else {
                crate::AudioFormat::Unknown
            };

            candidates.push((
                audio_format,
                itag,
                bitrate,
                sample_rate,
                url.to_string(),
                content_length,
            ));
        }

        if candidates.is_empty() {
            return None;
        }

        // Sort: decodable first (AAC > Mp3 > Flac > Opus > Unknown),
        // then bitrate desc, then sample rate desc
        candidates.sort_by(|a, b| {
            let codec_rank = |f: crate::AudioFormat| -> u32 {
                match f {
                    crate::AudioFormat::AacM4a => 100,
                    crate::AudioFormat::Mp3 => 90,
                    crate::AudioFormat::Flac => 80,
                    crate::AudioFormat::OpusWebm => 10,
                    crate::AudioFormat::Unknown => 0,
                }
            };
            codec_rank(b.0)
                .cmp(&codec_rank(a.0))
                .then(b.2.cmp(&a.2))
                .then(b.3.cmp(&a.3))
        });

        let best = &candidates[0];
        Some(crate::StreamResult {
            url: best.4.clone(),
            format: best.0,
            itag: if best.1 > 0 { Some(best.1) } else { None },
            bitrate_kbps: if best.2 > 0 {
                Some(best.2 / 1000)
            } else {
                None
            },
            sample_rate_hz: if best.3 > 0 { Some(best.3) } else { None },
            content_length: best.5,
            loudness_db,
            headers: Vec::new(),
        })
    }
}
