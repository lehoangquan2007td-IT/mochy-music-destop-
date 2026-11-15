//! Shared background services: tokio runtime handle, message bus back to the
//! UI thread, lazily-initialised InnerTube client, HTTP client, DB worker.

use std::future::Future;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, OnceLock};

use mochy_core::models::{Album, Artist, Playlist, SearchResults, Shelf, Song};
use mochy_innertube::{StreamResult, YtmClient, YtmResult};

use crate::db::DbWorker;
use crate::images::ImageMsg;
use crate::ui::lyrics_view::LyricsState;

/// Everything a background task can tell the UI thread.
pub enum AppMsg {
    Engine(mochy_audio::Event),
    /// Result of initialising the YTM client (first network contact).
    YtmReady(Result<(), String>),
    Home(Result<Vec<Shelf>, String>),
    Suggestions { query: String, items: Vec<String> },
    Search { query: String, result: Result<SearchResults, String> },
    Album { id: String, result: Result<(Album, Vec<Song>), String> },
    Artist { id: String, result: Result<(Artist, Vec<Shelf>), String> },
    Playlist { id: String, result: Result<(Playlist, Vec<Song>), String> },
    Related { video_id: String, result: Result<Vec<Song>, String> },
    /// A track is ready to hand to the engine (or failed to resolve).
    Prepared { generation: u64, uid: u64, result: Result<crate::controller::Prepared, String> },
    /// Next-track URL prefetch.
    Prefetched { video_id: String, stream: StreamResult },
    /// Radio / autoplay results to append.
    Radio { seed: String, result: Result<Vec<Song>, String>, play_after: bool },
    QueueRestored { songs: Vec<Song>, index: usize },
    Lyrics { song_id: String, state: LyricsState },
    Image(ImageMsg),
    Library(LibraryMsg),
    Scan(ScanMsg),
    MediaKey(crate::media_keys::MediaKey),
    OutputDevices(Vec<String>),
    Toast(String),
}

pub enum LibraryMsg {
    Liked(Vec<Song>),
    History(Vec<Song>),
    MostPlayed(Vec<Song>),
    Local(Vec<Song>),
    Playlists(Vec<Playlist>),
    PlaylistSongs { id: String, songs: Vec<Song> },
    SearchHistory(Vec<String>),
}

pub enum ScanMsg {
    Started(std::path::PathBuf),
    Progress { scanned: usize, added: usize },
    Done { added: usize },
    Cancelled,
}

/// Sender side of the UI message channel; wakes the UI on every message.
#[derive(Clone)]
pub struct Bus {
    tx: Sender<AppMsg>,
    ctx: Arc<OnceLock<egui::Context>>,
}

impl Bus {
    pub fn new() -> (Self, Receiver<AppMsg>) {
        let (tx, rx) = std::sync::mpsc::channel();
        (Self { tx, ctx: Arc::new(OnceLock::new()) }, rx)
    }

    pub fn set_ctx(&self, ctx: egui::Context) {
        let _ = self.ctx.set(ctx);
    }

    pub fn send(&self, msg: AppMsg) {
        let _ = self.tx.send(msg);
        if let Some(ctx) = self.ctx.get() {
            ctx.request_repaint();
        }
    }

    pub fn toast(&self, text: impl Into<String>) {
        self.send(AppMsg::Toast(text.into()));
    }
}

#[derive(Clone)]
pub struct Services {
    pub rt: tokio::runtime::Handle,
    pub bus: Bus,
    /// Shared HTTP client for images & lyrics.
    pub http: reqwest::Client,
    pub db: DbWorker,
    ytm: Arc<tokio::sync::OnceCell<Arc<YtmClient>>>,
}

impl Services {
    pub fn new(rt: tokio::runtime::Handle, bus: Bus, db: DbWorker) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(mochy_innertube::clients::WEB_UA)
            .gzip(true)
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(40))
            .build()
            .unwrap_or_default();
        Self { rt, bus, http, db, ytm: Arc::new(tokio::sync::OnceCell::new()) }
    }

    /// The InnerTube client, created on first use (retried on later calls if
    /// creation failed, e.g. offline at startup).
    pub async fn ytm(&self) -> Result<Arc<YtmClient>, String> {
        let bus = self.bus.clone();
        self.ytm
            .get_or_try_init(|| async move {
                match YtmClient::new().await {
                    Ok(c) => {
                        tracing::info!("YouTube Music client ready");
                        bus.send(AppMsg::YtmReady(Ok(())));
                        Ok(Arc::new(c))
                    }
                    Err(e) => {
                        tracing::error!("YouTube Music client init failed: {e}");
                        bus.send(AppMsg::YtmReady(Err(e.to_string())));
                        Err(e.to_string())
                    }
                }
            })
            .await
            .cloned()
    }

    pub fn spawn<F>(&self, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.rt.spawn(fut);
    }

    /// Run an InnerTube call on the runtime and post `map(result)` to the UI.
    pub fn ytm_call<T, Fut, F, M>(&self, what: &'static str, f: F, map: M)
    where
        T: Send + 'static,
        Fut: Future<Output = YtmResult<T>> + Send + 'static,
        F: FnOnce(Arc<YtmClient>) -> Fut + Send + 'static,
        M: FnOnce(Result<T, String>) -> AppMsg + Send + 'static,
    {
        let svc = self.clone();
        self.rt.spawn(async move {
            let res = match svc.ytm().await {
                Ok(client) => f(client).await.map_err(|e| e.to_string()),
                Err(e) => Err(format!("offline: {e}")),
            };
            if let Err(e) = &res {
                tracing::warn!("{what} failed: {e}");
            }
            svc.bus.send(map(res));
        });
    }
}
