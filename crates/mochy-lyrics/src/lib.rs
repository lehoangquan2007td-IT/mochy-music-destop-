//! mochy-lyrics — lyrics providers + LRC parsing (spec §3).
//!
//! Contract for mochy-app: [`fetch`] with a provider order. Providers are
//! queried in parallel with a per-provider timeout (35 s, spec §3.3), scored
//! and the best is returned. MVP implements the network providers that don't
//! require the InnerTube client: BetterLyrics, YouLyPlus, Paxsenix, LrcRed,
//! SimpMusic, LrcLib, Deezer, NetEase, QQMusic, Kugou. The three
//! YouTube-based providers are declared but skipped in MVP.

use std::time::Duration;

use mochy_core::models::TrackMeta;
use thiserror::Error;

/// Lyric sync level (spec §3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncKind {
    #[default]
    None,
    Plain,
    Line,
    Word,
}

/// One timed word inside a line (seconds).
#[derive(Debug, Clone, PartialEq)]
pub struct LyricWord {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// One lyric line.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LyricLine {
    /// Line start in seconds.
    pub time: f64,
    pub text: String,
    /// Word timings when `SyncKind::Word`.
    pub words: Vec<LyricWord>,
    /// Background vocals marker `{bg}`.
    pub bg: bool,
    /// `{agent:X}` voice tag.
    pub agent: Option<String>,
}

/// Final lyrics for a track.
#[derive(Debug, Clone, Default)]
pub struct Lyrics {
    pub provider: String,
    pub sync: SyncKind,
    pub lines: Vec<LyricLine>,
    /// Unsynced full text when available.
    pub plain: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderId {
    BetterLyrics,
    YouLyPlus,
    Paxsenix,
    LrcRed,
    SimpMusic,
    LrcLib,
    Deezer,
    YouTubeSubtitle,
    YouTubeMusic,
    NetEase,
    QQMusic,
    Kugou,
    YouTubeCaptions,
}

impl ProviderId {
    /// All variants in default priority order (spec §3.1).
    pub fn all() -> &'static [ProviderId] {
        &[
            ProviderId::BetterLyrics,
            ProviderId::YouLyPlus,
            ProviderId::Paxsenix,
            ProviderId::LrcRed,
            ProviderId::SimpMusic,
            ProviderId::LrcLib,
            ProviderId::Deezer,
            ProviderId::YouTubeSubtitle,
            ProviderId::YouTubeMusic,
            ProviderId::NetEase,
            ProviderId::QQMusic,
            ProviderId::Kugou,
            ProviderId::YouTubeCaptions,
        ]
    }

    /// Display name (BetterLyrics is shown as "Apple Music" — spec §3.1).
    pub fn display_name(self) -> &'static str {
        match self {
            ProviderId::BetterLyrics => "Apple Music",
            ProviderId::YouLyPlus => "YouLyPlus",
            ProviderId::Paxsenix => "Paxsenix",
            ProviderId::LrcRed => "LrcRed",
            ProviderId::SimpMusic => "SimpMusic",
            ProviderId::LrcLib => "LrcLib",
            ProviderId::Deezer => "Deezer",
            ProviderId::YouTubeSubtitle => "YouTubeSubtitle",
            ProviderId::YouTubeMusic => "YouTubeMusic",
            ProviderId::NetEase => "NetEase",
            ProviderId::QQMusic => "QQMusic",
            ProviderId::Kugou => "Kugou",
            ProviderId::YouTubeCaptions => "YouTubeCaptions",
        }
    }

    /// Parse a stored id string back to a variant.
    pub fn from_id(id: &str) -> Option<ProviderId> {
        let _ = id;
        todo!("ProviderId::from_id")
    }

    /// Stable id string for persistence.
    pub fn id(self) -> &'static str {
        match self {
            ProviderId::BetterLyrics => "BETTERLYRICS",
            ProviderId::YouLyPlus => "YOULYPLUS",
            ProviderId::Paxsenix => "PAXSENIX",
            ProviderId::LrcRed => "LRCRED",
            ProviderId::SimpMusic => "SIMPMUSIC",
            ProviderId::LrcLib => "LRCLIB",
            ProviderId::Deezer => "DEEZER",
            ProviderId::YouTubeSubtitle => "YOUTUBESUBTITLE",
            ProviderId::YouTubeMusic => "YOUTUBEMUSIC",
            ProviderId::NetEase => "NETEASE",
            ProviderId::QQMusic => "QQMUSIC",
            ProviderId::Kugou => "KUGOU",
            ProviderId::YouTubeCaptions => "YOUTUBECAPTIONS",
        }
    }

    /// True when MVP implements this provider.
    pub fn implemented(self) -> bool {
        !matches!(
            self,
            ProviderId::YouTubeSubtitle
                | ProviderId::YouTubeMusic
                | ProviderId::YouTubeCaptions
                | ProviderId::LrcRed
        )
    }
}

#[derive(Debug, Error)]
pub enum LyricsError {
    #[error("network: {0}")]
    Network(String),
    #[error("parse: {0}")]
    Parse(String),
    #[error("{0}")]
    Other(String),
}

pub type LyricsResult<T> = Result<T, LyricsError>;

/// Fetch configuration.
#[derive(Debug, Clone)]
pub struct FetchConfig {
    /// Ordered, enabled providers (first = highest priority).
    pub order: Vec<ProviderId>,
    /// Per-provider timeout (spec: 35 s).
    pub timeout: Duration,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            order: ProviderId::all()
                .iter()
                .copied()
                .filter(|p| p.implemented())
                .collect(),
            timeout: Duration::from_secs(35),
        }
    }
}

/// Fetch the best lyrics for a track. `None` when every provider misses.
/// Queries eligible providers in parallel, scores results
/// (Script 10000 / Sync 1000 / Duet 100, then order tiebreak — spec §3.3)
/// and returns the winner.
pub async fn fetch(
    meta: &TrackMeta,
    cfg: &FetchConfig,
    http: &reqwest::Client,
) -> Option<Lyrics> {
    let _ = (meta, cfg, http);
    todo!("lyrics::fetch")
}

/// Fetch from a single provider (used by tests and the "pick provider" UI).
pub async fn fetch_from_provider(
    meta: &TrackMeta,
    provider: ProviderId,
    http: &reqwest::Client,
) -> LyricsResult<Option<Lyrics>> {
    let _ = (meta, provider, http);
    todo!("lyrics::fetch_from_provider")
}

// ---------------------------------------------------------------------------
// Parsing — pure functions, unit-testable (spec §3.4).
// ---------------------------------------------------------------------------

/// Parse the Mochy LRC dialect: `[mm:ss.mmm]` lines, next-line word timings
/// `<text:start:end>` (seconds), rich `<mm:ss.cc>word`, `{agent:X}`, `{bg}`.
/// All lines sharing one timestamp ⇒ `SyncKind::Plain`.
pub fn parse_lrc(src: &str) -> (Vec<LyricLine>, SyncKind) {
    let _ = src;
    todo!("parse_lrc")
}

/// Serialize lines back to the LRC dialect (for the `lyrics` DB table).
pub fn lines_to_lrc(lines: &[LyricLine]) -> String {
    let _ = lines;
    todo!("lines_to_lrc")
}

/// Parse TTML (Apple Music style, BetterLyrics/Paxsenix) into lines.
pub fn parse_ttml(xml: &str) -> LyricsResult<Vec<LyricLine>> {
    let _ = xml;
    todo!("parse_ttml")
}

/// Index of the line active at `t_secs` with the 300 ms lead-in
/// (spec §3.4 `LineLeadInMs`); `offset_ms` shifts the query time
/// (positive shows lines earlier — Android `song.lyricsOffset`).
pub fn active_line_index(
    lines: &[LyricLine],
    t_secs: f64,
    offset_ms: i32,
) -> Option<usize> {
    let _ = (lines, t_secs, offset_ms);
    todo!("active_line_index")
}
