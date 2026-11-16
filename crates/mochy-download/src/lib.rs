//! mochy-download — the file side of downloads (port of Android
//! `DownloadExporter` / `ExportTags` / `OggOpusRemux` / `songedit/LyricsFile*`).
//!
//! Pure, UI-free building blocks; the queue/manager lives in the app:
//! - [`fetch`]: ranged HTTPS download of a stream URL to a file, with
//!   progress, retries and cancellation.
//! - [`webm`] + [`opus`] + [`ogg`]: YouTube's WebM-Opus moved into a plain
//!   `.opus` (Ogg Opus) file without re-encoding, with the song's tags in its
//!   comment header.
//! - [`tags`]: what gets written into a file (title / artist / album / album
//!   artist / year / lyrics / front cover) — vorbis comments for Opus, lofty
//!   for MP4 and everything else.
//! - [`naming`]: file-name sanitizing and the filename template.
//! - [`sniff`]: container detection from the first bytes.
//! - [`cover`]: artwork → square ≤1200 px JPEG.
//! - [`lyrics_file`]: a picked lyrics file (.lrc/.ttml/.srt/.txt/…) → the
//!   text the `lyrics` table keeps.

pub mod cover;
pub mod fetch;
pub mod lyrics_file;
pub mod naming;
pub mod ogg;
pub mod opus;
pub mod sniff;
pub mod tags;
pub mod webm;

pub use naming::{NameFields, render_template, safe_name, unique_path};
pub use sniff::Container;
pub use tags::{Cover, ExportTags};
