//! Signed-in YouTube account: sign-in (WebView2 or pasted cookie), the
//! encrypted session, one-way library sync (YouTube → local library) and the
//! extra Home shelves (youtube.com mixes / music videos, YouTube history).
//!
//! `app.rs` owns a [`YtAccount`], forwards [`AccountMsg`]s to
//! [`YtAccount::on_msg`], calls [`YtAccount::frame`] once per frame (sign-in
//! window, requests from the settings page, cookie persistence) and runs the
//! returned [`AccountEffect`]s. The settings page calls [`settings_section`];
//! Home reads [`home_extras`]. Like `integrations`, the settings page talks
//! to the account through a process-wide [`Shared`].
//!
//! Sync is read-only on the YouTube side: nothing in the app changes the
//! YouTube account.

mod signin;
pub mod store;
mod ui;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use mochy_core::models::{Album, Artist, Playlist, ShelfItem, Song};
use mochy_core::settings::{AppSettings, YoutubeAccountSettings};
use mochy_innertube::account::{AccountInfo, LibrarySnapshot};
use mochy_innertube::explore::Section;
use mochy_innertube::youtube_home::YoutubeHome;
use mochy_storage::Db;

use crate::services::{AppMsg, Services};
use crate::ui::i18n::tr;
use store::{AccountFile, Session, SyncCounts};

pub use ui::settings_section;

/// Automatic sync on start runs at most this often.
const AUTO_SYNC_EVERY_MS: i64 = 6 * 3600 * 1000;
/// Mixes wanted from youtube.com before paging stops.
const WANTED_MIXES: usize = 16;
const MAX_HOME_PAGES: usize = 5;

/// Results of background work, routed through the app's message bus.
pub enum AccountMsg {
    /// `account_info` for a fresh sign-in (`new_session`) or a refresh.
    Info { result: Result<AccountInfo, String>, new_session: Option<Session> },
    Synced(Result<SyncOutcome, String>),
    YtHome(Result<YoutubeHome, String>),
    /// The sign-in window finished.
    SignIn(signin::SignInResult),
    /// YouTube Music history for the Home shelf (fetched without a sync).
    History(Vec<Song>),
}

/// What `app.rs` should do after an account change.
pub enum AccountEffect {
    /// The feed changed (signed in / out): reload the YouTube Music home.
    ReloadHome,
    /// Library rows changed (sync).
    RefreshLibrary,
    Toast(String),
}

pub struct SyncOutcome {
    counts: SyncCounts,
    synced_playlists: BTreeSet<String>,
    synced_albums: BTreeSet<String>,
    synced_artists: BTreeSet<String>,
    history: Vec<Song>,
}

/// Requests from the settings page.
#[derive(Debug, Clone)]
enum Request {
    /// Open the Google sign-in window (`true`: wipe the old profile first).
    SignIn { fresh: bool },
    CancelSignIn,
    PasteCookie(String),
    SignOut,
    SyncNow,
}

/// Shelves Home adds for a signed-in account.
#[derive(Default)]
pub struct HomeExtras {
    /// Mixes from the youtube.com home ("Mix – …", "My Mix").
    pub mixes: Section,
    /// Music videos from the youtube.com home.
    pub videos: Section,
    /// YouTube Music history (most recent first).
    pub history: Arc<Vec<Song>>,
    pub flags: YoutubeAccountSettings,
}

/// Where the account is, as the settings page shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Status {
    #[default]
    SignedOut,
    /// Sign-in window open.
    SigningIn,
    /// Checking a captured / pasted session.
    Verifying,
    SignedIn,
    /// The stored cookie stopped working.
    Expired,
}

#[derive(Default)]
struct View {
    status: Status,
    account: AccountFile,
    syncing: bool,
    error: Option<String>,
    loading_home: bool,
    extras: Arc<HomeExtras>,
}

#[derive(Default)]
pub struct Shared {
    view: Mutex<View>,
    requests: Mutex<Vec<Request>>,
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

fn shared() -> Option<&'static Arc<Shared>> {
    SHARED.get()
}

fn request(r: Request) {
    if let Some(sh) = shared() {
        sh.requests.lock().expect("account requests").push(r);
    }
}

/// Extra Home shelves, `None` when signed out.
pub fn home_extras() -> Option<Arc<HomeExtras>> {
    let sh = shared()?;
    let v = sh.view.lock().ok()?;
    matches!(v.status, Status::SignedIn).then(|| v.extras.clone())
}

fn now_ms() -> i64 {
    crate::browse::now_ms()
}

pub struct YtAccount {
    svc: Services,
    shared: Arc<Shared>,
    /// The running sign-in window (`mochy-signin` helper).
    signin: Option<signin::Helper>,
    /// Wipe the WebView2 profile on the next sign-in (after signing out).
    clear_next_login: bool,
    vietnamese: bool,
    /// Last seen `yt_session.bin` mtime: the standalone sign-in window writes it.
    seen_session_mtime: Option<std::time::SystemTime>,
    session: Option<Session>,
    last_check: Instant,
    seen_rotations: u64,
    flags: YoutubeAccountSettings,
    started: bool,
}

impl YtAccount {
    /// Load the stored session and apply it to the InnerTube client — call
    /// before the first Home load so the feed is the account's.
    pub fn new(svc: Services) -> Self {
        let shared = SHARED.get_or_init(|| Arc::new(Shared::default())).clone();
        let session = store::load_session();
        let account = store::load_account();
        {
            let mut v = shared.view.lock().expect("account view");
            v.account = account;
            v.status = if session.is_some() { Status::SignedIn } else { Status::SignedOut };
        }
        if let Some(s) = &session {
            svc.set_auth(Some(store::to_auth(&s)), s.visitor_data.clone());
            tracing::info!("YouTube account session loaded");
        }
        // Dev hook for visual checks: open the sign-in window at start.
        if std::env::var_os("MOCHY_YT_SIGNIN").is_some() {
            shared.requests.lock().expect("account requests").push(Request::SignIn { fresh: false });
        }
        Self {
            svc,
            shared,
            signin: None,
            clear_next_login: false,
            vietnamese: false,
            seen_session_mtime: store::session_mtime(),
            session,
            last_check: Instant::now(),
            seen_rotations: 0,
            flags: YoutubeAccountSettings::default(),
            started: false,
        }
    }

    fn view(&self) -> std::sync::MutexGuard<'_, View> {
        self.shared.view.lock().expect("account view")
    }

    /// Per-frame driver: settings requests, session upkeep.
    pub fn frame(&mut self, settings: &AppSettings) -> Vec<AccountEffect> {
        let mut fx = Vec::new();
        self.vietnamese = matches!(settings.appearance.language, mochy_core::settings::Language::Vi);
        let flags = settings.youtube_account.clone();
        if !self.started {
            self.started = true;
            self.flags = flags.clone();
            self.publish_flags();
            self.on_start(&mut fx);
        } else if flags != self.flags {
            let want_home = (flags.home_mixes && !self.flags.home_mixes) || (flags.home_music_videos && !self.flags.home_music_videos);
            self.flags = flags;
            self.publish_flags();
            if want_home && self.session.is_some() {
                self.load_youtube_home();
            }
        }

        let requests: Vec<Request> = std::mem::take(&mut *self.shared.requests.lock().expect("account requests"));
        for r in requests {
            self.handle_request(r, &mut fx);
        }

        if self.last_check.elapsed() >= Duration::from_secs(5) {
            self.last_check = Instant::now();
            self.upkeep(&mut fx);
        }
        fx
    }

    fn publish_flags(&self) {
        let mut v = self.view();
        let extras = HomeExtras {
            mixes: v.extras.mixes.clone(),
            videos: v.extras.videos.clone(),
            history: v.extras.history.clone(),
            flags: self.flags.clone(),
        };
        v.extras = Arc::new(extras);
    }

    fn on_start(&mut self, _fx: &mut Vec<AccountEffect>) {
        if self.session.is_none() {
            return;
        }
        self.refresh_info(None);
        if self.flags.home_mixes || self.flags.home_music_videos {
            self.load_youtube_home();
        }
        let last = self.view().account.last_sync_ms.unwrap_or(0);
        if self.flags.sync_on_start && now_ms() - last >= AUTO_SYNC_EVERY_MS {
            self.start_sync();
        } else if self.flags.home_history {
            // History is not stored; fetch it for the Home shelf.
            self.load_history();
        }
    }

    fn handle_request(&mut self, r: Request, fx: &mut Vec<AccountEffect>) {
        match r {
            Request::SignIn { fresh } => {
                if let Some(old) = self.signin.take() {
                    old.cancel();
                }
                match signin::Helper::spawn(self.svc.bus.clone(), fresh || self.clear_next_login, self.vietnamese) {
                    Ok(h) => {
                        self.signin = Some(h);
                        self.clear_next_login = false;
                        let mut v = self.view();
                        v.error = None;
                        v.status = Status::SigningIn;
                    }
                    Err(e) => {
                        tracing::warn!("sign-in window: {e}");
                        self.view().error = Some(format!("{} {e}", tr("The sign-in window could not open:")));
                        fx.push(AccountEffect::Toast(tr("Sign-in window failed — you can paste the cookie instead.").into()));
                    }
                }
            }
            Request::CancelSignIn => {
                if let Some(h) = self.signin.take() {
                    h.cancel();
                }
                let mut v = self.view();
                v.status = if self.session.is_some() { Status::SignedIn } else { Status::SignedOut };
            }
            Request::PasteCookie(text) => {
                let session = Session { cookie: mochy_innertube::auth::normalize_cookie(&text), data_sync_id: None, visitor_data: None };
                if !store::to_auth(&session).is_signed_in() {
                    self.view().error = Some(tr("That cookie has no SAPISID — copy the whole Cookie header from a signed-in music.youtube.com request.").into());
                } else {
                    self.verify(session);
                }
            }
            Request::SignOut => self.sign_out(fx),
            Request::SyncNow => self.start_sync(),
        }
    }

    fn on_sign_in(&mut self, result: signin::SignInResult, fx: &mut Vec<AccountEffect>) {
        self.signin = None;
        match result {
            signin::SignInResult::Session(session) => self.verify(session),
            signin::SignInResult::Cancelled => {
                let mut v = self.view();
                if v.status == Status::SigningIn {
                    v.status = if self.session.is_some() { Status::SignedIn } else { Status::SignedOut };
                }
            }
            signin::SignInResult::Failed(e) => {
                tracing::warn!("sign-in window failed: {e}");
                let mut v = self.view();
                v.status = if self.session.is_some() { Status::SignedIn } else { Status::SignedOut };
                v.error = Some(format!("{} {e}", tr("The sign-in window could not open:")));
                fx.push(AccountEffect::Toast(tr("Sign-in window failed — you can paste the cookie instead.").into()));
            }
        }
    }

    /// Try a new session: apply it, ask YouTube who it is, keep it on success.
    fn verify(&mut self, session: Session) {
        {
            let mut v = self.view();
            v.status = Status::Verifying;
            v.error = None;
        }
        self.svc.set_auth(Some(store::to_auth(&session)), session.visitor_data.clone());
        self.refresh_info(Some(session));
    }

    fn refresh_info(&self, new_session: Option<Session>) {
        let svc = self.svc.clone();
        self.svc.spawn(async move {
            let result = match svc.ytm().await {
                Ok(c) => c.account_info().await.map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            svc.bus.send(AppMsg::Account(AccountMsg::Info { result, new_session }));
        });
    }

    fn load_youtube_home(&self) {
        self.view().loading_home = true;
        let svc = self.svc.clone();
        self.svc.spawn(async move {
            let result = match svc.ytm().await {
                Ok(c) => c.youtube_home(WANTED_MIXES, MAX_HOME_PAGES).await.map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            svc.bus.send(AppMsg::Account(AccountMsg::YtHome(result)));
        });
    }

    fn load_history(&self) {
        let svc = self.svc.clone();
        self.svc.spawn(async move {
            let Ok(c) = svc.ytm().await else { return };
            match c.history().await {
                Ok(history) => svc.bus.send(AppMsg::Account(AccountMsg::History(history))),
                Err(e) => tracing::warn!("YouTube history failed: {e}"),
            }
        });
    }

    fn start_sync(&self) {
        {
            let mut v = self.view();
            if v.syncing || self.session.is_none() {
                return;
            }
            v.syncing = true;
            v.error = None;
        }
        let svc = self.svc.clone();
        self.svc.spawn(async move {
            let result = sync(&svc).await;
            svc.bus.send(AppMsg::Account(AccountMsg::Synced(result)));
        });
    }

    fn sign_out(&mut self, fx: &mut Vec<AccountEffect>) {
        if let Some(h) = self.signin.take() {
            h.cancel();
        }
        self.session = None;
        self.clear_next_login = true;
        self.svc.set_auth(None, None);
        store::delete_session();
        store::delete_account();
        wipe_webview_profile();
        {
            let mut v = self.view();
            v.status = Status::SignedOut;
            v.account = AccountFile::default();
            v.error = None;
            v.syncing = false;
            v.extras = Arc::new(HomeExtras { flags: self.flags.clone(), ..Default::default() });
        }
        fx.push(AccountEffect::ReloadHome);
        fx.push(AccountEffect::Toast(tr("Signed out of YouTube").into()));
    }

    /// Persist rotated cookies; notice a dead session; pick up a session
    /// saved by `mochy-signin` started on its own.
    fn upkeep(&mut self, fx: &mut Vec<AccountEffect>) {
        let mtime = store::session_mtime();
        if mtime != self.seen_session_mtime {
            self.seen_session_mtime = mtime;
            let busy = self.signin.is_some() || self.view().status == Status::Verifying;
            if !busy
                && let Some(s) = store::load_session()
                && self.session.as_ref() != Some(&s)
            {
                tracing::info!("YouTube session saved by the sign-in window; checking it");
                self.verify(s);
            }
        }
        let Some(c) = self.svc.ytm_now() else { return };
        if self.session.is_none() {
            return;
        }
        let rotations = c.cookie_rotations();
        if rotations != self.seen_rotations {
            self.seen_rotations = rotations;
            if let (Some(auth), Some(session)) = (c.auth(), self.session.as_mut())
                && auth.cookie != session.cookie
            {
                session.cookie = auth.cookie;
                let s = session.clone();
                std::thread::spawn(move || {
                    if let Err(e) = store::save_session(&s) {
                        tracing::warn!("saving the YouTube session failed: {e}");
                    }
                });
            }
        }
        if c.session_expired() {
            let mut v = self.view();
            if v.status == Status::SignedIn {
                v.status = Status::Expired;
                drop(v);
                fx.push(AccountEffect::Toast(tr("YouTube session expired — sign in again in Settings").into()));
            }
        }
    }

    /// Apply a background result.
    pub fn on_msg(&mut self, msg: AccountMsg) -> Vec<AccountEffect> {
        let mut fx = Vec::new();
        match msg {
            AccountMsg::Info { result, new_session } => self.on_info(result, new_session, &mut fx),
            AccountMsg::History(history) => self.set_history(history),
            AccountMsg::SignIn(result) => self.on_sign_in(result, &mut fx),
            AccountMsg::Synced(result) => {
                let mut v = self.view();
                v.syncing = false;
                match result {
                    Ok(o) => {
                        let c = &o.counts;
                        let msg = crate::ui::i18n::yt_sync_summary(c.liked_new, c.playlists, c.albums, c.artists);
                        v.account.last_sync_ms = Some(now_ms());
                        v.account.last_report = Some(o.counts.clone());
                        v.account.synced_playlists = o.synced_playlists;
                        v.account.synced_albums = o.synced_albums;
                        v.account.synced_artists = o.synced_artists;
                        store::save_account(&v.account);
                        if !o.counts.errors.is_empty() {
                            v.error = Some(format!("{} {}", tr("Some lists could not be read:"), o.counts.errors.join(", ")));
                        }
                        drop(v);
                        self.set_history(o.history);
                        fx.push(AccountEffect::RefreshLibrary);
                        fx.push(AccountEffect::Toast(msg));
                    }
                    Err(e) => {
                        v.error = Some(format!("{} {e}", tr("Sync failed:")));
                    }
                }
            }
            AccountMsg::YtHome(result) => {
                let mut v = self.view();
                v.loading_home = false;
                match result {
                    Ok(home) => {
                        let extras = HomeExtras {
                            mixes: mixes_section(&home),
                            videos: videos_section(&home),
                            history: v.extras.history.clone(),
                            flags: self.flags.clone(),
                        };
                        v.extras = Arc::new(extras);
                    }
                    Err(e) => tracing::warn!("youtube.com home failed: {e}"),
                }
            }
        }
        fx
    }

    fn set_history(&self, history: Vec<Song>) {
        let mut v = self.view();
        let extras = HomeExtras {
            mixes: v.extras.mixes.clone(),
            videos: v.extras.videos.clone(),
            history: Arc::new(history),
            flags: self.flags.clone(),
        };
        v.extras = Arc::new(extras);
    }

    fn on_info(&mut self, result: Result<AccountInfo, String>, new_session: Option<Session>, fx: &mut Vec<AccountEffect>) {
        match (result, new_session) {
            (Ok(info), Some(session)) => {
                if let Err(e) = store::save_session(&session) {
                    tracing::warn!("saving the YouTube session failed: {e}");
                    fx.push(AccountEffect::Toast(format!("{} {e}", tr("Could not save the session:"))));
                }
                self.session = Some(session);
                self.seen_rotations = self.svc.ytm_now().map(|c| c.cookie_rotations()).unwrap_or(0);
                {
                    let mut v = self.view();
                    let switched = v.account.email.is_some() && v.account.email != info.email;
                    if switched {
                        // Another account: forget what the old one synced.
                        v.account = AccountFile::default();
                    }
                    apply_info(&mut v.account, &info);
                    store::save_account(&v.account);
                    v.status = Status::SignedIn;
                    v.error = None;
                }
                fx.push(AccountEffect::Toast(format!("{} {}", tr("Signed in as"), info.name)));
                fx.push(AccountEffect::ReloadHome);
                if self.flags.home_mixes || self.flags.home_music_videos {
                    self.load_youtube_home();
                }
                self.start_sync();
            }
            (Err(e), Some(_)) => {
                // The new session doesn't work: go back to the previous one.
                let prev = self.session.clone();
                self.svc.set_auth(prev.as_ref().map(store::to_auth), prev.as_ref().and_then(|s| s.visitor_data.clone()));
                let mut v = self.view();
                v.status = if prev.is_some() { Status::SignedIn } else { Status::SignedOut };
                v.error = Some(format!("{} {e}", tr("YouTube did not accept the session:")));
            }
            (Ok(info), None) => {
                let mut v = self.view();
                apply_info(&mut v.account, &info);
                store::save_account(&v.account);
            }
            (Err(e), None) => {
                tracing::warn!("account info refresh failed: {e}");
                if self.svc.ytm_now().is_some_and(|c| c.session_expired()) {
                    let mut v = self.view();
                    v.status = Status::Expired;
                }
            }
        }
    }
}

/// Delete the sign-in window's WebView2 profile (Google's own login cookies),
/// so signing out leaves nothing of the account on disk. Runs in the
/// background and retries: a just-closed `msedgewebview2` may hold file locks
/// for a moment. The next sign-in also starts clean (`--fresh`) in any case.
fn wipe_webview_profile() {
    let dir = store::webview_dir();
    if !dir.exists() {
        return;
    }
    std::thread::Builder::new()
        .name("mochy-webview-wipe".into())
        .spawn(move || {
            for attempt in 0..10 {
                match std::fs::remove_dir_all(&dir) {
                    Ok(()) => return tracing::info!("removed the sign-in WebView profile"),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
                    Err(e) if attempt == 9 => tracing::warn!("removing {} failed: {e}", dir.display()),
                    Err(_) => std::thread::sleep(Duration::from_millis(500)),
                }
            }
        })
        .ok();
}

fn apply_info(a: &mut AccountFile, info: &AccountInfo) {
    a.name = info.name.clone();
    a.email = info.email.clone();
    a.channel_handle = info.channel_handle.clone();
    a.thumbnail_url = info.thumbnail_url.clone();
}

fn mixes_section(home: &YoutubeHome) -> Section {
    let mut sec = Section { title: tr("Mixes from YouTube").to_string(), ..Default::default() };
    for m in &home.mixes {
        sec.items.push(ShelfItem::Playlist(m.to_playlist()));
        sec.subtitles.push(m.subtitle.clone().unwrap_or_default());
    }
    sec
}

fn videos_section(home: &YoutubeHome) -> Section {
    let mut sec = Section { title: tr("Music videos from YouTube").to_string(), ..Default::default() };
    for s in &home.music_videos {
        sec.subtitles.push(s.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", "));
        sec.items.push(ShelfItem::Song(s.clone()));
    }
    sec
}

// ---------------------------------------------------------------------------
// Sync
// ---------------------------------------------------------------------------

async fn sync(svc: &Services) -> Result<SyncOutcome, String> {
    let client = svc.ytm().await?;
    let snap = client.library_snapshot().await.map_err(|e| e.to_string())?;
    let prev = store::load_account();
    svc.db.query(move |db| apply_snapshot(db, snap, &prev)).await.map_err(|_| "database worker stopped".to_string())
}

/// Write a library snapshot into the local DB (one way, additive for likes):
/// - liked songs become liked locally (local likes are never removed);
/// - playlists / albums / artists are saved to the library; ones a previous
///   sync added and that left the YouTube library are removed again;
/// - entries the user had saved themselves are left alone either way.
fn apply_snapshot(db: &Db, snap: LibrarySnapshot, prev: &AccountFile) -> SyncOutcome {
    use crate::db::{ensure_song, log_err};
    let failed = |what: &str| snap.errors.iter().any(|(w, _)| w == what);
    let saved_before = log_err("saved_ids", db.saved_ids()).unwrap_or_default();
    let mut counts = SyncCounts {
        liked_total: snap.liked_songs.len(),
        playlists: snap.playlists.len(),
        albums: snap.albums.len(),
        artists: snap.artists.len(),
        history: snap.history.len(),
        errors: snap.errors.iter().map(|(w, e)| format!("{w} ({e})")).collect(),
        ..Default::default()
    };

    // Oldest like first so the newest ends up with the latest like date.
    for s in snap.liked_songs.iter().rev() {
        let already = db.get_song(&s.id).ok().flatten().is_some_and(|e| e.liked);
        if !already {
            ensure_song(db, s);
            if log_err("set_liked", db.set_liked(&s.id, true)).is_some() {
                counts.liked_new += 1;
            }
        }
    }

    let mut synced_playlists = BTreeSet::new();
    if failed("playlists") {
        synced_playlists = prev.synced_playlists.clone();
    } else {
        for p in &snap.playlists {
            let mine = saved_before.playlists.contains(&p.id) && !prev.synced_playlists.contains(&p.id);
            if mine {
                continue;
            }
            log_err("set_playlist_saved", db.set_playlist_saved(p, true));
            synced_playlists.insert(p.id.clone());
        }
        for id in prev.synced_playlists.difference(&synced_playlists) {
            let p = db.playlist_by_id(id).ok().flatten().unwrap_or_else(|| Playlist { id: id.clone(), ..Default::default() });
            log_err("unsave playlist", db.set_playlist_saved(&p, false));
            counts.removed += 1;
        }
    }

    let mut synced_albums = BTreeSet::new();
    if failed("albums") {
        synced_albums = prev.synced_albums.clone();
    } else {
        for a in &snap.albums {
            if saved_before.albums.contains(&a.id) && !prev.synced_albums.contains(&a.id) {
                continue;
            }
            log_err("set_album_saved", db.set_album_saved(a, &[], true));
            synced_albums.insert(a.id.clone());
        }
        for id in prev.synced_albums.difference(&synced_albums) {
            let a = db.get_album(id).ok().flatten().unwrap_or_else(|| Album { id: id.clone(), ..Default::default() });
            log_err("unsave album", db.set_album_saved(&a, &[], false));
            counts.removed += 1;
        }
    }

    let mut synced_artists = BTreeSet::new();
    if failed("artists") {
        synced_artists = prev.synced_artists.clone();
    } else {
        for a in &snap.artists {
            if saved_before.artists.contains(&a.id) && !prev.synced_artists.contains(&a.id) {
                continue;
            }
            log_err("set_artist_saved", db.set_artist_saved(a, true));
            synced_artists.insert(a.id.clone());
        }
        for id in prev.synced_artists.difference(&synced_artists) {
            let a = db.get_artist(id).ok().flatten().unwrap_or_else(|| Artist { id: id.clone(), ..Default::default() });
            log_err("unsave artist", db.set_artist_saved(&a, false));
            counts.removed += 1;
        }
    }

    SyncOutcome { counts, synced_playlists, synced_albums, synced_artists, history: snap.history }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mochy_core::models::ArtistRef;

    fn song(id: &str) -> Song {
        Song { id: id.into(), video_id: Some(id.into()), title: format!("T {id}"), artists: vec![ArtistRef { id: None, name: "A".into() }], ..Default::default() }
    }

    fn playlist(id: &str) -> Playlist {
        Playlist { id: id.into(), name: format!("P {id}"), browse_id: Some(format!("VL{id}")), ..Default::default() }
    }

    #[test]
    fn snapshot_sync_adds_and_prunes_only_its_own_entries() {
        let db = Db::open_in_memory().unwrap();
        // The user saved PLmine themselves before any sync.
        db.set_playlist_saved(&playlist("PLmine"), true).unwrap();
        // A local like that YouTube doesn't know about.
        db.upsert_song(&song("local")).unwrap();
        db.set_liked("local", true).unwrap();

        let snap = LibrarySnapshot {
            liked_songs: vec![song("new1"), song("new2"), song("local")],
            playlists: vec![playlist("PLmine"), playlist("PLyt1"), playlist("PLyt2")],
            albums: vec![Album { id: "MPREa".into(), title: "Al".into(), ..Default::default() }],
            artists: vec![Artist { id: "UCa".into(), name: "Ar".into(), ..Default::default() }],
            history: vec![song("h1")],
            errors: vec![],
        };
        let out = apply_snapshot(&db, snap, &AccountFile::default());
        assert_eq!(out.counts.liked_new, 2);
        assert!(db.get_song("new1").unwrap().unwrap().liked);
        assert_eq!(out.synced_playlists.iter().map(String::as_str).collect::<Vec<_>>(), ["PLyt1", "PLyt2"]);
        assert_eq!(out.synced_albums.len(), 1);
        assert_eq!(out.synced_artists.len(), 1);
        let saved = db.saved_ids().unwrap();
        assert!(saved.playlists.contains("PLyt1") && saved.playlists.contains("PLmine"));
        assert!(saved.albums.contains("MPREa") && saved.artists.contains("UCa"));

        // Second sync: PLyt2, PLmine, the album and the artist left the YouTube library.
        let prev = AccountFile {
            synced_playlists: out.synced_playlists,
            synced_albums: out.synced_albums,
            synced_artists: out.synced_artists,
            ..Default::default()
        };
        let snap2 = LibrarySnapshot { playlists: vec![playlist("PLyt1")], ..Default::default() };
        let out2 = apply_snapshot(&db, snap2, &prev);
        let saved = db.saved_ids().unwrap();
        assert!(saved.playlists.contains("PLyt1"));
        assert!(!saved.playlists.contains("PLyt2"), "sync-added playlist is pruned");
        assert!(saved.playlists.contains("PLmine"), "user's own save is kept");
        assert!(!saved.albums.contains("MPREa") && !saved.artists.contains("UCa"));
        assert_eq!(out2.counts.removed, 3);
        // Likes are never removed.
        assert!(db.get_song("new1").unwrap().unwrap().liked);
        assert!(db.get_song("local").unwrap().unwrap().liked);
    }

    #[test]
    fn failed_lists_keep_previous_entries() {
        let db = Db::open_in_memory().unwrap();
        db.set_playlist_saved(&playlist("PLyt1"), true).unwrap();
        let prev = AccountFile { synced_playlists: ["PLyt1".to_string()].into(), ..Default::default() };
        let snap = LibrarySnapshot { errors: vec![("playlists".into(), "HTTP 500".into())], ..Default::default() };
        let out = apply_snapshot(&db, snap, &prev);
        assert!(out.synced_playlists.contains("PLyt1"));
        assert!(db.saved_ids().unwrap().playlists.contains("PLyt1"));
        assert_eq!(out.counts.removed, 0);
    }
}
