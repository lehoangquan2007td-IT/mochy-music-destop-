//! Integrations hub: Discord Rich Presence, Last.fm / Libre.fm scrobbling and
//! the playlist importer, wired to the player without touching the controller.
//!
//! `app.rs` calls [`Integrations::frame`] once per frame (it watches the
//! player's public state the same way the OS media controls do) and
//! [`Integrations::ui`] to draw the importer window; the settings page calls
//! [`settings_section`]. Everything else lives in `mochy-integrations` and
//! `mochy-importer`.

mod importer_ui;
mod settings_ui;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use mochy_core::integrations::IntegrationSettings;
use mochy_core::models::Song;
use mochy_core::settings::AppSettings;
use mochy_integrations::discord::{NowPlaying, PresenceConnection, PresenceHandle};
use mochy_integrations::scrobble::scrobbler::account_mut;
use mochy_integrations::scrobble::{ScrobbleRules, ScrobbleSite, ScrobbleTimer, Scrobbler, ScrobblerEvent, scrobble_track};
use mochy_integrations::{unix_millis, unix_secs};

use crate::controller::{PlayState, PlayerController};
use crate::services::Services;

pub use settings_ui::settings_section;

/// A result from a background sign-in, applied to the settings on the UI thread.
#[derive(Debug, Clone)]
enum Patch {
    SignedIn { site: ScrobbleSite, name: String, key: String },
    SignedOut(ScrobbleSite),
}

/// Where a scrobble sign-in is.
#[derive(Debug, Clone, Default, PartialEq)]
enum SiteAuth {
    #[default]
    Idle,
    Working,
    /// Waiting for the user to approve `token` in the browser.
    Browser { token: String, url: String, opened: bool },
    Error(String),
}

/// State the settings page reads without access to the hub itself.
struct Shared {
    svc: Services,
    importer_open: AtomicBool,
    presence: Mutex<(PresenceConnection, Option<String>)>,
    now_playing: Mutex<Option<NowPlaying>>,
    pending: AtomicUsize,
    auth: Mutex<[SiteAuth; 2]>,
    /// Username / password typed into the sign-in form (never saved).
    login: Mutex<[(String, String); 2]>,
    patches: Mutex<Vec<Patch>>,
    scrobbler: Scrobbler,
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

fn shared() -> Option<&'static Arc<Shared>> {
    SHARED.get()
}

fn site_index(site: ScrobbleSite) -> usize {
    match site {
        ScrobbleSite::LastFm => 0,
        ScrobbleSite::LibreFm => 1,
    }
}

impl Shared {
    fn repaint(&self) {
        // Any message wakes the UI thread; a Toast with empty text is not shown,
        // so use the bus' own repaint by sending nothing heavier than needed.
        if let Some(ctx) = CTX.get() {
            ctx.request_repaint();
        }
    }

    fn set_auth(&self, site: ScrobbleSite, a: SiteAuth) {
        self.auth.lock().unwrap()[site_index(site)] = a;
        self.repaint();
    }

    fn push_patch(&self, p: Patch) {
        self.patches.lock().unwrap().push(p);
        self.repaint();
    }

    /// Move finished sign-ins into the settings.
    fn apply_patches(&self, s: &mut AppSettings) {
        let patches = std::mem::take(&mut *self.patches.lock().unwrap());
        for p in patches {
            match p {
                Patch::SignedIn { site, name, key } => {
                    let acc = account_mut(&mut s.integrations.scrobble, site);
                    acc.session_key = key;
                    acc.username = name;
                    acc.enabled = true;
                }
                Patch::SignedOut(site) => {
                    let acc = account_mut(&mut s.integrations.scrobble, site);
                    acc.session_key.clear();
                    acc.username.clear();
                    acc.enabled = false;
                }
            }
        }
    }
}

static CTX: OnceLock<egui::Context> = OnceLock::new();

/// What the hub last saw of the player.
#[derive(Default)]
struct Observed {
    /// (queue uid, song id) of the current entry.
    key: Option<(u64, String)>,
    /// The scrobble timer has been told this song started.
    started: bool,
    playing: bool,
    last_pos: f64,
    /// Last NowPlaying handed to Discord.
    presence: Option<NowPlaying>,
    /// Import was running last frame.
    import_running: bool,
}

/// What the scrobble timer has to hear about after one look at the player.
#[derive(Debug, Clone, PartialEq)]
enum Step {
    None,
    /// The song changed before it started playing.
    Stop,
    /// A song started playing (`stopped`: a different one was current before).
    Start { stopped: bool },
    /// Paused / resumed.
    Playing(bool),
}

impl Observed {
    /// One look at the player: the current entry `key`, its state and position.
    fn step(&mut self, key: Option<(u64, String)>, state: &PlayState, pos: f64, duration: Option<f64>) -> Step {
        let mut changed = false;
        if key != self.key {
            changed = true;
            self.key = key;
            self.started = false;
            self.playing = false;
            self.last_pos = 0.0;
        }
        let playing = *state == PlayState::Playing;
        // repeat-one: the same entry starts over from the top
        let restarted = self.started
            && playing
            && pos + 5.0 < self.last_pos
            && pos < 5.0
            && duration.is_some_and(|d| self.last_pos > d * 0.9);
        self.last_pos = pos;
        if self.key.is_none() {
            return if changed { Step::Stop } else { Step::None };
        }
        if (!self.started && playing) || restarted {
            self.started = true;
            self.playing = true;
            return Step::Start { stopped: changed || restarted };
        }
        // buffering between songs/seeks neither pauses nor resumes the clock
        if self.started && playing != self.playing && *state != PlayState::Loading {
            self.playing = playing;
            return Step::Playing(playing);
        }
        if changed { Step::Stop } else { Step::None }
    }
}

pub struct Integrations {
    shared: Arc<Shared>,
    presence: PresenceHandle,
    timer: ScrobbleTimer,
    seen: Option<IntegrationSettings>,
    obs: Observed,
    importer: importer_ui::ImporterUi,
}

impl Integrations {
    pub fn new(svc: Services) -> Self {
        let data = mochy_storage::data_dir();
        let scrobbler = Scrobbler::new(svc.rt.clone(), data.join("scrobble_queue.json"));
        let presence = PresenceHandle::spawn(&svc.rt, Default::default());
        let shared = Arc::new(Shared {
            svc: svc.clone(),
            importer_open: AtomicBool::new(false),
            presence: Mutex::new((PresenceConnection::Off, None)),
            now_playing: Mutex::new(None),
            pending: AtomicUsize::new(scrobbler.pending()),
            auth: Mutex::new(Default::default()),
            login: Mutex::new(Default::default()),
            patches: Mutex::new(Vec::new()),
            scrobbler,
        });
        let _ = SHARED.set(shared.clone());
        let importer = importer_ui::ImporterUi::new(svc, data.join("import_matches.json"));
        Self { shared, presence, timer: ScrobbleTimer::new(ScrobbleRules::default()), seen: None, obs: Observed::default(), importer }
    }

    /// Per-frame housekeeping. Returns true when the library changed (an
    /// import or undo finished) and should be reloaded.
    pub fn frame(&mut self, ctx: &egui::Context, player: &PlayerController, settings: &mut AppSettings) -> bool {
        let _ = CTX.set(ctx.clone());
        self.shared.apply_patches(settings);
        self.sync_settings(settings);
        self.observe_scrobble(player);
        self.observe_presence(player);
        for ev in self.shared.scrobbler.take_events() {
            match ev {
                ScrobblerEvent::SignedOut(site) => {
                    self.shared.push_patch(Patch::SignedOut(site));
                    self.shared.svc.bus.toast(crate::ui::i18n::tr("The scrobbling service signed you out. Sign in again in Settings.").to_string());
                }
            }
        }
        self.shared.pending.store(self.shared.scrobbler.pending(), Ordering::Relaxed);
        if let Ok(mut p) = self.shared.presence.lock() {
            *p = (self.presence.status(), self.presence.user());
        }
        let running = self.importer.is_running();
        let finished = self.obs.import_running && !running;
        self.obs.import_running = running;
        if running {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        finished | self.importer.take_library_changed()
    }

    /// Draw the importer window (when open).
    pub fn ui(&mut self, ctx: &egui::Context) {
        let mut open = self.shared.importer_open.load(Ordering::Relaxed);
        if !open {
            return;
        }
        self.importer.show(ctx, &mut open);
        self.shared.importer_open.store(open, Ordering::Relaxed);
    }

    /// A like here becomes a love on Last.fm / Libre.fm, when asked for.
    pub fn on_like(&self, song: &Song, liked: bool) {
        self.shared.scrobbler.love(song.artist_name(), &song.title, liked);
    }

    fn sync_settings(&mut self, s: &AppSettings) {
        if self.seen.as_ref() == Some(&s.integrations) {
            return;
        }
        let i = &s.integrations;
        self.presence.set_settings(&i.discord);
        self.shared.scrobbler.configure(&i.scrobble);
        self.timer.rules = ScrobbleRules::from_settings(&i.scrobble);
        self.seen = Some(i.clone());
    }

    fn observe_scrobble(&mut self, player: &PlayerController) {
        let now = Instant::now();
        let song = player.current();
        let key = player.queue.current().map(|e| e.uid).zip(song.map(|s| s.id.clone()));
        let duration = player.duration.or(song.and_then(|s| s.duration_secs));
        match self.obs.step(key, &player.state, player.pos, duration) {
            Step::None => {}
            Step::Stop => self.timer.on_song_stop(),
            Step::Start { stopped } => {
                if stopped {
                    self.timer.on_song_stop();
                }
                if let Some(song) = song {
                    let track = scrobble_track(&song.title, song.artist_name(), song.album_name.as_deref());
                    if let Some(np) = self.timer.on_song_start(track, duration, unix_secs(), now) {
                        self.shared.scrobbler.now_playing(np);
                    }
                }
            }
            Step::Playing(p) => self.timer.on_playing_changed(p, now),
        }
        if let Some((track, started_at)) = self.timer.poll(now) {
            self.shared.scrobbler.scrobble(track, started_at);
        }
    }

    fn observe_presence(&mut self, player: &PlayerController) {
        let now = unix_millis();
        let next = match player.state {
            // between songs: leave the status as it is until the next one plays
            PlayState::Loading => return,
            PlayState::Playing | PlayState::Paused => player.current().map(|s| now_playing_of(s, player, now)),
            PlayState::Idle | PlayState::Ended | PlayState::Error(_) => None,
        };
        let changed = match (&self.obs.presence, &next) {
            (None, None) => false,
            (Some(a), Some(b)) => {
                a.media_id != b.media_id
                    || a.title != b.title
                    || a.is_playing != b.is_playing
                    || a.quality != b.quality
                    || a.artwork_url != b.artwork_url
                    || a.duration_ms != b.duration_ms
                    || (a.position_at(now) - b.position_ms).abs() > 1500
            }
            _ => true,
        };
        if changed {
            self.presence.update(next.clone());
            if let Ok(mut n) = self.shared.now_playing.lock() {
                n.clone_from(&next);
            }
            self.obs.presence = next;
        }
    }
}

fn now_playing_of(s: &Song, player: &PlayerController, now: i64) -> NowPlaying {
    let artwork = s.thumbnail_url.as_deref().filter(|u| u.starts_with("http")).map(|u| crate::images::upgrade_thumbnail(u, 512));
    let video = s.video_id.clone().filter(|_| !s.is_local());
    NowPlaying {
        media_id: video.clone().unwrap_or_else(|| s.id.clone()),
        title: s.title.clone(),
        artist: crate::widgets::artists_line(s),
        album: s.album_name.clone(),
        artwork_url: artwork,
        artist_artwork_url: None,
        duration_ms: (player.duration.or(s.duration_secs).unwrap_or(0.0) * 1000.0) as i64,
        position_ms: (player.pos * 1000.0) as i64,
        read_at_ms: now,
        speed: 1.0,
        is_playing: player.state == PlayState::Playing,
        quality: player.format.clone(),
        is_youtube: video.is_some(),
    }
}

/// Open the importer window (from the settings page).
pub fn open_importer() {
    if let Some(s) = shared() {
        s.importer_open.store(true, Ordering::Relaxed);
        s.repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(uid: u64) -> Option<(u64, String)> {
        Some((uid, format!("song{uid}")))
    }

    #[test]
    fn observer_reports_starts_pauses_and_repeats() {
        let mut o = Observed::default();
        assert_eq!(o.step(None, &PlayState::Idle, 0.0, None), Step::None);
        // queued but still loading: the song changed, nothing started
        assert_eq!(o.step(k(1), &PlayState::Loading, 0.0, Some(200.0)), Step::Stop);
        assert_eq!(o.step(k(1), &PlayState::Playing, 0.1, Some(200.0)), Step::Start { stopped: false });
        assert_eq!(o.step(k(1), &PlayState::Playing, 10.0, Some(200.0)), Step::None);
        assert_eq!(o.step(k(1), &PlayState::Paused, 10.0, Some(200.0)), Step::Playing(false));
        assert_eq!(o.step(k(1), &PlayState::Paused, 10.0, Some(200.0)), Step::None);
        // buffering after a seek isn't a pause
        assert_eq!(o.step(k(1), &PlayState::Playing, 10.0, Some(200.0)), Step::Playing(true));
        assert_eq!(o.step(k(1), &PlayState::Loading, 120.0, Some(200.0)), Step::None);
        assert_eq!(o.step(k(1), &PlayState::Playing, 199.0, Some(200.0)), Step::None);
        // repeat-one: same entry from the top
        assert_eq!(o.step(k(1), &PlayState::Playing, 0.5, Some(200.0)), Step::Start { stopped: true });
        // a seek back to the start mid-song is not a repeat
        assert_eq!(o.step(k(1), &PlayState::Playing, 60.0, Some(200.0)), Step::None);
        assert_eq!(o.step(k(1), &PlayState::Playing, 0.5, Some(200.0)), Step::None);
        // next song
        assert_eq!(o.step(k(2), &PlayState::Playing, 0.2, Some(180.0)), Step::Start { stopped: true });
        // queue cleared
        assert_eq!(o.step(None, &PlayState::Idle, 0.0, None), Step::Stop);
    }
}
