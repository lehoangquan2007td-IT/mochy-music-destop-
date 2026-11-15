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

use mochy_core::models::{Album, Artist, Playlist, QueueItem, SearchResults, Shelf, Song, TrackMeta};
use mochy_core::settings::AudioQuality;
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

impl YtmClient {
    /// Build a client, fetching/minting visitor data (spec §2.1).
    pub async fn new() -> YtmResult<Self> {
        todo!("YtmClient::new")
    }

    /// Search across all filters at once (songs/videos/albums/artists/playlists).
    pub async fn search_all(&self, query: &str) -> YtmResult<SearchResults> {
        let _ = query;
        todo!("YtmClient::search_all")
    }

    /// Search with a single filter; returns songs for SONG/VIDEO filters.
    pub async fn search_songs(&self, query: &str, filter_params: &str) -> YtmResult<Vec<Song>> {
        let _ = (query, filter_params);
        todo!("YtmClient::search_songs")
    }

    /// `music/get_search_suggestions`.
    pub async fn suggestions(&self, query: &str) -> YtmResult<Vec<String>> {
        let _ = query;
        todo!("YtmClient::suggestions")
    }

    /// Browse an album (`MPRE…` id).
    pub async fn album(&self, browse_id: &str) -> YtmResult<(Album, Vec<Song>)> {
        let _ = browse_id;
        todo!("YtmClient::album")
    }

    /// Browse an artist (`LA…` id): top songs + albums (name in `Shelf`s).
    pub async fn artist(&self, browse_id: &str) -> YtmResult<(Artist, Vec<Shelf>)> {
        let _ = browse_id;
        todo!("YtmClient::artist")
    }

    /// Browse a playlist (`VL…` community or `PL…` local).
    pub async fn playlist(&self, browse_id: &str) -> YtmResult<(Playlist, Vec<Song>)> {
        let _ = browse_id;
        todo!("YtmClient::playlist")
    }

    /// Home shelfs (spec §7.2 sources map to sections).
    pub async fn home(&self) -> YtmResult<Vec<Shelf>> {
        todo!("YtmClient::home")
    }

    /// Related songs for a video (from `next`).
    pub async fn related(&self, video_id: &str) -> YtmResult<Vec<Song>> {
        let _ = video_id;
        todo!("YtmClient::related")
    }

    /// Autoplay/radio queue for a seed video (capped at 1000).
    pub async fn get_queue(&self, video_id: &str) -> YtmResult<Vec<QueueItem>> {
        let _ = video_id;
        todo!("YtmClient::get_queue")
    }

    /// Resolve a validated, decodable stream for a video (client walk,
    /// validation probe, AAC-first ranking — spec §2.4–§2.6).
    pub async fn resolve_stream(
        &self,
        video_id: &str,
        quality: AudioQuality,
    ) -> YtmResult<StreamResult> {
        let _ = (video_id, quality);
        todo!("YtmClient::resolve_stream")
    }

    /// Track metadata for lyrics matching (title/artists/album/duration).
    pub async fn track_meta(&self, video_id: &str) -> YtmResult<TrackMeta> {
        let _ = video_id;
        todo!("YtmClient::track_meta")
    }
}

// ---------------------------------------------------------------------------
// Pure parsing helpers — public so tests can pin response shapes without network.
// ---------------------------------------------------------------------------

pub mod parse {
    use mochy_core::models::{Album, Artist, Playlist, SearchResults, Shelf, Song};
    use serde_json::Value;

    /// Extract tracks from a list of `musicResponsiveListItemRenderer`s.
    pub fn list_item_to_song(_r: &Value) -> Option<Song> {
        todo!("parse::list_item_to_song")
    }

    /// Parse a search response filtered by one filter params value.
    pub fn search_response(_json: &Value) -> SearchResults {
        todo!("parse::search_response")
    }

    /// Parse an album browse response.
    pub fn album_response(_json: &Value) -> (Album, Vec<Song>) {
        todo!("parse::album_response")
    }

    /// Parse an artist browse response.
    pub fn artist_response(_json: &Value) -> (Artist, Vec<Shelf>) {
        todo!("parse::artist_response")
    }

    /// Parse a playlist browse response.
    pub fn playlist_response(_json: &Value) -> (Playlist, Vec<Song>) {
        todo!("parse::playlist_response")
    }

    /// Parse `next` response into the radio/queue continuation list.
    pub fn queue_from_next(_json: &Value) -> Vec<mochy_core::models::QueueItem> {
        todo!("parse::queue_from_next")
    }

    /// Pick the best playable `adaptiveFormats` entry per MVP ranking:
    /// AAC-first (decodable), then bitrate/sample rate (spec §2.5 inverted).
    pub fn pick_audio_format(_player_json: &Value) -> Option<crate::StreamResult> {
        todo!("parse::pick_audio_format")
    }
}
