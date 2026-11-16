//! Lyrics loading for the current track: lyrics embedded in a local file (tags / sidecar
//! `.lrc`, Android reads these first, offline too) → DB cache → providers → DB cache.
//! Every call into `mochy_lyrics` runs in its own spawned task so a panic
//! there (e.g. an unimplemented provider) becomes `LyricsState::Error`
//! instead of taking the app down.

use std::future::Future;

use mochy_core::models::{Song, TrackMeta};
use mochy_core::settings::LyricsSettings;
use mochy_lyrics::{FetchConfig, Lyrics, ProviderId, SyncKind};

use crate::services::{AppMsg, Services};
use crate::ui::lyrics_view::LyricsState;

/// Map a stored id back to a provider (local mapping; does not rely on
/// `ProviderId::from_id`).
pub fn provider_from_id(id: &str) -> Option<ProviderId> {
    ProviderId::all().iter().copied().find(|p| p.id().eq_ignore_ascii_case(id))
}

/// Effective provider order (enabled + implemented), defaulting to all.
pub fn provider_order(s: &LyricsSettings) -> Vec<ProviderId> {
    let order: Vec<ProviderId> = s
        .provider_order
        .iter()
        .filter_map(|id| provider_from_id(id))
        .filter(|p| p.implemented())
        .collect();
    if order.is_empty() { FetchConfig::default().order } else { order }
}

async fn guarded<T: Send + 'static>(fut: impl Future<Output = T> + Send + 'static) -> Result<T, String> {
    tokio::spawn(fut).await.map_err(|e| {
        if e.is_panic() { "lyrics backend panicked (not implemented yet?)".to_string() } else { e.to_string() }
    })
}

/// Resolve lyrics for `song` (cache first).
pub async fn load(svc: Services, song: Song, order: Vec<ProviderId>) -> LyricsState {
    let key = song.video_id.clone().unwrap_or_else(|| song.id.clone());

    // 0. The local file's own lyrics (Android `LyricsHelper.getLyrics` → `readEmbeddedLyrics`).
    if let Some(path) = song.local_path.clone() {
        let embedded = tokio::task::spawn_blocking(move || mochy_lyrics::embedded::read_file_lyrics(&path)).await.ok().flatten();
        if let Some(lyrics) = embedded.and_then(|raw| Lyrics::from_lrc(mochy_lyrics::embedded::PROVIDER, &raw)) {
            return LyricsState::Loaded(lyrics);
        }
    }

    // 1. DB cache.
    let k = key.clone();
    let cached = svc.db.query(move |db| db.load_lyrics(&k).ok().flatten()).await.ok().flatten();
    if let Some((provider, text)) = cached.filter(|(_, t)| !t.trim().is_empty()) {
        match guarded(async move { mochy_lyrics::parse_lrc(&text) }).await {
            Ok((lines, sync)) if !lines.is_empty() => {
                return LyricsState::Loaded(Lyrics { provider, sync, lines, plain: None });
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("cached lyrics parse failed: {e}"),
        }
    }

    // 2. Providers.
    let meta = TrackMeta::from(&song);
    let http = svc.http.clone();
    // The app's InnerTube client for the YouTube sources (locale + visitor data); not worth
    // waiting long for, the crate falls back to a client of its own.
    let ytm = if song.video_id.is_some() {
        tokio::time::timeout(std::time::Duration::from_secs(5), svc.ytm()).await.ok().and_then(Result::ok).map(|c| (*c).clone())
    } else {
        None
    };
    let cfg = FetchConfig { order, ytm, ..FetchConfig::default() };
    let fetched = guarded(async move { mochy_lyrics::fetch(&meta, &cfg, &http).await }).await;
    match fetched {
        Ok(Some(lyrics)) => {
            // 3. Store (best effort; serialisation runs guarded too).
            let lines = lyrics.lines.clone();
            let provider = lyrics.provider.clone();
            let plain = lyrics.plain.clone();
            let is_plain = lyrics.sync == SyncKind::None || lines.is_empty();
            let db = svc.db.clone();
            tokio::spawn(async move {
                let text = if is_plain {
                    Ok(plain.unwrap_or_default())
                } else {
                    guarded(async move { mochy_lyrics::lines_to_lrc(&lines) }).await
                };
                match text {
                    Ok(t) if !t.trim().is_empty() => db.run(move |db| {
                        crate::db::log_err("save_lyrics", db.save_lyrics(&key, &provider, &t));
                    }),
                    Ok(_) => {}
                    Err(e) => tracing::warn!("lyrics serialise failed: {e}"),
                }
            });
            LyricsState::Loaded(lyrics)
        }
        Ok(None) => LyricsState::NotFound,
        Err(e) => LyricsState::Error(e),
    }
}

/// Spawn [`load`] and post the result as `AppMsg::Lyrics`.
pub fn spawn_load(svc: &Services, song: Song, order: Vec<ProviderId>) {
    let s = svc.clone();
    svc.spawn(async move {
        let id = song.id.clone();
        let title = song.title.clone();
        let state = load(s.clone(), song, order).await;
        match &state {
            LyricsState::Loaded(l) => tracing::info!(%title, provider = %l.provider, lines = l.lines.len(), "lyrics loaded"),
            LyricsState::NotFound => tracing::info!(%title, "no lyrics found"),
            LyricsState::Error(e) => tracing::warn!(%title, "lyrics error: {e}"),
            _ => {}
        }
        s.bus.send(AppMsg::Lyrics { song_id: id, state });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_ids_roundtrip_locally() {
        for p in ProviderId::all() {
            assert_eq!(provider_from_id(p.id()), Some(*p));
        }
        assert_eq!(provider_from_id("nope"), None);
    }

    #[test]
    fn order_filters_unimplemented_and_defaults() {
        let s = LyricsSettings { provider_order: vec!["LRCLIB".into(), "PAXSENIX".into(), "YOUTUBEMUSIC".into()], ..Default::default() };
        assert_eq!(provider_order(&s), vec![ProviderId::LrcLib, ProviderId::YouTubeMusic]);
        let empty = LyricsSettings::default();
        assert!(!provider_order(&empty).is_empty());
        assert!(provider_order(&empty).iter().all(|p| p.implemented()));
    }
}
