//! Finds the YouTube Music song for a track from another service (Android
//! `importer/TrackMatcher.kt`, ported exactly; spec §6.2). Scored on title,
//! artist, length and album; a song beats a music video unless none is close;
//! one matcher per import so repeats search once.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use mochy_core::models::{ArtistRef, Song};
use regex::Regex;
use unicode_normalization::UnicodeNormalization;

use crate::matches::ImportMatches;
use crate::models::{ImportSource, ImportedTrack};

const SONG_ACCEPT: f64 = 0.62;
const VIDEO_ACCEPT: f64 = 0.7;
const SONG_FALLBACK_ACCEPT: f64 = 0.5;
const MIN_TITLE: f64 = 0.45;
/// Within this many seconds of the source's length, a result is taken to be the same recording.
pub const CLOSE_SECONDS: f64 = 3.0;
const VERSION_PENALTY: f64 = 0.3;

const VERSION_WORDS: [&str; 28] = [
    "remix", "live", "instrumental", "inst", "acoustic", "karaoke", "cover", "sped up", "slowed", "nightcore", "reverb", "8d",
    "stage", "dance practice", "practice", "performance", "fancam", "concert", "rehearsal", "reaction", "1 hour", "hour", "loop",
    "extended", "japanese", "korean", "english", "chinese",
];
const LIVE_ALBUM_WORDS: [&str; 6] = ["live", "tour", "concert", "unplugged", "session", "sessions"];

static NOISE_IN_BRACKETS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)[(\[][^)\]]*\b(feat|ft|with|remaster|remastered|version|edit|mono|stereo|deluxe|explicit|clean|bonus|from|official|audio|video|lyrics?|visuali[sz]er|hd|hq|4k)\b[^)\]]*[)\]]",
    )
    .unwrap()
});
static DASH_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s[-–—]\s.*(remaster|version|edit|mono|stereo|mix\b|from |live at|bonus).*$").unwrap());
static FEAT_TAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s(feat\.?|ft\.?|featuring)\s.*$").unwrap());
static ARTIST_SPLIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s*(,|&|;|/|\sx\s|\sand\s|\sfeat\.?\s|\sft\.?\s|\sfeaturing\s|\swith\s)\s*").unwrap()
});
static NON_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}]+").unwrap());
static MARKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\p{M}+").unwrap());
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

pub fn split_artists(artist: &str) -> Vec<String> {
    let a = artist.replace('\u{00A0}', " ");
    ARTIST_SPLIT.split(&a).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

fn clean_title_for_search(title: &str) -> String {
    let t = NOISE_IN_BRACKETS.replace_all(title, " ");
    let t = DASH_SUFFIX.replace_all(&t, "");
    let t = FEAT_TAIL.replace_all(&t, "");
    let t = SPACES.replace_all(&t, " ");
    let t = t.trim();
    if t.is_empty() { title.to_string() } else { t.to_string() }
}

pub fn normalize_title(title: &str) -> String {
    normalize_text(&clean_title_for_search(title))
}

/// Lowercase, NFKD, marks stripped, non-letters/digits to single spaces.
pub fn normalize_text(text: &str) -> String {
    let lower = text.to_lowercase();
    let folded: String = lower.nfkd().collect();
    let folded = MARKS.replace_all(&folded, "");
    NON_WORD.replace_all(&folded, " ").trim().to_string()
}

/// Best of token overlap and character edit distance, so both reordered words
/// and scripts without spaces score fairly.
pub fn similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    let ta: std::collections::HashSet<&str> = a.split(' ').collect();
    let tb: std::collections::HashSet<&str> = b.split(' ').collect();
    let inter = ta.intersection(&tb).count();
    let jaccard = inter as f64 / (ta.len() + tb.len() - inter) as f64;
    let ca: Vec<char> = a.chars().filter(|c| *c != ' ').collect();
    let cb: Vec<char> = b.chars().filter(|c| *c != ' ').collect();
    let longest = ca.len().max(cb.len());
    let edit = if longest == 0 { 0.0 } else { 1.0 - levenshtein(&ca, &cb) as f64 / longest as f64 };
    // one title fully containing the other ("song" vs "song pt. 1") is close but not exact
    let contains = |x: &[char], y: &[char]| y.is_empty() || x.windows(y.len()).any(|w| w == y);
    let containment = if ca.len() >= 3 && cb.len() >= 3 && (contains(&ca, &cb) || contains(&cb, &ca)) { 0.8 } else { 0.0 };
    jaccard.max(edit).max(containment)
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    if a.len() > 200 || b.len() > 200 {
        return a.len().max(b.len()) - a.len().min(b.len());
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        curr[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            curr[j] = (curr[j - 1] + 1).min(prev[j] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// YouTube's music video type, as far as the desktop parser exposes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoType {
    /// The artist's own audio ("topic" track): `MUSIC_VIDEO_TYPE_ATV`.
    Atv,
    /// A fan upload: `MUSIC_VIDEO_TYPE_UGC`.
    Ugc,
    /// Some other known type (official video …).
    Other,
    /// Not reported.
    Unknown,
}

/// What a search result looks like to the scorer.
#[derive(Debug, Clone)]
pub struct CandidateInfo<'a> {
    pub title: &'a str,
    pub artists: Vec<&'a str>,
    pub album: Option<&'a str>,
    pub duration_secs: Option<i64>,
    pub video_type: VideoType,
}

impl<'a> CandidateInfo<'a> {
    /// From a desktop search result. The desktop parser only tells "audio
    /// track" (ATV / uploads) from "video" (OMV / UGC / …), so a song result
    /// counts as ATV and a video result as an unspecified other type.
    pub fn from_song(s: &'a Song) -> Self {
        Self {
            title: &s.title,
            artists: s.artists.iter().map(|a| a.name.as_str()).collect(),
            album: s.album_name.as_deref(),
            duration_secs: s.duration_secs.filter(|d| *d > 0.0).map(|d| d.round() as i64),
            video_type: if s.is_video { VideoType::Other } else { VideoType::Atv },
        }
    }
}

pub fn score(track: &ImportedTrack, c: &CandidateInfo) -> f64 {
    let target_title = normalize_title(&track.title);
    let candidate_title = normalize_title(c.title);
    let mut title_score = similarity(&target_title, &candidate_title);

    let target_artists: Vec<String> = split_artists(&track.artist).iter().map(|a| normalize_text(a)).filter(|a| !a.is_empty()).collect();
    let candidate_artists: Vec<String> = c.artists.iter().map(|a| normalize_text(a)).filter(|a| !a.is_empty()).collect();
    let mut artist_score = if target_artists.is_empty() {
        0.5
    } else {
        target_artists
            .iter()
            .map(|t| candidate_artists.iter().map(|ca| similarity(t, ca)).fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v)))).unwrap_or(0.0))
            .fold(f64::MIN, f64::max)
    };

    // videos are often titled "artist - title (official video)"
    if artist_score < 0.6 && target_artists.iter().any(|a| !a.is_empty() && candidate_title.contains(a.as_str())) {
        artist_score = 0.8;
        let stripped = target_artists.iter().fold(candidate_title.clone(), |acc, a| acc.replace(a.as_str(), " "));
        title_score = title_score.max(similarity(&target_title, stripped.trim()));
    }

    let duration_score = duration_score(track.duration_ms, c.duration_secs);

    // a remix/live/stage take the source didn't ask for is a different song; whole words
    // on the raw titles since cleaning drops live markers and "live" is inside "alive"
    let raw_target = format!(" {} ", normalize_text(&track.title));
    let raw_candidate = format!(" {} ", normalize_text(c.title));
    let version_penalty = VERSION_WORDS
        .iter()
        .filter(|w| {
            let needle = format!(" {w} ");
            raw_candidate.contains(&needle) != raw_target.contains(&needle)
        })
        .count() as f64
        * VERSION_PENALTY;

    if title_score < MIN_TITLE {
        return 0.0;
    }
    title_score * 0.5 + artist_score * 0.25 + duration_score * 0.25 + album_score(track.album.as_deref(), c.album) + upload_score(c)
        - version_penalty
}

/// Small lift for the same album and a penalty for a live/tour album the source isn't on.
fn album_score(source_album: Option<&str>, candidate_album: Option<&str>) -> f64 {
    let Some(candidate_album) = candidate_album.filter(|a| !a.trim().is_empty()) else { return 0.0 };
    let candidate = format!(" {} ", normalize_text(candidate_album));
    let source = format!(" {} ", normalize_text(source_album.unwrap_or("")));
    let live_album = LIVE_ALBUM_WORDS.iter().any(|w| {
        let n = format!(" {w} ");
        candidate.contains(&n) && !source.contains(&n)
    });
    if live_album {
        return -VERSION_PENALTY;
    }
    if source_album.is_none_or(|s| s.trim().is_empty()) {
        return 0.0;
    }
    if similarity(source.trim(), candidate.trim()) >= 0.8 { 0.05 } else { 0.0 }
}

/// The artist's own audio is the album version; a fan upload often isn't.
fn upload_score(c: &CandidateInfo) -> f64 {
    match c.video_type {
        VideoType::Atv => 0.05,
        VideoType::Ugc => -0.08,
        VideoType::Unknown => {
            if c.album.is_some() {
                0.05
            } else {
                0.0
            }
        }
        VideoType::Other => 0.0,
    }
}

fn duration_score(source_ms: Option<i64>, candidate_secs: Option<i64>) -> f64 {
    let (Some(ms), Some(cs)) = (source_ms.filter(|m| *m > 0), candidate_secs.filter(|c| *c > 0)) else { return 0.5 };
    let diff = (ms as f64 / 1000.0 - cs as f64).abs();
    if diff <= CLOSE_SECONDS {
        1.0
    } else if diff <= 8.0 {
        0.6
    } else if diff <= 20.0 {
        0.1
    } else if diff <= 60.0 {
        -0.3
    } else {
        -1.0
    }
}

/// Best result at or above `accept`, looking first at ones within a few
/// seconds of the track's length.
pub fn pick<'s>(track: &ImportedTrack, scored: &'s [(Song, f64)], accept: f64) -> Option<&'s Song> {
    let max_by = |it: &mut dyn Iterator<Item = &'s (Song, f64)>| -> Option<&'s (Song, f64)> {
        // maxByOrNull keeps the first of equal maxima
        let mut best: Option<&(Song, f64)> = None;
        for x in it {
            if best.is_none_or(|b| x.1 > b.1) {
                best = Some(x);
            }
        }
        best
    };
    if let Some(src) = track.duration_ms.filter(|d| *d > 0).map(|d| d as f64 / 1000.0) {
        let mut close = scored.iter().filter(|(s, _)| {
            CandidateInfo::from_song(s).duration_secs.is_some_and(|d| (d as f64 - src).abs() <= CLOSE_SECONDS)
        });
        if let Some(b) = max_by(&mut close).filter(|b| b.1 >= accept) {
            return Some(&b.0);
        }
    }
    max_by(&mut scored.iter()).filter(|b| b.1 >= accept).map(|b| &b.0)
}

pub fn search_query(track: &ImportedTrack) -> String {
    let first = split_artists(&track.artist).into_iter().next().unwrap_or_default();
    format!("{} {}", clean_title_for_search(&track.title), first).trim().to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFilter {
    Song,
    Video,
}

/// The search the matcher runs (YouTube Music in the app, a fake in tests).
pub trait SongSearch: Send + Sync {
    fn search(&self, query: &str, filter: SearchFilter) -> impl Future<Output = Result<Vec<Song>, String>> + Send;
}

impl SongSearch for Arc<mochy_innertube::YtmClient> {
    async fn search(&self, query: &str, filter: SearchFilter) -> Result<Vec<Song>, String> {
        let params = match filter {
            SearchFilter::Song => mochy_innertube::search_filters::SONG,
            SearchFilter::Video => mochy_innertube::search_filters::VIDEO,
        };
        self.search_songs(query, params).await.map_err(|e| e.to_string())
    }
}

/// A song for a track that already carries its YouTube id.
pub fn song_for_video_id(track: &ImportedTrack, id: &str) -> Song {
    let mut s = Song::remote(id, track.title.clone());
    s.artists = split_artists(&track.artist).into_iter().map(ArtistRef::new).collect();
    s.duration_secs = track.duration_ms.map(|ms| (ms / 1000) as f64).filter(|d| *d > 0.0);
    s.thumbnail_url = Some(track.thumbnail_url.clone().unwrap_or_else(|| format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg")));
    s
}

pub struct TrackMatcher<S: SongSearch> {
    search: S,
    matches: Option<Arc<ImportMatches>>,
    source: ImportSource,
    cache: Mutex<HashMap<String, Option<Song>>>,
}

impl<S: SongSearch> TrackMatcher<S> {
    pub fn new(search: S, matches: Option<Arc<ImportMatches>>, source: ImportSource) -> Self {
        Self { search, matches, source, cache: Mutex::new(HashMap::new()) }
    }

    pub async fn match_track(&self, track: &ImportedTrack) -> Option<Song> {
        if let Some(item) = &track.yt_item {
            return Some(item.clone());
        }
        if let Some(id) = &track.video_id {
            return Some(song_for_video_id(track, id));
        }
        if let Some(m) = self.matches.as_ref().and_then(|m| m.pinned(track)) {
            return Some(m.pinned_song());
        }
        let key = format!(
            "{}|{}|{}",
            normalize_title(&track.title),
            normalize_text(&track.artist),
            track.duration_ms.map(|d| (d / 5000).to_string()).unwrap_or_else(|| "null".into())
        );
        let cached = self.cache.lock().unwrap().get(&key).cloned();
        let result = match cached {
            Some(r) => r,
            None => {
                let r = self.find_best(track).await;
                self.cache.lock().unwrap().insert(key, r.clone());
                r
            }
        };
        if let (Some(r), Some(m)) = (&result, &self.matches) {
            m.remember(track, self.source, r);
        }
        result
    }

    /// Writes the matches noted so far (once per batch).
    pub fn flush(&self) {
        if let Some(m) = &self.matches {
            m.flush();
        }
    }

    /// Everything a search turns up for `track`, best fit first ("choose different version").
    pub async fn candidates(&self, track: &ImportedTrack) -> Vec<(Song, f64)> {
        let q = search_query(track);
        let mut found = self.search(&q, SearchFilter::Song).await;
        found.extend(self.search(&q, SearchFilter::Video).await);
        let mut seen = std::collections::HashSet::new();
        let mut out: Vec<(Song, f64)> = found
            .into_iter()
            .filter(|s| seen.insert(s.id.clone()))
            .map(|s| {
                let sc = score(track, &CandidateInfo::from_song(&s));
                (s, sc)
            })
            .collect();
        out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        out
    }

    async fn find_best(&self, track: &ImportedTrack) -> Option<Song> {
        let q = search_query(track);
        let scored = |list: Vec<Song>| -> Vec<(Song, f64)> {
            list.into_iter()
                .map(|s| {
                    let sc = score(track, &CandidateInfo::from_song(&s));
                    (s, sc)
                })
                .collect()
        };
        let songs = scored(self.search(&q, SearchFilter::Song).await);
        if let Some(s) = pick(track, &songs, SONG_ACCEPT) {
            return Some(s.clone());
        }
        // some tracks only exist as uploads/videos on youtube
        let videos = scored(self.search(&q, SearchFilter::Video).await);
        if let Some(s) = pick(track, &videos, VIDEO_ACCEPT) {
            return Some(s.clone());
        }
        pick(track, &songs, SONG_FALLBACK_ACCEPT).cloned()
    }

    /// Two tries, 800 ms apart; top 10 results.
    async fn search(&self, q: &str, filter: SearchFilter) -> Vec<Song> {
        for attempt in 0..2 {
            match self.search.search(q, filter).await {
                Ok(mut r) => {
                    r.truncate(10);
                    return r;
                }
                Err(e) => {
                    tracing::debug!("search {q:?} failed: {e}");
                    if attempt == 0 {
                        tokio::time::sleep(Duration::from_millis(800)).await;
                    }
                }
            }
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Port of `TrackMatcherTest`.
    #[test]
    fn title_noise_is_stripped() {
        assert_eq!(normalize_title("Yesterday - Remastered 2009"), "yesterday");
        assert_eq!(normalize_title("Solar Eclipse (feat. Don Toliver)"), "solar eclipse");
        assert_eq!(normalize_title("Song [Official Music Video]"), "song");
        assert_eq!(normalize_title("Song feat. Someone"), "song");
        // a remix is a different song, so it must survive cleaning
        assert_eq!(normalize_title("Song (Remix)"), "song remix");
        assert_eq!(normalize_text("Café"), "cafe");
    }

    #[test]
    fn artists_split_on_common_separators() {
        assert_eq!(split_artists("Pitbull, Sensato"), vec!["Pitbull", "Sensato"]);
        assert_eq!(split_artists("Drake & Don Toliver"), vec!["Drake", "Don Toliver"]);
        assert_eq!(split_artists("A feat. B, C"), vec!["A", "B", "C"]);
    }

    #[test]
    fn similarity_handles_scripts_without_spaces() {
        assert_eq!(similarity("夜に駆ける", "夜に駆ける"), 1.0);
        assert!(similarity("夜に駆ける", "夜に駆けるyoasobi") >= 0.8);
        assert!(similarity("blinding lights", "lights blinding") >= 0.99);
        assert!(similarity("blinding lights", "save your tears") < 0.4);
    }

    // ---- desktop additions --------------------------------------------------------

    fn song(id: &str, title: &str, artist: &str, secs: f64, video: bool, album: Option<&str>) -> Song {
        let mut s = Song::remote(id, title);
        s.artists = vec![ArtistRef::new(artist)];
        s.duration_secs = Some(secs);
        s.is_video = video;
        s.album_name = album.map(str::to_string);
        s
    }

    fn track(title: &str, artist: &str, secs: i64) -> ImportedTrack {
        ImportedTrack { duration_ms: Some(secs * 1000), ..ImportedTrack::new(title, artist, None) }
    }

    #[test]
    fn score_terms() {
        let t = track("Blinding Lights", "The Weeknd", 200);
        let exact = song("a", "Blinding Lights", "The Weeknd", 200.0, false, Some("After Hours"));
        // 0.5 + 0.25 + 0.25 + 0 (no source album) + 0.05 (ATV)
        assert!((score(&t, &CandidateInfo::from_song(&exact)) - 1.05).abs() < 1e-9);
        let remix = song("b", "Blinding Lights (Remix)", "The Weeknd", 200.0, false, None);
        assert!(score(&t, &CandidateInfo::from_song(&remix)) < 0.8);
        let live_album = song("c", "Blinding Lights", "The Weeknd", 200.0, false, Some("Live at SoFi Stadium"));
        assert!((score(&t, &CandidateInfo::from_song(&live_album)) - 0.75).abs() < 1e-9);
        let far = song("d", "Blinding Lights", "The Weeknd", 400.0, false, None);
        // duration -1.0 → 0.5 + 0.25 - 0.25 + 0.05
        assert!((score(&t, &CandidateInfo::from_song(&far)) - 0.55).abs() < 1e-9);
        let other = song("e", "Save Your Tears", "The Weeknd", 200.0, false, None);
        assert_eq!(score(&t, &CandidateInfo::from_song(&other)), 0.0);
        // "Artist - Title (Official Video)" uploaded by someone else
        let vid = song("f", "The Weeknd - Blinding Lights (Official Video)", "Some Channel", 201.0, true, None);
        let s = score(&t, &CandidateInfo::from_song(&vid));
        assert!(s > 0.7, "{s}");
        // "alive" isn't "live"
        let alive = track("Alive", "Band", 180);
        assert!(score(&alive, &CandidateInfo::from_song(&song("g", "Alive", "Band", 180.0, false, None))) > 1.0);
    }

    #[test]
    fn pick_prefers_close_length() {
        let t = track("Song", "Band", 200);
        let scored = vec![
            (song("long", "Song", "Band", 260.0, false, None), 0.9),
            (song("close", "Song", "Band", 201.0, false, None), 0.8),
        ];
        assert_eq!(pick(&t, &scored, SONG_ACCEPT).unwrap().id, "close");
        assert!(pick(&t, &scored, 0.95).is_none());
        let no_len = ImportedTrack::new("Song", "Band", None);
        assert_eq!(pick(&no_len, &scored, SONG_ACCEPT).unwrap().id, "long");
    }

    struct Fake {
        songs: Vec<Song>,
        videos: Vec<Song>,
        calls: Mutex<usize>,
    }

    impl SongSearch for Fake {
        async fn search(&self, _q: &str, f: SearchFilter) -> Result<Vec<Song>, String> {
            *self.calls.lock().unwrap() += 1;
            Ok(match f {
                SearchFilter::Song => self.songs.clone(),
                SearchFilter::Video => self.videos.clone(),
            })
        }
    }

    #[tokio::test]
    async fn matcher_order_cache_and_ids() {
        let fake = Fake {
            songs: vec![song("wrong", "Different Track", "Band", 200.0, false, None)],
            videos: vec![song("vid", "Song (Official Video)", "Band", 201.0, true, None)],
            calls: Mutex::new(0),
        };
        let m = TrackMatcher::new(fake, None, ImportSource::Other);
        let t = track("Song", "Band", 200);
        assert_eq!(m.match_track(&t).await.unwrap().id, "vid");
        assert_eq!(*m.search.calls.lock().unwrap(), 2);
        // Same track again: cached, no new search.
        assert_eq!(m.match_track(&t).await.unwrap().id, "vid");
        assert_eq!(*m.search.calls.lock().unwrap(), 2);
        // A known id needs no search at all.
        let with_id = ImportedTrack { video_id: Some("dQw4w9WgXcQ".into()), ..track("X", "A, B", 10) };
        let s = m.match_track(&with_id).await.unwrap();
        assert_eq!(s.id, "dQw4w9WgXcQ");
        assert_eq!(s.artists.len(), 2);
        assert_eq!(s.thumbnail_url.as_deref(), Some("https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"));
        assert_eq!(*m.search.calls.lock().unwrap(), 2);
        let c = m.candidates(&t).await;
        assert_eq!(c[0].0.id, "vid");
    }

    /// Live: match a few real tracks on YouTube Music.
    /// `cargo test -p mochy-importer -- --ignored live_`
    #[tokio::test]
    #[ignore]
    async fn live_match_on_youtube_music() {
        let ytm = Arc::new(mochy_innertube::YtmClient::new().await.unwrap());
        let m = TrackMatcher::new(ytm, None, ImportSource::Spotify);
        for (title, artist, secs) in [("Blinding Lights", "The Weeknd", 200), ("Get Lucky (feat. Pharrell Williams)", "Daft Punk, Pharrell Williams", 369), ("夜に駆ける", "YOASOBI", 261)] {
            let r = m.match_track(&track(title, artist, secs)).await;
            eprintln!("{title} -> {:?}", r.as_ref().map(|s| (&s.id, &s.title, s.artist_name(), s.duration_secs)));
            assert!(r.is_some(), "{title} not matched");
        }
    }

    #[test]
    fn query_uses_clean_title_and_first_artist() {
        assert_eq!(search_query(&ImportedTrack::new("Starboy (feat. Daft Punk)", "The Weeknd, Daft Punk", None)), "Starboy The Weeknd");
        assert_eq!(search_query(&ImportedTrack::new("Song", "", None)), "Song");
    }
}
