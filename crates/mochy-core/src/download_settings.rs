//! Downloads / offline / stream-cache settings (`AppSettings::downloads`).
//! Android keys: `downloadQuality`, `autoDownloadPlaylists` (liked part),
//! `maxSongCacheSize`; the rest are desktop additions.

use serde::{Deserialize, Serialize};

use crate::settings::AudioQuality;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadSettings {
    /// Target folder; empty = `%USERPROFILE%\Music\Mochy`.
    pub folder: String,
    /// File name template, `/` for sub-folders. Tokens: `{title}`,
    /// `{artist}`, `{album}`, `{album_artist}`, `{year}`, `{id}`.
    pub filename_template: String,
    pub quality: AudioQuality,
    /// Ask for Opus (saved as `.opus`) before AAC (`.m4a`).
    pub prefer_opus: bool,
    /// Downloads running at once (Android `maxParallelDownloads = 3`).
    pub max_parallel: u8,
    pub embed_cover: bool,
    pub embed_lyrics: bool,
    /// Also write the lyrics next to the song as `.lrc` / `.txt`.
    pub lyrics_file: bool,
    /// Play a downloaded song from its file instead of streaming it.
    pub play_offline: bool,
    /// Download songs as they get liked (Android `AutoDownloads` for the
    /// liked list).
    pub auto_download_liked: bool,
    /// Liked-date mark (epoch ms) from which liked songs count: songs liked
    /// before auto-download was switched on aren't fetched.
    pub auto_download_liked_since: Option<i64>,
    /// Stream cache size in MB (Android `maxSongCacheSize`): 0 = off,
    /// -1 = unlimited. Played songs are fetched a second time in the
    /// background so they play again without a connection.
    pub cache_max_mb: i32,
}

impl Default for DownloadSettings {
    fn default() -> Self {
        Self {
            folder: String::new(),
            filename_template: "{artist} - {title}".into(),
            quality: AudioQuality::High,
            prefer_opus: true,
            max_parallel: 3,
            embed_cover: true,
            embed_lyrics: true,
            lyrics_file: false,
            play_offline: true,
            auto_download_liked: false,
            auto_download_liked_since: None,
            cache_max_mb: 0,
        }
    }
}
