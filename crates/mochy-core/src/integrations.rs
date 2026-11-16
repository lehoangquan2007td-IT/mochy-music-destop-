//! Settings for the integrations area: Discord Rich Presence and
//! Last.fm / Libre.fm scrobbling (spec §8). Kept in its own file so the
//! integration feature only adds one field to [`crate::settings::AppSettings`].
//!
//! Secrets (API keys/secrets, session keys) are entered by the user and stored
//! only in the local settings file. Nothing here ships with a default secret.

use serde::{Deserialize, Serialize};

/// Root of the integrations settings (`AppSettings::integrations`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct IntegrationSettings {
    pub discord: DiscordSettings,
    pub scrobble: ScrobbleSettings,
}

// ---------------------------------------------------------------------------
// Discord
// ---------------------------------------------------------------------------

/// The line Discord writes above the status (Android `PresenceKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PresenceKind {
    #[default]
    Listening,
    Playing,
}

impl PresenceKind {
    /// Discord activity type number.
    pub fn discord_type(self) -> u8 {
        match self {
            PresenceKind::Listening => 2,
            PresenceKind::Playing => 0,
        }
    }
}

/// What a status picture shows (Android `PresenceImage`, minus the rotating
/// GIF variants that need a third-party upload host).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PresenceImage {
    Off,
    #[default]
    Artwork,
    Artist,
    PlaybackStatus,
    AppLogo,
    /// A picture URL the user typed in.
    Custom,
}

impl PresenceImage {
    pub const ALL: [PresenceImage; 6] = [
        PresenceImage::Off,
        PresenceImage::Artwork,
        PresenceImage::Artist,
        PresenceImage::PlaybackStatus,
        PresenceImage::AppLogo,
        PresenceImage::Custom,
    ];

    /// Usable for the big picture (Android `forLargeImage`).
    pub fn for_large_image(self) -> bool {
        matches!(self, PresenceImage::Artwork | PresenceImage::Artist | PresenceImage::AppLogo | PresenceImage::Custom)
    }

    pub fn label(self) -> &'static str {
        match self {
            PresenceImage::Off => "Turn it off",
            PresenceImage::Artwork => "Music artwork",
            PresenceImage::Artist => "Artist artwork",
            PresenceImage::PlaybackStatus => "Playback status",
            PresenceImage::AppLogo => "Mochy logo",
            PresenceImage::Custom => "Your own picture",
        }
    }
}

/// Where a status button takes whoever presses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PresenceButtonTarget {
    #[default]
    YoutubeMusic,
    Spotify,
    AppleMusic,
    Tidal,
    Mochy,
}

impl PresenceButtonTarget {
    pub const ALL: [PresenceButtonTarget; 5] = [
        PresenceButtonTarget::YoutubeMusic,
        PresenceButtonTarget::Spotify,
        PresenceButtonTarget::AppleMusic,
        PresenceButtonTarget::Tidal,
        PresenceButtonTarget::Mochy,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PresenceButtonTarget::YoutubeMusic => "YouTube Music",
            PresenceButtonTarget::Spotify => "Spotify",
            PresenceButtonTarget::AppleMusic => "Apple Music",
            PresenceButtonTarget::Tidal => "Tidal",
            PresenceButtonTarget::Mochy => "Mochy on GitHub",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PresenceButton {
    /// Label template (placeholders allowed); empty hides the button.
    pub text: String,
    pub target: PresenceButtonTarget,
}

impl Default for PresenceButton {
    fn default() -> Self {
        Self { text: String::new(), target: PresenceButtonTarget::YoutubeMusic }
    }
}

/// Discord Rich Presence (Android `PresenceSettings`). Desktop talks to the
/// local Discord client over IPC, which needs a Discord application id; the
/// upstream build injects one at build time, so it is user-entered here and
/// presence stays off until it is set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscordSettings {
    pub enabled: bool,
    /// Discord application (client) id, digits only.
    pub application_id: String,
    pub kind: PresenceKind,
    /// Seconds between status refreshes (1–10).
    pub refresh_seconds: u32,
    pub details: String,
    pub state: String,
    pub large_text: String,
    pub small_text: String,
    pub large_image: PresenceImage,
    pub small_image: PresenceImage,
    pub button1: PresenceButton,
    pub button2: PresenceButton,
    /// Take the status down the moment the music pauses.
    pub clear_on_pause: bool,
    /// `https://` picture used by [`PresenceImage::Custom`].
    pub large_custom_image: String,
    pub small_custom_image: String,
}

impl DiscordSettings {
    pub const MIN_REFRESH_SECONDS: u32 = 1;
    pub const MAX_REFRESH_SECONDS: u32 = 10;
    pub const DEFAULT_REFRESH_SECONDS: u32 = 3;
}

impl Default for DiscordSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            application_id: String::new(),
            kind: PresenceKind::Listening,
            refresh_seconds: Self::DEFAULT_REFRESH_SECONDS,
            details: "{name}".into(),
            state: "{artist}".into(),
            large_text: String::new(),
            small_text: "{quality}".into(),
            large_image: PresenceImage::Artwork,
            small_image: PresenceImage::PlaybackStatus,
            button1: PresenceButton { text: "See on YouTube Music".into(), target: PresenceButtonTarget::YoutubeMusic },
            button2: PresenceButton { text: "Open Mochy".into(), target: PresenceButtonTarget::Mochy },
            clear_on_pause: false,
            large_custom_image: String::new(),
            small_custom_image: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Scrobbling
// ---------------------------------------------------------------------------

/// One Audioscrobbler-compatible account (Last.fm or Libre.fm).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ScrobbleAccount {
    /// API key / shared secret of the user's own API account.
    pub api_key: String,
    pub api_secret: String,
    /// Session key from sign-in; empty = signed out.
    pub session_key: String,
    pub username: String,
    /// Scrobbling on or paused, with the account kept either way.
    pub enabled: bool,
}

impl ScrobbleAccount {
    pub fn is_configured(&self) -> bool {
        !self.api_key.trim().is_empty() && !self.api_secret.trim().is_empty()
    }
    pub fn is_signed_in(&self) -> bool {
        !self.session_key.is_empty()
    }
}

/// Scrobbling (Android `lastfm*`, `librefm*`, `scrobble*` keys).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScrobbleSettings {
    pub lastfm: ScrobbleAccount,
    pub librefm: ScrobbleAccount,
    /// `lastfmUseNowPlaying` (default on).
    pub use_now_playing: bool,
    /// `lastfmUseSendLikes`: a like here becomes a love there (default off).
    pub send_likes: bool,
    /// `scrobbleDelayPercent`.
    pub delay_percent: f32,
    /// `scrobbleMinSongDuration` (seconds).
    pub min_song_seconds: u32,
    /// `scrobbleDelaySeconds`.
    pub delay_seconds: u32,
}

impl ScrobbleSettings {
    pub const DEFAULT_MIN_SONG_SECONDS: u32 = 30;
    pub const DEFAULT_PLAYED_FRACTION: f32 = 0.5;
    pub const DEFAULT_MAX_WAIT_SECONDS: u32 = 240;
}

impl Default for ScrobbleSettings {
    fn default() -> Self {
        Self {
            lastfm: ScrobbleAccount::default(),
            librefm: ScrobbleAccount::default(),
            use_now_playing: true,
            send_likes: false,
            delay_percent: Self::DEFAULT_PLAYED_FRACTION,
            min_song_seconds: Self::DEFAULT_MIN_SONG_SECONDS,
            delay_seconds: Self::DEFAULT_MAX_WAIT_SECONDS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_roundtrip() {
        let s = IntegrationSettings::default();
        assert!(!s.discord.enabled);
        assert!(s.discord.application_id.is_empty());
        assert_eq!(s.discord.details, "{name}");
        assert!(s.scrobble.use_now_playing);
        assert_eq!(s.scrobble.min_song_seconds, 30);
        let json = serde_json::to_string(&s).unwrap();
        let back: IntegrationSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
        // Missing fields fall back to defaults.
        let partial: IntegrationSettings = serde_json::from_str(r#"{"discord":{"enabled":true}}"#).unwrap();
        assert!(partial.discord.enabled);
        assert_eq!(partial.discord.refresh_seconds, 3);
        assert_eq!(partial.scrobble.delay_seconds, 240);
    }
}
