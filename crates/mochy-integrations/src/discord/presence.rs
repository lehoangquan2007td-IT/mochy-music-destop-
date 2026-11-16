//! Keeps the Discord status in step with the player (Android
//! `DiscordPresence`). The app reports what is playing with
//! [`PresenceHandle::update`]; a background task sends it to the local Discord
//! client when something visible changed, no faster than the refresh interval
//! and Discord's own limit (5 updates / 20 s), takes it down when the music
//! stops, and reconnects when Discord starts later.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mochy_core::integrations::DiscordSettings;
use tokio::sync::watch;

use super::activity::{NowPlaying, PresenceActivity};
use super::ipc::{IpcClient, IpcError};
use crate::unix_millis;

/// How the Discord status is doing, for the line under the switch.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PresenceConnection {
    /// Turned off, or no application id set.
    #[default]
    Off,
    /// Connected, nothing playing (status cleared).
    Idle,
    Connecting,
    /// The status is up.
    Live,
    /// Discord isn't running (or the pipe broke); trying again.
    Retrying,
    /// Discord refused something (bad application id …).
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
struct Input {
    track: Option<NowPlaying>,
    settings: DiscordSettings,
}

const KEEP_PAUSED_MS: i64 = 10 * 60_000;
const BURST_LIMIT: usize = 5;
const BURST_WINDOW: Duration = Duration::from_secs(20);
const SETTINGS_SETTLE: Duration = Duration::from_millis(600);
const BACKOFF_SECS: [u64; 5] = [2, 5, 15, 30, 60];

/// Handle owned by the app; dropping it stops the worker (and clears the status).
pub struct PresenceHandle {
    tx: watch::Sender<Input>,
    status: Arc<Mutex<PresenceConnection>>,
    user: Arc<Mutex<Option<String>>>,
}

impl PresenceHandle {
    /// Start the worker on `rt`.
    pub fn spawn(rt: &tokio::runtime::Handle, settings: DiscordSettings) -> Self {
        let (tx, rx) = watch::channel(Input { track: None, settings });
        let status = Arc::new(Mutex::new(PresenceConnection::Off));
        let user = Arc::new(Mutex::new(None));
        rt.spawn(Worker::new(rx, status.clone(), user.clone()).run());
        Self { tx, status, user }
    }

    pub fn update(&self, track: Option<NowPlaying>) {
        self.tx.send_if_modified(|i| {
            if i.track != track {
                i.track = track;
                true
            } else {
                false
            }
        });
    }

    pub fn set_settings(&self, settings: &DiscordSettings) {
        self.tx.send_if_modified(|i| {
            if &i.settings != settings {
                i.settings = settings.clone();
                true
            } else {
                false
            }
        });
    }

    pub fn status(&self) -> PresenceConnection {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Discord user name of the connected client, when known.
    pub fn user(&self) -> Option<String> {
        self.user.lock().ok().and_then(|u| u.clone())
    }

    /// Take the status down now (app exit). Best effort, bounded wait.
    pub fn shutdown(&self) {
        self.tx.send_modify(|i| {
            i.track = None;
            i.settings.enabled = false;
        });
    }
}

struct Worker {
    rx: watch::Receiver<Input>,
    status: Arc<Mutex<PresenceConnection>>,
    user: Arc<Mutex<Option<String>>>,
    client: Option<IpcClient>,
    client_for: String,
    last_sent: Option<PresenceActivity>,
    cleared: bool,
    failures: usize,
    next_connect_at: Option<Instant>,
    recent_sends: VecDeque<Instant>,
    /// Older Discord clients refuse `details_url`; dropped after the first refusal.
    details_url_ok: bool,
}

impl Worker {
    fn new(rx: watch::Receiver<Input>, status: Arc<Mutex<PresenceConnection>>, user: Arc<Mutex<Option<String>>>) -> Self {
        Self {
            rx,
            status,
            user,
            client: None,
            client_for: String::new(),
            last_sent: None,
            cleared: true,
            failures: 0,
            next_connect_at: None,
            recent_sends: VecDeque::new(),
            details_url_ok: true,
        }
    }

    fn set_status(&self, s: PresenceConnection) {
        if let Ok(mut g) = self.status.lock() {
            *g = s;
        }
    }

    async fn run(mut self) {
        loop {
            let input = self.rx.borrow_and_update().clone();
            let refresh = input.settings.refresh_seconds.clamp(DiscordSettings::MIN_REFRESH_SECONDS, DiscordSettings::MAX_REFRESH_SECONDS);
            self.step(&input).await;

            // Wait for a change or the next refresh tick.
            let tick = tokio::time::sleep(Duration::from_secs(refresh as u64));
            tokio::select! {
                changed = self.rx.changed() => {
                    if changed.is_err() {
                        // Handle dropped: clear and stop.
                        self.hide(false).await;
                        return;
                    }
                    let settings_changed = self.rx.borrow().settings != input.settings;
                    if settings_changed {
                        // Typing in a text box shouldn't send a status per letter.
                        tokio::time::sleep(SETTINGS_SETTLE).await;
                    }
                }
                _ = tick => {}
            }
        }
    }

    async fn step(&mut self, input: &Input) {
        let s = &input.settings;
        let app_id = s.application_id.trim();
        if !s.enabled || app_id.is_empty() {
            self.hide(false).await;
            self.set_status(PresenceConnection::Off);
            return;
        }
        if self.client.is_some() && self.client_for != app_id {
            // Application id changed: reconnect under the new one.
            self.hide(false).await;
            self.failures = 0;
            self.next_connect_at = None;
        }
        let now = unix_millis();
        let track = match &input.track {
            Some(t) if !(!t.is_playing && s.clear_on_pause) && !(!t.is_playing && now - t.read_at_ms > KEEP_PAUSED_MS) => t,
            _ => {
                // Nothing to show: clear, keep the pipe open.
                self.hide(true).await;
                // a refusal stays visible until something is shown again
                if self.client.is_some() || !matches!(self.current_status(), PresenceConnection::Error(_)) {
                    self.set_status(PresenceConnection::Idle);
                }
                return;
            }
        };
        let activity = PresenceActivity::build(track, s, now);
        if activity.looks_like(self.last_sent.as_ref()) && self.client.is_some() {
            return;
        }
        if !self.connect(app_id).await {
            return;
        }
        self.wait_for_turn(s.refresh_seconds).await;
        let Some(client) = self.client.as_mut() else { return };
        let mut result = client.set_activity(Some(&activity.to_ipc_json(self.details_url_ok))).await;
        if let Err(IpcError::Rejected { .. }) = &result
            && self.details_url_ok
        {
            self.details_url_ok = false;
            result = client.set_activity(Some(&activity.to_ipc_json(false))).await;
        }
        match result {
            Ok(()) => {
                self.recent_sends.push_back(Instant::now());
                self.last_sent = Some(activity);
                self.cleared = false;
                self.set_status(PresenceConnection::Live);
            }
            Err(IpcError::Rejected { code, message }) => {
                tracing::warn!(code, %message, "Discord rejected the status");
                // Don't hammer: remember it as sent so only a change retries.
                self.last_sent = Some(activity);
                self.set_status(PresenceConnection::Error(message));
            }
            Err(e) => {
                tracing::debug!("Discord presence send failed: {e}");
                self.drop_client();
                self.schedule_retry();
            }
        }
    }

    fn current_status(&self) -> PresenceConnection {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// A ready connection, making one if the backoff allows. False while
    /// Discord can't be reached.
    async fn connect(&mut self, app_id: &str) -> bool {
        if self.client.is_some() {
            return true;
        }
        if self.next_connect_at.is_some_and(|t| Instant::now() < t) {
            return false;
        }
        self.set_status(if self.failures == 0 { PresenceConnection::Connecting } else { PresenceConnection::Retrying });
        match IpcClient::connect(app_id).await {
            Ok(c) => {
                tracing::info!(user = ?c.user, "connected to Discord");
                if let Ok(mut u) = self.user.lock() {
                    u.clone_from(&c.user);
                }
                self.client = Some(c);
                self.client_for = app_id.to_string();
                self.failures = 0;
                self.next_connect_at = None;
                self.last_sent = None;
                self.cleared = true;
                true
            }
            Err(IpcError::InvalidClientId(m)) => {
                tracing::warn!("Discord refused application id: {m}");
                self.failures += 1;
                self.next_connect_at = Some(Instant::now() + Duration::from_secs(60));
                self.set_status(PresenceConnection::Error(format!("Invalid application id ({m})")));
                false
            }
            Err(e) => {
                tracing::debug!("Discord not reachable: {e}");
                self.schedule_retry();
                false
            }
        }
    }

    fn schedule_retry(&mut self) {
        self.failures += 1;
        let secs = BACKOFF_SECS[(self.failures - 1).min(BACKOFF_SECS.len() - 1)];
        self.next_connect_at = Some(Instant::now() + Duration::from_secs(secs));
        self.set_status(PresenceConnection::Retrying);
    }

    fn drop_client(&mut self) {
        self.client = None;
        self.last_sent = None;
        if let Ok(mut u) = self.user.lock() {
            *u = None;
        }
    }

    /// Clear the status; with `keep_open` the pipe stays connected.
    async fn hide(&mut self, keep_open: bool) {
        self.last_sent = None;
        if let Some(c) = self.client.as_mut()
            && !self.cleared
        {
            if c.set_activity(None).await.is_err() {
                self.drop_client();
            }
            self.cleared = true;
        }
        if !keep_open && let Some(c) = self.client.take() {
            c.close().await;
            if let Ok(mut u) = self.user.lock() {
                *u = None;
            }
        }
    }

    /// Space messages by the refresh interval and Discord's burst limit.
    async fn wait_for_turn(&mut self, refresh_seconds: u32) {
        let now = Instant::now();
        while self.recent_sends.front().is_some_and(|t| now.duration_since(*t) > BURST_WINDOW) {
            self.recent_sends.pop_front();
        }
        let interval = Duration::from_secs(refresh_seconds.max(1) as u64);
        let interval_wait = self.recent_sends.back().map(|t| (*t + interval).saturating_duration_since(now)).unwrap_or_default();
        let burst_wait = if self.recent_sends.len() >= BURST_LIMIT {
            self.recent_sends.front().map(|t| (*t + BURST_WINDOW).saturating_duration_since(now)).unwrap_or_default()
        } else {
            Duration::ZERO
        };
        let wait = interval_wait.max(burst_wait);
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Discord absent (no pipe in the test environment): enabling presence must
    /// not panic or hang, and the status ends up Retrying (or Live if a real
    /// Discord happens to run with this id — not the case for a dummy id).
    #[tokio::test]
    async fn absent_discord_is_graceful() {
        let rt = tokio::runtime::Handle::current();
        let h = PresenceHandle::spawn(&rt, DiscordSettings::default());
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(h.status(), PresenceConnection::Off);
        let s = DiscordSettings { enabled: true, application_id: "1".into(), ..Default::default() };
        h.set_settings(&s);
        h.update(Some(NowPlaying::sample(unix_millis())));
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let st = h.status();
        assert!(
            matches!(st, PresenceConnection::Retrying | PresenceConnection::Connecting | PresenceConnection::Error(_)),
            "unexpected status {st:?}"
        );
        h.shutdown();
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert_eq!(h.status(), PresenceConnection::Off);
    }
}
