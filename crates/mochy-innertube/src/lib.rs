//! mochy-innertube — YouTube Music InnerTube client (spec §2).
//!
//! Contract for mochy-app: async methods on [`YtmClient`], returning
//! `mochy-core` model types. All network access is async (tokio). The client
//! is cheap to `Clone` (shared `Arc` state) and `Send + Sync`.
//!
//! Browse/search/next use the `WEB_REMIX` client against
//! `music.youtube.com`. Streams follow the Kotlin client walk
//! (`YTPlayerUtils.kt`): **VISIONOS first** (direct URLs that serve the whole
//! file, no PoToken, its own visitor id), then the untokened native clients.
//! Clients that need a BotGuard PoToken or JS signature/`n` deciphering
//! (WEB_REMIX, WEB, TV/embedded) are skipped — that needs a JS engine.
//! Every candidate URL is validated with a ranged GET at its last byte
//! (`contentLength - 1`, or byte 1 MiB when unknown) because untokened URLs
//! are cut off after ~786 KB.
//!
//! Codec preference: **mp4a (AAC, itag 140/139) over opus** by default — the
//! audio engine may not decode opus. Opus candidates stay available in
//! [`StreamResult::alternatives`] and via [`CodecPreference::PreferOpus`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use mochy_core::models::{Album, Artist, Playlist, QueueItem, SearchResults, Shelf, ShelfItem, Song, TrackMeta};
use mochy_core::settings::AudioQuality;
use serde_json::{Value, json};
use thiserror::Error;

pub mod parse;
mod stream;

pub use clients::ClientSpec;

pub const BASE_URL: &str = "https://music.youtube.com/youtubei/v1";
pub const ORIGIN: &str = "https://music.youtube.com";

/// Client ids/versions (spec §2.2, `YouTubeClient.kt`).
pub mod clients {
    pub const WEB_ID: &str = "1";
    pub const WEB_VERSION: &str = "2.20260708.00.00";
    pub const WEB_REMIX_ID: &str = "67";
    pub const WEB_REMIX_VERSION: &str = "1.20260707.12.00";
    pub const VISIONOS_ID: &str = "101";
    pub const VISIONOS_VERSION: &str = "1.04";
    pub const WEB_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:140.0) Gecko/20100101 Firefox/140.0";
    /// Untokened URLs serve only ~786 KB; probe here when the length is unknown (spec §2.6).
    pub const UNTOKENED_WINDOW_PROBE_BYTE: u64 = 1_048_576;
    /// Give-up time for one backend/player attempt (spec §2.4).
    pub const BACKEND_TIMEOUT_MS: u64 = 12_000;
    /// Queue cap (spec §2.3).
    pub const MAX_GET_QUEUE_SIZE: usize = 1000;

    pub const ORIGIN_YOUTUBE: &str = "https://www.youtube.com";
    pub const ORIGIN_YOUTUBE_MUSIC: &str = "https://music.youtube.com";

    /// One InnerTube client identity (port of Kotlin `YouTubeClient`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ClientSpec {
        pub name: &'static str,
        pub version: &'static str,
        /// Numeric id sent as `X-YouTube-Client-Name`.
        pub id: &'static str,
        pub user_agent: &'static str,
        pub os_name: Option<&'static str>,
        pub os_version: Option<&'static str>,
        pub device_make: Option<&'static str>,
        pub device_model: Option<&'static str>,
        pub android_sdk_version: Option<&'static str>,
        /// Where this client's API lives (`www.youtube.com` for app/TV clients).
        pub api_origin: &'static str,
        pub api_format_version: &'static str,
        pub client_screen: Option<&'static str>,
        /// `Some` for native apps: they send no Origin/Referer.
        pub platform: Option<&'static str>,
        /// Mints its own visitor id via `visitor_id` instead of the web one.
        pub own_visitor_id: bool,
        pub login_required: bool,
        pub use_signature_timestamp: bool,
        pub use_web_po_tokens: bool,
        pub is_embedded: bool,
    }

    impl ClientSpec {
        const fn base(name: &'static str, version: &'static str, id: &'static str, ua: &'static str) -> Self {
            Self {
                name,
                version,
                id,
                user_agent: ua,
                os_name: None,
                os_version: None,
                device_make: None,
                device_model: None,
                android_sdk_version: None,
                api_origin: ORIGIN_YOUTUBE_MUSIC,
                api_format_version: "1",
                client_screen: None,
                platform: None,
                own_visitor_id: false,
                login_required: false,
                use_signature_timestamp: false,
                use_web_po_tokens: false,
                is_embedded: false,
            }
        }

        /// Native apps send no Origin/Referer headers.
        pub fn is_native_app(&self) -> bool {
            self.platform.is_some()
        }

        /// Headers googlevideo expects on media requests from URLs minted by
        /// this client (`YTPlayerUtils.streamHeaders`).
        pub fn stream_headers(&self) -> Vec<(String, String)> {
            let origin = match self.name {
                "WEB_REMIX" => "https://music.youtube.com",
                "WEB_CREATOR" => "https://studio.youtube.com",
                _ => "https://www.youtube.com",
            };
            vec![
                ("User-Agent".into(), self.user_agent.into()),
                ("Accept".into(), "*/*".into()),
                ("Accept-Language".into(), "en-US,en;q=0.9".into()),
                ("Referer".into(), format!("{origin}/")),
                ("Origin".into(), origin.into()),
            ]
        }
    }

    pub const WEB: ClientSpec = ClientSpec { use_web_po_tokens: false, ..ClientSpec::base("WEB", WEB_VERSION, WEB_ID, WEB_UA) };

    pub const WEB_REMIX: ClientSpec = ClientSpec {
        use_signature_timestamp: true,
        use_web_po_tokens: true,
        ..ClientSpec::base("WEB_REMIX", WEB_REMIX_VERSION, WEB_REMIX_ID, WEB_UA)
    };

    /// The YouTube app on Apple Vision Pro — main stream client.
    pub const VISIONOS: ClientSpec = ClientSpec {
        os_name: Some("visionOS"),
        os_version: Some("26.6.0.23O770"),
        device_make: Some("Apple"),
        device_model: Some("RealityDevice17,1"),
        api_origin: ORIGIN_YOUTUBE,
        api_format_version: "2",
        client_screen: Some("WATCH"),
        platform: Some("MOBILE"),
        own_visitor_id: true,
        ..ClientSpec::base(
            "VISIONOS",
            VISIONOS_VERSION,
            VISIONOS_ID,
            "com.google.visionos.youtube/1.04(RealityDevice17,1; U; CPU visionOS 26_6_0 like Mac OS X; US)",
        )
    };

    pub const IPADOS: ClientSpec = ClientSpec {
        os_name: Some("iPadOS"),
        os_version: Some("17.7.10.21H450"),
        device_make: Some("Apple"),
        device_model: Some("iPad7,6"),
        ..ClientSpec::base(
            "IOS",
            "21.03.3",
            "5",
            "com.google.ios.youtube/21.03.3 (iPad7,6; U; CPU iPadOS 17_7_10 like Mac OS X; en-US)",
        )
    };

    pub const ANDROID_VR_1_43_32: ClientSpec = ClientSpec {
        os_name: Some("Android"),
        os_version: Some("12"),
        device_make: Some("Oculus"),
        device_model: Some("Quest 3"),
        android_sdk_version: Some("32"),
        ..ClientSpec::base(
            "ANDROID_VR",
            "1.43.32",
            "28",
            "com.google.android.apps.youtube.vr.oculus/1.43.32 (Linux; U; Android 12; en_US; Quest 3; Build/SQ3A.220605.009.A1; Cronet/107.0.5284.2)",
        )
    };

    pub const ANDROID_VR_1_61_48: ClientSpec = ClientSpec {
        os_name: Some("Android"),
        os_version: Some("12"),
        device_make: Some("Oculus"),
        device_model: Some("Quest 3"),
        android_sdk_version: Some("32"),
        ..ClientSpec::base(
            "ANDROID_VR",
            "1.61.48",
            "28",
            "com.google.android.apps.youtube.vr.oculus/1.61.48 (Linux; U; Android 12; en_US; Quest 3; Build/SQ3A.220605.009.A1; Cronet/132.0.6808.3)",
        )
    };

    pub const ANDROID_VR_1_65_10: ClientSpec = ClientSpec {
        os_name: Some("Android"),
        os_version: Some("12L"),
        device_make: Some("Oculus"),
        device_model: Some("Quest 3"),
        android_sdk_version: Some("32"),
        ..ClientSpec::base(
            "ANDROID_VR",
            "1.65.10",
            "28",
            "com.google.android.apps.youtube.vr.oculus/1.65.10 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip",
        )
    };

    pub const ANDROID_VR_NO_AUTH: ClientSpec = ClientSpec::base(
        "ANDROID_VR",
        "1.61.48",
        "28",
        "com.google.android.apps.youtube.vr.oculus/1.61.48 (Linux; U; Android 12; en_US; Oculus Quest 3; Build/SQ3A.220605.009.A1; Cronet/132.0.6808.3)",
    );

    pub const MOBILE: ClientSpec = ClientSpec {
        os_name: Some("Android"),
        os_version: Some("11"),
        use_signature_timestamp: true,
        ..ClientSpec::base("ANDROID", "21.26.364", "3", "com.google.android.youtube/21.26.364 (Linux; U; Android 11) gzip")
    };

    pub const ANDROID_CREATOR: ClientSpec = ClientSpec {
        os_name: Some("Android"),
        os_version: Some("15"),
        device_make: Some("Google"),
        device_model: Some("Pixel 9 Pro Fold"),
        android_sdk_version: Some("35"),
        use_signature_timestamp: true,
        ..ClientSpec::base(
            "ANDROID_CREATOR",
            "25.03.101",
            "14",
            "com.google.android.apps.youtube.creator/25.03.101 (Linux; U; Android 15; en_US; Pixel 9 Pro Fold; Build/AP3A.241005.015.A2; Cronet/132.0.6779.0)",
        )
    };

    /// Stream client walk (Kotlin `STREAM_FALLBACK_CLIENTS` order) restricted
    /// to clients that hand out direct URLs without login, PoToken or JS
    /// deciphering. Only VISIONOS has been measured to serve full files; the
    /// rest usually fail validation (cut off at ~786 KB) but cost one request.
    pub const STREAM_WALK: &[ClientSpec] = &[
        VISIONOS,
        IPADOS,
        ANDROID_VR_1_43_32,
        ANDROID_VR_1_61_48,
        ANDROID_VR_1_65_10,
        ANDROID_VR_NO_AUTH,
        ANDROID_CREATOR,
        MOBILE,
    ];
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
        matches!(self, AudioFormat::AacM4a | AudioFormat::OpusWebm | AudioFormat::Mp3 | AudioFormat::Flac)
    }
}

/// Which codec family to rank first when choosing a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodecPreference {
    /// AAC (mp4a) first, Opus kept as alternatives. Default.
    #[default]
    PreferAac,
    /// Opus (webm) first, AAC as alternatives.
    PreferOpus,
    /// AAC only (fail rather than return Opus).
    AacOnly,
}

/// One audio format from a player response (unvalidated unless it is the
/// primary of a [`StreamResult`]).
#[derive(Debug, Clone, PartialEq)]
pub struct StreamCandidate {
    pub url: String,
    pub format: AudioFormat,
    pub itag: Option<u32>,
    /// e.g. `audio/mp4; codecs="mp4a.40.2"`.
    pub mime_type: String,
    /// e.g. `mp4a.40.2` (AAC-LC), `mp4a.40.5` (HE-AAC), `opus`.
    pub codecs: Option<String>,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate_hz: Option<u32>,
    pub channels: Option<u32>,
    pub content_length: Option<u64>,
    pub approx_duration_ms: Option<u64>,
    /// dB above the −14 LKFS reference; normalization gain = `-loudness_db`.
    pub loudness_db: Option<f32>,
    /// `AUDIO_QUALITY_LOW|MEDIUM|HIGH`.
    pub audio_quality: Option<String>,
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
    /// Loudness relative to −14 LKFS (`loudnessDb`), used for normalization:
    /// apply `-loudness_db` dB of gain.
    pub loudness_db: Option<f32>,
    /// Headers that must accompany the media request (User-Agent, Origin, …).
    pub headers: Vec<(String, String)>,
    /// Unix seconds after which the URL stops working (`expire=` param;
    /// ~6 h after resolving). Re-resolve shortly before this.
    pub expires_at_unix: Option<u64>,
    /// InnerTube client whose URL validated (e.g. `VISIONOS`).
    pub client: String,
    pub mime_type: String,
    pub codecs: Option<String>,
    pub channels: Option<u32>,
    pub approx_duration_ms: Option<u64>,
    /// Other formats of the same response (same `headers`), best-first —
    /// includes Opus/webm when AAC was chosen. Not individually validated.
    pub alternatives: Vec<StreamCandidate>,
}

impl StreamResult {
    pub(crate) fn from_candidate(
        c: StreamCandidate,
        alternatives: Vec<StreamCandidate>,
        headers: Vec<(String, String)>,
        client: String,
        expires_at_unix: Option<u64>,
    ) -> Self {
        Self {
            url: c.url,
            format: c.format,
            itag: c.itag,
            bitrate_kbps: c.bitrate_kbps,
            sample_rate_hz: c.sample_rate_hz,
            content_length: c.content_length,
            loudness_db: c.loudness_db,
            headers,
            expires_at_unix,
            client,
            mime_type: c.mime_type,
            codecs: c.codecs,
            channels: c.channels,
            approx_duration_ms: c.approx_duration_ms,
            alternatives,
        }
    }

    /// True when the URL expires within `margin` from now (or is unknown).
    pub fn expires_within(&self, margin: Duration) -> bool {
        let Some(exp) = self.expires_at_unix else { return true };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        exp <= now + margin.as_secs()
    }

    /// First alternative with the given format (e.g. Opus fallback).
    pub fn alternative(&self, format: AudioFormat) -> Option<&StreamCandidate> {
        self.alternatives.iter().find(|c| c.format == format)
    }
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
    /// HTTP 429: YouTube is limiting this network; retry in a few minutes.
    #[error("rate limited by YouTube (HTTP 429)")]
    RateLimited,
}

pub type YtmResult<T> = Result<T, YtmError>;

/// Content language (`hl`) and country (`gl`) sent in every context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locale {
    pub hl: String,
    pub gl: String,
}

impl Default for Locale {
    fn default() -> Self {
        Self { hl: "en".into(), gl: "US".into() }
    }
}

#[derive(Debug, Default)]
struct Inner {
    /// Web visitor data (`sw.js_data` / `visitor_id`).
    visitor_data: RwLock<String>,
    /// Visitor ids minted by clients that bring their own (VISIONOS).
    own_visitors: Mutex<HashMap<&'static str, String>>,
    locale: RwLock<Locale>,
    /// VISIONOS is left alone until then after two bot checks in a row.
    visionos_resting_until: Mutex<Option<Instant>>,
}

/// InnerTube client. `Clone` is cheap and shares state (visitor data,
/// locale, bot-check backoff). `Send + Sync`.
#[derive(Debug, Clone)]
pub struct YtmClient {
    pub http: reqwest::Client,
    inner: Arc<Inner>,
}

const RETRY_ATTEMPTS: u32 = 3;
const RETRY_INITIAL_DELAY_MS: u64 = 500;
const BOT_CHECK_REST: Duration = Duration::from_secs(30 * 60);

// ---------------------------------------------------------------------------
// Transport helpers
// ---------------------------------------------------------------------------

impl YtmClient {
    fn build_http() -> YtmResult<reqwest::Client> {
        reqwest::Client::builder()
            .gzip(true)
            .deflate(true)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .map_err(|e| YtmError::Network(format!("reqwest client build: {e}")))
    }

    fn locale(&self) -> Locale {
        self.inner.locale.read().expect("locale lock").clone()
    }

    /// Build the InnerTube `context` for `client` (Kotlin `toContext`).
    fn context(&self, client: &ClientSpec, visitor: Option<&str>) -> Value {
        let loc = self.locale();
        let mut c = json!({
            "clientName": client.name,
            "clientVersion": client.version,
            "gl": loc.gl,
            "hl": loc.hl,
        });
        let opt = [
            ("clientScreen", client.client_screen),
            ("platform", client.platform),
            ("osName", client.os_name),
            ("osVersion", client.os_version),
            ("deviceMake", client.device_make),
            ("deviceModel", client.device_model),
            ("androidSdkVersion", client.android_sdk_version),
        ];
        for (k, v) in opt {
            if let Some(v) = v {
                c[k] = Value::String(v.to_string());
            }
        }
        if let Some(v) = visitor.filter(|v| !v.is_empty()) {
            c["visitorData"] = Value::String(v.to_string());
        }
        if client.client_screen.is_some() {
            c["utcOffsetMinutes"] = json!(0);
        }
        json!({
            "client": c,
            "request": { "internalExperimentFlags": [], "useSsl": true },
            "user": { "lockedSafetyMode": false },
        })
    }

    /// POST `{api_origin}/youtubei/v1/{endpoint}` with retry on transient
    /// failures (connect/timeout/5xx; 3 attempts, 500 ms backoff ×2).
    async fn post_api(
        &self,
        client: &ClientSpec,
        endpoint: &str,
        body: Value,
        visitor: Option<&str>,
        timeout: Duration,
    ) -> YtmResult<Value> {
        let url = format!("{}/youtubei/v1/{}?prettyPrint=false", client.api_origin, endpoint);
        let loc = self.locale();
        let accept_lang = format!("{}-{},{};q=0.9", loc.hl, loc.gl, loc.hl);
        let mut delay = Duration::from_millis(RETRY_INITIAL_DELAY_MS);
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut req = self
                .http
                .post(&url)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json")
                .header("Accept-Language", &accept_lang)
                .header("User-Agent", client.user_agent)
                .header("X-Goog-Api-Format-Version", client.api_format_version)
                .header("X-YouTube-Client-Name", client.id)
                .header("X-YouTube-Client-Version", client.version)
                .timeout(timeout)
                .json(&body);
            if !client.is_native_app() {
                req = req.header("X-Origin", client.api_origin).header("Referer", format!("{}/", client.api_origin));
            }
            if let Some(v) = visitor.filter(|v| !v.is_empty()) {
                req = req.header("X-Goog-Visitor-Id", v);
            }
            let transient: String = match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.as_u16() == 429 {
                        return Err(YtmError::RateLimited);
                    }
                    if status.is_server_error() {
                        format!("HTTP {status} from {endpoint}")
                    } else if !status.is_success() {
                        return Err(YtmError::Network(format!("HTTP {status} from {endpoint}")));
                    } else {
                        match resp.json::<Value>().await {
                            Ok(v) => return Ok(v),
                            Err(e) if e.is_timeout() || e.is_body() => format!("{endpoint}: {e}"),
                            Err(e) => return Err(YtmError::Parse(format!("{endpoint}: {e}"))),
                        }
                    }
                }
                Err(e) if e.is_timeout() || e.is_connect() || e.is_request() => format!("{endpoint}: {e}"),
                Err(e) => return Err(YtmError::Network(format!("{endpoint}: {e}"))),
            };
            if attempt >= RETRY_ATTEMPTS {
                return Err(YtmError::Network(transient));
            }
            tokio::time::sleep(delay).await;
            delay *= 2;
        }
    }

    /// WEB_REMIX call on music.youtube.com with the web visitor data.
    async fn remix(&self, endpoint: &str, extra: Value) -> YtmResult<Value> {
        let visitor = self.visitor_data();
        let mut body = json!({ "context": self.context(&clients::WEB_REMIX, Some(&visitor)) });
        if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                b.insert(k.clone(), v.clone());
            }
        }
        let resp = self
            .post_api(&clients::WEB_REMIX, endpoint, body, Some(&visitor), Duration::from_secs(20))
            .await?;
        // Adopt the visitor id YouTube assigns when we had none.
        if visitor.is_empty()
            && let Some(vd) = resp["responseContext"]["visitorData"].as_str()
        {
            self.set_visitor_data(vd);
        }
        Ok(resp)
    }

    async fn browse(&self, browse_id: Option<&str>, params: Option<&str>, continuation: Option<&str>) -> YtmResult<Value> {
        let mut extra = json!({});
        if let Some(b) = browse_id {
            extra["browseId"] = json!(b);
        }
        if let Some(p) = params {
            extra["params"] = json!(p);
        }
        if let Some(c) = continuation {
            extra["continuation"] = json!(c);
        }
        self.remix("browse", extra).await
    }

    async fn next_raw(&self, video_id: Option<&str>, playlist_id: Option<&str>, params: Option<&str>) -> YtmResult<Value> {
        let mut extra = json!({ "isAudioOnly": true, "enablePersistentPlaylistPanel": true });
        if let Some(v) = video_id {
            extra["videoId"] = json!(v);
        }
        if let Some(p) = playlist_id {
            extra["playlistId"] = json!(p);
        }
        if let Some(p) = params {
            extra["params"] = json!(p);
        }
        self.remix("next", extra).await
    }

    /// Web visitor data from `sw.js_data` (strip `)]}'`, `[0][2]`, first
    /// string matching `^Cg[ts]`), else minted via `visitor_id`.
    async fn mint_visitor(&self) -> YtmResult<String> {
        if let Ok(resp) = self
            .http
            .get("https://music.youtube.com/sw.js_data")
            .header("User-Agent", clients::WEB_UA)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            && let Ok(text) = resp.text().await
            && let Some(vd) = parse_sw_js_data(&text)
        {
            return Ok(vd);
        }
        self.mint_client_visitor(&clients::WEB_REMIX).await
    }

    /// `POST {apiOrigin}/youtubei/v1/visitor_id` as `client`.
    async fn mint_client_visitor(&self, client: &ClientSpec) -> YtmResult<String> {
        let body = json!({ "context": self.context(client, None) });
        let resp = self.post_api(client, "visitor_id", body, None, Duration::from_secs(10)).await?;
        resp["responseContext"]["visitorData"]
            .as_str()
            .or_else(|| resp["visitorData"].as_str())
            .map(str::to_string)
            .ok_or_else(|| YtmError::Parse("no visitorData in visitor_id response".into()))
    }
}

/// Parse `sw.js_data` → visitor data (Kotlin `YouTube.visitorData`).
pub fn parse_sw_js_data(text: &str) -> Option<String> {
    let body = text.get(5..)?;
    let is_vd = |s: &str| s.starts_with("Cgt") || s.starts_with("Cgs");
    if let Ok(v) = serde_json::from_str::<Value>(body)
        && let Some(arr) = v[0][2].as_array()
        && let Some(s) = arr.iter().filter_map(|x| x.as_str()).find(|s| is_vd(s))
    {
        return Some(s.to_string());
    }
    body.split('"').map(str::trim).find(|p| is_vd(p) && p.len() > 10).map(str::to_string)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

impl YtmClient {
    /// Build a client and fetch web visitor data (spec §2.1). Network
    /// failures while fetching visitor data are not fatal: the client works
    /// without it and adopts the one YouTube assigns on the first response.
    pub async fn new() -> YtmResult<Self> {
        let client = Self::new_offline()?;
        if let Ok(vd) = client.mint_visitor().await {
            client.set_visitor_data(&vd);
        }
        Ok(client)
    }

    /// Build a client without any network access (no visitor data yet).
    pub fn new_offline() -> YtmResult<Self> {
        Ok(Self { http: Self::build_http()?, inner: Arc::new(Inner::default()) })
    }

    /// Set content language/country, e.g. `("vi", "VN")`. Affects titles,
    /// home feed and charts. Defaults to `("en", "US")`.
    pub fn set_locale(&self, hl: impl Into<String>, gl: impl Into<String>) {
        let mut loc = self.inner.locale.write().expect("locale lock");
        loc.hl = hl.into();
        loc.gl = gl.into().to_uppercase();
    }

    /// Current `(hl, gl)`.
    pub fn locale_pair(&self) -> (String, String) {
        let l = self.locale();
        (l.hl, l.gl)
    }

    /// Current web visitor data ("" when none) — persist it across runs.
    pub fn visitor_data(&self) -> String {
        self.inner.visitor_data.read().expect("visitor lock").clone()
    }

    /// Restore previously persisted web visitor data.
    pub fn set_visitor_data(&self, vd: &str) {
        *self.inner.visitor_data.write().expect("visitor lock") = vd.to_string();
    }

    /// Search across all filters at once (songs/videos/albums/artists/playlists).
    pub async fn search_all(&self, query: &str) -> YtmResult<SearchResults> {
        let json = self.remix("search", json!({ "query": query })).await?;
        Ok(parse::search_response(&json))
    }

    /// Search with a single filter; returns songs for SONG/VIDEO filters.
    pub async fn search_songs(&self, query: &str, filter_params: &str) -> YtmResult<Vec<Song>> {
        let json = self.remix("search", json!({ "query": query, "params": filter_params })).await?;
        let results = parse::search_response(&json);
        let mut songs = results.songs;
        songs.extend(results.videos);
        Ok(songs)
    }

    /// Search with any filter, returning the full typed result set (albums,
    /// artists, playlists filters included).
    pub async fn search_filtered(&self, query: &str, filter_params: &str) -> YtmResult<SearchResults> {
        let json = self.remix("search", json!({ "query": query, "params": filter_params })).await?;
        Ok(parse::search_response(&json))
    }

    /// `music/get_search_suggestions`.
    pub async fn suggestions(&self, query: &str) -> YtmResult<Vec<String>> {
        let json = self.remix("music/get_search_suggestions", json!({ "input": query })).await?;
        Ok(parse::suggestions_response(&json))
    }

    /// Suggestions plus the recommended items (artists/songs) shown under them.
    pub async fn suggestions_with_items(&self, query: &str) -> YtmResult<(Vec<String>, Vec<ShelfItem>)> {
        let json = self.remix("music/get_search_suggestions", json!({ "input": query })).await?;
        Ok((parse::suggestions_response(&json), parse::suggestion_items(&json)))
    }

    /// Browse an album (`MPRE…` id). Tracks inherit album artists/artwork.
    pub async fn album(&self, browse_id: &str) -> YtmResult<(Album, Vec<Song>)> {
        let json = self.browse(Some(browse_id), None, None).await?;
        let (mut album, mut songs) = parse::album_response(&json);
        if album.id.is_empty() {
            album.id = browse_id.to_string();
            for s in &mut songs {
                s.album_id = Some(browse_id.to_string());
            }
        }
        if album.title.is_empty() && songs.is_empty() {
            return Err(YtmError::Parse(format!("album {browse_id}: unrecognised response")));
        }
        Ok((album, songs))
    }

    /// Browse an artist (`UC…`/`LA…` id): top songs + albums (name in `Shelf`s).
    pub async fn artist(&self, browse_id: &str) -> YtmResult<(Artist, Vec<Shelf>)> {
        let json = self.browse(Some(browse_id), None, None).await?;
        let (mut artist, shelves) = parse::artist_response(&json);
        if artist.id.is_empty() {
            artist.id = browse_id.to_string();
        }
        if artist.name.is_empty() && shelves.is_empty() {
            return Err(YtmError::Parse(format!("artist {browse_id}: unrecognised response")));
        }
        Ok((artist, shelves))
    }

    /// Browse a playlist. Accepts `VL…` browse ids or bare playlist ids
    /// (`PL…`, `RDCLAK…`, `OLAK5…`). Follows continuations up to
    /// [`clients::MAX_GET_QUEUE_SIZE`] tracks.
    pub async fn playlist(&self, browse_id: &str) -> YtmResult<(Playlist, Vec<Song>)> {
        let bid = if browse_id.starts_with("VL") { browse_id.to_string() } else { format!("VL{browse_id}") };
        let json = self.browse(Some(&bid), None, None).await?;
        let (mut playlist, mut songs) = parse::playlist_response(&json);
        if playlist.id.is_empty() {
            playlist.id = parse::playlist_id_from_browse(&bid);
            playlist.browse_id = Some(bid.clone());
        }
        if playlist.name.is_empty() && songs.is_empty() {
            return Err(YtmError::Parse(format!("playlist {browse_id}: unrecognised response")));
        }
        let mut token = parse::playlist_continuation_token(&json);
        while let Some(t) = token.take() {
            if songs.len() >= clients::MAX_GET_QUEUE_SIZE {
                break;
            }
            let page = self.browse(None, None, Some(&t)).await?;
            let (more, next) = parse::playlist_continuation(&page);
            let before = songs.len();
            for s in more {
                if !songs.iter().any(|x| x.id == s.id) {
                    songs.push(s);
                }
            }
            if songs.len() == before {
                break;
            }
            token = next;
        }
        songs.truncate(clients::MAX_GET_QUEUE_SIZE);
        Ok((playlist, songs))
    }

    /// Home shelves (spec §7.2), following up to 3 continuation pages.
    pub async fn home(&self) -> YtmResult<Vec<Shelf>> {
        let json = self.browse(Some("FEmusic_home"), None, None).await?;
        let (mut shelves, mut cont) = parse::home_response(&json);
        for _ in 0..3 {
            let Some(c) = cont.take() else { break };
            match self.browse(None, None, Some(&c)).await {
                Ok(page) => {
                    let (more, next) = parse::home_response(&page);
                    shelves.extend(more);
                    cont = next;
                }
                Err(_) => break,
            }
        }
        Ok(shelves)
    }

    /// Related songs for a video: the `next` response's Related tab
    /// (`MPTR…`), browsed. Audio tracks are preferred over videos.
    pub async fn related(&self, video_id: &str) -> YtmResult<Vec<Song>> {
        let next = self.next_raw(Some(video_id), None, None).await?;
        let Some(bid) = parse::next_related_browse_id(&next) else {
            return Ok(Vec::new());
        };
        let json = self.browse(Some(&bid), None, None).await?;
        Ok(parse::related_response(&json))
    }

    /// Autoplay/radio queue for a seed video (capped at 1000). Follows the
    /// automix (`RDAMVM…`) endpoint like Kotlin `YouTube.next`.
    pub async fn get_queue(&self, video_id: &str) -> YtmResult<Vec<QueueItem>> {
        let first = self.next_raw(Some(video_id), None, None).await?;
        let mut items = parse::queue_from_next(&first);
        if let Some((pid, params)) = parse::next_automix_endpoint(&first)
            && let Ok(mix) = self.next_raw(None, Some(&pid), params.as_deref()).await
        {
            for q in parse::queue_from_next(&mix) {
                if !items.iter().any(|i| i.video_id == q.video_id) {
                    items.push(q);
                }
            }
        }
        items.truncate(clients::MAX_GET_QUEUE_SIZE);
        Ok(items)
    }

    /// Queue for a playlist id (e.g. an album's `OLAK5…` or a `RDCLAK…` mix).
    pub async fn playlist_queue(&self, playlist_id: &str) -> YtmResult<Vec<QueueItem>> {
        let json = self.next_raw(None, Some(playlist_id), None).await?;
        let mut items = parse::queue_from_next(&json);
        items.truncate(clients::MAX_GET_QUEUE_SIZE);
        Ok(items)
    }

    /// YouTube Music's own (unsynced) lyrics text, when the track has any.
    pub async fn lyrics(&self, video_id: &str) -> YtmResult<Option<String>> {
        let next = self.next_raw(Some(video_id), None, None).await?;
        let Some(bid) = parse::next_lyrics_browse_id(&next) else { return Ok(None) };
        let json = self.browse(Some(&bid), None, None).await?;
        Ok(parse::lyrics_response(&json))
    }

    /// Resolve a validated, decodable stream for a video (client walk,
    /// validation probe, AAC-first ranking — spec §2.4–§2.6).
    pub async fn resolve_stream(&self, video_id: &str, quality: AudioQuality) -> YtmResult<StreamResult> {
        self.resolve_stream_with(video_id, quality, CodecPreference::PreferAac).await
    }

    /// Track metadata for lyrics matching (title/artists/album/duration),
    /// from the `next` panel (WEB_REMIX); falls back to the VISIONOS player's
    /// `videoDetails` when the panel is empty.
    pub async fn track_meta(&self, video_id: &str) -> YtmResult<TrackMeta> {
        if let Ok(next) = self.next_raw(Some(video_id), None, None).await {
            let songs = parse::songs_from_next(&next);
            if let Some(song) = songs.iter().find(|s| s.id == video_id).or(songs.first()) {
                let mut meta = TrackMeta::from(song);
                meta.video_id = Some(video_id.to_string());
                return Ok(meta);
            }
        }
        let player = self.player(&clients::VISIONOS, video_id).await?;
        let d = &player["videoDetails"];
        let title = d["title"].as_str().unwrap_or("").to_string();
        if title.is_empty() {
            return Err(YtmError::Parse(format!("no metadata for {video_id}")));
        }
        let author = d["author"].as_str().unwrap_or("").trim_end_matches(" - Topic").to_string();
        Ok(TrackMeta {
            title,
            artists: if author.is_empty() { Vec::new() } else { vec![author] },
            album: None,
            duration_secs: d["lengthSeconds"].as_str().and_then(|s| s.parse().ok()),
            video_id: Some(video_id.to_string()),
        })
    }
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync + Clone>() {}
    assert_send_sync::<YtmClient>();
};

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn clones_share_state_and_locale_applies() {
        let a = YtmClient::new_offline().unwrap();
        let b = a.clone();
        a.set_locale("vi", "vn");
        a.set_visitor_data("CgtTRVNTSU9O");
        assert_eq!(b.locale_pair(), ("vi".to_string(), "VN".to_string()));
        assert_eq!(b.visitor_data(), "CgtTRVNTSU9O");
        let ctx = b.context(&clients::WEB_REMIX, Some("CgtTRVNTSU9O"));
        assert_eq!(ctx["client"]["hl"], "vi");
        assert_eq!(ctx["client"]["gl"], "VN");
        assert_eq!(ctx["client"]["clientName"], "WEB_REMIX");
        assert_eq!(ctx["client"]["visitorData"], "CgtTRVNTSU9O");
        assert!(ctx["client"]["utcOffsetMinutes"].is_null());
    }

    #[test]
    fn visionos_context_matches_kotlin() {
        let c = YtmClient::new_offline().unwrap();
        let ctx = c.context(&clients::VISIONOS, None);
        let cl = &ctx["client"];
        assert_eq!(cl["clientName"], "VISIONOS");
        assert_eq!(cl["clientVersion"], "1.04");
        assert_eq!(cl["clientScreen"], "WATCH");
        assert_eq!(cl["platform"], "MOBILE");
        assert_eq!(cl["deviceModel"], "RealityDevice17,1");
        assert_eq!(cl["utcOffsetMinutes"], 0);
        assert!(cl["visitorData"].is_null());
        assert_eq!(ctx["user"]["lockedSafetyMode"], false);
        assert!(clients::VISIONOS.is_native_app());
        assert_eq!(clients::VISIONOS.api_origin, "https://www.youtube.com");
        let h = clients::VISIONOS.stream_headers();
        assert!(h.iter().any(|(k, v)| k == "Origin" && v == "https://www.youtube.com"));
        assert!(h.iter().any(|(k, v)| k == "User-Agent" && v.starts_with("com.google.visionos.youtube/1.04")));
    }

    #[test]
    fn stream_walk_has_no_unusable_clients() {
        assert_eq!(clients::STREAM_WALK[0], clients::VISIONOS);
        assert!(clients::STREAM_WALK.iter().all(|c| !c.login_required && !c.use_web_po_tokens));
    }
}
