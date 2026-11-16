//! mochy-lyrics — lyrics providers + LRC parsing (spec §3).
//!
//! Contract for mochy-app: [`fetch`] with a provider order. Providers are
//! queried in parallel with a per-provider timeout (35 s, spec §3.3), scored
//! and the best is returned. Implemented: BetterLyrics, YouLyPlus, Paxsenix, LrcRed,
//! SimpMusic, LrcLib, Deezer, YouTubeSubtitle (transcript), YouTubeMusic (lyrics tab), NetEase,
//! QQMusic, Kugou and YouTubeCaptions (auto captions, last resort). The YouTube sources use
//! [`FetchConfig::ytm`] (or a shared fallback client). Paxsenix is ported but disabled by
//! default because its service is down (see [`ProviderId::implemented`]).
//!
//! Extras: [`embedded`] (lyrics in local files' tags / sidecar `.lrc`), [`romanize`]
//! (Japanese/Korean/Chinese/Cyrillic/Hindi/Punjabi readings) and [`translate`] (keyless
//! Google Translate).
//!
//! `Lyrics::provider` is the provider's [`ProviderId::display_name`].
//!
//! Every provider produces a raw payload in the Mochy LRC dialect (exactly what Android
//! stores in the `lyrics` table); [`fetch`] grades those payloads with
//! [`select::pick_best`] and parses the winner with [`parse_lrc`].

use std::time::Duration;

use futures::future::join_all;
use mochy_core::models::TrackMeta;
use thiserror::Error;

pub mod autosync;
pub mod embedded;
mod lrc;
mod providers;
pub mod romanize;
pub mod script;
pub mod select;
pub mod translate;
mod ttml;
mod xml;

#[cfg(test)]
mod tests;

pub use script::Script;
pub use select::{Candidate, pick_best};

/// Sentinel stored in the `lyrics` table when no provider had lyrics (spec §3.5).
pub const LYRICS_NOT_FOUND: &str = "LYRICS_NOT_FOUND";

/// How early a line lights up before its own timestamp.
/// Set to 0 ms so lyrics do not run ahead of the music (spec §3.4 note).
pub const LINE_LEAD_IN_MS: i64 = 0;

/// Fallback duration of an untimed last line (Android `UntimedLineDurationMs`).
pub const UNTIMED_LINE_DURATION_MS: i64 = 5_000;

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

    /// Display name (BetterLyrics is shown as "Apple Music" — spec §3.1; the YouTube sources
    /// as Android's `LyricsProviderRegistry.getDisplayName`).
    pub fn display_name(self) -> &'static str {
        match self {
            ProviderId::BetterLyrics => "Apple Music",
            ProviderId::YouLyPlus => "YouLyPlus",
            ProviderId::Paxsenix => "Paxsenix",
            ProviderId::LrcRed => "LrcRed",
            ProviderId::SimpMusic => "SimpMusic",
            ProviderId::LrcLib => "LrcLib",
            ProviderId::Deezer => "Deezer",
            ProviderId::YouTubeSubtitle => "YouTube Subtitle",
            ProviderId::YouTubeMusic => "YouTube Music",
            ProviderId::NetEase => "NetEase",
            ProviderId::QQMusic => "QQMusic",
            ProviderId::Kugou => "Kugou",
            ProviderId::YouTubeCaptions => "YouTube Auto Captions",
        }
    }

    /// Parse a stored id string back to a variant.
    ///
    /// Accepts the [`id`](Self::id) strings, the Android provider names
    /// (`"BetterLyrics"`, `"Kugou"`, … as saved in `lyricsProviderOrder`) and the display names,
    /// case-insensitively.
    pub fn from_id(id: &str) -> Option<ProviderId> {
        let id = id.trim();
        if id.is_empty() {
            return None;
        }
        ProviderId::all()
            .iter()
            .copied()
            .find(|p| p.id().eq_ignore_ascii_case(id) || p.display_name().eq_ignore_ascii_case(id))
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

    /// True when this port implements the provider *and* it answered live when checked
    /// (2026-11-16). The default [`FetchConfig`] only asks these; the others can still be
    /// called through [`fetch_from_provider`] or an explicit `FetchConfig::order`.
    pub fn implemented(self) -> bool {
        match self {
            // Ported (providers/paxsenix.rs), but lyrics.paxsenix.org is down: every request
            // answered HTTP 502, or a Cloudflare "Attention Required" 403 challenge page, for
            // all test songs on 2026-11-16. Re-enable once the service is back.
            ProviderId::Paxsenix => false,
            _ => true,
        }
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
    /// InnerTube client for the YouTube sources (locale + visitor data shared with the app).
    /// `None` = a shared client of this crate's own.
    pub ytm: Option<mochy_innertube::YtmClient>,
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
            ytm: None,
        }
    }
}

impl Lyrics {
    /// Parse a stored / provider payload (Mochy LRC dialect, or plain text). `None` when it
    /// holds no lyrics (blank, [`LYRICS_NOT_FOUND`], nothing parseable).
    pub fn from_lrc(provider: impl Into<String>, raw: &str) -> Option<Lyrics> {
        let (lines, sync) = parse_lrc(raw);
        if sync == SyncKind::None || lines.is_empty() {
            return None;
        }
        let plain = (sync == SyncKind::Plain).then(|| {
            lines
                .iter()
                .map(|l| l.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        });
        Some(Lyrics {
            provider: provider.into(),
            sync,
            lines,
            plain,
        })
    }

    /// Serialize for the `lyrics` table. Untimed lyrics are written as plain text.
    pub fn to_lrc(&self) -> String {
        let untimed = self.sync == SyncKind::Plain
            && self
                .lines
                .iter()
                .all(|l| l.time == 0.0 && l.words.is_empty());
        match (&self.plain, untimed) {
            (Some(p), true) if !p.trim().is_empty() => p.clone(),
            (_, true) => self
                .lines
                .iter()
                .map(|l| l.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            _ => lines_to_lrc(&self.lines),
        }
    }
}

/// Ask one provider with a timeout and grade its answer.
async fn fetch_candidate(
    q: &providers::Query,
    provider: ProviderId,
    order: usize,
    timeout: Duration,
    http: &reqwest::Client,
) -> Option<Candidate> {
    match tokio::time::timeout(timeout, providers::fetch_raw(provider, q, http)).await {
        Ok(Ok(Some(raw))) => Candidate::new(provider, raw, order),
        _ => None,
    }
}

/// Fetch the best lyrics for a track. `None` when every provider misses.
/// Queries eligible providers in parallel, scores results
/// (Script 10000 / Sync 1000 / Duet 100, then order tiebreak — spec §3.3)
/// and returns the winner.
///
/// Like Android, YouTube's auto captions are only asked when nothing else had lyrics.
pub async fn fetch(meta: &TrackMeta, cfg: &FetchConfig, http: &reqwest::Client) -> Option<Lyrics> {
    let mut q = providers::Query::from_meta(meta);
    q.ytm = cfg.ytm.clone();
    let mut order: Vec<ProviderId> = Vec::new();
    for p in &cfg.order {
        if !order.contains(p) {
            order.push(*p);
        }
    }
    type Indexed = Vec<(usize, ProviderId)>;
    let (last_resort, regular): (Indexed, Indexed) = order
        .iter()
        .copied()
        .enumerate()
        .partition(|(_, p)| *p == ProviderId::YouTubeCaptions);
    let ask = |set: Indexed| {
        let q = &q;
        join_all(
            set.into_iter()
                .map(move |(i, p)| fetch_candidate(q, p, i, cfg.timeout, http)),
        )
    };
    let mut candidates: Vec<Candidate> = ask(regular).await.into_iter().flatten().collect();
    let mut best = pick_best(meta, &candidates, order.len()).cloned();
    if best.is_none() && !last_resort.is_empty() {
        candidates.extend(ask(last_resort).await.into_iter().flatten());
        best = pick_best(meta, &candidates, order.len()).cloned();
    }
    let best = best?;
    Lyrics::from_lrc(best.provider.display_name(), &best.raw)
}

/// Fetch from a single provider (used by tests and the "pick provider" UI).
/// `Ok(None)` when the provider has no lyrics for the track.
pub async fn fetch_from_provider(
    meta: &TrackMeta,
    provider: ProviderId,
    http: &reqwest::Client,
) -> LyricsResult<Option<Lyrics>> {
    let q = providers::Query::from_meta(meta);
    let raw = providers::fetch_raw(provider, &q, http).await?;
    Ok(raw.and_then(|r| Lyrics::from_lrc(provider.display_name(), &r)))
}

/// Raw payload (Mochy LRC dialect) from a single provider, ungraded — what Android stores.
pub async fn fetch_raw_from_provider(
    meta: &TrackMeta,
    provider: ProviderId,
    http: &reqwest::Client,
) -> LyricsResult<Option<String>> {
    providers::fetch_raw(provider, &providers::Query::from_meta(meta), http).await
}

// ---------------------------------------------------------------------------
// Parsing — pure functions, unit-testable (spec §3.4).
// ---------------------------------------------------------------------------

/// Parse the Mochy LRC dialect: `[mm:ss.mmm]` lines, next-line word timings
/// `<text:start:end>` (seconds), rich `<mm:ss.cc>word`, `{agent:X}`, `{bg}`.
/// All lines sharing one timestamp ⇒ `SyncKind::Plain`.
///
/// Text with no timestamps at all is returned as `Plain` lines at 0 s; blank input or
/// [`LYRICS_NOT_FOUND`] gives `(vec![], SyncKind::None)`. Lines are sorted by time; syllable
/// pieces glued together in the text are merged into whole words (Android `fixWordSpacing`).
pub fn parse_lrc(src: &str) -> (Vec<LyricLine>, SyncKind) {
    lrc::parse_lrc_impl(src)
}

/// Serialize lines back to the LRC dialect (for the `lyrics` DB table).
///
/// Format: `[mm:ss.mmm]{agent:X}{bg}text`, followed (for word-synced lines) by
/// `<word:start:end|…>` in seconds. Round-trips with [`parse_lrc`].
pub fn lines_to_lrc(lines: &[LyricLine]) -> String {
    lrc::lines_to_lrc_impl(lines)
}

/// Parse TTML (Apple Music style, BetterLyrics/Paxsenix) into lines.
///
/// Background-vocal spans become separate `bg` lines placed right after their parent line.
pub fn parse_ttml(xml: &str) -> LyricsResult<Vec<LyricLine>> {
    ttml::parse_ttml_impl(xml)
}

fn _ms(secs: f64) -> i64 {
    (secs * 1000.0).round() as i64
}

/// Index of the line active at `t_secs` with the 300 ms lead-in
/// (spec §3.4 `LineLeadInMs`); `offset_ms` shifts the query time
/// (positive shows lines earlier — Android `song.lyricsOffset`).
///
/// `None` before the first line lights up. After the last line has ended this still returns
/// the last index; use [`lyrics_end_secs`] to dim it during the outro (Android returns
/// `lines.size` there).
pub fn active_line_index(lines: &[LyricLine], t_secs: f64, offset_ms: i32) -> Option<usize> {
    let q = t_secs + offset_ms as f64 / 1000.0 + LINE_LEAD_IN_MS as f64 / 1000.0;
    lines.partition_point(|l| l.time <= q).checked_sub(1)
}

/// When line `index` stops being sung (seconds): its last word's end when word-synced,
/// otherwise the next line's start, otherwise its start + 5 s.
pub fn line_end_secs(lines: &[LyricLine], index: usize) -> f64 {
    let Some(line) = lines.get(index) else {
        return 0.0;
    };
    if let Some(w) = line.words.last() {
        return w.end;
    }
    if let Some(next) = lines.get(index + 1) {
        return next.time;
    }
    line.time + UNTIMED_LINE_DURATION_MS as f64 / 1000.0
}

/// Where the lyrics as a whole finish (seconds); 0 for no lines.
pub fn lyrics_end_secs(lines: &[LyricLine]) -> f64 {
    lines
        .len()
        .checked_sub(1)
        .map(|i| line_end_secs(lines, i))
        .unwrap_or(0.0)
}
