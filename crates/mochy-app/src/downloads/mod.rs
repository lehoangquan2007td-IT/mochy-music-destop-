//! Download manager (port of Android `DownloadUtil` + `AutoDownloads` +
//! `CachedSongs`, adapted to plain files on the desktop).
//!
//! - A queue with a concurrency limit (`max_parallel`, Android: 3), per-song
//!   progress, cancel / retry / remove, persisted in the `_download` table so
//!   unfinished downloads carry on after a restart.
//! - Each job resolves the stream with the player's own resolver (Opus
//!   first by default), fetches it in ranged chunks straight from the
//!   network, moves WebM-Opus into an `.opus` file (MP4/AAC stays `.m4a`),
//!   and writes the song's tags + cover + lyrics the way the Android export
//!   does, under the filename template in the chosen folder.
//! - Offline: [`offline_file`] hands the controller the downloaded file, so
//!   a downloaded song plays from disk instead of streaming.
//! - Auto-download liked songs (Android `AutoDownloads`' liked mark).
//! - Optional stream cache: a song that starts streaming is fetched a second
//!   time in the background (same URL, no extra YouTube lookups) into the
//!   cache folder, size-capped with LRU eviction.
//!
//! The manager is a process-wide singleton so pages, menus and the
//! controller reach it without threading state through the app.

pub mod ui;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use mochy_audio::TrackSource;
use mochy_core::download_settings::DownloadSettings;
use mochy_core::models::Song;
use mochy_core::settings::AppSettings;
use mochy_download::fetch::{self, FetchError};
use mochy_download::{Container, ExportTags, NameFields};
use mochy_innertube::CodecPreference;
use mochy_lyrics::ProviderId;
use mochy_storage::{DownloadRow, KIND_CACHE, KIND_DOWNLOAD};

use crate::controller::{PlayerController, Prepared};
use crate::services::Services;
use crate::ui::i18n::tr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Asked for by the user (or auto-download): tagged file in the music folder.
    Download,
    /// Kept by the stream cache: raw file in the cache folder.
    Cache,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Queued => "queued",
            Status::Running => "running",
            Status::Done => "done",
            Status::Failed => "failed",
            Status::Cancelled => "cancelled",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "running" | "queued" => Status::Queued,
            "done" => Status::Done,
            "cancelled" => Status::Cancelled,
            _ => Status::Failed,
        }
    }
    pub fn label(self) -> &'static str {
        tr(match self {
            Status::Queued => "Waiting",
            Status::Running => "Downloading",
            Status::Done => "Downloaded",
            Status::Failed => "Failed",
            Status::Cancelled => "Cancelled",
        })
    }
}

/// What a running job is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Resolving,
    Fetching,
    Tagging,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub song: Song,
    pub kind: Kind,
    pub status: Status,
    pub stage: Stage,
    pub done_bytes: u64,
    pub total_bytes: Option<u64>,
    pub path: Option<PathBuf>,
    pub error: Option<String>,
    pub created_at: i64,
    pub last_used_at: i64,
    /// Pre-resolved stream (cache items reuse the player's URL).
    stream: Option<(String, Vec<(String, String)>)>,
}

impl Item {
    pub fn progress(&self) -> f32 {
        match (self.status, self.total_bytes) {
            (Status::Done, _) => 1.0,
            (_, Some(t)) if t > 0 => (self.done_bytes as f32 / t as f32).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }
    pub fn is_active(&self) -> bool {
        matches!(self.status, Status::Queued | Status::Running)
    }
}

struct State {
    items: HashMap<String, Item>,
    /// Creation order of `items`.
    order: Vec<String>,
    cancels: HashMap<String, Arc<AtomicBool>>,
    cfg: DownloadSettings,
    lyrics_order: Vec<ProviderId>,
    /// Bumped on every change the UI may cache on.
    version: u64,
    batch_ok: usize,
    batch_failed: usize,
}

pub struct Manager {
    svc: Services,
    ctx: egui::Context,
    http: reqwest::Client,
    state: RwLock<State>,
}

static MANAGER: OnceLock<Manager> = OnceLock::new();

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// `%USERPROFILE%\Music\Mochy` (the platform music folder + `Mochy`).
pub fn default_folder() -> PathBuf {
    dirs::audio_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Music")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Mochy")
}

pub fn folder_of(cfg: &DownloadSettings) -> PathBuf {
    if cfg.folder.trim().is_empty() { default_folder() } else { PathBuf::from(cfg.folder.trim()) }
}

/// The download folder as currently configured.
pub fn current_folder() -> PathBuf {
    manager().map(|m| folder_of(&m.read().cfg)).unwrap_or_else(default_folder)
}

/// Where the stream cache keeps its files.
pub fn cache_folder() -> PathBuf {
    mochy_storage::cache_dir().join("songs")
}

fn temp_folder() -> PathBuf {
    mochy_storage::cache_dir().join("downloads-tmp")
}

fn manager() -> Option<&'static Manager> {
    MANAGER.get()
}

impl Manager {
    fn read(&self) -> RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(|e| e.into_inner())
    }
    fn write(&self) -> RwLockWriteGuard<'_, State> {
        self.state.write().unwrap_or_else(|e| e.into_inner())
    }

    fn persist(&self, item: &Item) {
        let Some(id) = item.song.video_id.clone() else { return };
        let row = DownloadRow {
            song_id: id,
            kind: if item.kind == Kind::Cache { KIND_CACHE } else { KIND_DOWNLOAD },
            state: item.status.as_str().to_string(),
            song_json: serde_json::to_string(&item.song).unwrap_or_else(|_| "{}".into()),
            file_path: item.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            bytes: item.done_bytes as i64,
            total_bytes: item.total_bytes.unwrap_or(0) as i64,
            error: item.error.clone(),
            created_at: item.created_at,
            updated_at: now_ms(),
            last_used_at: item.last_used_at,
        };
        self.svc.db.run(move |db| {
            crate::db::log_err("upsert_download", db.upsert_download(&row));
        });
    }

    /// Change one item under the lock, persist it and repaint.
    fn update(&self, id: &str, f: impl FnOnce(&mut Item)) {
        let snapshot = {
            let mut st = self.write();
            let Some(item) = st.items.get_mut(id) else { return };
            f(item);
            let snap = item.clone();
            st.version += 1;
            snap
        };
        self.persist(&snapshot);
        self.ctx.request_repaint();
    }

    /// Start queued jobs up to the limits (downloads: `max_parallel`, cache: 1).
    fn pump(&'static self) {
        let mut start: Vec<(String, Item, Arc<AtomicBool>)> = Vec::new();
        {
            let mut st = self.write();
            let limit = st.cfg.max_parallel.clamp(1, 8) as usize;
            let running = |st: &State, k: Kind| st.items.values().filter(|i| i.kind == k && i.status == Status::Running).count();
            let mut free_dl = limit.saturating_sub(running(&st, Kind::Download));
            let mut free_cache = 1usize.saturating_sub(running(&st, Kind::Cache));
            let order = st.order.clone();
            for id in order {
                if free_dl == 0 && free_cache == 0 {
                    break;
                }
                let Some(item) = st.items.get_mut(&id) else { continue };
                if item.status != Status::Queued {
                    continue;
                }
                let slot = if item.kind == Kind::Download { &mut free_dl } else { &mut free_cache };
                if *slot == 0 {
                    continue;
                }
                *slot -= 1;
                item.status = Status::Running;
                item.stage = Stage::Resolving;
                item.error = None;
                item.done_bytes = 0;
                let cancel = Arc::new(AtomicBool::new(false));
                start.push((id.clone(), item.clone(), cancel.clone()));
                st.cancels.insert(id, cancel);
            }
            if !start.is_empty() {
                st.version += 1;
            }
        }
        for (id, item, cancel) in start {
            self.persist(&item);
            let (cfg, order) = {
                let st = self.read();
                (st.cfg.clone(), st.lyrics_order.clone())
            };
            self.svc.spawn(async move {
                let result = run_job(self, &item, &cfg, order, &cancel).await;
                self.finish(&id, result, cancel.load(Ordering::Relaxed));
            });
        }
        self.ctx.request_repaint();
    }

    fn finish(&'static self, id: &str, result: Result<(PathBuf, u64), String>, cancelled: bool) {
        let mut kind = Kind::Download;
        let mut title = String::new();
        self.update(id, |item| {
            kind = item.kind;
            title = item.song.title.clone();
            match &result {
                Ok((path, size)) => {
                    item.status = Status::Done;
                    item.path = Some(path.clone());
                    item.done_bytes = *size;
                    item.total_bytes = Some(*size);
                    item.error = None;
                    item.last_used_at = now_ms();
                }
                Err(_) if cancelled => {
                    item.status = Status::Cancelled;
                    item.error = None;
                }
                Err(e) => {
                    item.status = Status::Failed;
                    item.error = Some(e.clone());
                }
            }
        });
        {
            let mut st = self.write();
            st.cancels.remove(id);
            if kind == Kind::Download {
                match &result {
                    Ok(_) => st.batch_ok += 1,
                    Err(_) if !cancelled => st.batch_failed += 1,
                    Err(_) => {}
                }
            }
        }
        match &result {
            Ok(_) if kind == Kind::Download => {
                let sid = id.to_string();
                self.svc.db.run(move |db| {
                    crate::db::log_err("set_song_downloaded", db.set_song_downloaded(&sid, true));
                });
            }
            Ok(_) => self.evict_cache(),
            Err(e) if !cancelled => tracing::warn!(%id, %title, "download failed: {e}"),
            Err(_) => {}
        }
        self.batch_toast();
        self.pump();
    }

    /// One toast when a batch of downloads has drained.
    fn batch_toast(&self) {
        let (ok, failed) = {
            let mut st = self.write();
            let busy = st.items.values().any(|i| i.kind == Kind::Download && i.is_active());
            if busy || st.batch_ok + st.batch_failed == 0 {
                return;
            }
            let r = (st.batch_ok, st.batch_failed);
            st.batch_ok = 0;
            st.batch_failed = 0;
            r
        };
        let mut text = tr("Downloads finished: {ok} saved").replace("{ok}", &ok.to_string());
        if failed > 0 {
            text.push_str(&tr(", {n} failed").replace("{n}", &failed.to_string()));
        }
        self.svc.bus.toast(text);
    }

    fn add(&'static self, song: Song, kind: Kind, stream: Option<(String, Vec<(String, String)>)>) -> bool {
        let Some(id) = song.video_id.clone() else { return false };
        let item = {
            let mut st = self.write();
            if let Some(existing) = st.items.get_mut(&id) {
                // Done or on its way: asking again would start a finished one over.
                let upgrade = existing.kind == Kind::Cache && kind == Kind::Download;
                if !upgrade && matches!(existing.status, Status::Done | Status::Queued | Status::Running) {
                    return false;
                }
                if upgrade && existing.status == Status::Running {
                    return false;
                }
                if upgrade
                    && existing.status == Status::Done
                    && let Some(p) = existing.path.take()
                {
                    let _ = std::fs::remove_file(p);
                }
                existing.kind = kind;
                existing.status = Status::Queued;
                existing.error = None;
                existing.song = song;
                existing.stream = stream;
                existing.clone()
            } else {
                let item = Item {
                    song,
                    kind,
                    status: Status::Queued,
                    stage: Stage::Resolving,
                    done_bytes: 0,
                    total_bytes: None,
                    path: None,
                    error: None,
                    created_at: now_ms(),
                    last_used_at: 0,
                    stream,
                };
                st.order.push(id.clone());
                st.items.insert(id.clone(), item.clone());
                item
            }
        };
        self.write().version += 1;
        self.persist(&item);
        true
    }

    /// Drop least-recently-used cache files over the size limit.
    fn evict_cache(&self) {
        let (limit_mb, mut cached): (i32, Vec<(String, u64, i64, Option<PathBuf>)>) = {
            let st = self.read();
            (
                st.cfg.cache_max_mb,
                st.items
                    .values()
                    .filter(|i| i.kind == Kind::Cache && i.status == Status::Done)
                    .map(|i| (i.song.video_id.clone().unwrap_or_default(), i.done_bytes, i.last_used_at.max(i.created_at), i.path.clone()))
                    .collect(),
            )
        };
        if limit_mb < 0 {
            return;
        }
        let limit = limit_mb as u64 * 1024 * 1024;
        let mut total: u64 = cached.iter().map(|c| c.1).sum();
        cached.sort_by_key(|c| c.2);
        for (id, size, _, path) in cached {
            if total <= limit {
                break;
            }
            if let Some(p) = path {
                let _ = std::fs::remove_file(p);
            }
            total = total.saturating_sub(size);
            self.forget(&id);
        }
    }

    fn forget(&self, id: &str) {
        {
            let mut st = self.write();
            st.items.remove(id);
            st.order.retain(|x| x != id);
            st.version += 1;
        }
        let sid = id.to_string();
        self.svc.db.run(move |db| {
            crate::db::log_err("delete_download", db.delete_download(&sid));
        });
        self.ctx.request_repaint();
    }

    fn set_stage(&self, id: &str, stage: Stage) {
        let mut st = self.write();
        if let Some(i) = st.items.get_mut(id) {
            i.stage = stage;
        }
        st.version += 1;
        drop(st);
        self.ctx.request_repaint();
    }

    fn progress(&self, id: &str, done: u64, total: Option<u64>) {
        {
            let mut st = self.write();
            if let Some(i) = st.items.get_mut(id) {
                i.done_bytes = done;
                i.total_bytes = total;
            }
        }
        self.ctx.request_repaint_after(Duration::from_millis(150));
    }
}

// ---------------------------------------------------------------------------
// The job
// ---------------------------------------------------------------------------

async fn resolve(svc: &Services, video_id: &str, cfg: &DownloadSettings) -> Result<(String, Vec<(String, String)>, Option<u64>), String> {
    let client = svc.ytm().await?;
    let pref = if cfg.prefer_opus { CodecPreference::PreferOpus } else { CodecPreference::PreferAac };
    let st = client.resolve_stream_with(video_id, cfg.quality, pref).await.map_err(|e| e.to_string())?;
    tracing::info!(%video_id, format = %crate::controller::format_label(&st), client = %st.client, "download stream resolved");
    Ok((st.url, st.headers, st.content_length))
}

async fn run_job(
    m: &'static Manager,
    item: &Item,
    cfg: &DownloadSettings,
    lyrics_order: Vec<ProviderId>,
    cancel: &AtomicBool,
) -> Result<(PathBuf, u64), String> {
    let mut song = item.song.clone();
    crate::songedit::apply(&mut song);
    let vid = song.video_id.clone().ok_or("song has no YouTube id")?;
    let (mut url, mut headers, mut len) = match &item.stream {
        Some((u, h)) => (u.clone(), h.clone(), None),
        None => resolve(&m.svc, &vid, cfg).await?,
    };
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    m.set_stage(&vid, Stage::Fetching);
    let tmp = temp_folder().join(format!("{}.part", mochy_download::safe_name(&vid, "song")));
    let mut re_resolved = false;
    let size = loop {
        let r = fetch::fetch_to_file(&m.http, &url, &headers, &tmp, len, cancel, |d, t| m.progress(&vid, d, t)).await;
        match r {
            Ok(n) => break n,
            // An expired / cut-off URL: one fresh resolve, then start over in one format.
            Err(FetchError::Refused(_)) if !re_resolved => {
                re_resolved = true;
                (url, headers, len) = resolve(&m.svc, &vid, cfg).await?;
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&tmp).await;
                return Err(e.to_string());
            }
        }
    };
    if size == 0 {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(tr("The server sent an empty file").to_string());
    }

    let result = if item.kind == Kind::Cache { finish_cache(&tmp, &vid).await } else { finish_download(m, &tmp, &song, cfg, lyrics_order).await };
    let _ = tokio::fs::remove_file(&tmp).await;
    result
}

async fn finish_cache(tmp: &Path, vid: &str) -> Result<(PathBuf, u64), String> {
    let head = read_head(tmp).await;
    let ext = mochy_download::sniff::sniff(&head).extension();
    let dest = cache_folder().join(format!("{}.{ext}", mochy_download::safe_name(vid, "song")));
    move_file(tmp, &dest).await?;
    let size = tokio::fs::metadata(&dest).await.map(|m| m.len()).unwrap_or(0);
    Ok((dest, size))
}

async fn read_head(path: &Path) -> Vec<u8> {
    use tokio::io::AsyncReadExt;
    let mut head = vec![0u8; 64];
    let n = match tokio::fs::File::open(path).await {
        Ok(mut f) => f.read(&mut head).await.unwrap_or(0),
        Err(_) => 0,
    };
    head.truncate(n);
    head
}

async fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(dir) = to.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    if tokio::fs::rename(from, to).await.is_ok() {
        return Ok(());
    }
    // Another drive: copy, then drop the temp file.
    tokio::fs::copy(from, to).await.map_err(|e| format!("{}: {e}", to.display()))?;
    let _ = tokio::fs::remove_file(from).await;
    Ok(())
}

/// The song's tags, with cover / lyrics as the settings ask.
async fn build_tags(svc: &Services, song: &Song, cfg: &DownloadSettings, lyrics_order: Vec<ProviderId>) -> (ExportTags, Option<String>) {
    let artists: Vec<String> = song.artists.iter().map(|a| a.name.clone()).filter(|n| !n.is_empty()).collect();
    let mut tags = ExportTags::for_song(&song.title, &artists, song.album_name.as_deref(), song.year);
    let lyrics = if cfg.embed_lyrics || cfg.lyrics_file { lyrics_for(svc, song, lyrics_order).await } else { None };
    if cfg.embed_lyrics {
        tags.lyrics = lyrics.as_deref().and_then(mochy_download::lyrics_file::to_standard_lrc);
    }
    if cfg.embed_cover
        && let Some(bytes) = cover_bytes(svc, song).await
    {
        tags.cover = tokio::task::spawn_blocking(move || mochy_download::cover::prepare_cover(&bytes)).await.ok().flatten();
    }
    (tags, lyrics)
}

/// Stored lyrics for the song; when there are none yet, the providers are
/// asked once (and the answer saved), so the download has them offline —
/// Android `DownloadUtil.saveLyrics`.
async fn lyrics_for(svc: &Services, song: &Song, order: Vec<ProviderId>) -> Option<String> {
    let key = song.video_id.clone().unwrap_or_else(|| song.id.clone());
    let k = key.clone();
    let stored = svc.db.query(move |db| db.load_lyrics(&k).ok().flatten()).await.ok().flatten();
    if let Some((_, text)) = stored {
        return Some(text);
    }
    let not_found = svc.db.query(move |db| db.lyrics_not_found(&key).unwrap_or(false)).await.unwrap_or(false);
    if not_found {
        return None;
    }
    let state = tokio::time::timeout(Duration::from_secs(40), crate::lyrics::load(svc.clone(), song.clone(), order)).await.ok()?;
    match state {
        crate::ui::lyrics_view::LyricsState::Loaded(l) => Some(l.to_lrc()).filter(|t| !t.trim().is_empty()),
        _ => None,
    }
}

/// Artwork bytes: a picked `file://` cover, else the best thumbnail URL.
async fn cover_bytes(svc: &Services, song: &Song) -> Option<Vec<u8>> {
    let url = song.thumbnail_url.clone().filter(|u| !u.is_empty())?;
    if let Some(p) = crate::songedit::file_url_path(&url) {
        return tokio::fs::read(p).await.ok();
    }
    let (_, urls, _) = crate::images::fetch_plan(&crate::images::ImageSrc::Url(url), mochy_download::cover::COVER_PX);
    for u in urls {
        if let Ok(resp) = svc.http.get(&u).send().await
            && resp.status().is_success()
            && let Ok(b) = resp.bytes().await
            && !b.is_empty()
        {
            return Some(b.to_vec());
        }
    }
    None
}

fn name_fields(song: &Song) -> NameFields {
    let artists: Vec<&str> = song.artists.iter().map(|a| a.name.as_str()).filter(|n| !n.is_empty()).collect();
    NameFields {
        title: song.title.clone(),
        artist: artists.join(", "),
        album: song.album_name.clone().unwrap_or_default(),
        album_artist: song.album_name.as_ref().and_then(|_| artists.first().map(|s| s.to_string())).unwrap_or_default(),
        year: song.year,
        id: song.video_id.clone().unwrap_or_else(|| song.id.clone()),
    }
}

async fn finish_download(
    m: &'static Manager,
    tmp: &Path,
    song: &Song,
    cfg: &DownloadSettings,
    lyrics_order: Vec<ProviderId>,
) -> Result<(PathBuf, u64), String> {
    let vid = song.video_id.clone().unwrap_or_default();
    m.set_stage(&vid, Stage::Tagging);
    let (tags, lyrics) = build_tags(&m.svc, song, cfg, lyrics_order).await;
    let bytes = tokio::fs::read(tmp).await.map_err(|e| e.to_string())?;
    let container = mochy_download::sniff::sniff(&bytes[..bytes.len().min(64)]);

    // Where it goes: folder / template (sub-folders allowed) + extension.
    let rel = mochy_download::render_template(&cfg.filename_template, &name_fields(song));
    let root = folder_of(cfg);
    let dir = rel.parent().map(|p| root.join(p)).unwrap_or_else(|| root.clone());
    let base = rel.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| vid.clone());
    let previous = m.read().items.get(&vid).and_then(|i| i.path.clone());
    tokio::fs::create_dir_all(&dir).await.map_err(|e| format!("{}: {e}", dir.display()))?;

    let (data, ext, tag_with_lofty) = if container == Container::WebM {
        let t = tags.clone();
        match tokio::task::spawn_blocking(move || mochy_download::opus::webm_to_ogg_opus(&bytes, &t).map_err(|e| (e, bytes))).await {
            Ok(Ok(ogg)) => (ogg, "opus", false),
            // Not Opus / unreadable: keep the original, like Android does.
            Ok(Err((e, bytes))) => {
                tracing::warn!(%vid, "remux to Ogg Opus failed ({e}); keeping WebM");
                (bytes, "webm", false)
            }
            Err(e) => return Err(e.to_string()),
        }
    } else {
        (bytes, container.extension(), true)
    };
    let dest = mochy_download::unique_path(&dir, &base, ext, |p| previous.as_deref() == Some(p));
    tokio::fs::write(&dest, &data).await.map_err(|e| format!("{}: {e}", dest.display()))?;
    if tag_with_lofty {
        let (d, t) = (dest.clone(), tags.clone());
        match tokio::task::spawn_blocking(move || mochy_download::tags::write_with_lofty(&d, &t)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(%vid, "tagging failed: {e}"),
            Err(e) => tracing::warn!(%vid, "tagging task failed: {e}"),
        }
    }
    if let Some(prev) = previous.filter(|p| *p != dest) {
        let _ = tokio::fs::remove_file(prev).await;
    }
    if cfg.lyrics_file
        && let Some(stored) = lyrics
        && let Some(text) = mochy_download::lyrics_file::to_standard_lrc(&stored)
    {
        let side = dest.with_extension(mochy_download::lyrics_file::sidecar_extension(&stored));
        if let Err(e) = tokio::fs::write(&side, text).await {
            tracing::warn!("lyrics file: {e}");
        }
    }
    let size = tokio::fs::metadata(&dest).await.map(|md| md.len()).unwrap_or(data.len() as u64);
    tracing::info!(%vid, path = %dest.display(), size, "download saved");
    Ok((dest, size))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Start the manager: load the table, resume unfinished downloads, follow
/// auto-downloads. Call once at startup.
pub fn init(svc: Services, ctx: egui::Context, settings: &AppSettings) {
    let state = State {
        items: HashMap::new(),
        order: Vec::new(),
        cancels: HashMap::new(),
        cfg: settings.downloads.clone(),
        lyrics_order: crate::lyrics::provider_order(&settings.lyrics),
        version: 0,
        batch_ok: 0,
        batch_failed: 0,
    };
    let m = Manager { http: fetch::client(), svc, ctx, state: RwLock::new(state) };
    if MANAGER.set(m).is_err() {
        return;
    }
    let Some(m) = manager() else { return };
    let rows = m.svc.db.query(|db| db.downloads().unwrap_or_default());
    m.svc.spawn(async move {
        let rows = rows.await.unwrap_or_default();
        load_rows(m, rows).await;
        m.pump();
        follow_auto_downloads(m).await;
    });
}

async fn load_rows(m: &'static Manager, rows: Vec<DownloadRow>) {
    let mut missing_cache = Vec::new();
    let mut items = Vec::new();
    for r in rows {
        let Ok(song) = serde_json::from_str::<Song>(&r.song_json) else { continue };
        let kind = if r.kind == KIND_CACHE { Kind::Cache } else { Kind::Download };
        let mut status = Status::parse(&r.state);
        let path = r.file_path.map(PathBuf::from);
        let mut error = r.error;
        if status == Status::Done && !path.as_ref().is_some_and(|p| p.exists()) {
            if kind == Kind::Cache {
                missing_cache.push(r.song_id);
                continue;
            }
            status = Status::Failed;
            error = Some(tr("File missing").to_string());
        }
        items.push(Item {
            song,
            kind,
            status,
            stage: Stage::Resolving,
            done_bytes: r.bytes.max(0) as u64,
            total_bytes: (r.total_bytes > 0).then_some(r.total_bytes as u64),
            path,
            error,
            created_at: r.created_at,
            last_used_at: r.last_used_at,
            stream: None,
        });
    }
    {
        let mut st = m.write();
        for item in items {
            let Some(id) = item.song.video_id.clone() else { continue };
            // Cache items queued at exit had a now-expired player URL.
            if item.kind == Kind::Cache && item.status != Status::Done {
                missing_cache.push(id);
                continue;
            }
            if !st.items.contains_key(&id) {
                st.order.push(id.clone());
            }
            st.items.insert(id, item);
        }
        st.version += 1;
    }
    for id in missing_cache {
        m.forget(&id);
    }
    tracing::info!(count = m.read().items.len(), "downloads loaded");
}

/// Liked songs since the mark start downloading as they come in.
async fn follow_auto_downloads(m: &'static Manager) {
    loop {
        let since = {
            let st = m.read();
            st.cfg.auto_download_liked.then_some(st.cfg.auto_download_liked_since).flatten()
        };
        if let Some(since) = since {
            let liked = m.svc.db.query(|db| db.liked_songs().unwrap_or_default()).await.unwrap_or_default();
            let fresh: Vec<Song> = liked
                .into_iter()
                .filter(|s| s.video_id.is_some() && !s.is_local() && s.liked_date.unwrap_or(0) >= since)
                .filter(|s| !m.read().items.contains_key(s.video_id.as_deref().unwrap_or("")))
                .collect();
            if !fresh.is_empty() {
                tracing::info!(count = fresh.len(), "auto-downloading liked songs");
                for s in fresh {
                    m.add(s, Kind::Download, None);
                }
                m.pump();
            }
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

/// Settings changed (called by the app whenever settings change).
pub fn on_settings(s: &AppSettings) {
    let Some(m) = manager() else { return };
    let (pump, evict, clear) = {
        let mut st = m.write();
        if st.cfg == s.downloads && st.lyrics_order == crate::lyrics::provider_order(&s.lyrics) {
            return;
        }
        let pump = s.downloads.max_parallel != st.cfg.max_parallel;
        let evict = s.downloads.cache_max_mb != st.cfg.cache_max_mb;
        let clear = s.downloads.cache_max_mb == 0 && st.cfg.cache_max_mb != 0;
        st.cfg = s.downloads.clone();
        st.lyrics_order = crate::lyrics::provider_order(&s.lyrics);
        (pump, evict, clear)
    };
    if clear {
        clear_cache();
    } else if evict {
        m.evict_cache();
    }
    if pump {
        m.pump();
    }
}

/// Queue songs for download (songs without a YouTube id are skipped).
pub fn enqueue(songs: Vec<Song>) {
    let Some(m) = manager() else { return };
    let mut added = 0usize;
    for song in songs {
        if song.is_local() || song.video_id.is_none() {
            continue;
        }
        let s = song.clone();
        m.svc.db.run(move |db| crate::db::ensure_song(db, &s));
        if m.add(song, Kind::Download, None) {
            added += 1;
        }
    }
    if added > 0 {
        m.svc.bus.toast(if added == 1 { tr("Download started").to_string() } else { tr("Downloading {n} songs").replace("{n}", &added.to_string()) });
        m.pump();
    }
}

pub fn cancel(id: &str) {
    let Some(m) = manager() else { return };
    let flag = m.read().cancels.get(id).cloned();
    match flag {
        Some(f) => f.store(true, Ordering::Relaxed),
        None => m.update(id, |i| {
            if i.status == Status::Queued {
                i.status = Status::Cancelled;
            }
        }),
    }
}

pub fn cancel_all() {
    let Some(m) = manager() else { return };
    let ids: Vec<String> = m.read().items.values().filter(|i| i.kind == Kind::Download && i.is_active()).filter_map(|i| i.song.video_id.clone()).collect();
    for id in ids {
        cancel(&id);
    }
}

pub fn retry(id: &str) {
    let Some(m) = manager() else { return };
    m.update(id, |i| {
        if matches!(i.status, Status::Failed | Status::Cancelled) {
            i.status = Status::Queued;
            i.error = None;
            i.stream = None;
        }
    });
    m.pump();
}

pub fn retry_failed() {
    let Some(m) = manager() else { return };
    let ids: Vec<String> = m
        .read()
        .items
        .values()
        .filter(|i| i.kind == Kind::Download && matches!(i.status, Status::Failed | Status::Cancelled))
        .filter_map(|i| i.song.video_id.clone())
        .collect();
    for id in &ids {
        m.update(id, |i| {
            i.status = Status::Queued;
            i.error = None;
        });
    }
    m.pump();
}

/// Delete the downloaded file and forget the download.
pub fn remove(id: &str) {
    let Some(m) = manager() else { return };
    cancel(id);
    let path = m.read().items.get(id).and_then(|i| i.path.clone());
    if let Some(p) = path {
        if let Err(e) = std::fs::remove_file(&p)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            m.svc.bus.toast(format!("{}: {e}", tr("Couldn't delete the file")));
            return;
        }
        for ext in ["lrc", "txt"] {
            let side = p.with_extension(ext);
            if side.exists() {
                let _ = std::fs::remove_file(side);
            }
        }
    }
    m.forget(id);
    let sid = id.to_string();
    m.svc.db.run(move |db| {
        crate::db::log_err("set_song_downloaded", db.set_song_downloaded(&sid, false));
    });
}

/// Forget finished-with-error / cancelled rows.
pub fn clear_failed() {
    let Some(m) = manager() else { return };
    let ids: Vec<String> = m
        .read()
        .items
        .values()
        .filter(|i| i.kind == Kind::Download && matches!(i.status, Status::Failed | Status::Cancelled))
        .filter_map(|i| i.song.video_id.clone())
        .collect();
    for id in ids {
        m.forget(&id);
    }
}

/// Status of a song's download (cache entries count as not downloaded).
pub fn status(id: &str) -> Option<(Status, f32)> {
    let m = manager()?;
    let st = m.read();
    let i = st.items.get(id)?;
    (i.kind == Kind::Download).then(|| (i.status, i.progress()))
}

/// Download-kind items in creation order, plus the change counter.
pub fn snapshot() -> (u64, Vec<Item>) {
    let Some(m) = manager() else { return (0, Vec::new()) };
    let st = m.read();
    let items = st.order.iter().filter_map(|id| st.items.get(id)).filter(|i| i.kind == Kind::Download).cloned().collect();
    (st.version, items)
}

/// Bytes on disk: (downloads, stream cache).
pub fn usage() -> (u64, u64) {
    let Some(m) = manager() else { return (0, 0) };
    let st = m.read();
    let mut out = (0, 0);
    for i in st.items.values().filter(|i| i.status == Status::Done) {
        match i.kind {
            Kind::Download => out.0 += i.done_bytes,
            Kind::Cache => out.1 += i.done_bytes,
        }
    }
    out
}

/// Empty the stream cache (downloads stay).
pub fn clear_cache() {
    let Some(m) = manager() else { return };
    let ids: Vec<(String, Option<PathBuf>)> = m
        .read()
        .items
        .values()
        .filter(|i| i.kind == Kind::Cache && i.status != Status::Running)
        .map(|i| (i.song.video_id.clone().unwrap_or_default(), i.path.clone()))
        .collect();
    for (id, path) in ids {
        if let Some(p) = path {
            let _ = std::fs::remove_file(p);
        }
        m.forget(&id);
    }
}

/// The local file to play for `song` instead of streaming it: a finished
/// download (when "play downloads offline" is on) or a stream-cache copy.
pub fn offline_file(song: &Song) -> Option<PathBuf> {
    let m = manager()?;
    let vid = song.video_id.as_deref()?;
    let (path, kind) = {
        let st = m.read();
        let i = st.items.get(vid)?;
        if i.status != Status::Done {
            return None;
        }
        match i.kind {
            Kind::Download if !st.cfg.play_offline => return None,
            Kind::Cache if st.cfg.cache_max_mb == 0 => return None,
            _ => {}
        }
        (i.path.clone()?, i.kind)
    };
    if !path.exists() {
        if kind == Kind::Cache {
            m.forget(vid);
        } else {
            m.update(vid, |i| {
                i.status = Status::Failed;
                i.error = Some(tr("File missing").to_string());
            });
        }
        return None;
    }
    let at = now_ms();
    {
        let mut st = m.write();
        if let Some(i) = st.items.get_mut(vid) {
            i.last_used_at = at;
        }
    }
    let id = vid.to_string();
    m.svc.db.run(move |db| {
        crate::db::log_err("touch_download", db.touch_download(&id, at));
    });
    tracing::info!(%vid, path = %path.display(), "playing from disk");
    Some(path)
}

/// Stream cache: a song that just resolved to a remote stream is fetched in
/// the background from the same URL (call before `on_prepared`).
pub fn observe_prepared(player: &PlayerController, uid: u64, result: &Result<Prepared, String>) {
    let Some(m) = manager() else { return };
    if m.read().cfg.cache_max_mb == 0 {
        return;
    }
    let (Some(entry), Ok(p)) = (player.queue.current(), result) else { return };
    if entry.uid != uid {
        return;
    }
    let TrackSource::Remote { url, headers } = &p.source else { return };
    let song = entry.song.clone();
    let Some(vid) = song.video_id.clone() else { return };
    if m.read().items.contains_key(&vid) {
        return;
    }
    if m.add(song, Kind::Cache, Some((url.clone(), headers.clone()))) {
        m.pump();
    }
}

/// Rewrite a downloaded file's tags after a song edit.
pub fn retag(song: &Song) {
    let Some(m) = manager() else { return };
    let Some(vid) = song.video_id.clone() else { return };
    let (path, cfg, order) = {
        let st = m.read();
        let Some(i) = st.items.get(&vid).filter(|i| i.kind == Kind::Download && i.status == Status::Done) else { return };
        let Some(p) = i.path.clone() else { return };
        (p, st.cfg.clone(), st.lyrics_order.clone())
    };
    let mut song = song.clone();
    crate::songedit::apply(&mut song);
    m.svc.spawn(async move {
        let (tags, _) = build_tags(&m.svc, &song, &cfg, order).await;
        let p = path.clone();
        match tokio::task::spawn_blocking(move || mochy_download::tags::write_with_lofty(&p, &tags)).await {
            Ok(Ok(())) => tracing::info!(path = %path.display(), "retagged after edit"),
            Ok(Err(e)) => tracing::warn!("retag failed: {e}"),
            Err(e) => tracing::warn!("retag task failed: {e}"),
        }
        m.update(&vid, |i| i.song = song);
    });
}
