//! Async artwork loader: reqwest → disk cache (`art_cache_file`) → decode off
//! the UI thread → egui texture. LRU-capped by texture bytes. Local files use
//! embedded artwork via lofty (or a `cover.jpg`/`folder.jpg` next to them).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{ColorImage, TextureHandle, TextureOptions};
use mochy_core::models::Song;

use crate::services::{AppMsg, Services};

/// Edge length of list / card thumbnails.
pub const THUMB_PX: u32 = 226;
/// Edge length of now-playing artwork.
pub const LARGE_PX: u32 = 544;

const MAX_TEXTURE_BYTES: usize = 160 * 1024 * 1024;
const RETRY_FAILED_AFTER: Duration = Duration::from_secs(45);

/// Tiny RGBA copy of an image for palette extraction.
#[derive(Debug)]
pub struct SmallRgba {
    pub rgba: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

pub enum ImageMsg {
    Loaded { key: String, image: ColorImage, small: Arc<SmallRgba> },
    Failed { key: String, err: String },
}

enum State {
    Loading,
    Ready { tex: TextureHandle, small: Arc<SmallRgba>, bytes: usize },
    Failed(Instant),
}

struct Entry {
    state: State,
    last_used: u64,
}

pub struct ImageCache {
    svc: Services,
    map: HashMap<String, Entry>,
    frame: u64,
    bytes: usize,
    sem: Arc<tokio::sync::Semaphore>,
}

/// Where an image comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum ImageSrc {
    Url(String),
    Local(PathBuf),
}

impl ImageSrc {
    pub fn key(&self, px: u32) -> String {
        match self {
            ImageSrc::Url(u) => upgrade_thumbnail(u, px),
            ImageSrc::Local(p) => format!("local:{px}:{}", p.display()),
        }
    }
}

/// Image source for a song's artwork.
pub fn song_src(song: &Song) -> Option<ImageSrc> {
    if let Some(u) = song.thumbnail_url.as_deref().filter(|u| !u.is_empty()) {
        return Some(ImageSrc::Url(u.to_string()));
    }
    song.local_path.as_ref().map(|p| ImageSrc::Local(p.clone()))
}

impl ImageCache {
    pub fn new(svc: Services) -> Self {
        Self {
            svc,
            map: HashMap::new(),
            frame: 0,
            bytes: 0,
            sem: Arc::new(tokio::sync::Semaphore::new(6)),
        }
    }

    /// Call once per frame: advances the LRU clock and evicts old textures.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        if self.bytes <= MAX_TEXTURE_BYTES {
            return;
        }
        let mut ready: Vec<(u64, String)> = self
            .map
            .iter()
            .filter(|(_, e)| matches!(e.state, State::Ready { .. }))
            .map(|(k, e)| (e.last_used, k.clone()))
            .collect();
        ready.sort();
        for (used, key) in ready {
            if self.bytes <= MAX_TEXTURE_BYTES * 3 / 4 || used + 2 >= self.frame {
                break;
            }
            if let Some(Entry { state: State::Ready { bytes, .. }, .. }) = self.map.remove(&key) {
                self.bytes = self.bytes.saturating_sub(bytes);
            }
        }
    }

    /// Texture for `src` at `px`, starting a load when missing.
    pub fn get(&mut self, src: &ImageSrc, px: u32) -> Option<TextureHandle> {
        let key = src.key(px);
        let frame = self.frame;
        if let Some(e) = self.map.get_mut(&key) {
            e.last_used = frame;
            match &e.state {
                State::Ready { tex, .. } => return Some(tex.clone()),
                State::Loading => return None,
                State::Failed(at) if at.elapsed() < RETRY_FAILED_AFTER => return None,
                State::Failed(_) => {}
            }
        }
        self.map.insert(key.clone(), Entry { state: State::Loading, last_used: frame });
        self.start_load(key, src.clone(), px);
        None
    }

    /// Small RGBA copy (for accent extraction) when the image is loaded.
    pub fn small(&self, src: &ImageSrc, px: u32) -> Option<Arc<SmallRgba>> {
        match &self.map.get(&src.key(px))?.state {
            State::Ready { small, .. } => Some(small.clone()),
            _ => None,
        }
    }

    pub fn on_msg(&mut self, ctx: &egui::Context, msg: ImageMsg) {
        match msg {
            ImageMsg::Loaded { key, image, small } => {
                let bytes = image.width() * image.height() * 4;
                let tex = ctx.load_texture(key.clone(), image, TextureOptions::LINEAR);
                let last_used = self.map.get(&key).map(|e| e.last_used).unwrap_or(self.frame);
                if let Some(Entry { state: State::Ready { bytes: old, .. }, .. }) =
                    self.map.insert(key, Entry { state: State::Ready { tex, small, bytes }, last_used })
                {
                    self.bytes = self.bytes.saturating_sub(old);
                }
                self.bytes += bytes;
            }
            ImageMsg::Failed { key, err } => {
                tracing::debug!(%key, "image failed: {err}");
                if let Some(e) = self.map.get_mut(&key) {
                    e.state = State::Failed(Instant::now());
                }
            }
        }
    }

    /// Drop every texture (e.g. after clearing the disk cache).
    pub fn clear(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }

    fn start_load(&self, key: String, src: ImageSrc, px: u32) {
        let svc = self.svc.clone();
        let sem = self.sem.clone();
        self.svc.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            let bytes: Result<Vec<u8>, String> = match &src {
                ImageSrc::Url(_) => fetch_cached(&svc.http, &key).await,
                ImageSrc::Local(p) => {
                    let p = p.clone();
                    tokio::task::spawn_blocking(move || local_artwork(&p).ok_or_else(|| "no artwork".to_string()))
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()))
                }
            };
            let msg = match bytes {
                Ok(bytes) => {
                    let k = key.clone();
                    match tokio::task::spawn_blocking(move || decode(&bytes, px)).await {
                        Ok(Ok((image, small))) => ImageMsg::Loaded { key: k, image, small: Arc::new(small) },
                        Ok(Err(e)) => ImageMsg::Failed { key: k, err: e },
                        Err(e) => ImageMsg::Failed { key: k, err: e.to_string() },
                    }
                }
                Err(err) => ImageMsg::Failed { key, err },
            };
            svc.bus.send(AppMsg::Image(msg));
        });
    }
}

/// GET with the on-disk art cache in front.
async fn fetch_cached(http: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let path = mochy_storage::art_cache_file(url);
    if let Ok(b) = tokio::fs::read(&path).await
        && !b.is_empty()
    {
        return Ok(b);
    }
    let resp = http.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();
    if let Some(dir) = path.parent() {
        let _ = tokio::fs::create_dir_all(dir).await;
    }
    if let Err(e) = tokio::fs::write(&path, &bytes).await {
        tracing::debug!("art cache write failed: {e}");
    }
    Ok(bytes)
}

fn decode(bytes: &[u8], px: u32) -> Result<(ColorImage, SmallRgba), String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let img = if img.width() > px || img.height() > px { img.thumbnail(px, px) } else { img };
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color = ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    let small = image::imageops::thumbnail(&rgba, 48, 48);
    let small = SmallRgba { w: small.width(), h: small.height(), rgba: small.into_raw() };
    Ok((color, small))
}

/// Embedded artwork bytes of a local audio file (or a sidecar cover image).
pub fn local_artwork(path: &Path) -> Option<Vec<u8>> {
    use lofty::prelude::*;
    if let Ok(tagged) = lofty::read_from_path(path) {
        let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
        if let Some(pic) = tag.and_then(|t| t.pictures().first()) {
            return Some(pic.data().to_vec());
        }
    }
    let dir = path.parent()?;
    for name in ["cover.jpg", "cover.png", "folder.jpg", "folder.png", "front.jpg", "Cover.jpg", "Folder.jpg"] {
        if let Ok(b) = std::fs::read(dir.join(name)) {
            return Some(b);
        }
    }
    None
}

/// Rewrite a YouTube Music thumbnail URL to request roughly `px`×`px`.
///
/// - `lh3.googleusercontent.com/...=w60-h60-l90-rj` → `=w{px}-h{px}-l90-rj`
/// - `yt3.ggpht.com/...=s88-c-k-...` → `=s{px}-c-k-...`
/// - googleusercontent URL without size suffix → `=w{px}-h{px}-l90-rj` appended
/// - unsigned `i.ytimg.com/vi/<id>/default|mqdefault.jpg` → `hqdefault.jpg` (for px > 180)
/// - anything else unchanged.
pub fn upgrade_thumbnail(url: &str, px: u32) -> String {
    let google = url.contains("googleusercontent.com") || url.contains("ggpht.com");
    if google {
        let slash = url.rfind('/').unwrap_or(0);
        let (base, suffix) = match url[slash..].find('=') {
            Some(eq) => (&url[..slash + eq], Some(&url[slash + eq + 1..])),
            None => (url, None),
        };
        let Some(suffix) = suffix else {
            return format!("{base}=w{px}-h{px}-l90-rj");
        };
        let mut parts: Vec<String> = suffix.split('-').map(str::to_string).collect();
        let is_num = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
        let mut changed = false;
        for p in parts.iter_mut() {
            if let Some(rest) = p.strip_prefix('w')
                && is_num(rest)
            {
                *p = format!("w{px}");
                changed = true;
            } else if let Some(rest) = p.strip_prefix('h')
                && is_num(rest)
            {
                *p = format!("h{px}");
                changed = true;
            } else if let Some(rest) = p.strip_prefix('s')
                && is_num(rest)
            {
                *p = format!("s{px}");
                changed = true;
            }
        }
        if !changed {
            return format!("{base}=w{px}-h{px}-l90-rj");
        }
        return format!("{base}={}", parts.join("-"));
    }
    if url.contains("i.ytimg.com/vi") && !url.contains('?') && px > 180 {
        for small in ["/default.jpg", "/mqdefault.jpg"] {
            if let Some(stripped) = url.strip_suffix(small) {
                return format!("{stripped}/hqdefault.jpg");
            }
        }
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::upgrade_thumbnail as up;

    #[test]
    fn lh3_width_height_suffix() {
        assert_eq!(
            up("https://lh3.googleusercontent.com/abcDEF=w60-h60-l90-rj", 544),
            "https://lh3.googleusercontent.com/abcDEF=w544-h544-l90-rj"
        );
        assert_eq!(
            up("https://lh3.googleusercontent.com/abc=w120-h120-s-l90-rj", 226),
            "https://lh3.googleusercontent.com/abc=w226-h226-s-l90-rj"
        );
    }

    #[test]
    fn ggpht_s_suffix() {
        assert_eq!(
            up("https://yt3.ggpht.com/xyz=s88-c-k-c0x00ffffff-no-rj", 544),
            "https://yt3.ggpht.com/xyz=s544-c-k-c0x00ffffff-no-rj"
        );
    }

    #[test]
    fn google_without_suffix_gets_one() {
        assert_eq!(
            up("https://lh3.googleusercontent.com/abc", 300),
            "https://lh3.googleusercontent.com/abc=w300-h300-l90-rj"
        );
        // unknown suffix is replaced
        assert_eq!(
            up("https://lh3.googleusercontent.com/abc=rj", 300),
            "https://lh3.googleusercontent.com/abc=w300-h300-l90-rj"
        );
    }

    #[test]
    fn ytimg_small_variants_upgrade() {
        assert_eq!(up("https://i.ytimg.com/vi/ID123/default.jpg", 544), "https://i.ytimg.com/vi/ID123/hqdefault.jpg");
        assert_eq!(up("https://i.ytimg.com/vi/ID123/mqdefault.jpg", 226), "https://i.ytimg.com/vi/ID123/hqdefault.jpg");
        // signed urls and small sizes are left alone
        let signed = "https://i.ytimg.com/vi/ID123/mqdefault.jpg?sqp=abc&rs=def";
        assert_eq!(up(signed, 544), signed);
        assert_eq!(up("https://i.ytimg.com/vi/ID123/default.jpg", 120), "https://i.ytimg.com/vi/ID123/default.jpg");
    }

    #[test]
    fn other_urls_unchanged() {
        assert_eq!(up("https://example.com/a.png", 544), "https://example.com/a.png");
        assert_eq!(up("https://i.ytimg.com/vi/ID/sddefault.jpg", 544), "https://i.ytimg.com/vi/ID/sddefault.jpg");
    }
}
