//! The importer window: links, files / pasted lists, account libraries,
//! progress, review of unmatched tracks, history (undo) and re-sync.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use egui::{Color32, RichText, Ui};
use mochy_core::models::Song;
use mochy_importer::account::{AccountLibrary, AccountService, AccountTokens, RemotePlaylist};
use mochy_importer::link::{self, LinkImporter};
use mochy_importer::matches::ImportMatches;
use mochy_importer::runner::{DbExec, ImportRunner, ImportState, ResyncRead, ResyncRequest, ResyncRoute, resync_route};
use mochy_importer::store::{self, SessionInfo};
use mochy_importer::{ImportProgress, ImportSource, ImportedTrack, ParsedImport, PlaylistOrigin, UnmatchedTrack};
use mochy_innertube::YtmClient;
use mochy_storage::Db;

use crate::db::DbWorker;
use crate::services::Services;
use crate::ui::i18n::tr;
use crate::widgets::{self, TEXT, TEXT_DIM};

const ERROR: Color32 = Color32::from_rgb(0xFF, 0x8A, 0x8A);

/// The app's DB thread as the importer's executor.
#[derive(Clone)]
struct AppDb(DbWorker);

impl DbExec for AppDb {
    async fn exec<R: Send + 'static>(&self, f: impl FnOnce(&Db) -> R + Send + 'static) -> Option<R> {
        self.0.query(f).await.ok()
    }
}

type Runner = ImportRunner<AppDb, Arc<YtmClient>>;
/// An unmatched track and its search results (None while searching).
type Review = (UnmatchedTrack, Option<Vec<(Song, f64)>>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Link,
    File,
    Account,
    History,
}

/// Reading a source, before importing.
#[derive(Default)]
enum Reading {
    #[default]
    Idle,
    Busy(String),
    Ready(ParsedImport),
    Error(String),
}

#[derive(Default)]
enum Acc {
    #[default]
    Idle,
    Busy(String),
    Connected { lib: Arc<AccountLibrary>, playlists: Vec<(RemotePlaylist, bool)> },
    Error(String),
}

#[derive(Clone)]
struct PlaylistEntry {
    name: String,
    source: String,
    route: Option<ResyncRoute>,
}

#[derive(Default, Clone)]
struct History {
    sessions: Vec<SessionInfo>,
    playlists: Vec<PlaylistEntry>,
    counts: Vec<(String, usize)>,
}

pub struct ImporterUi {
    svc: Services,
    matches: Arc<ImportMatches>,
    state: ImportState,
    tab: Tab,
    link: String,
    text: String,
    text_name: String,
    service: AccountService,
    bearer: String,
    user_token: String,
    reading: Arc<Mutex<Reading>>,
    acc: Arc<Mutex<Acc>>,
    history: Arc<Mutex<Option<History>>>,
    history_loading: bool,
    /// Unmatched track being reviewed and its search results.
    review: Arc<Mutex<Option<Review>>>,
    library_changed: Arc<AtomicBool>,
    note: Arc<Mutex<Option<String>>>,
}

fn repaint() {
    if let Some(c) = super::CTX.get() {
        c.request_repaint();
    }
}

fn source_label(key: &str) -> &'static str {
    ImportSource::from_key(key).map(|s| s.label()).unwrap_or("other sources")
}

/// "YYYY-MM-DD HH:MM" (UTC) from epoch ms.
fn date_label(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // civil_from_days (H. Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, (rem / 60) % 60)
}

impl ImporterUi {
    pub fn new(svc: Services, matches_file: PathBuf) -> Self {
        Self {
            svc,
            matches: Arc::new(ImportMatches::open(matches_file)),
            state: ImportState::default(),
            tab: Tab::Link,
            link: String::new(),
            text: String::new(),
            text_name: String::new(),
            service: AccountService::Spotify,
            bearer: String::new(),
            user_token: String::new(),
            reading: Default::default(),
            acc: Default::default(),
            history: Default::default(),
            history_loading: false,
            review: Default::default(),
            library_changed: Default::default(),
            note: Default::default(),
        }
    }

    pub fn is_running(&self) -> bool {
        self.state.is_running()
    }

    pub fn take_library_changed(&self) -> bool {
        self.library_changed.swap(false, Ordering::SeqCst)
    }

    fn fail_progress(state: &ImportState, e: String) {
        *state.progress.lock().unwrap() = ImportProgress { is_done: true, error: Some(e), ..Default::default() };
        repaint();
    }

    // ---- actions -------------------------------------------------------------------

    fn start_import(&self, parsed: ParsedImport) {
        let svc = self.svc.clone();
        let base = self.runner_parts();
        self.svc.rt.spawn(async move {
            match svc.ytm().await {
                Ok(ytm) => {
                    let runner = base.build(ytm);
                    if !runner.run_import(parsed).await {
                        svc.bus.toast(tr("An import is still running. Try again once it's done.").to_string());
                    }
                }
                Err(e) => Self::fail_progress(&base.state, format!("YouTube Music: {e}")),
            }
        });
    }

    fn runner_parts(&self) -> RunnerParts {
        RunnerParts { db: AppDb(self.svc.db.clone()), matches: self.matches.clone(), state: self.state.clone() }
    }

    fn read_link(&self) {
        let link = self.link.trim().to_string();
        if link.is_empty() {
            return;
        }
        let (svc, reading) = (self.svc.clone(), self.reading.clone());
        *reading.lock().unwrap() = Reading::Busy(tr("Reading the playlist…").to_string());
        self.svc.rt.spawn(async move {
            let ytm = if link::detect(&link) == Some(link::LinkSource::Youtube) { svc.ytm().await.ok() } else { None };
            let r = LinkImporter::new().fetch(&link, ytm.as_deref()).await;
            *reading.lock().unwrap() = match r {
                Ok(p) => Reading::Ready(p),
                Err(e) => Reading::Error(e.0),
            };
            repaint();
        });
    }

    fn pick_file(&self) {
        let reading = self.reading.clone();
        std::thread::spawn(move || {
            let Some(path) = rfd::FileDialog::new().set_title("Import playlist file").add_filter("CSV / text", &["csv", "txt", "tsv"]).pick_file() else {
                return;
            };
            let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Imported playlist".into());
            let r = std::fs::read(&path).map(|b| String::from_utf8_lossy(&b).into_owned());
            *reading.lock().unwrap() = match r {
                Ok(text) => {
                    let p = mochy_importer::csv::parse_file(&text, &stem);
                    if p.total_songs() == 0 && p.total_playlists() == 0 {
                        Reading::Error(tr("No songs found in that file. It needs a title and artist column (TuneMyMusic, Exportify or Mochy CSV), or one \"Artist - Title\" per line.").to_string())
                    } else {
                        Reading::Ready(p)
                    }
                }
                Err(e) => Reading::Error(e.to_string()),
            };
            repaint();
        });
    }

    fn connect_account(&self) {
        let (svc, acc) = (self.svc.clone(), self.acc.clone());
        let tokens = AccountTokens { bearer: self.bearer.clone(), user_token: self.user_token.clone() };
        let service = self.service;
        *acc.lock().unwrap() = Acc::Busy(tr("Connecting…").to_string());
        svc.rt.spawn(async move {
            let r = async {
                let lib = AccountLibrary::connect(service, &tokens).await?;
                let playlists = lib.playlists().await?;
                Ok::<_, mochy_importer::ImportError>((lib, playlists))
            }
            .await;
            *acc.lock().unwrap() = match r {
                Ok((lib, playlists)) => Acc::Connected { lib: Arc::new(lib), playlists: playlists.into_iter().map(|p| (p, true)).collect() },
                Err(e) => Acc::Error(e.0),
            };
            repaint();
        });
    }

    fn read_account(&self, lib: Arc<AccountLibrary>, chosen: Vec<RemotePlaylist>) {
        let reading = self.reading.clone();
        *reading.lock().unwrap() = Reading::Busy(tr("Reading your library…").to_string());
        self.svc.rt.spawn(async move {
            let r2 = reading.clone();
            let r = lib
                .read_selection(&chosen, move |done, total, name| {
                    *r2.lock().unwrap() = Reading::Busy(format!("{} {done}/{total} {name}", tr("Reading")));
                    repaint();
                })
                .await;
            *reading.lock().unwrap() = match r {
                Ok(p) => Reading::Ready(p),
                Err(e) => Reading::Error(e.0),
            };
            repaint();
        });
    }

    fn load_history(&mut self) {
        self.history_loading = true;
        let history = self.history.clone();
        self.svc.db.run(move |db| {
            let names: std::collections::HashMap<String, (String, Option<String>)> =
                db.playlists().unwrap_or_default().into_iter().map(|p| (p.id.clone(), (p.name, p.browse_id))).collect();
            let playlists = store::imported_playlists(db)
                .into_iter()
                .filter_map(|row| {
                    let (name, browse) = names.get(&row.playlist_id)?.clone();
                    let route = resync_route(&row, &name, browse.as_deref(), store::imported_track_count(db, &row.playlist_id));
                    Some(PlaylistEntry { name, source: row.source.clone(), route })
                })
                .collect();
            *history.lock().unwrap() = Some(History { sessions: store::sessions(db), playlists, counts: store::imported_counts(db) });
            repaint();
        });
    }

    fn undo(&mut self, session: Option<i64>, source: Option<String>) {
        let (history, changed, note, bus) = (self.history.clone(), self.library_changed.clone(), self.note.clone(), self.svc.bus.clone());
        self.svc.db.run(move |db| {
            let removed = match (session, source) {
                (Some(id), _) => store::undo_session(db, id),
                (None, Some(src)) => store::delete_imports_from(db, &src),
                (None, None) => store::undo_latest(db).unwrap_or(0),
            };
            changed.store(true, Ordering::SeqCst);
            *history.lock().unwrap() = None;
            let msg = tr("Import undone: {n} songs left the library.").replace("{n}", &removed.to_string());
            *note.lock().unwrap() = Some(msg.clone());
            bus.toast(msg);
        });
        self.history_loading = false;
    }

    fn start_resync(&self, requests: Vec<ResyncRequest>) {
        if requests.is_empty() {
            return;
        }
        let svc = self.svc.clone();
        let base = self.runner_parts();
        let history = self.history.clone();
        self.svc.rt.spawn(async move {
            let ytm = match svc.ytm().await {
                Ok(y) => y,
                Err(e) => return Self::fail_progress(&base.state, format!("YouTube Music: {e}")),
            };
            let runner = base.build(ytm.clone());
            let li = Arc::new(LinkImporter::new());
            runner
                .run_resync(requests, move |read| {
                    let (ytm, li) = (ytm.clone(), li.clone());
                    async move { read_for_resync(&li, &ytm, read).await }
                })
                .await;
            *history.lock().unwrap() = None;
        });
    }

    fn find_candidates(&self, um: UnmatchedTrack) {
        let (svc, review, base) = (self.svc.clone(), self.review.clone(), self.runner_parts());
        *review.lock().unwrap() = Some((um.clone(), None));
        self.svc.rt.spawn(async move {
            let Ok(ytm) = svc.ytm().await else { return };
            let c = base.build(ytm).candidates(&um.track).await;
            let mut g = review.lock().unwrap();
            if let Some((cur, slot)) = g.as_mut()
                && cur.track == um.track
            {
                *slot = Some(c);
            }
            repaint();
        });
    }

    fn use_candidate(&self, um: UnmatchedTrack, song: Song) {
        let (svc, review, base, changed) = (self.svc.clone(), self.review.clone(), self.runner_parts(), self.library_changed.clone());
        *review.lock().unwrap() = None;
        self.svc.rt.spawn(async move {
            let Ok(ytm) = svc.ytm().await else { return };
            base.build(ytm).resolve_unmatched(&um, song).await;
            changed.store(true, Ordering::SeqCst);
            repaint();
        });
    }

    // ---- drawing --------------------------------------------------------------------

    pub fn show(&mut self, ctx: &egui::Context, open: &mut bool) {
        egui::Window::new(tr("Import playlists"))
            .id(egui::Id::unique("mochy_importer_window"))
            .open(open)
            .collapsible(false)
            .resizable(true)
            .default_size([640.0, 560.0])
            .min_width(480.0)
            .show(ctx, |ui| {
                let progress = self.state.snapshot();
                if progress.is_running || progress.is_done {
                    self.progress_view(ui, &progress);
                    return;
                }
                if matches!(*self.reading.lock().unwrap(), Reading::Ready(_)) {
                    self.preview_view(ui);
                    return;
                }
                ui.horizontal(|ui| {
                    for (t, label) in [(Tab::Link, tr("Link")), (Tab::File, tr("File or list")), (Tab::Account, tr("Account")), (Tab::History, tr("History"))] {
                        if widgets::pill(ui, label, self.tab == t).clicked() {
                            self.tab = t;
                        }
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().id_salt("importer_scroll").auto_shrink([false, false]).show(ui, |ui| match self.tab {
                    Tab::Link => self.link_tab(ui),
                    Tab::File => self.file_tab(ui),
                    Tab::Account => self.account_tab(ui),
                    Tab::History => self.history_tab(ui),
                });
            });
    }

    fn reading_status(&self, ui: &mut Ui) {
        match &*self.reading.lock().unwrap() {
            Reading::Busy(l) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(l);
                });
            }
            Reading::Error(e) => {
                ui.label(RichText::new(e).color(ERROR));
            }
            _ => {}
        }
    }

    fn link_tab(&mut self, ui: &mut Ui) {
        ui.label(tr("Paste a link to a public playlist or album."));
        for s in [link::LinkSource::Spotify, link::LinkSource::AppleMusic, link::LinkSource::Deezer, link::LinkSource::Youtube] {
            ui.label(RichText::new(format!("{} — {}", s.label(), s.example())).size(12.0).color(TEXT_DIM));
        }
        let busy = matches!(*self.reading.lock().unwrap(), Reading::Busy(_));
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.link).hint_text("https://…").desired_width(420.0));
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.add_enabled(!busy, egui::Button::new(tr("Read"))).clicked() || enter) && !busy {
                self.read_link();
            }
        });
        hint(ui, tr("Spotify links only share the first 100 songs of a playlist, and private playlists can't be read by link; use a CSV export or the account import for those."));
        self.reading_status(ui);
    }

    fn file_tab(&mut self, ui: &mut Ui) {
        ui.label(tr("A CSV from TuneMyMusic, Exportify or Mochy, or a text file with one \"Artist - Title\" per line."));
        if ui.button(tr("Choose a file…")).clicked() {
            self.pick_file();
        }
        ui.add_space(8.0);
        ui.label(tr("Or paste a list:"));
        ui.add(egui::TextEdit::multiline(&mut self.text).hint_text("Daft Punk - Get Lucky\nThe Weeknd - Blinding Lights").desired_rows(8).desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            ui.label(tr("Playlist name"));
            ui.add(egui::TextEdit::singleline(&mut self.text_name).hint_text(tr("Imported playlist")).desired_width(240.0));
            if ui.button(tr("Read list")).clicked() && !self.text.trim().is_empty() {
                let name = if self.text_name.trim().is_empty() { tr("Imported playlist").to_string() } else { self.text_name.trim().to_string() };
                let p = mochy_importer::csv::parse_file(&self.text, &name);
                *self.reading.lock().unwrap() = if p.total_songs() == 0 {
                    Reading::Error(tr("No songs found. Write one \"Artist - Title\" per line.").to_string())
                } else {
                    Reading::Ready(p)
                };
            }
        });
        self.reading_status(ui);
    }

    fn account_tab(&mut self, ui: &mut Ui) {
        ui.label(tr("Read your own library from a signed-in account. Mochy has no built-in browser, so copy the session token from the service's web player yourself; it is only kept in memory and only sent to that service."));
        ui.horizontal(|ui| {
            ui.label(tr("Service"));
            let before = self.service;
            egui::ComboBox::from_id_salt("acc_service").selected_text(self.service.label()).show_ui(ui, |ui| {
                for s in AccountService::ALL {
                    ui.selectable_value(&mut self.service, s, s.label());
                }
            });
            if before != self.service {
                *self.acc.lock().unwrap() = Acc::Idle;
            }
            if ui.small_button(tr("Open web player")).clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(self.service.web_player()));
            }
        });
        hint(
            ui,
            match self.service {
                AccountService::Spotify => tr("Sign in at open.spotify.com, press F12 → Network, pick a request to api.spotify.com or api-partner.spotify.com and copy the text after \"Bearer \" in its authorization header. Tokens expire after about an hour."),
                AccountService::AppleMusic => tr("Sign in at music.apple.com, press F12 → Network, pick a request to amp-api.music.apple.com and copy the Bearer token from its authorization header and the media-user-token header."),
                AccountService::Tidal => tr("Sign in at listen.tidal.com, press F12 → Network, pick a request to api.tidal.com and copy the Bearer token from its authorization header."),
            },
        );
        ui.horizontal(|ui| {
            ui.add_sized([130.0, 18.0], egui::Label::new(if self.service == AccountService::AppleMusic { tr("Developer token") } else { tr("Access token") }));
            ui.add(egui::TextEdit::singleline(&mut self.bearer).password(true).desired_width(360.0));
        });
        if self.service == AccountService::AppleMusic {
            ui.horizontal(|ui| {
                ui.add_sized([130.0, 18.0], egui::Label::new("media-user-token"));
                ui.add(egui::TextEdit::singleline(&mut self.user_token).password(true).desired_width(360.0));
            });
        }
        let mut connect = false;
        let mut disconnect = false;
        let mut read: Option<(Arc<AccountLibrary>, Vec<RemotePlaylist>)> = None;
        {
            let mut acc = self.acc.lock().unwrap();
            match &mut *acc {
                Acc::Idle => connect = ui.button(tr("Connect")).clicked(),
                Acc::Busy(l) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(l.as_str());
                    });
                }
                Acc::Error(e) => {
                    ui.label(RichText::new(e.as_str()).color(ERROR));
                    connect = ui.button(tr("Connect")).clicked();
                }
                Acc::Connected { lib, playlists } => {
                    let who = lib.account_name.clone().unwrap_or_else(|| lib.service.label().to_string());
                    ui.label(RichText::new(format!("{} {who}", tr("Connected to"))).strong());
                    ui.horizontal(|ui| {
                        if ui.small_button(tr("Select all")).clicked() {
                            playlists.iter_mut().for_each(|p| p.1 = true);
                        }
                        if ui.small_button(tr("Select none")).clicked() {
                            playlists.iter_mut().for_each(|p| p.1 = false);
                        }
                    });
                    for (p, on) in playlists.iter_mut() {
                        let mut label = p.name.clone();
                        if let Some(n) = p.track_count {
                            label.push_str(&format!(" · {n}"));
                        }
                        ui.horizontal(|ui| {
                            ui.checkbox(on, label);
                            if let Some(s) = &p.subtitle {
                                ui.label(RichText::new(s).size(12.0).color(TEXT_DIM));
                            }
                        });
                    }
                    let chosen: Vec<RemotePlaylist> = playlists.iter().filter(|p| p.1).map(|p| p.0.clone()).collect();
                    let busy = matches!(*self.reading.lock().unwrap(), Reading::Busy(_));
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!chosen.is_empty() && !busy, egui::Button::new(tr("Read selected"))).clicked() {
                            read = Some((lib.clone(), chosen));
                        }
                        if ui.small_button(tr("Disconnect")).clicked() {
                            disconnect = true;
                        }
                    });
                }
            }
        }
        if disconnect {
            *self.acc.lock().unwrap() = Acc::Idle;
        }
        if connect {
            self.connect_account();
        }
        if let Some((lib, chosen)) = read {
            self.read_account(lib, chosen);
        }
        self.reading_status(ui);
    }

    fn history_tab(&mut self, ui: &mut Ui) {
        let data = self.history.lock().unwrap().clone();
        let Some(h) = data else {
            if !self.history_loading {
                self.load_history();
            }
            ui.spinner();
            return;
        };
        self.history_loading = false;
        if let Some(n) = self.note.lock().unwrap().take() {
            ui.label(RichText::new(n).color(TEXT_DIM));
        }
        ui.label(RichText::new(tr("Imports")).strong().color(TEXT));
        if h.sessions.is_empty() {
            hint(ui, tr("Nothing imported yet."));
        }
        let mut undo: Option<(Option<i64>, Option<String>)> = None;
        for s in &h.sessions {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{} · {} · {}",
                    source_label(&s.source),
                    date_label(s.started_at),
                    tr("{songs} songs, {playlists} playlists").replace("{songs}", &s.songs.to_string()).replace("{playlists}", &s.playlists.to_string())
                ));
                if ui.small_button(tr("Undo")).clicked() {
                    undo = Some((Some(s.id), None));
                }
            });
        }
        if !h.counts.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new(tr("Songs by source")).strong().color(TEXT));
            for (src, n) in &h.counts {
                ui.horizontal(|ui| {
                    ui.label(format!("{}: {n}", source_label(src)));
                    if ui.small_button(tr("Delete all imports from it")).clicked() {
                        undo = Some((None, Some(src.clone())));
                    }
                });
            }
        }
        ui.add_space(8.0);
        ui.label(RichText::new(tr("Imported playlists")).strong().color(TEXT));
        hint(ui, tr("Re-sync reads a playlist again and adds the songs that are new on the service; songs you removed here stay out."));
        let direct: Vec<ResyncRequest> = h
            .playlists
            .iter()
            .filter_map(|p| match &p.route {
                Some(ResyncRoute::Direct(r)) => Some(r.clone()),
                _ => None,
            })
            .collect();
        let mut resync: Vec<ResyncRequest> = Vec::new();
        for p in &h.playlists {
            ui.horizontal(|ui| {
                ui.label(format!("{} ({})", p.name, source_label(&p.source)));
                match &p.route {
                    Some(ResyncRoute::Direct(r)) => {
                        if ui.small_button(tr("Re-sync")).clicked() {
                            resync.push(r.clone());
                        }
                    }
                    Some(ResyncRoute::ThroughAccount(_)) => hint(ui, tr("re-sync with the account import")),
                    Some(ResyncRoute::NeedsLink) => hint(ui, tr("no link saved — import its link again")),
                    None => hint(ui, tr("follows YouTube")),
                }
            });
        }
        if !direct.is_empty() && ui.button(tr("Re-sync all")).clicked() {
            resync = direct;
        }
        ui.horizontal(|ui| {
            if ui.small_button(tr("Refresh")).clicked() {
                *self.history.lock().unwrap() = None;
            }
        });
        if let Some((sid, src)) = undo {
            self.undo(sid, src);
        }
        if !resync.is_empty() {
            self.start_resync(resync);
        }
    }

    fn preview_view(&mut self, ui: &mut Ui) {
        let mut import = None;
        let mut back = false;
        if let Reading::Ready(p) = &*self.reading.lock().unwrap() {
            ui.label(RichText::new(tr("Ready to import")).size(16.0).strong());
            let from = p.provider.map(|s| s.label()).unwrap_or(tr("a file"));
            ui.label(format!("{} {from}", tr("From")));
            for w in &p.warnings {
                ui.label(RichText::new(w).color(Color32::from_rgb(0xFF, 0xC8, 0x6A)));
            }
            egui::ScrollArea::vertical().id_salt("preview_scroll").max_height(300.0).show(ui, |ui| {
                if !p.liked_songs.is_empty() {
                    ui.label(format!("♥ {} · {}", tr("Liked songs"), p.liked_songs.len()));
                }
                if !p.library_songs.is_empty() {
                    ui.label(format!("{} · {}", tr("Library songs"), p.library_songs.len()));
                }
                for (name, tracks) in &p.playlists {
                    egui::CollapsingHeader::new(format!("{name} · {}", tracks.len())).id_salt(("pv", name)).show(ui, |ui| {
                        for t in tracks.iter().take(200) {
                            ui.label(RichText::new(format!("{} — {}", t.title, t.artist)).size(12.0).color(TEXT_DIM));
                        }
                        if tracks.len() > 200 {
                            ui.label(RichText::new("…").color(TEXT_DIM));
                        }
                    });
                }
            });
            hint(ui, tr("Each song is looked up on YouTube Music; playlists you imported before are topped up instead of copied again."));
            ui.horizontal(|ui| {
                if ui.button(RichText::new(tr("Import")).strong()).clicked() {
                    import = Some(p.clone());
                }
                if ui.button(tr("Back")).clicked() {
                    back = true;
                }
            });
        }
        if let Some(p) = import {
            *self.reading.lock().unwrap() = Reading::Idle;
            self.start_import(p);
        }
        if back {
            *self.reading.lock().unwrap() = Reading::Idle;
        }
    }

    fn progress_view(&mut self, ui: &mut Ui, p: &ImportProgress) {
        let src = p.source.map(|s| s.label()).unwrap_or(tr("a file"));
        if p.is_running {
            let title = if p.resync.is_some() { tr("Re-syncing from") } else { tr("Importing from") };
            ui.label(RichText::new(format!("{title} {src}")).size(16.0).strong());
            let frac = if p.total_tracks == 0 { 0.0 } else { p.processed_tracks as f32 / p.total_tracks as f32 };
            ui.add(egui::ProgressBar::new(frac).text(format!("{} / {}", p.processed_tracks, p.total_tracks)));
            ui.label(RichText::new(&p.current_label).size(12.0).color(TEXT_DIM));
            ui.label(format!("{}: {}", tr("Found"), p.matched_tracks));
            if ui.button(tr("Stop")).clicked() {
                self.state.cancel();
            }
            hint(ui, tr("You can close this window; the import keeps going."));
            return;
        }
        // finished
        if let Some(e) = &p.error {
            ui.label(RichText::new(e).color(ERROR));
        }
        if let Some(r) = &p.resync {
            ui.label(RichText::new(tr("Re-sync finished")).size(16.0).strong());
            ui.label(tr("{added} new songs in {changed} of {n} playlists").replace("{added}", &r.added.to_string()).replace("{changed}", &r.playlists_changed.to_string()).replace("{n}", &r.playlists.to_string()));
            if !r.failed.is_empty() {
                ui.label(RichText::new(format!("{} {}", tr("Couldn't read:"), r.failed.join(", "))).color(ERROR));
            }
            if let Some(n) = &r.note {
                hint(ui, n);
            }
        } else if p.error.is_none() || p.total_tracks > 0 {
            ui.label(RichText::new(if p.cancelled { tr("Import stopped") } else { tr("Import finished") }).size(16.0).strong());
            ui.label(tr("Found {m} of {t} songs").replace("{m}", &p.matched_tracks.to_string()).replace("{t}", &p.total_tracks.to_string()));
            if p.playlists_created > 0 {
                ui.label(tr("{n} playlists created").replace("{n}", &p.playlists_created.to_string()));
            }
            if p.liked_added > 0 {
                ui.label(tr("{n} songs liked").replace("{n}", &p.liked_added.to_string()));
            }
        }
        if !p.unmatched.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new(format!("{} ({})", tr("Not found"), p.unmatched.len())).strong().color(TEXT));
            hint(ui, tr("Pick the right song yourself; the choice is remembered for later imports and re-syncs."));
            let review = self.review.lock().unwrap().clone();
            let mut find = None;
            let mut use_song = None;
            let mut close_review = false;
            egui::ScrollArea::vertical().id_salt("unmatched_scroll").max_height(320.0).show(ui, |ui| {
                for um in &p.unmatched {
                    ui.horizontal(|ui| {
                        let place = um.playlist_name.clone().unwrap_or_else(|| tr("Liked songs").to_string());
                        ui.label(format!("{} — {}", um.track.title, um.track.artist));
                        ui.label(RichText::new(place).size(12.0).color(TEXT_DIM));
                        if ui.small_button(tr("Find")).clicked() {
                            find = Some(um.clone());
                        }
                    });
                    if let Some((cur, cands)) = &review
                        && cur.track == um.track
                    {
                        egui::Frame::new().fill(widgets::SURFACE_2).corner_radius(8).inner_margin(8).show(ui, |ui| match cands {
                            None => {
                                ui.spinner();
                            }
                            Some(list) if list.is_empty() => {
                                hint(ui, tr("Nothing found on YouTube Music."));
                            }
                            Some(list) => {
                                for (song, score) in list.iter().take(8) {
                                    ui.horizontal(|ui| {
                                        let len = song.duration_secs.map(widgets::fmt_time).unwrap_or_default();
                                        ui.label(format!("{} — {} {len}", song.title, widgets::artists_line(song)));
                                        ui.label(RichText::new(format!("{:.0}%", (score * 100.0).clamp(0.0, 100.0))).size(11.0).color(TEXT_DIM));
                                        if ui.small_button(tr("Use")).clicked() {
                                            use_song = Some((um.clone(), song.clone()));
                                        }
                                    });
                                }
                                if ui.small_button(tr("Close")).clicked() {
                                    close_review = true;
                                }
                            }
                        });
                    }
                }
            });
            if let Some(um) = find {
                self.find_candidates(um);
            }
            if let Some((um, s)) = use_song {
                self.use_candidate(um, s);
            }
            if close_review {
                *self.review.lock().unwrap() = None;
            }
        }
        ui.add_space(8.0);
        if ui.button(tr("Done")).clicked() {
            self.state.clear_finished();
            *self.review.lock().unwrap() = None;
            *self.history.lock().unwrap() = None;
        }
    }
}

fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(TEXT_DIM));
}

/// What a runner needs besides the YouTube client (fetched async).
struct RunnerParts {
    db: AppDb,
    matches: Arc<ImportMatches>,
    state: ImportState,
}

impl RunnerParts {
    fn build(&self, ytm: Arc<YtmClient>) -> Runner {
        ImportRunner { db: self.db.clone(), search: ytm, matches: self.matches.clone(), state: self.state.clone(), notify: Arc::new(repaint) }
    }
}

/// Read one playlist again for a re-sync (Android `PlaylistResync.readLink/readYouTube`).
async fn read_for_resync(li: &LinkImporter, ytm: &YtmClient, read: ResyncRead) -> Result<(Vec<ImportedTrack>, PlaylistOrigin, Option<String>), String> {
    match read {
        ResyncRead::Link(url) => {
            let parsed = li.fetch(&url, Some(ytm)).await.map_err(|e| e.0)?;
            let tracks = parsed.playlists.first().map(|p| p.1.clone()).unwrap_or_default();
            let note = if parsed.provider == Some(ImportSource::Spotify) && tracks.len() >= link::SPOTIFY_EMBED_LIMIT {
                Some(tr("Spotify only shares a playlist's first 100 songs by link. Use the account import to check the rest.").to_string())
            } else {
                parsed.warnings.first().cloned()
            };
            let origin = parsed.playlist_origins.values().next().cloned().unwrap_or(PlaylistOrigin { remote_id: None, remote_url: Some(url) });
            Ok((tracks, origin, note))
        }
        ResyncRead::YouTube(id) => {
            let parsed = if id.starts_with("MPREb_") {
                li.fetch(&format!("https://music.youtube.com/browse/{id}"), Some(ytm)).await
            } else {
                link::fetch_youtube_playlist(ytm, &id).await
            }
            .map_err(|e| e.0)?;
            Ok((parsed.playlists.first().map(|p| p.1.clone()).unwrap_or_default(), PlaylistOrigin { remote_id: Some(id), remote_url: None }, None))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(date_label(1_717_713_548_000), "2024-06-06 22:39");
        assert_eq!(date_label(0), "1970-01-01 00:00");
    }
}
