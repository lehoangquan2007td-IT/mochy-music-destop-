//! eframe application: state, message pump, layout, actions, shortcuts.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use egui::{Align, Color32, Id, Key, Layout, Modifiers, RichText, TextureHandle, Ui, vec2};
use mochy_core::models::Song;
use mochy_core::settings::AppSettings;
use mochy_storage::SettingsStore;

use crate::actions::{Action, Ctx, LoadState};
use crate::controller::{CtlCfg, PlayState, PlayerController, apply_audio_lab};
use crate::ui::i18n::tr;
use crate::db::DbWorker;
use crate::images::{ImageCache, THUMB_PX};
use crate::media_keys::{MediaKey, MediaKeys};
use crate::nav::{LibTab, Nav, Route};
use crate::pages::detail::DetailEvent;
use crate::pages::library::{LibEvent, LibUi};
use crate::pages::now_playing::{NpUi, SideData};
use crate::pages::search::{SearchEvent, SearchState};
use crate::pages::settings::{SettingsCtx, SettingsEvent};
use crate::pages::{self, Data, NowInfo, SideTab};
use crate::services::{AppMsg, Bus, LibraryMsg, ScanMsg, Services};
use crate::ui::audio_lab::AudioLabView;
use crate::ui::lyrics_view::{LyricsState, LyricsView};
use crate::ui::theme;
use crate::widgets::{self, icons, icons::Icon};

const ICON_PNG: &[u8] = include_bytes!("../assets/icon/icon.png");
const SETTINGS_DEBOUNCE: Duration = Duration::from_millis(400);
const TOAST_SECS: f32 = 4.0;

pub fn run(rt: tokio::runtime::Handle, instance: Option<crate::tray::Instance>) -> eframe::Result {
    let store = SettingsStore::open_default();
    let settings = match store.load() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("settings load failed ({e}); using defaults");
            AppSettings::default()
        }
    };
    let mut vp = egui::ViewportBuilder::default()
        .with_title("Mochy Music")
        .with_app_id("mochy-music")
        .with_min_inner_size([900.0, 600.0])
        .with_inner_size(if settings.misc.window_width >= 900.0 && settings.misc.window_height >= 600.0 {
            [settings.misc.window_width, settings.misc.window_height]
        } else {
            [1280.0, 820.0]
        })
        .with_maximized(settings.misc.window_maximized);
    if let Some(icon) = load_icon() {
        vp = vp.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions { viewport: vp, ..Default::default() };
    eframe::run_native(
        "Mochy Music",
        options,
        Box::new(move |cc| Ok(Box::new(MochyApp::new(cc, rt, store, settings, instance)) as Box<dyn eframe::App>)),
    )
}

fn load_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(ICON_PNG).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width, height })
}

struct Toast {
    text: String,
    at: Instant,
}

#[allow(clippy::large_enum_variant)]
enum Dialog {
    NewPlaylist { name: String, add: Option<Song> },
    Rename { id: String, name: String },
    ConfirmDelete { id: String, name: String },
}

pub struct MochyApp {
    svc: Services,
    rx: Receiver<AppMsg>,
    settings: AppSettings,
    /// Last state persisted / applied (change detection).
    settings_seen: AppSettings,
    settings_dirty: Option<Instant>,
    player: PlayerController,
    muted_from: Option<f32>,
    images: ImageCache,
    nav: Nav,
    data: Data,
    search: SearchState,
    lib: LibUi,
    np: NpUi,
    np_open: bool,
    np_tab: SideTab,
    side_panel: Option<SideTab>,
    lyrics: LyricsState,
    lyrics_for: Option<String>,
    lyrics_view: LyricsView,
    audio_lab_view: AudioLabView,
    accent: Color32,
    accent_target: Color32,
    accent_applied: (Color32, bool),
    art_colors: Option<(String, Color32, (Color32, Color32))>,
    media: Option<MediaKeys>,
    toasts: Vec<Toast>,
    actions: Vec<Action>,
    last_uid: Option<u64>,
    devices: Option<Vec<String>>,
    ytm_status: Option<Result<(), String>>,
    db_error: Option<String>,
    scan_cancel: Arc<AtomicBool>,
    log_file: Option<PathBuf>,
    dialog: Option<Dialog>,
    logo: Option<TextureHandle>,
    detail_arc: Option<(String, Arc<Vec<Song>>)>,
    window_seen: (f32, f32, bool),
    started: Instant,
    copy_request: Option<String>,
    /// `MOCHY_SHOTS=<dir>`: visual-check automation (screenshot pages, then exit).
    shots: Option<ShotRunner>,
    /// Discord presence, scrobbling, playlist importer.
    integrations: crate::integrations::Integrations,
    /// Explore / browse / library parity / stats state.
    browse: crate::browse::BrowseState,
    /// Notification-area icon (run in background).
    tray: crate::tray::Tray,
    /// Window hidden to the tray; the app keeps playing.
    hidden_to_tray: bool,
    /// Quit was chosen (tray menu): let the close go through.
    quitting: bool,
    /// Signed-in YouTube account (sign-in window, sync, youtube.com mixes).
    account: crate::account::YtAccount,
}

impl MochyApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        rt: tokio::runtime::Handle,
        _store: SettingsStore,
        settings: AppSettings,
        instance: Option<crate::tray::Instance>,
    ) -> Self {
        let ctx = cc.egui_ctx.clone();
        let (bus, rx) = Bus::new();
        bus.set_ctx(ctx.clone());

        let accent = initial_accent(&settings);
        theme::apply(&ctx, accent, settings.appearance.pure_black);

        let db_bus = bus.clone();
        let db = DbWorker::spawn(move |r| {
            if let Err(e) = r {
                db_bus.toast(format!("Database error: {e}"));
                db_bus.send(AppMsg::Toast("Using a temporary in-memory database".into()));
            }
        });
        let svc = Services::new(rt, bus.clone(), db);

        // Audio engine + event forwarder.
        let (etx, erx) = std::sync::mpsc::channel();
        let engine = match mochy_audio::spawn_engine(etx) {
            Ok(e) => Some(e),
            Err(e) => {
                tracing::error!("audio engine failed to start: {e}");
                bus.toast(format!("Audio engine failed: {e}"));
                None
            }
        };
        {
            let bus = bus.clone();
            std::thread::Builder::new()
                .name("mochy-engine-events".into())
                .spawn(move || {
                    while let Ok(ev) = erx.recv() {
                        bus.send(AppMsg::Engine(ev));
                    }
                })
                .ok();
        }
        if let Some(e) = &engine {
            if settings.playback.output_device.is_some() {
                e.send(mochy_audio::Command::SetOutputDevice { name: settings.playback.output_device.clone() });
            }
            apply_audio_lab(e, &settings.audio_lab);
            crate::controller::apply_transition_settings(e, &settings);
        }

        let mut player = PlayerController::new(Some(svc.clone()), engine, CtlCfg::from_settings(&settings));
        player.set_volume(settings.playback.player_volume);
        player.set_repeat(settings.playback.repeat_mode);
        player.set_shuffle(settings.playback.shuffle_mode);
        player.load_song_tunings();
        if settings.misc.persistent_queue {
            player.request_restore();
        }

        if let Some(i) = &instance {
            i.listen(bus.clone());
        }
        let tray = crate::tray::Tray::new(bus.clone(), ICON_PNG);
        // Media keys (needs the native window handle on Windows).
        let hwnd = native_hwnd(cc);
        let media = match MediaKeys::new(bus.clone(), hwnd) {
            Ok(m) => {
                tracing::info!("OS media controls attached");
                Some(m)
            }
            Err(e) => {
                tracing::warn!("OS media controls unavailable: {e}");
                None
            }
        };

        let logo = load_icon().map(|i| {
            let img = egui::ColorImage::from_rgba_unmultiplied([i.width as usize, i.height as usize], &i.rgba);
            ctx.load_texture("mochy_logo", img, egui::TextureOptions::LINEAR)
        });

        crate::downloads::init(svc.clone(), ctx.clone(), &settings);
        crate::songedit::init(&svc);
        let images = ImageCache::new(svc.clone());
        let window_seen = (settings.misc.window_width, settings.misc.window_height, settings.misc.window_maximized);
        let mut app = Self {
            rx,
            settings_seen: settings.clone(),
            settings,
            settings_dirty: None,
            player,
            muted_from: None,
            images,
            nav: Nav::default(),
            data: Data::default(),
            search: SearchState::default(),
            lib: LibUi::default(),
            np: NpUi::default(),
            np_open: false,
            np_tab: SideTab::Lyrics,
            side_panel: None,
            lyrics: LyricsState::Idle,
            lyrics_for: None,
            lyrics_view: LyricsView::default(),
            audio_lab_view: AudioLabView::default(),
            accent,
            accent_target: accent,
            accent_applied: (accent, false),
            art_colors: None,
            media,
            toasts: Vec::new(),
            actions: Vec::new(),
            last_uid: None,
            devices: None,
            ytm_status: None,
            db_error: None,
            scan_cancel: Arc::new(AtomicBool::new(false)),
            log_file: Some(crate::logging::log_file()),
            dialog: None,
            logo,
            detail_arc: None,
            window_seen,
            started: Instant::now(),
            copy_request: None,
            integrations: crate::integrations::Integrations::new(svc.clone()),
            // Applies the stored YouTube session before the first Home load below.
            account: crate::account::YtAccount::new(svc.clone()),
            tray,
            hidden_to_tray: false,
            quitting: false,
            shots: std::env::var_os("MOCHY_SHOTS").map(|d| ShotRunner { dir: d.into(), started: std::time::Instant::now(), step: 0, taken: false }),
            svc,
            browse: Default::default(),
        };
        app.accent_applied.1 = app.settings.appearance.pure_black;
        app.load_home();
        app.refresh_library();
        app.refresh_devices();
        app
    }

    // ---------------------------------------------------------------------
    // Requests
    // ---------------------------------------------------------------------

    fn load_home(&mut self) {
        // Home feed with chips + continuation lives in `BrowseState`.
        self.browse.load_home(&self.svc);
    }

    fn refresh_library(&self) {
        let bus = self.svc.bus.clone();
        self.svc.db.run(move |db| {
            use crate::db::log_err;
            if let Some(v) = log_err("liked_songs", db.liked_songs()) {
                bus.send(AppMsg::Library(LibraryMsg::Liked(v)));
            }
            if let Some(v) = log_err("recent_history", db.recent_history(200)) {
                bus.send(AppMsg::Library(LibraryMsg::History(v)));
            }
            if let Some(v) = log_err("most_played", db.most_played(100)) {
                bus.send(AppMsg::Library(LibraryMsg::MostPlayed(v)));
            }
            if let Some(v) = log_err("all_songs", db.all_songs()) {
                let mut local: Vec<Song> = v.into_iter().filter(|s| s.is_local()).collect();
                local.sort_by_key(|a| a.title.to_lowercase());
                bus.send(AppMsg::Library(LibraryMsg::Local(local)));
            }
            if let Some(v) = log_err("playlists", db.playlists()) {
                for p in &v {
                    if let Some(s) = log_err("playlist_songs", db.playlist_songs(&p.id)) {
                        bus.send(AppMsg::Library(LibraryMsg::PlaylistSongs { id: p.id.clone(), songs: s }));
                    }
                }
                bus.send(AppMsg::Library(LibraryMsg::Playlists(v)));
            }
            if let Some(v) = log_err("search_history", db.search_history(30)) {
                bus.send(AppMsg::Library(LibraryMsg::SearchHistory(v)));
            }
        });
        self.browse.refresh_library(&self.svc);
    }

    fn refresh_playlists(&self) {
        let bus = self.svc.bus.clone();
        self.svc.db.run(move |db| {
            if let Some(v) = crate::db::log_err("playlists", db.playlists()) {
                bus.send(AppMsg::Library(LibraryMsg::Playlists(v)));
            }
        });
    }

    fn refresh_playlist_songs(&self, id: &str) {
        let bus = self.svc.bus.clone();
        let id = id.to_string();
        self.svc.db.run(move |db| {
            if let Some(s) = crate::db::log_err("playlist_songs", db.playlist_songs(&id)) {
                bus.send(AppMsg::Library(LibraryMsg::PlaylistSongs { id, songs: s }));
            }
        });
    }

    fn refresh_devices(&self) {
        let bus = self.svc.bus.clone();
        std::thread::spawn(move || bus.send(AppMsg::OutputDevices(mochy_audio::output_devices())));
    }

    fn ensure_route_data(&mut self) {
        match self.nav.current().clone() {
            Route::Album(id) if !self.data.albums.contains_key(&id) => {
                self.data.albums.insert(id.clone(), LoadState::Loading);
                let i2 = id.clone();
                self.svc.ytm_call("album", move |c| async move { c.album(&i2).await }, move |result| AppMsg::Album { id, result });
            }
            Route::Artist(id) if !self.data.artists.contains_key(&id) => {
                self.data.artists.insert(id.clone(), LoadState::Loading);
                let i2 = id.clone();
                self.svc.ytm_call("artist", move |c| async move { c.artist(&i2).await }, move |result| AppMsg::Artist { id, result });
            }
            Route::Playlist(id) if !self.data.playlists_remote.contains_key(&id) => {
                self.data.playlists_remote.insert(id.clone(), LoadState::Loading);
                let i2 = id.clone();
                self.svc.ytm_call("playlist", move |c| async move { c.playlist(&i2).await }, move |result| AppMsg::Playlist { id, result });
            }
            Route::LocalPlaylist(id) if !self.data.playlist_songs.contains_key(&id) => {
                self.refresh_playlist_songs(&id);
            }
            r => self.browse.ensure(&r, &self.svc),
        }
    }

    fn ensure_related(&mut self) {
        let Some(vid) = self.player.current().and_then(|s| s.video_id.clone()) else { return };
        if self.data.related.contains_key(&vid) {
            return;
        }
        self.data.related.insert(vid.clone(), LoadState::Loading);
        let v2 = vid.clone();
        self.svc.ytm_call("related", move |c| async move { c.related(&v2).await }, move |result| AppMsg::Related { video_id: vid, result });
    }

    fn ensure_lyrics(&mut self) {
        if !self.settings.lyrics.show {
            return;
        }
        let Some(song) = self.player.current().cloned() else { return };
        if self.lyrics_for.as_deref() == Some(song.id.as_str()) {
            return;
        }
        self.lyrics_for = Some(song.id.clone());
        self.lyrics = LyricsState::Loading;
        crate::lyrics::spawn_load(&self.svc, song, crate::lyrics::provider_order(&self.settings.lyrics));
    }

    fn submit_search(&mut self, q: String) {
        self.search.submitted = Some(q.clone());
        self.search.suggest_open = false;
        self.search.results = Some(LoadState::Loading);
        self.search.query = q.clone();
        self.search.filter = crate::pages::search::Filter::All;
        let q2 = q.clone();
        let q3 = q.clone();
        self.svc.ytm_call("search", move |c| async move { c.search_all(&q2).await }, move |result| AppMsg::Search { query: q3, result });
        let bus = self.svc.bus.clone();
        self.svc.db.run(move |db| {
            crate::db::log_err("add_search_history", db.add_search_history(&q));
            if let Some(v) = crate::db::log_err("search_history", db.search_history(30)) {
                bus.send(AppMsg::Library(LibraryMsg::SearchHistory(v)));
            }
        });
        if !matches!(self.nav.current(), Route::Search) {
            self.nav.go(Route::Search);
        }
    }

    // ---------------------------------------------------------------------
    // Messages
    // ---------------------------------------------------------------------

    fn pump(&mut self, ctx: &egui::Context) {
        let mut n = 0;
        while let Ok(msg) = self.rx.try_recv() {
            self.on_msg(ctx, msg);
            n += 1;
            if n > 500 {
                ctx.request_repaint();
                break;
            }
        }
    }

    fn on_msg(&mut self, ctx: &egui::Context, msg: AppMsg) {
        match msg {
            AppMsg::Engine(ev) => {
                if matches!(ev, mochy_audio::Event::Finished | mochy_audio::Event::NextStarted { .. }) {
                    // History changed.
                    self.browse.mark_stale();
                    self.refresh_library_soon();
                }
                self.player.on_engine(ev);
            }
            AppMsg::YtmReady(r) => {
                if r.is_ok() && self.browse.home_failed() {
                    self.load_home();
                }
                self.ytm_status = Some(r);
            }
            AppMsg::Home(r) => {
                if let Ok(h) = &r {
                    tracing::info!(shelves = h.len(), "home loaded");
                }
                self.data.home = Some(LoadState::from_result(r));
            }
            AppMsg::Suggestions { query, items } => {
                if query == self.search.query.trim() {
                    self.search.suggestions_for = query;
                    self.search.suggestions = items;
                }
            }
            AppMsg::Search { query, result } => self.search.set_results(&query, result),
            AppMsg::Album { id, result } => {
                let r = result.map(|(al, mut songs)| {
                    for s in &mut songs {
                        if s.album_id.is_none() {
                            s.album_id = Some(al.id.clone());
                        }
                        if s.album_name.is_none() {
                            s.album_name = Some(al.title.clone());
                        }
                        if s.thumbnail_url.is_none() {
                            s.thumbnail_url = al.thumbnail_url.clone();
                        }
                    }
                    (al, songs)
                });
                self.data.albums.insert(id, LoadState::from_result(r));
            }
            AppMsg::Artist { id, result } => {
                self.data.artists.insert(id, LoadState::from_result(result));
            }
            AppMsg::Playlist { id, result } => {
                self.data.playlists_remote.insert(id, LoadState::from_result(result));
            }
            AppMsg::Related { video_id, result } => {
                self.data.related.insert(video_id, LoadState::from_result(result));
            }
            AppMsg::Prepared { generation, uid, result } => {
                crate::downloads::observe_prepared(&self.player, uid, &result);
                self.player.on_prepared(generation, uid, result)
            }
            AppMsg::Prefetched { video_id, stream } => self.player.on_prefetched(video_id, stream),
            AppMsg::Radio { seed, result, play_after } => self.player.on_radio(seed, result, play_after),
            AppMsg::QueueRestored { songs, index } => self.player.on_restored(songs, index),
            AppMsg::Lyrics { song_id, state } => {
                if self.lyrics_for.as_deref() == Some(song_id.as_str()) {
                    self.lyrics = state;
                }
            }
            AppMsg::Image(m) => self.images.on_msg(ctx, m),
            AppMsg::Library(m) => match m {
                LibraryMsg::Liked(v) => {
                    self.data.liked_ids = v.iter().map(|s| s.id.clone()).collect::<std::collections::HashSet<_>>();
                    self.data.liked = Arc::new(v);
                }
                LibraryMsg::History(v) => self.data.history = Arc::new(v),
                LibraryMsg::MostPlayed(v) => self.data.most_played = Arc::new(v),
                LibraryMsg::Local(v) => self.data.local = Arc::new(v),
                LibraryMsg::Playlists(v) => self.data.playlists = v,
                LibraryMsg::PlaylistSongs { id, songs } => {
                    self.data.playlist_songs.insert(id, Arc::new(songs));
                }
                LibraryMsg::SearchHistory(v) => self.data.search_history = v,
            },
            AppMsg::Scan(m) => match m {
                ScanMsg::Started(dir) => {
                    self.lib.scanning = true;
                    self.lib.scan_status = Some(crate::ui::i18n::scan_dir(&dir.display().to_string()));
                }
                ScanMsg::Progress { scanned, added } => {
                    self.lib.scan_status = Some(crate::ui::i18n::scan_progress(scanned, added));
                }
                ScanMsg::Done { added } => {
                    self.lib.scanning = false;
                    self.lib.scan_status = Some(crate::ui::i18n::scan_added(added));
                    self.refresh_library();
                }
                ScanMsg::Cancelled => {
                    self.lib.scanning = false;
                    self.lib.scan_status = Some(tr("Scan cancelled").into());
                    self.refresh_library();
                }
            },
            AppMsg::MediaKey(k) => self.on_media_key(ctx, k),
            AppMsg::Tray(c) => self.on_tray(ctx, c),
            AppMsg::OutputDevices(d) => {
                tracing::info!(count = d.len(), "output devices");
                self.devices = Some(d);
            }
            AppMsg::Toast(t) => self.toast(t),
            AppMsg::Account(m) => {
                let fx = self.account.on_msg(m);
                self.run_account_effects(fx);
            }
            AppMsg::Browse(m) => {
                let effects = self.browse.on_msg(ctx, m, &self.svc);
                self.run_browse_effects(effects);
            }
        }
    }

    fn refresh_library_soon(&mut self) {
        // Cheap enough to just do now (one DB round trip).
        self.refresh_library();
    }

    fn run_account_effects(&mut self, fx: Vec<crate::account::AccountEffect>) {
        use crate::account::AccountEffect;
        for e in fx {
            match e {
                AccountEffect::ReloadHome => self.load_home(),
                AccountEffect::RefreshLibrary => self.refresh_library(),
                AccountEffect::Toast(t) => self.toast(t),
            }
        }
    }

    fn toast(&mut self, text: impl Into<String>) {
        let text = text.into();
        tracing::info!("toast: {text}");
        self.toasts.push(Toast { text, at: Instant::now() });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }

    fn on_media_key(&mut self, ctx: &egui::Context, k: MediaKey) {
        match k {
            MediaKey::Play => self.player.play(),
            MediaKey::Pause => self.player.pause(),
            MediaKey::Toggle => self.player.toggle(),
            MediaKey::Next => self.player.next(),
            MediaKey::Previous => self.player.prev(),
            MediaKey::Stop => self.player.pause(),
            MediaKey::SeekBy(d) => self.player.seek(self.player.pos + d),
            MediaKey::SetPosition(p) => self.player.seek(p),
            MediaKey::Raise => self.show_window(ctx),
        }
    }

    // ---------------------------------------------------------------------
    // Actions
    // ---------------------------------------------------------------------

    fn apply_actions(&mut self) {
        let actions = std::mem::take(&mut self.actions);
        for a in actions {
            self.apply(a);
        }
    }

    fn apply(&mut self, a: Action) {
        match a {
            Action::Navigate(r) => {
                if matches!(r, Route::Library(_)) {
                    self.refresh_library();
                }
                if let Route::LocalPlaylist(id) = &r {
                    self.refresh_playlist_songs(id);
                }
                // Retry failed detail pages on navigation.
                match &r {
                    Route::Album(id) if matches!(self.data.albums.get(id), Some(LoadState::Failed(_))) => {
                        self.data.albums.remove(id);
                    }
                    Route::Artist(id) if matches!(self.data.artists.get(id), Some(LoadState::Failed(_))) => {
                        self.data.artists.remove(id);
                    }
                    Route::Playlist(id) if matches!(self.data.playlists_remote.get(id), Some(LoadState::Failed(_))) => {
                        self.data.playlists_remote.remove(id);
                    }
                    _ => {}
                }
                if matches!((self.nav.current(), &r), (Route::Library(_), Route::Library(_))) {
                    self.nav.replace(r);
                } else {
                    self.nav.go(r);
                }
            }
            Action::Back => self.nav.back(),
            Action::Forward => self.nav.forward(),
            Action::PlayList { songs, start, shuffle } => {
                let list = songs.as_ref().clone();
                if shuffle {
                    let start = if list.len() > 1 { pseudo_rand(list.len()) } else { 0 };
                    self.player.play_songs(list, start, true);
                    self.settings.playback.shuffle_mode = true;
                } else {
                    self.player.play_songs(list, start, self.player.queue.shuffle());
                }
            }
            Action::PlaySong(s) | Action::StartRadio(s) => {
                if s.video_id.is_some() {
                    self.player.play_with_radio(s);
                } else {
                    self.player.play_songs(vec![s], 0, false);
                }
            }
            Action::PlayNext(s) => {
                self.toast(format!("{}: {}", tr("Playing next"), s.title));
                self.player.play_next(s);
            }
            Action::AddToQueue(v) => {
                self.toast(if v.len() == 1 { format!("{}: {}", tr("Added to queue"), v[0].title) } else { crate::ui::i18n::added_to_queue(v.len()) });
                self.player.add_to_queue(v);
            }
            Action::ToggleLike(s) => {
                let like = !self.data.liked_ids.contains(&s.id);
                self.integrations.on_like(&s, like);
                if like {
                    self.data.liked_ids.insert(s.id.clone());
                } else {
                    self.data.liked_ids.remove(&s.id);
                }
                self.player.queue.for_each_song_mut(&s.id, |x| x.liked = like);
                let bus = self.svc.bus.clone();
                self.svc.db.run(move |db| {
                    crate::db::ensure_song(db, &s);
                    crate::db::log_err("set_liked", db.set_liked(&s.id, like));
                    if like {
                        crate::db::log_err("set_in_library", db.set_in_library(&s.id, true));
                    }
                    if let Some(v) = crate::db::log_err("liked_songs", db.liked_songs()) {
                        bus.send(AppMsg::Library(LibraryMsg::Liked(v)));
                    }
                });
            }
            Action::AddToPlaylist { playlist_id, song } => {
                let name = self.data.playlists.iter().find(|p| p.id == playlist_id).map(|p| p.name.clone()).unwrap_or_default();
                self.toast(format!("{} {name}", tr("Added to")));
                let bus = self.svc.bus.clone();
                self.svc.db.run(move |db| {
                    crate::db::ensure_song(db, &song);
                    crate::db::log_err("add_to_playlist", db.add_to_playlist(&playlist_id, &song, None));
                    if let Some(s) = crate::db::log_err("playlist_songs", db.playlist_songs(&playlist_id)) {
                        bus.send(AppMsg::Library(LibraryMsg::PlaylistSongs { id: playlist_id, songs: s }));
                    }
                });
            }
            Action::NewPlaylist(add) => self.dialog = Some(Dialog::NewPlaylist { name: String::new(), add }),
            Action::RemoveFromPlaylist { playlist_id, song_id } => {
                let bus = self.svc.bus.clone();
                self.svc.db.run(move |db| {
                    crate::db::log_err("remove_from_playlist", db.remove_from_playlist(&playlist_id, &song_id));
                    if let Some(s) = crate::db::log_err("playlist_songs", db.playlist_songs(&playlist_id)) {
                        bus.send(AppMsg::Library(LibraryMsg::PlaylistSongs { id: playlist_id, songs: s }));
                    }
                });
            }
            Action::RenamePlaylist(id) => {
                let name = self.data.playlists.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default();
                self.dialog = Some(Dialog::Rename { id, name });
            }
            Action::DeletePlaylist(id) => {
                let name = self.data.playlists.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default();
                self.dialog = Some(Dialog::ConfirmDelete { id, name });
            }
            Action::ChangePlaylistCover(id) => crate::playlist_cover::pick(&self.svc, id),
            Action::RemovePlaylistCover(id) => crate::playlist_cover::remove(&self.svc, id),
            Action::SetNowPlaying(open) => {
                self.np_open = open;
                if open {
                    self.np.scroll_to_current = true;
                }
            }
            Action::QueuePlay(i) => self.player.play_index(i),
            Action::QueueRemove(i) => self.player.remove(i),
            Action::QueueMove { from, to } => self.player.move_entry(from, to),
            Action::QueueClear => self.player.clear_queue(),
            Action::Seek(t) => self.player.seek(t),
            Action::CopyText(t) => {
                self.copy_request = Some(t);
                self.toast(tr("Link copied"));
            }
            Action::Toggle => self.player.toggle(),
            Action::Next => self.player.next(),
            Action::Prev => self.player.prev(),
            Action::SetVolume(v) => {
                self.muted_from = None;
                self.player.set_volume(v);
                self.settings.playback.player_volume = v;
            }
            Action::ToggleMute => {
                if let Some(v) = self.muted_from.take() {
                    self.player.set_volume(v);
                } else {
                    self.muted_from = Some(self.player.volume);
                    let keep = self.player.volume;
                    self.player.set_volume(0.0);
                    self.player.volume = keep;
                }
            }
            Action::ToggleShuffle => {
                let on = !self.player.queue.shuffle();
                self.player.set_shuffle(on);
                self.settings.playback.shuffle_mode = on;
            }
            Action::CycleRepeat => {
                self.settings.playback.repeat_mode = self.player.cycle_repeat();
            }
            Action::ToggleSidePanel(tab) => {
                if self.side_panel == Some(tab) {
                    self.side_panel = None;
                } else {
                    self.side_panel = Some(tab);
                    self.np.scroll_to_current = true;
                }
            }
            Action::LyricsOffset(delta) => {
                if let Some(id) = self.player.current().map(|s| s.id.clone()) {
                    let mut new_offset = 0;
                    self.player.queue.for_each_song_mut(&id, |s| {
                        s.lyrics_offset_ms += delta;
                        new_offset = s.lyrics_offset_ms;
                    });
                    self.svc.db.run(move |db| {
                        crate::db::log_err("set_lyrics_offset", db.set_lyrics_offset(&id, new_offset));
                    });
                }
            }
            Action::Transitions(t) => crate::ui::transitions_ui::apply(&mut self.player, &mut self.settings, t),
            Action::Browse(a) => {
                let effects = self.browse.apply(a, &self.svc, &self.data);
                self.run_browse_effects(effects);
            }
        }
    }

    fn handle_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.dialog else { return };
        let mut close = false;
        let mut confirm = false;
        let title = match dialog {
            Dialog::NewPlaylist { .. } => tr("New playlist"),
            Dialog::Rename { .. } => tr("Rename playlist"),
            Dialog::ConfirmDelete { .. } => tr("Delete playlist"),
        };
        let modal = egui::Modal::new(Id::unique("mochy_dialog")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.label(RichText::new(title).size(18.0).strong());
            ui.add_space(8.0);
            match dialog {
                Dialog::NewPlaylist { name, .. } | Dialog::Rename { name, .. } => {
                    let r = ui.add(egui::TextEdit::singleline(name).hint_text(tr("Playlist name")).desired_width(f32::INFINITY));
                    r.request_focus();
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !name.trim().is_empty() {
                        confirm = true;
                    }
                }
                Dialog::ConfirmDelete { name, .. } => {
                    ui.label(crate::ui::i18n::delete_confirm(name));
                }
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let ok = match dialog {
                    Dialog::ConfirmDelete { .. } => tr("Delete"),
                    Dialog::Rename { .. } => tr("Rename"),
                    Dialog::NewPlaylist { .. } => tr("Create"),
                };
                if ui.button(ok).clicked() {
                    confirm = true;
                }
                if ui.button(tr("Cancel")).clicked() {
                    close = true;
                }
            });
        });
        if modal.should_close() {
            close = true;
        }
        if confirm {
            let d = self.dialog.take().expect("dialog");
            let bus = self.svc.bus.clone();
            match d {
                Dialog::NewPlaylist { name, add } => {
                    let name = name.trim().to_string();
                    if name.is_empty() {
                        return;
                    }
                    self.toast(format!("{} {name}", tr("Created playlist")));
                    self.svc.db.run(move |db| {
                        if let Some(id) = crate::db::log_err("create_playlist", db.create_playlist(&name))
                            && let Some(song) = add
                        {
                            crate::db::ensure_song(db, &song);
                            crate::db::log_err("add_to_playlist", db.add_to_playlist(&id, &song, None));
                            if let Some(s) = crate::db::log_err("playlist_songs", db.playlist_songs(&id)) {
                                bus.send(AppMsg::Library(LibraryMsg::PlaylistSongs { id, songs: s }));
                            }
                        }
                        if let Some(v) = crate::db::log_err("playlists", db.playlists()) {
                            bus.send(AppMsg::Library(LibraryMsg::Playlists(v)));
                        }
                    });
                }
                Dialog::Rename { id, name } => {
                    let name = name.trim().to_string();
                    self.svc.db.run(move |db| {
                        crate::db::log_err("rename_playlist", db.rename_playlist(&id, &name));
                        if let Some(v) = crate::db::log_err("playlists", db.playlists()) {
                            bus.send(AppMsg::Library(LibraryMsg::Playlists(v)));
                        }
                    });
                }
                Dialog::ConfirmDelete { id, name } => {
                    self.toast(format!("{} {name}", tr("Deleted")));
                    if matches!(self.nav.current(), Route::LocalPlaylist(x) if *x == id) {
                        self.nav.go(Route::Library(LibTab::Playlists));
                    }
                    self.data.playlist_songs.remove(&id);
                    self.svc.db.run(move |db| {
                        crate::db::log_err("delete_playlist", db.delete_playlist(&id));
                        crate::playlist_cover::delete_files(&id, None);
                        if let Some(v) = crate::db::log_err("playlists", db.playlists()) {
                            bus.send(AppMsg::Library(LibraryMsg::Playlists(v)));
                        }
                    });
                }
            }
            self.refresh_playlists();
        } else if close {
            self.dialog = None;
        }
    }

    // ---------------------------------------------------------------------
    // Per-frame housekeeping
    // ---------------------------------------------------------------------

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input();
        let mut acts = Vec::new();
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::COMMAND, Key::F) {
                acts.push(0);
            }
            if i.consume_key(Modifiers::COMMAND, Key::ArrowRight) {
                acts.push(1);
            }
            if i.consume_key(Modifiers::COMMAND, Key::ArrowLeft) {
                acts.push(2);
            }
            if i.consume_key(Modifiers::COMMAND, Key::ArrowUp) {
                acts.push(3);
            }
            if i.consume_key(Modifiers::COMMAND, Key::ArrowDown) {
                acts.push(4);
            }
            if i.consume_key(Modifiers::ALT, Key::ArrowLeft) || i.pointer.button_pressed(egui::PointerButton::Extra1) {
                acts.push(9);
            }
            if i.consume_key(Modifiers::ALT, Key::ArrowRight) || i.pointer.button_pressed(egui::PointerButton::Extra2) {
                acts.push(10);
            }
            if !typing {
                if i.consume_key(Modifiers::NONE, Key::Space) {
                    acts.push(5);
                }
                if i.consume_key(Modifiers::NONE, Key::ArrowRight) {
                    acts.push(6);
                }
                if i.consume_key(Modifiers::NONE, Key::ArrowLeft) {
                    acts.push(7);
                }
                if i.consume_key(Modifiers::NONE, Key::Escape) {
                    acts.push(8);
                }
            }
            if i.consume_key(Modifiers::NONE, Key::MediaPlayPause) {
                acts.push(5);
            }
            if i.consume_key(Modifiers::NONE, Key::MediaTrackNext) {
                acts.push(1);
            }
            if i.consume_key(Modifiers::NONE, Key::MediaTrackPrevious) {
                acts.push(2);
            }
        });
        for a in acts {
            match a {
                0 => {
                    self.np_open = false;
                    self.nav.go(Route::Search);
                    self.search.focus_request = true;
                }
                1 => self.player.next(),
                2 => self.player.prev(),
                3 => self.apply(Action::SetVolume((self.player.volume + 0.05).min(1.0))),
                4 => self.apply(Action::SetVolume((self.player.volume - 0.05).max(0.0))),
                5 => self.player.toggle(),
                6 => self.player.seek(self.player.pos + 5.0),
                7 => self.player.seek(self.player.pos - 5.0),
                8 => {
                    if self.np_open {
                        self.np_open = false;
                    } else if self.side_panel.is_some() {
                        self.side_panel = None;
                    }
                }
                9 => self.nav.back(),
                10 => self.nav.forward(),
                _ => {}
            }
        }
    }

    fn settings_housekeeping(&mut self, ctx: &egui::Context) {
        if self.settings != self.settings_seen {
            let old = std::mem::replace(&mut self.settings_seen, self.settings.clone());
            self.settings_dirty = Some(Instant::now());
            self.player.cfg = CtlCfg::from_settings(&self.settings);
            crate::downloads::on_settings(&self.settings);
            if let Some(e) = self.player.engine() {
                crate::controller::apply_transition_settings(e, &self.settings);
            }
            if old.audio_lab.normalize_replay_gain != self.settings.audio_lab.normalize_replay_gain
                || old.audio_lab.normalize_preamp_db != self.settings.audio_lab.normalize_preamp_db
            {
                // Takes effect from the next track (gain is computed at load).
                tracing::debug!("replay gain settings changed");
            }
            if old.lyrics.provider_order != self.settings.lyrics.provider_order || (!old.lyrics.show && self.settings.lyrics.show) {
                self.lyrics_for = None;
            }
        }
        if let Some(t) = self.settings_dirty {
            if t.elapsed() >= SETTINGS_DEBOUNCE {
                self.settings_dirty = None;
                self.save_settings();
            } else {
                ctx.request_repaint_after(SETTINGS_DEBOUNCE);
            }
        }
    }

    fn save_settings(&self) {
        let s = self.settings.clone();
        self.svc.db.run(move |_| {
            if let Err(e) = SettingsStore::open_default().save(&s) {
                tracing::warn!("settings save failed: {e}");
            }
        });
    }

    fn window_housekeeping(&mut self, ctx: &egui::Context) {
        let (rect, maximized) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().maximized));
        let max = maximized.unwrap_or(false);
        let (mut w, mut h) = (self.window_seen.0, self.window_seen.1);
        if !max && let Some(r) = rect {
            w = r.width().round();
            h = r.height().round();
        }
        if (w, h, max) != self.window_seen {
            self.window_seen = (w, h, max);
            // Don't record the transient size of the very first frames.
            if self.started.elapsed() > Duration::from_secs(1) {
                self.settings.misc.window_width = w;
                self.settings.misc.window_height = h;
                self.settings.misc.window_maximized = max;
            }
        }
    }

    fn theme_housekeeping(&mut self, ctx: &egui::Context) {
        // Colors from the current artwork.
        let src = self.player.current().and_then(crate::images::song_src);
        if let Some(src) = &src {
            let key = src.key(THUMB_PX);
            if self.art_colors.as_ref().map(|a| &a.0) != Some(&key) {
                // Request (no-op if loaded) so the small copy exists.
                let _ = self.images.get(src, THUMB_PX);
                if let Some(small) = self.images.small(src, THUMB_PX) {
                    let a = theme::extract_accent(&small.rgba, small.w, small.h);
                    let g = theme::extract_gradient(&small.rgba, small.w, small.h);
                    self.art_colors = Some((key, a, g));
                }
            }
        } else {
            self.art_colors = None;
        }
        let ap = &self.settings.appearance;
        self.accent_target = if ap.dynamic_theme {
            self.art_colors.as_ref().map(|a| a.1).unwrap_or(theme::DEFAULT_ACCENT)
        } else {
            ap.selected_theme_color.map(rgb_u32).unwrap_or(theme::DEFAULT_ACCENT)
        };
        if self.accent != self.accent_target {
            let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
            let speed = 5.0 * ap.animation_speed.max(0.1);
            let t = 1.0 - (-dt * speed).exp();
            let next = widgets::lerp_color(self.accent, self.accent_target, t.max(0.04));
            let close = |a: u8, b: u8| a.abs_diff(b) <= 2;
            self.accent = if next == self.accent
                || (close(next.r(), self.accent_target.r()) && close(next.g(), self.accent_target.g()) && close(next.b(), self.accent_target.b()))
            {
                self.accent_target
            } else {
                next
            };
            ctx.request_repaint();
        }
        // theme::apply only when settled (it may rebuild fonts/visuals).
        let want = (self.accent, self.settings.appearance.pure_black);
        if self.accent == self.accent_target && want != self.accent_applied {
            theme::apply(ctx, want.0, want.1);
            self.accent_applied = want;
        }
    }

    fn track_change_housekeeping(&mut self) {
        let uid = self.player.queue.current().map(|e| e.uid);
        if uid == self.last_uid {
            return;
        }
        self.last_uid = uid;
        crate::songedit::apply_queue(&mut self.player.queue);
        self.np.scroll_to_current = true;
        if self.lyrics_for.as_deref() != self.player.current().map(|s| s.id.as_str()) {
            self.lyrics = LyricsState::Idle;
            self.lyrics_for = None;
        }
    }

    fn media_housekeeping(&mut self) {
        let Some(m) = &mut self.media else { return };
        match self.player.current() {
            Some(s) => {
                let cover = s.thumbnail_url.as_deref().map(|u| crate::images::upgrade_thumbnail(u, 544));
                m.set_track(&s.id, &s.title, &widgets::artists_line(s), s.album_name.as_deref(), cover.as_deref(), self.player.duration);
                let playing = match self.player.state {
                    PlayState::Playing => Some(true),
                    PlayState::Idle | PlayState::Ended => None,
                    _ => Some(false),
                };
                m.set_playback(playing, self.player.pos);
            }
            None => m.set_playback(None, 0.0),
        }
    }

    fn now_info(&self) -> NowInfo {
        NowInfo {
            song: self.player.current().cloned(),
            state: self.player.state.clone(),
            playing: self.player.is_playing(),
            pos: self.player.pos,
            duration: self.player.duration.unwrap_or(0.0),
            volume: self.player.volume,
            muted: self.muted_from.is_some(),
            shuffle: self.player.queue.shuffle(),
            repeat: self.player.queue.repeat,
            format: self.player.format.clone(),
        }
    }

    fn gradient_colors(&self) -> (Color32, Color32) {
        self.art_colors.as_ref().map(|a| a.2).unwrap_or((Color32::from_rgb(0x59, 0x59, 0x59), Color32::from_rgb(0x0D, 0x0D, 0x0D)))
    }

    // ---------------------------------------------------------------------
    // Drawing
    // ---------------------------------------------------------------------

    fn draw(&mut self, ui: &mut Ui) {
        let now = self.now_info();
        let bg = if self.settings.appearance.pure_black { Color32::BLACK } else { Color32::from_rgb(0x0B, 0x0B, 0x0C) };
        let radius = self.settings.appearance.thumbnail_corner_radius;
        let current_id = now.song.as_ref().map(|s| s.id.clone());

        let needs_lyrics = (self.np_open && self.np_tab == SideTab::Lyrics) || (!self.np_open && self.side_panel == Some(SideTab::Lyrics));
        let needs_related = (self.np_open && self.np_tab == SideTab::Related) || (!self.np_open && self.side_panel == Some(SideTab::Related));
        if needs_lyrics {
            self.ensure_lyrics();
        }
        self.lyrics_view.update_extras(&self.svc, &self.settings.lyrics, &self.lyrics, self.player.current(), ui.ctx());
        if needs_related {
            self.ensure_related();
        }
        self.ensure_route_data();
        let related_key = now.song.as_ref().and_then(|s| s.video_id.clone());
        let grad = self.gradient_colors();

        let mut cx = Ctx {
            images: &mut self.images,
            actions: &mut self.actions,
            liked: &self.data.liked_ids,
            playlists: &self.data.playlists,
            accent: self.accent,
            radius,
            current_id: current_id.as_deref(),
            playing: now.playing,
        };

        if self.np_open {
            egui::CentralPanel::no_frame().show(ui, |ui| {
                let d = SideData {
                    queue: &self.player.queue,
                    lyrics: &self.lyrics,
                    related: related_key.as_ref().and_then(|k| self.data.related.get(k)),
                    lyrics_scale: self.settings.lyrics.text_size,
                    lyrics_enabled: self.settings.lyrics.show,
                    lyrics_global_offset_ms: self.settings.lyrics.global_offset_ms,
                };
                pages::now_playing::expanded(ui, &mut cx, &now, &d, &mut self.np_tab, &mut self.np, &mut self.lyrics_view, grad);
            });
            return;
        }

        // ---- bottom bar --------------------------------------------------------
        egui::Panel::bottom("player_bar")
            .exact_size(pages::player_bar::BAR_H)
            .frame(egui::Frame::new().fill(widgets::lerp_color(bg, Color32::from_rgb(0x1A, 0x1A, 0x1C), 0.8)))
            .show_separator_line(false)
            .show(ui, |ui| {
                // Thin accent progress line on top of the bar.
                let r = ui.max_rect();
                let frac = if now.duration > 0.0 { (now.pos / now.duration).clamp(0.0, 1.0) as f32 } else { 0.0 };
                let mut line = egui::Rect::from_min_size(r.min, vec2(r.width(), 1.0));
                ui.painter().rect_filled(line, 0.0, Color32::from_white_alpha(14));
                line.set_right(r.left() + r.width() * frac);
                ui.painter().rect_filled(line, 0.0, widgets::accent_light(self.accent).gamma_multiply(0.7));
                pages::player_bar::show(ui, &mut cx, &now, self.side_panel);
            });

        // ---- sidebar -------------------------------------------------------------
        egui::Panel::left("sidebar")
            .exact_size(232.0)
            .frame(egui::Frame::new().fill(widgets::lerp_color(bg, Color32::from_rgb(0x12, 0x12, 0x14), 0.9)).inner_margin(egui::Margin::symmetric(10, 4)))
            .show_separator_line(false)
            .show(ui, |ui| {
                pages::sidebar::show(ui, &mut cx, self.nav.current(), &self.data.playlists, self.logo.as_ref());
            });

        // ---- right side panel (lyrics / queue / related) ------------------------
        if let Some(mut tab) = self.side_panel {
            egui::Panel::right("side_panel")
                .default_size(380.0)
                .size_range(300.0..=560.0)
                .resizable(true)
                .frame(egui::Frame::new().fill(widgets::lerp_color(bg, Color32::from_rgb(0x14, 0x14, 0x16), 0.9)).inner_margin(14))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        pages::now_playing::tabs(ui, &mut tab);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icons::icon_button(ui, Icon::Close, 28.0, false, cx.accent).clicked() {
                                cx.act(Action::ToggleSidePanel(tab));
                            }
                        });
                    });
                    ui.add_space(10.0);
                    let d = SideData {
                        queue: &self.player.queue,
                        lyrics: &self.lyrics,
                        related: related_key.as_ref().and_then(|k| self.data.related.get(k)),
                        lyrics_scale: self.settings.lyrics.text_size,
                        lyrics_enabled: self.settings.lyrics.show,
                        lyrics_global_offset_ms: self.settings.lyrics.global_offset_ms,
                    };
                    pages::now_playing::tab_content(ui, &mut cx, tab, &now, &d, &mut self.np, &mut self.lyrics_view);
                });
            if self.side_panel.is_some() {
                self.side_panel = Some(tab);
            }
        }

        // ---- main content -----------------------------------------------------------
        let mut lib_ev = None;
        let mut search_ev = None;
        let mut settings_ev = Vec::new();
        let mut detail_ev = None;
        let mut home_retry = false;
        let mut lab_changed = false;
        egui::CentralPanel::no_frame()
            .frame(egui::Frame::new().fill(bg).inner_margin(egui::Margin { left: 28, right: 20, top: 12, bottom: 0 }))
            .show(ui, |ui| {
                // Top bar: back / forward.
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(self.nav.can_back(), |ui| {
                        if icons::icon_button(ui, Icon::Back, 32.0, false, cx.accent).on_hover_text("Back (Alt+←)").clicked() {
                            cx.act(Action::Back);
                        }
                    });
                    ui.add_enabled_ui(self.nav.can_forward(), |ui| {
                        if icons::icon_button(ui, Icon::Forward, 32.0, false, cx.accent).on_hover_text("Forward (Alt+→)").clicked() {
                            cx.act(Action::Forward);
                        }
                    });
                    if let Some(Err(e)) = &self.ytm_status {
                        ui.add_space(12.0);
                        ui.label(RichText::new(format!("Offline: {}", short(e, 80))).size(12.0).color(Color32::from_rgb(0xFF, 0x9A, 0x9A)));
                    }
                    // Top-right corner: the YouTube account button.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        crate::account::corner_button(ui, &mut cx);
                    });
                });
                ui.add_space(6.0);
                let route = self.nav.current().clone();
                egui::ScrollArea::vertical().id_salt(("page", format!("{route:?}"))).auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_max_width(ui.available_width() - 8.0);
                    match &route {
                        Route::Home => home_retry = pages::home::show(ui, &mut cx, &self.data, &self.browse),
                        Route::Search => search_ev = pages::search::show(ui, &mut cx, &mut self.search, &self.data.search_history),
                        Route::Library(tab) => lib_ev = pages::library::show(ui, &mut cx, *tab, &self.data, &self.lib, &mut self.browse),
                        Route::LocalPlaylist(id) => {
                            let pl = self.data.playlists.iter().find(|p| p.id == *id);
                            pages::detail::local_playlist(ui, &mut cx, pl, self.data.playlist_songs.get(id));
                        }
                        Route::Album(id) => detail_ev = pages::detail::album(ui, &mut cx, self.data.albums.get(id), &mut self.detail_arc),
                        Route::Artist(id) => detail_ev = pages::detail::artist(ui, &mut cx, self.data.artists.get(id), &mut self.detail_arc),
                        Route::Playlist(id) => detail_ev = pages::detail::playlist(ui, &mut cx, self.data.playlists_remote.get(id), &mut self.detail_arc),
                        Route::AudioLab => {
                            if let Some(e) = self.player.engine() {
                                self.audio_lab_view.set_spectrum(e.spectrum());
                            }
                            lab_changed = self.audio_lab_view.show(ui, &mut self.settings.audio_lab, cx.accent);
                            if self.player.is_playing() {
                                crate::ui::pace::animate_after(ui.ctx(), Duration::from_millis(33));
                            }
                        }
                        Route::Settings => {
                            settings_ev = pages::settings::show(
                                ui,
                                &mut self.settings,
                                SettingsCtx {
                                    devices: self.devices.as_ref(),
                                    log_file: self.log_file.as_ref(),
                                    ytm_status: self.ytm_status.as_ref(),
                                    db_error: self.db_error.as_ref(),
                                },
                            );
                        }
                        // ---- library/explore/stats ----
                        Route::Explore => pages::explore::explore(ui, &mut cx, &self.browse),
                        Route::MoodsGenres => pages::explore::moods(ui, &mut cx, &self.browse),
                        Route::Charts | Route::NewReleases | Route::Browse { .. } | Route::ArtistMore { .. } => {
                            pages::explore::browse_page(ui, &mut cx, &self.browse, &route)
                        }
                        Route::HomeSection(key) => pages::explore::home_section(ui, &mut cx, &self.browse, &self.data, key),
                        Route::LibraryArtist(id) => pages::library_views::local_artist(ui, &mut cx, &self.browse, id),
                        Route::LibraryAlbum(id) => pages::library_views::local_album(ui, &mut cx, &self.browse, id),
                        Route::Stats => pages::stats::show(ui, &mut cx, &mut self.browse),
                    }
                });
            });

        // ---- page events --------------------------------------------------------------
        if home_retry {
            self.load_home();
        }
        if lab_changed && let Some(e) = self.player.engine() {
            apply_audio_lab(e, &self.settings.audio_lab);
        }
        if let Some(DetailEvent::Retry) = detail_ev {
            match self.nav.current().clone() {
                Route::Album(id) => {
                    self.data.albums.remove(&id);
                }
                Route::Artist(id) => {
                    self.data.artists.remove(&id);
                }
                Route::Playlist(id) => {
                    self.data.playlists_remote.remove(&id);
                }
                _ => {}
            }
        }
        match search_ev {
            Some(SearchEvent::Submit(q)) => self.submit_search(q),
            Some(SearchEvent::ClearSuggestions) => {
                self.search.suggestions.clear();
                self.search.suggestions_for.clear();
            }
            None => {}
        }
        match lib_ev {
            Some(LibEvent::AddFolder) => self.pick_folder(),
            Some(LibEvent::CancelScan) => self.scan_cancel.store(true, Ordering::Relaxed),
            Some(LibEvent::Refresh) => self.refresh_library(),
            None => {}
        }
        for e in settings_ev {
            match e {
                SettingsEvent::RefreshDevices => {
                    self.devices = None;
                    self.refresh_devices();
                }
                SettingsEvent::SetOutputDevice(name) => {
                    tracing::info!(?name, "output device selected");
                    self.player.send(mochy_audio::Command::SetOutputDevice { name });
                }
                SettingsEvent::ClearArtCache => self.clear_art_cache(),
                SettingsEvent::OpenDir(p) => open_dir(&p),
            }
        }
    }

    fn pick_folder(&mut self) {
        let svc = self.svc.clone();
        self.scan_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.scan_cancel.clone();
        // rfd's blocking dialog runs on its own thread so the UI keeps painting.
        std::thread::spawn(move || {
            match rfd::FileDialog::new().set_title("Add music folder").pick_folder() {
                Some(dir) => crate::local_scan::spawn_scan(svc, dir, cancel),
                None => tracing::debug!("folder picker cancelled"),
            }
        });
    }

    fn clear_art_cache(&mut self) {
        self.images.clear();
        let bus = self.svc.bus.clone();
        std::thread::spawn(move || {
            let dir = mochy_storage::art_cache_dir();
            let r = std::fs::remove_dir_all(&dir).and_then(|_| std::fs::create_dir_all(&dir));
            bus.toast(match r {
                Ok(()) => "Artwork cache cleared".to_string(),
                Err(e) => format!("Couldn't clear cache: {e}"),
            });
        });
    }

    fn draw_toasts(&mut self, ctx: &egui::Context) {
        self.toasts.retain(|t| t.at.elapsed().as_secs_f32() < TOAST_SECS);
        if self.toasts.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(250));
        let bottom = if self.np_open { 24.0 } else { pages::player_bar::BAR_H + 16.0 };
        egui::Area::new(Id::unique("mochy_toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-20.0, -bottom))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                for t in &self.toasts {
                    let age = t.at.elapsed().as_secs_f32();
                    let alpha = ((TOAST_SECS - age) / 0.4).clamp(0.0, 1.0);
                    egui::Frame::new()
                        .fill(Color32::from_rgb(0x26, 0x26, 0x2A).gamma_multiply(alpha))
                        .corner_radius(12)
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.set_max_width(380.0);
                            ui.label(RichText::new(&t.text).color(Color32::WHITE.gamma_multiply(alpha)));
                        });
                    ui.add_space(6.0);
                }
            });
    }
}

impl eframe::App for MochyApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump(ctx);
        self.background_housekeeping(ctx);
        self.player.tick();
        self.track_change_housekeeping();
        self.media_housekeeping();
        if self.integrations.frame(ctx, &self.player, &mut self.settings) {
            self.refresh_library();
        }
        if let Some(q) = self.search.due_suggestions() {
            let q2 = q.clone();
            self.svc.ytm_call("suggestions", move |c| async move { c.suggestions(&q2).await }, move |r| AppMsg::Suggestions {
                query: q,
                items: r.unwrap_or_default(),
            });
        } else if self.search.edited_at.is_some() {
            ctx.request_repaint_after(crate::pages::search::SUGGEST_DEBOUNCE);
        }
        self.settings_housekeeping(ctx);
        if self.player.is_playing() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // UI language, pushed before drawing so every tr() this frame agrees.
        crate::ui::i18n::set_lang(self.settings.appearance.language);
        crate::ui::pace::update(&ctx);
        crate::pages::player_styles::begin_frame(&ctx, &mut self.settings, self.player.current(), self.player.volume);
        self.images.begin_frame();
        self.browse.publish_saved(&ctx);
        self.shortcuts(&ctx);
        self.window_housekeeping(&ctx);
        self.theme_housekeeping(&ctx);
        crate::ui::transitions_ui::set_snapshot(self.player.transition_snapshot(&self.settings));
        self.draw(ui);
        self.handle_dialog(&ctx);
        // Song editing (songedit): edit window, then refresh what an edit touched.
        crate::songedit::ui::show(&ctx, &self.svc, &mut self.images);
        let edited = crate::songedit::take_changed();
        if !edited.is_empty() {
            crate::songedit::apply_queue(&mut self.player.queue);
            if self.lyrics_for.as_ref().is_some_and(|id| edited.contains(id)) {
                self.lyrics_for = None;
            }
            self.refresh_library();
        }
        self.integrations.ui(&ctx);
        let account_fx = self.account.frame(&self.settings);
        self.run_account_effects(account_fx);
        self.draw_toasts(&ctx);
        if let Some(t) = self.copy_request.take() {
            ctx.copy_text(t);
        }
        self.apply_actions();
        self.run_shots(&ctx);
        // Settings changed by actions (volume/shuffle) are picked up next frame.
        if self.settings != self.settings_seen {
            ctx.request_repaint();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        tracing::info!("shutting down");
        self.player.shutdown();
        // Save synchronously so nothing is lost.
        if let Err(e) = SettingsStore::open_default().save(&self.settings) {
            tracing::warn!("settings save on exit failed: {e}");
        }
        // Give the DB worker a moment to flush queued jobs (queue / play record).
        let done = self.svc.db.query(|_| ());
        let _ = self.svc.rt.block_on(async { tokio::time::timeout(Duration::from_secs(2), done).await });
    }
}

fn initial_accent(s: &AppSettings) -> Color32 {
    if !s.appearance.dynamic_theme
        && let Some(c) = s.appearance.selected_theme_color
    {
        return rgb_u32(c);
    }
    theme::DEFAULT_ACCENT
}

fn rgb_u32(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() > n { format!("{}…", s.chars().take(n).collect::<String>()) } else { s.to_string() }
}

fn pseudo_rand(n: usize) -> usize {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    (t as usize) % n.max(1)
}

fn open_dir(p: &std::path::Path) {
    let _ = std::fs::create_dir_all(p);
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("explorer").arg(p).spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(p).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(p).spawn();
    if let Err(e) = r {
        tracing::warn!("open dir failed: {e}");
    }
}

/// Native window handle for SMTC (Windows); `None` elsewhere.
fn native_hwnd(cc: &eframe::CreationContext<'_>) -> Option<*mut std::ffi::c_void> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let h = cc.window_handle().ok()?;
    match h.as_raw() {
        RawWindowHandle::Win32(w) => Some(w.hwnd.get() as *mut std::ffi::c_void),
        _ => None,
    }
}

/// Screenshot automation for visual checks (`MOCHY_SHOTS=<dir>`).
struct ShotRunner {
    dir: std::path::PathBuf,
    started: std::time::Instant,
    step: usize,
    taken: bool,
}

impl MochyApp {
    fn run_shots(&mut self, ctx: &egui::Context) {
        let Some(sr) = self.shots.as_mut() else { return };
        // (name, route, seconds to wait before the shot — Home waits for network/art)
        let steps: [(&str, Route, f32); 16] = [
            ("home", Route::Home, 12.0),
            ("search", Route::Search, 3.0),
            ("library", Route::Library(LibTab::History), 3.0),
            ("audio_lab", Route::AudioLab, 3.0),
            ("settings", Route::Settings, 3.0),
            ("now_playing", Route::Home, 8.0),
            // ---- library/explore/stats ----
            ("explore", Route::Explore, 8.0),
            ("moods", Route::MoodsGenres, 5.0),
            ("charts", Route::Charts, 6.0),
            ("mood_page", Route::Browse { id: "FEmusic_moods_and_genres_category".into(), params: Some("ggMPOg1uX1JOQWZFeDByc2Jm".into()) }, 7.0),
            ("lib_songs", Route::Library(LibTab::Songs), 3.0),
            ("lib_albums", Route::Library(LibTab::Albums), 3.0),
            ("lib_artists", Route::Library(LibTab::Artists), 3.0),
            ("lib_recent", Route::Library(LibTab::RecentlyAdded), 3.0),
            ("stats", Route::Stats, 4.0),
            ("recap", Route::Stats, 3.0),
        ];
        ctx.request_repaint_after(Duration::from_millis(100));
        // `MOCHY_SHOTS_ONLY=now_playing,settings`: just those shots.
        if let Ok(only) = std::env::var("MOCHY_SHOTS_ONLY") {
            while sr.step < steps.len() && !only.split(',').any(|n| n.trim() == steps[sr.step].0) {
                sr.step += 1;
            }
        }
        if sr.step >= steps.len() {
            if sr.started.elapsed().as_secs_f32() > 1.0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }
        let (name, route, wait) = steps[sr.step].clone();
        self.np_open = name == "now_playing";
        if self.np_open {
            self.np_tab = SideTab::Lyrics;
        }
        // Start a song for the Now Playing shot (`MOCHY_SHOTS_VIDEO=<videoId>`).
        if sr.step == 0
            && !sr.taken
            && self.player.current().is_none()
            && let Ok(vid) = std::env::var("MOCHY_SHOTS_VIDEO")
        {
            let mut song = mochy_core::models::Song::remote(vid.clone(), "Loading…");
            song.thumbnail_url = Some(format!("https://i.ytimg.com/vi/{vid}/hqdefault.jpg"));
            self.player.play_with_radio(song);
        }
        if self.nav.current() != &route {
            self.nav.go(route);
        }
        if name == "recap" && self.browse.stats.recap.is_none() {
            self.browse.stats.recap = Some(crate::browse::RecapUi { page: 1, page_started: Instant::now() });
        }
        let t = sr.started.elapsed().as_secs_f32();
        if !sr.taken && t >= wait {
            sr.taken = true;
            let path = sr.dir.join(format!("{name}.png"));
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                let [w, h] = img.size;
                let mut buf = Vec::with_capacity(w * h * 4);
                for p in &img.pixels {
                    buf.extend_from_slice(&p.to_array());
                }
                if let Some(rgba) = image::RgbaImage::from_raw(w as u32, h as u32, buf) {
                    let _ = rgba.save(&path);
                }
            })));
        } else if sr.taken && t >= wait + 0.7 {
            sr.step += 1;
            sr.taken = false;
            sr.started = std::time::Instant::now();
        }
    }
}

// ---- library/explore/stats ----
impl MochyApp {
    /// Run what `BrowseState` asked for after an action or a message.
    fn run_browse_effects(&mut self, effects: Vec<crate::browse::Effect>) {
        use crate::browse::Effect;
        for e in effects {
            match e {
                Effect::Act(a) => self.apply(a),
                Effect::PlayNext(songs) => {
                    self.toast(if songs.len() == 1 { format!("{}: {}", tr("Playing next"), songs[0].title) } else { format!("{}: {}", tr("Playing next"), crate::ui::i18n::songs_count(songs.len())) });
                    for s in songs.into_iter().rev() {
                        self.player.play_next(s);
                    }
                }
                Effect::Toast(t) => self.toast(t),
            }
        }
    }
}

// ---- run in background (tray) ----
impl MochyApp {
    /// Close → hide to tray when background mode is on; keep the tray icon in sync.
    fn background_housekeeping(&mut self, ctx: &egui::Context) {
        let enabled = self.settings.misc.run_in_background;
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            if enabled && self.tray.is_active() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                self.hidden_to_tray = true;
                self.np_open = false;
                tracing::info!("window hidden to the notification area");
            }
        }
        let now = self.player.current().map(|s| {
            let artist = s.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ");
            if artist.is_empty() { s.title.clone() } else { format!("{} · {}", s.title, artist) }
        });
        let playing = self.player.is_playing();
        self.tray.sync(enabled, now, playing);
        // Background mode switched off while hidden: bring the window back.
        if !enabled && self.hidden_to_tray {
            self.show_window(ctx);
        }
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        self.hidden_to_tray = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }

    fn on_tray(&mut self, ctx: &egui::Context, cmd: crate::tray::TrayCmd) {
        use crate::tray::TrayCmd;
        match cmd {
            TrayCmd::Show => self.show_window(ctx),
            TrayCmd::Toggle => self.player.toggle(),
            TrayCmd::Next => self.player.next(),
            TrayCmd::Prev => self.player.prev(),
            TrayCmd::Quit => {
                self.quitting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                ctx.request_repaint();
            }
        }
    }
}
