//! One Discord status worked out from the song and the user's settings
//! (Android `PresenceActivity`, `PresenceText`, `PresenceImages`, `NowPlaying`).

use mochy_core::integrations::{DiscordSettings, PresenceButtonTarget, PresenceImage, PresenceKind};
use serde_json::{Map, Value, json};

pub const APP_NAME: &str = "Mochy";
pub const REPO_URL: &str = "https://github.com/realidkroo/Mochy-Music";

/// Status badge and logo images live in the upstream repo (assets/discord,
/// drawn by scripts/discord_presence_images.py); Discord fetches them itself.
const ASSETS_BASE: &str = "https://raw.githubusercontent.com/realidkroo/Mochy-Music/main/assets/discord/v1";
pub const LOGO_URL: &str = "https://raw.githubusercontent.com/realidkroo/Mochy-Music/main/assets/discord/v1/mochy.png";

/// The ring on the play/pause badge moves in 5% steps.
pub const STATUS_STEPS: u32 = 20;

/// Times drift a little with every read of the player; only a jump counts.
const TIME_SLACK_MS: i64 = 2_500;

/// What the player is on, as the Discord status needs it.
#[derive(Debug, Clone, PartialEq)]
pub struct NowPlaying {
    pub media_id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub artwork_url: Option<String>,
    pub artist_artwork_url: Option<String>,
    pub duration_ms: i64,
    /// Where the song was at `read_at_ms`.
    pub position_ms: i64,
    pub read_at_ms: i64,
    pub speed: f32,
    pub is_playing: bool,
    /// "OPUS 160 kbps", or None until known.
    pub quality: Option<String>,
    /// `media_id` is a YouTube video id, so links can open this exact song.
    pub is_youtube: bool,
}

impl NowPlaying {
    pub fn position_at(&self, now_ms: i64) -> i64 {
        if !self.is_playing {
            return self.position_ms;
        }
        let moved = ((now_ms - self.read_at_ms) as f64 * self.speed as f64) as i64;
        (self.position_ms + moved).clamp(0, self.duration_ms.max(0))
    }

    pub fn progress_at(&self, now_ms: i64) -> f32 {
        if self.duration_ms <= 0 { 0.0 } else { self.position_at(now_ms) as f32 / self.duration_ms as f32 }
    }

    /// For a preview when nothing is playing.
    pub fn sample(now_ms: i64) -> Self {
        Self {
            media_id: String::new(),
            title: "Fake Plastic Trees".into(),
            artist: "Radiohead".into(),
            album: Some("The Bends".into()),
            artwork_url: None,
            artist_artwork_url: None,
            duration_ms: 290_000,
            position_ms: 96_000,
            read_at_ms: now_ms,
            speed: 1.0,
            is_playing: true,
            quality: Some("OPUS · 160 kbps".into()),
            is_youtube: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

/// The placeholders the user can drop into any line or button label.
pub const PLACEHOLDERS: [(&str, &str); 5] = [
    ("{name}", "Song name"),
    ("{artist}", "Artist name"),
    ("{album}", "Album"),
    ("{app}", APP_NAME),
    ("{quality}", "Format and bitrate"),
];

/// An empty-looking character that still counts as one (Discord's 2-letter minimum).
const BLANK: char = '\u{2800}';

pub fn fill(template: &str, t: &NowPlaying) -> String {
    template
        .replace("{name}", &t.title)
        .replace("{artist}", &t.artist)
        .replace("{album}", t.album.as_deref().unwrap_or(""))
        .replace("{app}", APP_NAME)
        .replace("{quality}", t.quality.as_deref().unwrap_or(""))
        .replace("{apple}", "")
}

/// A filled-in line as Discord takes it: None when empty, padded when one
/// letter, cut at Discord's limit with an ellipsis.
pub fn line(template: &str, t: &NowPlaying, max_len: usize) -> Option<String> {
    let filled = fill(template, t);
    let text = filled.split_whitespace().collect::<Vec<_>>().join(" ");
    let n = text.chars().count();
    if n == 0 {
        None
    } else if n == 1 {
        Some(format!("{text}{BLANK}"))
    } else if n > max_len {
        let cut: String = text.chars().take(max_len - 1).collect();
        Some(format!("{}…", cut.trim_end()))
    } else {
        Some(text)
    }
}

// ---------------------------------------------------------------------------
// Pictures
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum PresencePicture {
    Web(String),
    /// Play/pause badge with its ring `step` of [`STATUS_STEPS`] round.
    Status { step: u32, playing: bool },
    Logo,
}

impl PresencePicture {
    /// The address Discord fetches.
    pub fn url(&self) -> String {
        match self {
            PresencePicture::Web(u) => u.clone(),
            PresencePicture::Status { step, playing } => {
                let state = if *playing { "playing" } else { "paused" };
                format!("{ASSETS_BASE}/status_{state}_{step:02}.png")
            }
            PresencePicture::Logo => LOGO_URL.to_string(),
        }
    }
}

/// Only http(s) pictures can be fetched by Discord.
pub fn is_web_image(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

fn still(url: Option<&str>) -> Option<PresencePicture> {
    url.filter(|u| is_web_image(u)).map(|u| PresencePicture::Web(u.to_string()))
}

/// What `source` shows for `t`; missing artwork falls back to the logo.
pub fn pick_image(source: PresenceImage, t: &NowPlaying, now_ms: i64, custom: &str) -> Option<PresencePicture> {
    let artwork = t.artwork_url.as_deref().filter(|u| !u.is_empty());
    let artist_art = t.artist_artwork_url.as_deref().filter(|u| !u.is_empty());
    match source {
        PresenceImage::Off => None,
        PresenceImage::Artwork => Some(still(artwork).unwrap_or(PresencePicture::Logo)),
        PresenceImage::Artist => Some(still(artist_art.or(artwork)).unwrap_or(PresencePicture::Logo)),
        PresenceImage::AppLogo => Some(PresencePicture::Logo),
        PresenceImage::PlaybackStatus => Some(PresencePicture::Status {
            step: ((t.progress_at(now_ms) * STATUS_STEPS as f32).round() as i64).clamp(0, STATUS_STEPS as i64) as u32,
            playing: t.is_playing,
        }),
        PresenceImage::Custom => Some(still(Some(custom.trim()).filter(|c| !c.is_empty())).unwrap_or(PresencePicture::Logo)),
    }
}

// ---------------------------------------------------------------------------
// Links
// ---------------------------------------------------------------------------

/// Android `Uri.encode`: everything but `A-Za-z0-9_-!.~'()*` is %-escaped.
pub fn uri_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"_-!.~'()*".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The page a button opens for `t`: the song itself where known, a search otherwise.
pub fn link_for(target: PresenceButtonTarget, t: &NowPlaying) -> String {
    let query = uri_encode(format!("{} {}", t.title, t.artist).trim());
    match target {
        PresenceButtonTarget::YoutubeMusic => {
            if t.is_youtube {
                format!("https://music.youtube.com/watch?v={}", t.media_id)
            } else {
                format!("https://music.youtube.com/search?q={query}")
            }
        }
        PresenceButtonTarget::Spotify => format!("https://open.spotify.com/search/{query}"),
        PresenceButtonTarget::AppleMusic => format!("https://music.apple.com/search?term={query}"),
        PresenceButtonTarget::Tidal => format!("https://listen.tidal.com/search?q={query}"),
        PresenceButtonTarget::Mochy => REPO_URL.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Activity
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub label: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PresenceActivity {
    pub kind: PresenceKind,
    pub details: Option<String>,
    /// Where clicking the song name goes.
    pub details_url: Option<String>,
    pub state: Option<String>,
    pub large_image: Option<PresencePicture>,
    pub large_text: Option<String>,
    pub small_image: Option<PresencePicture>,
    pub small_text: Option<String>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub buttons: Vec<Link>,
}

impl PresenceActivity {
    pub fn build(t: &NowPlaying, s: &DiscordSettings, now_ms: i64) -> Self {
        let (start, end) = times_of(t, s.kind, now_ms);
        let large_source = if s.large_image.for_large_image() { s.large_image } else { PresenceImage::Artwork };
        Self {
            kind: s.kind,
            details: line(&s.details, t, 128),
            details_url: t.is_youtube.then(|| link_for(PresenceButtonTarget::YoutubeMusic, t)),
            state: line(&s.state, t, 128),
            large_image: pick_image(large_source, t, now_ms, &s.large_custom_image).or(Some(PresencePicture::Logo)),
            large_text: line(&s.large_text, t, 128),
            small_image: pick_image(s.small_image, t, now_ms, &s.small_custom_image),
            small_text: line(&s.small_text, t, 128),
            start_ms: start,
            end_ms: end,
            buttons: [&s.button1, &s.button2]
                .into_iter()
                .filter_map(|b| line(&b.text, t, 32).map(|label| Link { label, url: link_for(b.target, t) }))
                .collect(),
        }
    }

    /// Whether sending this instead of `other` would change anything visible.
    pub fn looks_like(&self, other: Option<&PresenceActivity>) -> bool {
        let Some(o) = other else { return false };
        fn near(a: Option<i64>, b: Option<i64>) -> bool {
            match (a, b) {
                (None, None) => true,
                (Some(a), Some(b)) => (a - b).abs() < TIME_SLACK_MS,
                _ => false,
            }
        }
        let strip = |a: &PresenceActivity| PresenceActivity { start_ms: None, end_ms: None, ..a.clone() };
        strip(self) == strip(o) && near(self.start_ms, o.start_ms) && near(self.end_ms, o.end_ms)
    }

    /// The activity as Discord's local RPC (`SET_ACTIVITY`) takes it. Pictures
    /// go as plain https addresses, which the RPC proxies itself.
    /// `with_details_url` lets the caller drop the newer field if an older
    /// Discord client rejects it.
    pub fn to_ipc_json(&self, with_details_url: bool) -> Value {
        let mut m = Map::new();
        m.insert("type".into(), json!(self.kind.discord_type()));
        if let Some(d) = &self.details {
            m.insert("details".into(), json!(d));
        }
        if with_details_url && let Some(u) = &self.details_url {
            m.insert("details_url".into(), json!(u));
        }
        if let Some(s) = &self.state {
            m.insert("state".into(), json!(s));
        }
        if let Some(start) = self.start_ms {
            let mut ts = Map::new();
            ts.insert("start".into(), json!(start));
            if let Some(end) = self.end_ms {
                ts.insert("end".into(), json!(end));
            }
            m.insert("timestamps".into(), Value::Object(ts));
        }
        // Discord only shows the small picture next to a big one.
        if let Some(large) = &self.large_image {
            let mut a = Map::new();
            a.insert("large_image".into(), json!(large.url()));
            if let Some(t) = &self.large_text {
                a.insert("large_text".into(), json!(t));
            }
            if let Some(small) = &self.small_image {
                a.insert("small_image".into(), json!(small.url()));
                if let Some(t) = &self.small_text {
                    a.insert("small_text".into(), json!(t));
                }
            }
            m.insert("assets".into(), Value::Object(a));
        }
        if !self.buttons.is_empty() {
            m.insert("buttons".into(), Value::Array(self.buttons.iter().map(|b| json!({"label": b.label, "url": b.url})).collect()));
        }
        m.insert("instance".into(), json!(false));
        Value::Object(m)
    }
}

/// Listening gets a bar from start to end; Playing counts up. A paused song
/// gets neither, or Discord would carry on moving the bar.
fn times_of(t: &NowPlaying, kind: PresenceKind, now_ms: i64) -> (Option<i64>, Option<i64>) {
    if !t.is_playing || t.duration_ms <= 0 {
        return (None, None);
    }
    let speed = t.speed.max(0.1) as f64;
    let start = now_ms - (t.position_at(now_ms) as f64 / speed) as i64;
    let end = start + (t.duration_ms as f64 / speed) as i64;
    (Some(start), (kind == PresenceKind::Listening).then_some(end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(now: i64) -> NowPlaying {
        NowPlaying {
            media_id: "abcdefghijk".into(),
            title: "Song".into(),
            artist: "Band".into(),
            album: None,
            artwork_url: Some("https://lh3.googleusercontent.com/x=w544-h544".into()),
            artist_artwork_url: None,
            duration_ms: 200_000,
            position_ms: 50_000,
            read_at_ms: now,
            speed: 1.0,
            is_playing: true,
            quality: Some("OPUS 160 kbps".into()),
            is_youtube: true,
        }
    }

    #[test]
    fn lines_follow_discord_rules() {
        let t = track(0);
        assert_eq!(line("{name}", &t, 128).as_deref(), Some("Song"));
        assert_eq!(line("  {album}  ", &t, 128), None);
        assert_eq!(line("x", &t, 128).unwrap().chars().count(), 2);
        let long = "a".repeat(40);
        let cut = line(&long, &t, 32).unwrap();
        assert_eq!(cut.chars().count(), 32);
        assert!(cut.ends_with('…'));
        assert_eq!(line("{artist}  on   {app}", &t, 128).as_deref(), Some("Band on Mochy"));
    }

    #[test]
    fn build_listening_activity() {
        let now = 1_000_000;
        let s = DiscordSettings::default();
        let a = PresenceActivity::build(&track(now), &s, now);
        assert_eq!(a.details.as_deref(), Some("Song"));
        assert_eq!(a.state.as_deref(), Some("Band"));
        assert_eq!(a.start_ms, Some(now - 50_000));
        assert_eq!(a.end_ms, Some(now + 150_000));
        assert_eq!(a.large_image, Some(PresencePicture::Web("https://lh3.googleusercontent.com/x=w544-h544".into())));
        assert_eq!(a.small_image, Some(PresencePicture::Status { step: 5, playing: true }));
        assert_eq!(a.buttons.len(), 2);
        assert_eq!(a.buttons[0].url, "https://music.youtube.com/watch?v=abcdefghijk");
        assert_eq!(a.buttons[1].url, REPO_URL);
        let j = a.to_ipc_json(true);
        assert_eq!(j["type"], 2);
        assert_eq!(j["timestamps"]["end"], now + 150_000);
        assert_eq!(j["assets"]["small_image"], format!("{ASSETS_BASE}/status_playing_05.png"));
        assert_eq!(j["buttons"][0]["label"], "See on YouTube Music");
        assert_eq!(j["details_url"], "https://music.youtube.com/watch?v=abcdefghijk");
        assert!(a.to_ipc_json(false).get("details_url").is_none());
    }

    #[test]
    fn paused_and_playing_kinds() {
        let now = 5_000;
        let mut t = track(now);
        t.is_playing = false;
        let a = PresenceActivity::build(&t, &DiscordSettings::default(), now);
        assert_eq!((a.start_ms, a.end_ms), (None, None));
        assert!(a.to_ipc_json(true).get("timestamps").is_none());
        let s = DiscordSettings { kind: PresenceKind::Playing, ..Default::default() };
        let b = PresenceActivity::build(&track(now), &s, now);
        assert!(b.start_ms.is_some() && b.end_ms.is_none());
        assert_eq!(b.to_ipc_json(true)["type"], 0);
    }

    #[test]
    fn looks_like_ignores_small_drift() {
        let now = 100_000;
        let s = DiscordSettings::default();
        let a = PresenceActivity::build(&track(now), &s, now);
        let mut drift = a.clone();
        drift.start_ms = drift.start_ms.map(|v| v + 1_000);
        assert!(drift.looks_like(Some(&a)));
        let mut seek = a.clone();
        seek.start_ms = seek.start_ms.map(|v| v + 10_000);
        assert!(!seek.looks_like(Some(&a)));
        assert!(!a.looks_like(None));
    }

    #[test]
    fn non_youtube_links_search_and_missing_art_uses_logo() {
        let mut t = track(0);
        t.is_youtube = false;
        t.artwork_url = None;
        t.title = "Café & Co".into();
        assert_eq!(link_for(PresenceButtonTarget::YoutubeMusic, &t), "https://music.youtube.com/search?q=Caf%C3%A9%20%26%20Co%20Band");
        assert_eq!(link_for(PresenceButtonTarget::Spotify, &t), "https://open.spotify.com/search/Caf%C3%A9%20%26%20Co%20Band");
        let a = PresenceActivity::build(&t, &DiscordSettings::default(), 0);
        assert_eq!(a.large_image, Some(PresencePicture::Logo));
        assert!(a.details_url.is_none());
        assert_eq!(pick_image(PresenceImage::Custom, &t, 0, "file://x.png"), Some(PresencePicture::Logo));
        assert_eq!(pick_image(PresenceImage::Custom, &t, 0, "https://x/y.png"), Some(PresencePicture::Web("https://x/y.png".into())));
        assert_eq!(pick_image(PresenceImage::Off, &t, 0, ""), None);
    }
}
