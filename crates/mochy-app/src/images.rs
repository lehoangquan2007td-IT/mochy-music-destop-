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
pub const THUMB_PX: u32 = 288;
/// Edge length of now-playing artwork.
pub const LARGE_PX: u32 = 800;

/// Artwork kept on the GPU (the driver also keeps a copy in RAM); textures
/// not drawn lately are dropped past this and reload from the disk cache.
const MAX_TEXTURE_BYTES: usize = 64 * 1024 * 1024;
const RETRY_FAILED_AFTER: Duration = Duration::from_secs(45);

/// Texture options with mipmaps: thumbnails shrink smoothly at any size
/// (36 px queue art from the same texture as 168 px cards) instead of
/// shimmering.
pub const TEX_OPTS: TextureOptions =
    TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));

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
        fetch_plan(self, px).0
    }

    /// A `file://` link (a picture the user picked, e.g. a playlist cover)
    /// is read from disk like [`ImageSrc::Local`].
    fn resolved(&self) -> std::borrow::Cow<'_, ImageSrc> {
        match self {
            ImageSrc::Url(u) if u.starts_with("file://") => match crate::songedit::file_url_path(u) {
                Some(p) => std::borrow::Cow::Owned(ImageSrc::Local(p)),
                None => std::borrow::Cow::Borrowed(self),
            },
            _ => std::borrow::Cow::Borrowed(self),
        }
    }
}

/// Image source for a song's artwork.
pub fn song_src(song: &Song) -> Option<ImageSrc> {
    if let Some(u) = song.thumbnail_url.as_deref().filter(|u| !u.is_empty()) {
        // A picture picked in "Edit song" (songedit) is a file:// cover.
        if let Some(p) = crate::songedit::file_url_path(u) {
            return Some(ImageSrc::Local(p));
        }
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
        self.start_load(src.clone(), px);
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
                // RGBA plus a third more for the mipmap chain.
                let bytes = image.width() * image.height() * 4 * 4 / 3;
                let tex = ctx.load_texture(key.clone(), image, TEX_OPTS);
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

    fn start_load(&self, src: ImageSrc, px: u32) {
        let svc = self.svc.clone();
        let sem = self.sem.clone();
        self.svc.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            let (key, urls, bars) = fetch_plan(&src, px);
            let src = src.resolved().into_owned();
            let bytes: Result<Vec<u8>, String> = match &src {
                ImageSrc::Url(_) => fetch_candidates(&svc.http, &urls).await,
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
                    match tokio::task::spawn_blocking(move || decode(&bytes, px, bars)).await {
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

/// GET with the on-disk art cache in front, trying each candidate url in
/// order; the first non-empty hit wins.
async fn fetch_candidates(http: &reqwest::Client, urls: &[String]) -> Result<Vec<u8>, String> {
    let mut err = "no source".to_string();
    for url in urls {
        match fetch_cached(http, url).await {
            Ok(b) if !b.is_empty() => return Ok(b),
            Ok(_) => err = "empty response".to_string(),
            Err(e) => err = e,
        }
    }
    Err(err)
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

fn decode(bytes: &[u8], px: u32, bars: bool) -> Result<(ColorImage, SmallRgba), String> {
    let mut img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    if bars {
        img = crop_black_bars(img);
    }
    let img = if img.width() > px || img.height() > px { img.thumbnail(px, px) } else { img };
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color = ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    let small = image::imageops::thumbnail(&rgba, 48, 48);
    let small = SmallRgba { w: small.width(), h: small.height(), rgba: small.into_raw() };
    Ok((color, small))
}

/// Crop `hqdefault`-style letterbox bars (near-black rows at the top and
/// bottom of a 4:3 image) down to the central 16:9 area. Images that are not
/// 4:3, or whose edge rows are not dark, pass through untouched.
fn crop_black_bars(img: image::DynamicImage) -> image::DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w < 16 || h < 16 || w * 3 != h * 4 {
        return img;
    }
    let rgba = img.to_rgba8();
    let row_dark = |y: u32| {
        let mut sum = 0u32;
        let mut n = 0u32;
        for x in (0..w).step_by(4) {
            let p = rgba.get_pixel(x, y);
            sum += (p.0[0] as u32 + p.0[1] as u32 + p.0[2] as u32) / 3;
            n += 1;
        }
        sum / n.max(1) < 16
    };
    let dark_top = (0..4).all(row_dark);
    let dark_bottom = (h - 4..h).all(row_dark);
    if !(dark_top && dark_bottom) {
        return img;
    }
    let target_h = (w * 9 / 16).min(h);
    let y0 = (h - target_h) / 2;
    let cropped = image::imageops::crop_imm(&rgba, 0, y0, w, target_h);
    image::DynamicImage::ImageRgba8(cropped.to_image())
}

/// Embedded artwork bytes of a local audio file (or a sidecar cover image).
pub fn local_artwork(path: &Path) -> Option<Vec<u8>> {
    use lofty::prelude::*;
    // An image file itself (edited song covers).
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "bmp" | "gif") {
        return std::fs::read(path).ok();
    }
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

/// Rewrite a googleusercontent-style artwork URL to request roughly `px`×`px`.
///
/// - `lh3.googleusercontent.com/...=w60-h60-l90-rj` → `=w{px}-h{px}-l90-rj`
/// - `yt3.ggpht.com/...=s88-c-k-...` → `=s{px}-c-k-...`
/// - googleusercontent URL without size suffix → `=w{px}-h{px}-l90-rj` appended
/// - anything else unchanged.
///
/// YouTube video thumbnails (`i.ytimg.com/vi/…`) are not resized here — see
/// [`fetch_plan`], which picks among their fixed sizes with fallbacks.
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
    url.to_string()
}

/// Resolve `src` at `px` into a cache key plus candidate urls to try in order
/// (the first hit wins), and whether the source may carry letterbox bars.
///
/// YouTube video thumbnails exist at fixed sizes — `maxresdefault` (1280×720),
/// `hq720` (720×405), `hqdefault` (480×360, black bars for 16:9 videos) and
/// `mqdefault` (320×180). Big artwork asks for `maxresdefault` and falls back
/// down the list; smaller ones use the size that always exists. Signed query
/// urls (`?sqp=…`) are re-requested clean, which also upgrades a 400×225
/// search-result thumb to full resolution for the large art box.
pub fn fetch_plan(src: &ImageSrc, px: u32) -> (String, Vec<String>, bool) {
    let src = &*src.resolved();
    let ImageSrc::Url(u) = src else {
        let ImageSrc::Local(p) = src else { unreachable!("ImageSrc has two variants") };
        return (format!("local:{px}:{}", p.display()), Vec::new(), false);
    };
    if u.contains("i.ytimg.com/vi") {
        let end = u.find(['?', '#']).unwrap_or(u.len());
        let path = &u[..end];
        let base = match path.rfind('/') {
            Some(slash) if path[slash..].contains('.') => &path[..slash],
            _ => path,
        };
        if px > 400 {
            let maxres = format!("{base}/maxresdefault.jpg");
            let urls = vec![maxres.clone(), format!("{base}/hq720.jpg"), format!("{base}/hqdefault.jpg")];
            (maxres, urls, true)
        } else if px > 180 {
            let hq = format!("{base}/hqdefault.jpg");
            (hq.clone(), vec![hq], true)
        } else {
            let mq = format!("{base}/mqdefault.jpg");
            (mq.clone(), vec![mq], true)
        }
    } else {
        let up = upgrade_thumbnail(u, px);
        let key = if up == *u { format!("{px}:{u}") } else { up.clone() };
        (key, vec![up], false)
    }
}

#[cfg(test)]
mod tests {
    use super::{ImageSrc, crop_black_bars, fetch_plan, upgrade_thumbnail};

    fn plan(url: &str, px: u32) -> (String, Vec<String>, bool) {
        fetch_plan(&ImageSrc::Url(url.to_string()), px)
    }

    #[test]
    fn file_url_is_read_from_disk() {
        let (key, urls, bars) = plan("file:///C:/data/covers/playlist-LPx-1.jpg", 544);
        assert_eq!(key, fetch_plan(&ImageSrc::Local("C:/data/covers/playlist-LPx-1.jpg".into()), 544).0);
        assert!(key.starts_with("local:544:"), "{key}");
        assert!(urls.is_empty() && !bars);
    }

    #[test]
    fn lh3_width_height_suffix() {
        assert_eq!(
            upgrade_thumbnail("https://lh3.googleusercontent.com/abcDEF=w60-h60-l90-rj", 544),
            "https://lh3.googleusercontent.com/abcDEF=w544-h544-l90-rj"
        );
        assert_eq!(
            upgrade_thumbnail("https://lh3.googleusercontent.com/abc=w120-h120-s-l90-rj", 226),
            "https://lh3.googleusercontent.com/abc=w226-h226-s-l90-rj"
        );
    }

    #[test]
    fn ggpht_s_suffix() {
        assert_eq!(
            upgrade_thumbnail("https://yt3.ggpht.com/xyz=s88-c-k-c0x00ffffff-no-rj", 544),
            "https://yt3.ggpht.com/xyz=s544-c-k-c0x00ffffff-no-rj"
        );
    }

    #[test]
    fn google_without_suffix_gets_one() {
        assert_eq!(
            upgrade_thumbnail("https://lh3.googleusercontent.com/abc", 300),
            "https://lh3.googleusercontent.com/abc=w300-h300-l90-rj"
        );
        // unknown suffix is replaced
        assert_eq!(
            upgrade_thumbnail("https://lh3.googleusercontent.com/abc=rj", 300),
            "https://lh3.googleusercontent.com/abc=w300-h300-l90-rj"
        );
    }

    #[test]
    fn ytimg_large_asks_maxres_with_fallback() {
        let (key, urls, bars) = plan("https://i.ytimg.com/vi/ID123/default.jpg", 800);
        assert_eq!(key, "https://i.ytimg.com/vi/ID123/maxresdefault.jpg");
        assert_eq!(
            urls,
            vec![
                "https://i.ytimg.com/vi/ID123/maxresdefault.jpg".to_string(),
                "https://i.ytimg.com/vi/ID123/hq720.jpg".to_string(),
                "https://i.ytimg.com/vi/ID123/hqdefault.jpg".to_string(),
            ]
        );
        assert!(bars);
    }

    #[test]
    fn ytimg_signed_large_uses_video_id() {
        let (key, urls, _) = plan("https://i.ytimg.com/vi/ID9/hqdefault.jpg?sqp=abc&rs=def", 544);
        assert_eq!(key, "https://i.ytimg.com/vi/ID9/maxresdefault.jpg");
        assert_eq!(urls.len(), 3);
        assert!(urls.iter().all(|u| !u.contains('?')));
    }

    #[test]
    fn ytimg_thumb_uses_hqdefault() {
        let (key, urls, bars) = plan("https://i.ytimg.com/vi/ID/mqdefault.jpg?sqp=x", 288);
        assert_eq!(key, "https://i.ytimg.com/vi/ID/hqdefault.jpg");
        assert_eq!(urls, vec![key.clone()]);
        assert!(bars);
    }

    #[test]
    fn ytimg_small_uses_mqdefault() {
        let (key, urls, _) = plan("https://i.ytimg.com/vi/ID/hqdefault.jpg", 120);
        assert_eq!(key, "https://i.ytimg.com/vi/ID/mqdefault.jpg");
        assert_eq!(urls.len(), 1);
    }

    #[test]
    fn other_urls_keyed_per_size() {
        let (key, urls, bars) = plan("https://example.com/a.png", 288);
        assert_eq!(key, "288:https://example.com/a.png");
        assert_eq!(urls, vec!["https://example.com/a.png".to_string()]);
        assert!(!bars);
    }

    fn flat(w: u32, h: u32, rgb: [u8; 3]) -> image::DynamicImage {
        image::DynamicImage::ImageRgba8(image::ImageBuffer::from_pixel(w, h, image::Rgba([rgb[0], rgb[1], rgb[2], 255])))
    }

    #[test]
    fn black_bars_cropped_from_4by3() {
        let mut img = flat(480, 360, [200, 180, 160]).into_rgba8();
        for y in 0..45 {
            for x in 0..480 {
                img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
                img.put_pixel(x, 359 - y, image::Rgba([0, 0, 0, 255]));
            }
        }
        let out = crop_black_bars(image::DynamicImage::ImageRgba8(img));
        assert_eq!((out.width(), out.height()), (480, 270));
    }

    #[test]
    fn bright_4by3_not_cropped() {
        let out = crop_black_bars(flat(480, 360, [220, 220, 220]));
        assert_eq!((out.width(), out.height()), (480, 360));
    }

    #[test]
    fn sixteen_by_nine_not_cropped() {
        // 16:9 maxres-style image, even with bars, is left alone
        let mut img = flat(640, 360, [180, 180, 180]).into_rgba8();
        for y in 0..30 {
            for x in 0..640 {
                img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        let out = crop_black_bars(image::DynamicImage::ImageRgba8(img));
        assert_eq!((out.width(), out.height()), (640, 360));
    }
}
