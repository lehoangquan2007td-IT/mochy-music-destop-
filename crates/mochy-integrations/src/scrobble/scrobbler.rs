//! Sends what's played to Last.fm and Libre.fm (Android `Scrobbler`). Every
//! play is saved to the [`ScrobbleQueue`] first and sent from there, so nothing
//! is lost offline: what's left is tried again later with growing waits (the
//! desktop stand-in for WorkManager). Now Playing goes straight out and is never
//! queued.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mochy_core::integrations::{ScrobbleAccount, ScrobbleSettings};

use super::audioscrobbler::{Audioscrobbler, MAX_BATCH, ScrobbleError, ScrobbleTrack};
use super::queue::{PendingScrobble, ScrobbleQueue, ScrobbleSite};
use crate::unix_secs;

/// Something the app should know about (shown as a toast / applied to settings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScrobblerEvent {
    /// The service took back access: forget the session in settings.
    SignedOut(ScrobbleSite),
}

#[derive(Clone)]
struct SiteState {
    site: ScrobbleSite,
    api: Audioscrobbler,
    session: String,
    enabled: bool,
}

#[derive(Clone, Default)]
struct Config {
    sites: Vec<SiteState>,
    use_now_playing: bool,
    send_likes: bool,
    fingerprint: String,
}

struct Inner {
    rt: tokio::runtime::Handle,
    queue: Mutex<ScrobbleQueue>,
    config: Mutex<Config>,
    sending: tokio::sync::Mutex<()>,
    events: Mutex<Vec<ScrobblerEvent>>,
    retry: Mutex<RetryState>,
}

#[derive(Default)]
struct RetryState {
    scheduled: bool,
    attempts: u32,
}

#[derive(Clone)]
pub struct Scrobbler {
    inner: Arc<Inner>,
}

/// The API client for one site from its settings.
pub fn api_for(site: ScrobbleSite, acc: &ScrobbleAccount) -> Audioscrobbler {
    match site {
        ScrobbleSite::LastFm => Audioscrobbler::new(Audioscrobbler::LASTFM_ROOT, Audioscrobbler::LASTFM_AUTH, &acc.api_key, &acc.api_secret, false),
        ScrobbleSite::LibreFm => Audioscrobbler::new(Audioscrobbler::LIBREFM_ROOT, Audioscrobbler::LIBREFM_AUTH, &acc.api_key, &acc.api_secret, true),
    }
}

pub fn account(s: &ScrobbleSettings, site: ScrobbleSite) -> &ScrobbleAccount {
    match site {
        ScrobbleSite::LastFm => &s.lastfm,
        ScrobbleSite::LibreFm => &s.librefm,
    }
}

pub fn account_mut(s: &mut ScrobbleSettings, site: ScrobbleSite) -> &mut ScrobbleAccount {
    match site {
        ScrobbleSite::LastFm => &mut s.lastfm,
        ScrobbleSite::LibreFm => &mut s.librefm,
    }
}

impl Scrobbler {
    /// `queue_file`: where unsent plays are kept (`scrobble_queue.json`).
    pub fn new(rt: tokio::runtime::Handle, queue_file: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                rt,
                queue: Mutex::new(ScrobbleQueue::open(queue_file)),
                config: Mutex::new(Config::default()),
                sending: tokio::sync::Mutex::new(()),
                events: Mutex::new(Vec::new()),
                retry: Mutex::new(RetryState::default()),
            }),
        }
    }

    /// Apply the current settings (cheap when unchanged). Plays left from last
    /// time go out as soon as an account is configured.
    pub fn configure(&self, s: &ScrobbleSettings) {
        let fingerprint = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            s.lastfm.api_key, s.lastfm.api_secret, s.lastfm.session_key, s.lastfm.enabled,
            s.librefm.api_key, s.librefm.api_secret, s.librefm.session_key, s.librefm.enabled,
            s.use_now_playing, s.send_likes
        );
        let had_sites;
        {
            let mut c = self.inner.config.lock().unwrap();
            if c.fingerprint == fingerprint {
                return;
            }
            had_sites = c.sites.iter().map(|x| x.site).collect::<Vec<_>>();
            c.sites = ScrobbleSite::ALL
                .iter()
                .filter_map(|&site| {
                    let acc = account(s, site);
                    (acc.is_configured() && acc.is_signed_in()).then(|| SiteState {
                        site,
                        api: api_for(site, acc),
                        session: acc.session_key.clone(),
                        enabled: acc.enabled,
                    })
                })
                .collect();
            c.use_now_playing = s.use_now_playing;
            c.send_likes = s.send_likes;
            c.fingerprint = fingerprint;
        }
        // Signed out in settings: nothing waiting for that site can go any more.
        for site in had_sites {
            if !account(s, site).is_signed_in() {
                self.inner.queue.lock().unwrap().forget(site);
            }
        }
        if self.pending() > 0 {
            self.spawn_send();
        }
    }

    /// Plays still waiting to be sent, for the settings page.
    pub fn pending(&self) -> usize {
        self.inner.queue.lock().map(|q| q.len()).unwrap_or(0)
    }

    pub fn take_events(&self) -> Vec<ScrobblerEvent> {
        std::mem::take(&mut *self.inner.events.lock().unwrap())
    }

    /// Signed-in services; paused ones only with `include_paused`.
    fn sites(&self, include_paused: bool) -> Vec<SiteState> {
        let c = self.inner.config.lock().unwrap();
        c.sites.iter().filter(|s| include_paused || s.enabled).cloned().collect()
    }

    pub fn has_active_site(&self) -> bool {
        !self.sites(false).is_empty()
    }

    pub fn now_playing(&self, track: ScrobbleTrack) {
        if !self.inner.config.lock().unwrap().use_now_playing {
            return;
        }
        let me = self.clone();
        self.inner.rt.spawn(async move {
            for s in me.sites(false) {
                if let Err(e) = s.api.update_now_playing(&s.session, &track).await {
                    // nothing to keep: by the next try it's another song
                    if e.is_signed_out() {
                        me.signed_out(s.site);
                    }
                    tracing::debug!("{} Now Playing didn't go through: {e}", s.site.label());
                }
            }
        });
    }

    /// A play that counted, which started at `started_at` (unix seconds).
    pub fn scrobble(&self, track: ScrobbleTrack, started_at: i64) {
        let sites: std::collections::BTreeSet<ScrobbleSite> = self.sites(false).iter().map(|s| s.site).collect();
        if sites.is_empty() {
            return;
        }
        tracing::info!(artist = %track.artist, track = %track.track, "scrobble queued");
        self.inner.queue.lock().unwrap().add(PendingScrobble { track, started_at, sites });
        self.spawn_send();
    }

    /// A like here becomes a love there, when the user asked for that.
    pub fn love(&self, artist: &str, track: &str, love: bool) {
        if artist.trim().is_empty() || !self.inner.config.lock().unwrap().send_likes {
            return;
        }
        let (me, artist, track) = (self.clone(), artist.to_string(), track.to_string());
        self.inner.rt.spawn(async move {
            for s in me.sites(false) {
                if let Err(e) = s.api.love(&s.session, &artist, &track, love).await {
                    if e.is_signed_out() {
                        me.signed_out(s.site);
                    }
                    tracing::debug!("{} love didn't go through: {e}", s.site.label());
                }
            }
        });
    }

    fn spawn_send(&self) {
        let me = self.clone();
        self.inner.rt.spawn(async move {
            me.send().await;
        });
    }

    /// Sends what's waiting, a batch at a time, until each service has it all
    /// or can't take more right now. False when something has to be tried
    /// again later (which also books that try).
    pub async fn send(&self) -> bool {
        let _guard = self.inner.sending.lock().await;
        let mut all_sent = true;
        // paused services still get plays from before the pause
        for s in self.sites(true) {
            loop {
                let batch = self.inner.queue.lock().unwrap().next_for(s.site, MAX_BATCH, unix_secs());
                if batch.is_empty() {
                    break;
                }
                let plays: Vec<(ScrobbleTrack, i64)> = batch.iter().map(|p| (p.track.clone(), p.started_at)).collect();
                match s.api.scrobble(&s.session, &plays).await {
                    Ok(()) => {
                        tracing::info!("{} took {} scrobbles", s.site.label(), batch.len());
                        self.inner.queue.lock().unwrap().mark_sent(s.site, &batch);
                    }
                    Err(e) if e.is_signed_out() => {
                        self.signed_out(s.site);
                        break;
                    }
                    // turned down for good (bad song data): dropped, it would hold back every play after it
                    Err(e) if e.is_rejected() => {
                        tracing::warn!("{} turned down {} scrobbles: {e}", s.site.label(), batch.len());
                        self.inner.queue.lock().unwrap().mark_sent(s.site, &batch);
                    }
                    Err(e) => {
                        tracing::debug!("{} scrobbles will be sent later: {e}", s.site.label());
                        all_sent = false;
                        break;
                    }
                }
            }
        }
        if all_sent {
            self.inner.retry.lock().unwrap().attempts = 0;
        } else {
            self.send_later();
        }
        all_sent
    }

    /// Try again after 30 s, then 1, 2, 4 … minutes (capped at 30).
    fn send_later(&self) {
        let delay = {
            let mut r = self.inner.retry.lock().unwrap();
            if r.scheduled {
                return;
            }
            r.scheduled = true;
            r.attempts += 1;
            retry_delay(r.attempts)
        };
        let me = self.clone();
        self.inner.rt.spawn(async move {
            tokio::time::sleep(delay).await;
            me.inner.retry.lock().unwrap().scheduled = false;
            me.send().await;
        });
    }

    fn signed_out(&self, site: ScrobbleSite) {
        tracing::warn!("{} signed us out", site.label());
        {
            let mut c = self.inner.config.lock().unwrap();
            c.sites.retain(|s| s.site != site);
            c.fingerprint.clear();
        }
        self.inner.queue.lock().unwrap().forget(site);
        let mut ev = self.inner.events.lock().unwrap();
        if !ev.contains(&ScrobblerEvent::SignedOut(site)) {
            ev.push(ScrobblerEvent::SignedOut(site));
        }
    }
}

/// Retry schedule: 30 s, then exponential from one minute, capped at 30 min.
pub fn retry_delay(attempt: u32) -> Duration {
    if attempt <= 1 {
        return Duration::from_secs(30);
    }
    let mins = 1u64 << (attempt - 2).min(5);
    Duration::from_secs((mins * 60).min(30 * 60))
}

/// Human-readable message for a sign-in failure.
pub fn sign_in_error_message(e: &ScrobbleError) -> String {
    if e.is_wrong_login() {
        "Wrong username or password.".into()
    } else if e.is_unauthorized_token() {
        "The request hasn't been approved in the browser yet.".into()
    } else if e.is_network() {
        "Couldn't reach the server — check your connection.".into()
    } else if e.code == 10 || e.code == 26 {
        "The API key isn't valid.".into()
    } else if e.code == 13 {
        "The API secret doesn't match the key.".into()
    } else {
        e.message.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_schedule() {
        assert_eq!(retry_delay(1), Duration::from_secs(30));
        assert_eq!(retry_delay(2), Duration::from_secs(60));
        assert_eq!(retry_delay(3), Duration::from_secs(120));
        assert_eq!(retry_delay(20), Duration::from_secs(30 * 60));
    }

    #[tokio::test]
    async fn plays_need_a_signed_in_site() {
        let dir = std::env::temp_dir().join(format!("mochy-scrobbler-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = Scrobbler::new(tokio::runtime::Handle::current(), dir.join("q.json"));
        let t = ScrobbleTrack { artist: "A".into(), track: "T".into(), album: None, duration_seconds: Some(100) };
        s.scrobble(t.clone(), 1);
        assert_eq!(s.pending(), 0, "no account: nothing queued");
        let mut cfg = ScrobbleSettings::default();
        cfg.lastfm = ScrobbleAccount { api_key: "k".into(), api_secret: "s".into(), session_key: "sk".into(), username: "u".into(), enabled: false };
        s.configure(&cfg);
        assert!(!s.has_active_site());
        s.scrobble(t.clone(), 1);
        assert_eq!(s.pending(), 0, "paused account: nothing queued");
        // Signing out in settings forgets what waits for that site.
        cfg.lastfm.enabled = true;
        s.configure(&cfg);
        assert!(s.has_active_site());
        s.inner.queue.lock().unwrap().add(PendingScrobble { track: t, started_at: unix_secs(), sites: [ScrobbleSite::LastFm].into() });
        cfg.lastfm.session_key.clear();
        s.configure(&cfg);
        assert_eq!(s.pending(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
