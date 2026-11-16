//! Decides when a play counts, by Last.fm's rule (Android `ScrobbleTimer`):
//! a song longer than `min_song_seconds`, once it has played for
//! `played_fraction` of its length or `max_wait_seconds`, whichever comes
//! first. Only time spent playing counts, so a pause holds the clock.
//!
//! Pure state machine: the caller passes the clock in and polls.

use std::time::{Duration, Instant};

use mochy_core::integrations::ScrobbleSettings;

use super::audioscrobbler::ScrobbleTrack;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrobbleRules {
    pub min_song_seconds: u32,
    pub played_fraction: f32,
    pub max_wait_seconds: u32,
}

impl Default for ScrobbleRules {
    fn default() -> Self {
        Self {
            min_song_seconds: ScrobbleSettings::DEFAULT_MIN_SONG_SECONDS,
            played_fraction: ScrobbleSettings::DEFAULT_PLAYED_FRACTION,
            max_wait_seconds: ScrobbleSettings::DEFAULT_MAX_WAIT_SECONDS,
        }
    }
}

impl ScrobbleRules {
    pub fn from_settings(s: &ScrobbleSettings) -> Self {
        Self {
            min_song_seconds: s.min_song_seconds,
            played_fraction: s.delay_percent.clamp(0.05, 1.0),
            max_wait_seconds: s.delay_seconds.max(1),
        }
    }

    /// Playing time a song of `seconds` needs before it counts; None when it is
    /// too short to count at all.
    pub fn required(&self, seconds: u32) -> Option<Duration> {
        if seconds <= self.min_song_seconds {
            return None;
        }
        let ms = ((seconds as f64 * 1000.0 * self.played_fraction as f64) as u64).min(self.max_wait_seconds as u64 * 1000);
        Some(Duration::from_millis(ms))
    }
}

#[derive(Debug, Default)]
pub struct ScrobbleTimer {
    pub rules: ScrobbleRules,
    track: Option<ScrobbleTrack>,
    /// When the song started, unix seconds: what the scrobble is sent with.
    started_at: i64,
    /// Playing time still to go; None once scrobbled or for a short song.
    remaining: Option<Duration>,
    /// When the clock last started running; None while paused.
    running_since: Option<Instant>,
}

impl ScrobbleTimer {
    pub fn new(rules: ScrobbleRules) -> Self {
        Self { rules, ..Default::default() }
    }

    pub fn track(&self) -> Option<&ScrobbleTrack> {
        self.track.as_ref()
    }

    /// A new song started playing. Returns the track to send as Now Playing.
    /// `duration_secs` is the player's length (song metadata as fallback).
    pub fn on_song_start(
        &mut self,
        track: Option<ScrobbleTrack>,
        duration_secs: Option<f64>,
        now_unix: i64,
        now: Instant,
    ) -> Option<ScrobbleTrack> {
        self.on_song_stop();
        let seconds = duration_secs.filter(|d| d.is_finite() && *d > 0.0).map(|d| d as u32).unwrap_or(0);
        let mut track = track?;
        track.duration_seconds = (seconds > 0).then_some(seconds);
        self.track = Some(track.clone());
        self.started_at = now_unix;
        self.remaining = self.rules.required(seconds);
        self.resume(now);
        Some(track)
    }

    pub fn on_playing_changed(&mut self, is_playing: bool, now: Instant) {
        if is_playing { self.resume(now) } else { self.pause(now) }
    }

    pub fn on_song_stop(&mut self) {
        self.track = None;
        self.remaining = None;
        self.running_since = None;
    }

    fn resume(&mut self, now: Instant) {
        if self.track.is_none() || self.remaining.is_none() || self.running_since.is_some() {
            return;
        }
        self.running_since = Some(now);
    }

    fn pause(&mut self, now: Instant) {
        let (Some(since), Some(rem)) = (self.running_since.take(), self.remaining) else { return };
        // at least 1 ms left, like upstream: the timer only fires while playing
        self.remaining = Some(rem.saturating_sub(now.duration_since(since)).max(Duration::from_millis(1)));
    }

    /// Fires once, when enough playing time has passed: `(track, started_at)`.
    pub fn poll(&mut self, now: Instant) -> Option<(ScrobbleTrack, i64)> {
        let since = self.running_since?;
        let rem = self.remaining?;
        if now.duration_since(since) < rem {
            return None;
        }
        self.remaining = None;
        self.running_since = None;
        self.track.clone().map(|t| (t, self.started_at))
    }
}

/// The first artist only: services keep one artist per play. None for a song
/// with no title or artist, which no service takes.
pub fn scrobble_track(title: &str, first_artist: &str, album: Option<&str>) -> Option<ScrobbleTrack> {
    let artist = first_artist.trim();
    let title = title.trim();
    if artist.is_empty() || title.is_empty() {
        return None;
    }
    Some(ScrobbleTrack {
        artist: artist.into(),
        track: title.into(),
        album: album.map(str::trim).filter(|a| !a.is_empty()).map(str::to_string),
        duration_seconds: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> Option<ScrobbleTrack> {
        scrobble_track("Song", "Band", Some(" LP "))
    }

    #[test]
    fn half_or_four_minutes() {
        let r = ScrobbleRules::default();
        assert_eq!(r.required(30), None);
        assert_eq!(r.required(31), Some(Duration::from_millis(15_500)));
        assert_eq!(r.required(200), Some(Duration::from_secs(100)));
        // 10 minutes: four minutes cap.
        assert_eq!(r.required(600), Some(Duration::from_secs(240)));
    }

    #[test]
    fn fires_once_after_enough_play() {
        let t0 = Instant::now();
        let mut timer = ScrobbleTimer::new(ScrobbleRules::default());
        let np = timer.on_song_start(t(), Some(200.0), 1_700_000_000, t0).unwrap();
        assert_eq!(np.duration_seconds, Some(200));
        assert_eq!(np.album.as_deref(), Some("LP"));
        assert!(timer.poll(t0 + Duration::from_secs(99)).is_none());
        let (track, at) = timer.poll(t0 + Duration::from_secs(100)).unwrap();
        assert_eq!(track.track, "Song");
        assert_eq!(at, 1_700_000_000);
        assert!(timer.poll(t0 + Duration::from_secs(500)).is_none());
    }

    #[test]
    fn pause_holds_the_clock() {
        let t0 = Instant::now();
        let mut timer = ScrobbleTimer::new(ScrobbleRules::default());
        timer.on_song_start(t(), Some(100.0), 0, t0);
        timer.on_playing_changed(false, t0 + Duration::from_secs(40));
        // paused for ten minutes: nothing
        assert!(timer.poll(t0 + Duration::from_secs(640)).is_none());
        timer.on_playing_changed(true, t0 + Duration::from_secs(640));
        assert!(timer.poll(t0 + Duration::from_secs(649)).is_none());
        assert!(timer.poll(t0 + Duration::from_secs(650)).is_some());
    }

    #[test]
    fn short_songs_and_missing_data() {
        let t0 = Instant::now();
        let mut timer = ScrobbleTimer::new(ScrobbleRules::default());
        // Now Playing still goes out for a short song, but it never scrobbles.
        assert!(timer.on_song_start(t(), Some(25.0), 0, t0).is_some());
        assert!(timer.poll(t0 + Duration::from_secs(3600)).is_none());
        // Unknown length counts as too short.
        assert!(timer.on_song_start(t(), None, 0, t0).is_some());
        assert!(timer.poll(t0 + Duration::from_secs(3600)).is_none());
        assert!(scrobble_track("", "Band", None).is_none());
        assert!(scrobble_track("Song", " ", None).is_none());
        assert!(timer.on_song_start(None, Some(200.0), 0, t0).is_none());
        // Stop before it counted: nothing.
        timer.on_song_start(t(), Some(200.0), 0, t0);
        timer.on_song_stop();
        assert!(timer.poll(t0 + Duration::from_secs(3600)).is_none());
    }

    #[test]
    fn custom_rules_from_settings() {
        let s = ScrobbleSettings { delay_percent: 0.8, min_song_seconds: 60, delay_seconds: 120, ..Default::default() };
        let r = ScrobbleRules::from_settings(&s);
        assert_eq!(r.required(60), None);
        assert_eq!(r.required(100), Some(Duration::from_secs(80)));
        assert_eq!(r.required(1000), Some(Duration::from_secs(120)));
    }
}
