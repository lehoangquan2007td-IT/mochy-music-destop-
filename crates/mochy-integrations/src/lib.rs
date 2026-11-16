//! mochy-integrations — Discord Rich Presence over the local Discord IPC pipe
//! and Last.fm / Libre.fm scrobbling (spec §8), ported from the Android
//! `discord/*`, `scrobble/*` and `providers/lastfm` sources.
//!
//! Both services are UI-agnostic: the app feeds them what the player is doing
//! ([`discord::NowPlaying`], [`scrobble::ScrobbleTimer`]) and reads back their
//! status for the settings page.

pub mod discord;
pub mod scrobble;

/// Seconds since 1970.
pub fn unix_secs() -> i64 {
    unix_millis() / 1000
}

/// Milliseconds since 1970.
pub fn unix_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
