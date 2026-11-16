//! Lyrics a local audio file carries itself (Android `EmbeddedLyrics` / `EmbeddedLyricsFile`):
//! ID3v2 in MP3 (USLT, millisecond-timed SYLT, TXXX "LYRICS"), Vorbis comments in FLAC / Opus /
//! Ogg, and the `©lyr` atom in M4A. Only the tags are read, never the audio, and cover art is
//! skipped rather than loaded.
//!
//! Desktop addition: a sidecar `<file>.lrc` next to the song (the usual desktop convention) is
//! also read; synced lyrics win over plain text across both.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

/// What `Lyrics::provider` says for lyrics read from the file (Android `EmbeddedLyrics.Provider`).
pub const PROVIDER: &str = "Embedded";

/// Tags carry cover art too, which can run to megabytes; past this something's wrong.
const MAX_TAG_BYTES: usize = 16 * 1024 * 1024;
const VORBIS_COMMENT_BLOCK: u8 = 4;
const MAX_FLAC_BLOCKS: usize = 64;
const MAX_OGG_PAGES: usize = 1024;
const LYRIC_KEYS: &[&str] = &["LYRICS", "UNSYNCEDLYRICS", "UNSYNCED LYRICS", "SYNCEDLYRICS"];

static LRC_TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\d{1,2}:\d{2}([.:]\d{1,3})?\]").expect("regex"));

/// Random access to a file's bytes.
pub trait Source {
    fn size(&self) -> u64;
    /// Up to `len` bytes from `pos`: fewer at the end, none past it.
    fn read_at(&self, pos: u64, len: usize) -> Vec<u8>;
}

impl Source for [u8] {
    fn size(&self) -> u64 {
        self.len() as u64
    }
    fn read_at(&self, pos: u64, len: usize) -> Vec<u8> {
        if pos >= self.len() as u64 || len == 0 {
            return Vec::new();
        }
        let start = pos as usize;
        self[start..(start + len).min(self.len())].to_vec()
    }
}

impl Source for Vec<u8> {
    fn size(&self) -> u64 {
        self.as_slice().size()
    }
    fn read_at(&self, pos: u64, len: usize) -> Vec<u8> {
        self.as_slice().read_at(pos, len)
    }
}

struct FileSource {
    file: std::cell::RefCell<File>,
    size: u64,
}

impl Source for FileSource {
    fn size(&self) -> u64 {
        self.size
    }
    fn read_at(&self, pos: u64, len: usize) -> Vec<u8> {
        if pos >= self.size || len == 0 {
            return Vec::new();
        }
        let len = (len as u64).min(self.size - pos) as usize;
        let mut f = self.file.borrow_mut();
        if f.seek(SeekFrom::Start(pos)).is_err() {
            return Vec::new();
        }
        let mut buf = vec![0u8; len];
        let mut got = 0;
        while got < len {
            match f.read(&mut buf[got..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => got += n,
            }
        }
        buf.truncate(got);
        buf
    }
}

/// `len` bytes of `base` from `offset`, read as a file of their own.
struct Slice<'a> {
    base: &'a dyn Source,
    offset: u64,
    size: u64,
}

impl Source for Slice<'_> {
    fn size(&self) -> u64 {
        self.size
    }
    fn read_at(&self, pos: u64, len: usize) -> Vec<u8> {
        if pos >= self.size || len == 0 {
            return Vec::new();
        }
        self.base.read_at(self.offset + pos, (len as u64).min(self.size - pos) as usize)
    }
}

/// The lyrics in `source`: synced over plain. `None` when it has none.
pub fn find(source: &dyn Source) -> Option<String> {
    let head = source.read_at(0, 12);
    if head.starts_with(b"ID3") {
        id3(source).or_else(|| {
            // a FLAC can carry an ID3 tag in front of its own
            let end = id3_tag_end(source)?;
            source.read_at(end, 4).starts_with(b"fLaC").then(|| flac(source, end)).flatten()
        })
    } else if head.starts_with(b"fLaC") {
        flac(source, 0)
    } else if head.starts_with(b"OggS") {
        ogg(source)
    } else if head.len() >= 8 && &head[4..8] == b"ftyp" {
        mp4(source)
    } else {
        None
    }
}

/// Lyrics from the tags of the file at `path`; `None` when none or unreadable.
pub fn read_tags(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let src = FileSource { file: std::cell::RefCell::new(file), size };
    find(&src)
}

/// Candidate sidecar files: `song.lrc` and `song.mp3.lrc`.
pub fn sidecar_paths(path: &Path) -> Vec<PathBuf> {
    let mut out = vec![path.with_extension("lrc")];
    let mut appended = path.as_os_str().to_owned();
    appended.push(".lrc");
    out.push(PathBuf::from(appended));
    out.dedup();
    out
}

fn read_sidecar(path: &Path) -> Option<String> {
    for p in sidecar_paths(path) {
        if p == path {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&p)
            && bytes.len() <= MAX_TAG_BYTES
        {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if let Some(t) = pick(vec![text]) {
                return Some(t);
            }
        }
    }
    None
}

/// The lyrics a local file has: its tags and a sidecar `.lrc`, synced over plain.
pub fn read_file_lyrics(path: &Path) -> Option<String> {
    let found: Vec<String> = [read_tags(path), read_sidecar(path)].into_iter().flatten().collect();
    pick(found)
}

// ---- ID3v2 (MP3) ----------------------------------------------------------------------------

fn syncsafe(b: &[u8], at: usize) -> usize {
    ((b[at] as usize & 0x7F) << 21) | ((b[at + 1] as usize & 0x7F) << 14) | ((b[at + 2] as usize & 0x7F) << 7) | (b[at + 3] as usize & 0x7F)
}

fn int32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn id3_tag_end(source: &dyn Source) -> Option<u64> {
    let h = source.read_at(0, 10);
    if h.len() < 10 {
        return None;
    }
    let footer = h[3] == 4 && h[5] & 0x10 != 0;
    Some(10 + syncsafe(&h, 6) as u64 + if footer { 10 } else { 0 })
}

/// ID3's unsynchronisation: every 0xFF 0x00 was written for a 0xFF.
fn de_unsync(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        out.push(bytes[i]);
        if bytes[i] == 0xFF && i + 1 < bytes.len() && bytes[i + 1] == 0 {
            i += 1;
        }
        i += 1;
    }
    out
}

fn id3(source: &dyn Source) -> Option<String> {
    let h = source.read_at(0, 10);
    if h.len() < 10 {
        return None;
    }
    let major = h[3];
    if major != 3 && major != 4 {
        return None;
    }
    let flags = h[5];
    let tag_size = syncsafe(&h, 6);
    if tag_size == 0 {
        return None;
    }
    // A v2.3 tag written unsynchronised has to be undone as a whole before its frames can be
    // found; otherwise frames are walked straight off the file, skipping cover art.
    let owned: Vec<u8>;
    let slice;
    let tag: &dyn Source = if major == 3 && flags & 0x80 != 0 {
        if tag_size > MAX_TAG_BYTES {
            return None;
        }
        owned = de_unsync(&source.read_at(10, tag_size));
        &owned
    } else {
        slice = Slice { base: source, offset: 10, size: (tag_size as u64).min(source.size().saturating_sub(10)) };
        &slice
    };

    let mut pos: u64 = 0;
    if flags & 0x40 != 0 {
        // the extended header: v2.4 counts itself in its size, v2.3 doesn't
        let ext = tag.read_at(0, 4);
        if ext.len() < 4 {
            return None;
        }
        pos = if major == 4 { syncsafe(&ext, 0) as u64 } else { int32(&ext, 0) as u64 + 4 };
    }
    let mut found = Vec::new();
    while pos + 10 <= tag.size() {
        let fh = tag.read_at(pos, 10);
        if fh.len() < 10 || fh[0] == 0 {
            break; // padding
        }
        let id = &fh[0..4];
        let frame_size = if major == 4 { syncsafe(&fh, 4) } else { int32(&fh, 4) as usize };
        let frame_flags = ((fh[8] as u16) << 8) | fh[9] as u16;
        let start = pos + 10;
        if frame_size == 0 || start + frame_size as u64 > tag.size() {
            break;
        }
        pos = start + frame_size as u64;
        let compressed_or_encrypted = if major == 4 { frame_flags & 0x000C != 0 } else { frame_flags & 0x00C0 != 0 };
        let is_lyric = matches!(id, b"USLT" | b"SYLT" | b"TXXX");
        if compressed_or_encrypted || !is_lyric || frame_size > MAX_TAG_BYTES {
            continue;
        }
        let mut skip = 0usize;
        if (major == 4 && frame_flags & 0x0040 != 0) || (major == 3 && frame_flags & 0x0020 != 0) {
            skip += 1; // group id
        }
        if major == 4 && frame_flags & 0x0001 != 0 {
            skip += 4; // data length indicator
        }
        if skip >= frame_size {
            continue;
        }
        let mut body = tag.read_at(start + skip as u64, frame_size - skip);
        if major == 4 && frame_flags & 0x0002 != 0 {
            body = de_unsync(&body);
        }
        let text = match id {
            b"SYLT" => sylt(&body),
            b"USLT" => uslt(&body),
            _ => txxx(&body),
        };
        if let Some(t) = text {
            found.push(t);
        }
    }
    pick(found)
}

/// Just past the string starting at `from`: its terminator is two bytes in UTF-16.
fn terminator_end(b: &[u8], from: usize, encoding: u8) -> usize {
    if encoding == 1 || encoding == 2 {
        let mut i = from;
        while i + 1 < b.len() {
            if b[i] == 0 && b[i + 1] == 0 {
                return i + 2;
            }
            i += 2;
        }
    } else {
        for (i, &x) in b.iter().enumerate().skip(from) {
            if x == 0 {
                return i + 1;
            }
        }
    }
    b.len()
}

fn decode(b: &[u8], from: usize, to: usize, encoding: u8) -> Option<String> {
    if from >= to || to > b.len() {
        return None;
    }
    let bytes = &b[from..to];
    let s = match encoding {
        0 => bytes.iter().map(|&c| c as char).collect(),
        1 | 2 => {
            let (mut be, mut data) = (encoding == 2, bytes);
            if encoding == 1 && data.len() >= 2 {
                match (data[0], data[1]) {
                    (0xFE, 0xFF) => {
                        be = true;
                        data = &data[2..];
                    }
                    (0xFF, 0xFE) => {
                        be = false;
                        data = &data[2..];
                    }
                    _ => be = true, // Java's UTF-16 without a BOM reads big-endian
                }
            }
            let units: Vec<u16> = data
                .chunks_exact(2)
                .map(|c| if be { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) })
                .collect();
            String::from_utf16_lossy(&units)
        }
        3 => String::from_utf8_lossy(bytes).into_owned(),
        _ => return None,
    };
    Some(s.trim_end_matches('\0').to_string())
}

/// Unsynchronised lyrics: encoding, language, a description, then the text.
fn uslt(body: &[u8]) -> Option<String> {
    if body.len() < 5 || body[0] > 3 {
        return None;
    }
    let start = terminator_end(body, 4, body[0]);
    decode(body, start, body.len(), body[0])
}

/// A user text frame; lyrics when its description says so.
fn txxx(body: &[u8]) -> Option<String> {
    if body.len() < 2 || body[0] > 3 {
        return None;
    }
    let enc = body[0];
    let desc_end = terminator_end(body, 1, enc);
    let desc = decode(body, 1, desc_end, enc)?.trim().to_uppercase();
    if !LYRIC_KEYS.contains(&desc.trim_end_matches('\0')) {
        return None;
    }
    decode(body, desc_end, body.len(), enc)
}

/// Millisecond-timed synced lyrics, as LRC. Entries starting with a line break are joined into
/// lines; without any, each entry is a line.
fn sylt(body: &[u8]) -> Option<String> {
    if body.len() < 7 || body[0] > 3 || body[4] != 2 {
        return None;
    }
    let enc = body[0];
    let mut pos = terminator_end(body, 6, enc);
    let mut entries: Vec<(u32, String)> = Vec::new();
    while pos < body.len() {
        let text_end = terminator_end(body, pos, enc);
        if text_end + 4 > body.len() {
            break;
        }
        let text = decode(body, pos, text_end, enc).unwrap_or_default();
        entries.push((int32(body, text_end), text));
        pos = text_end + 4;
    }
    if entries.is_empty() {
        return None;
    }
    let breaks = entries.iter().any(|(_, t)| t.starts_with('\n') || t.starts_with('\r'));
    let mut lines: Vec<(u32, String)> = Vec::new();
    for (time, text) in entries {
        let starts = !breaks || lines.is_empty() || text.starts_with('\n') || text.starts_with('\r');
        if starts {
            lines.push((time, text.trim().to_string()));
        } else if let Some(last) = lines.last_mut() {
            last.1.push_str(&text);
        }
    }
    let out = lines
        .into_iter()
        .filter(|(_, t)| !t.trim().is_empty())
        .map(|(ms, t)| format!("{}{}", lrc_time(ms), t.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    (!out.trim().is_empty()).then_some(out)
}

fn lrc_time(ms: u32) -> String {
    format!("[{:02}:{:02}.{:02}]", ms / 60_000, ms / 1000 % 60, ms % 1000 / 10)
}

// ---- FLAC -------------------------------------------------------------------------------------

fn flac(source: &dyn Source, start: u64) -> Option<String> {
    let mut pos = start + 4;
    for _ in 0..MAX_FLAC_BLOCKS {
        let h = source.read_at(pos, 4);
        if h.len() < 4 {
            return None;
        }
        let last = h[0] & 0x80 != 0;
        let kind = h[0] & 0x7F;
        let len = ((h[1] as usize) << 16) | ((h[2] as usize) << 8) | h[3] as usize;
        if kind == VORBIS_COMMENT_BLOCK {
            return if (1..=MAX_TAG_BYTES).contains(&len) { vorbis_comments(&source.read_at(pos + 4, len)) } else { None };
        }
        if last {
            return None;
        }
        pos += 4 + len as u64;
    }
    None
}

fn vorbis_comments(block: &[u8]) -> Option<String> {
    let mut pos = 0usize;
    if pos + 4 > block.len() {
        return None;
    }
    pos += 4 + le32(block, pos) as usize; // vendor string
    if pos + 4 > block.len() {
        return None;
    }
    let count = le32(block, pos);
    pos += 4;
    let mut found = Vec::new();
    for _ in 0..count {
        if pos + 4 > block.len() {
            break;
        }
        let len = le32(block, pos) as usize;
        pos += 4;
        if pos + len > block.len() {
            break;
        }
        let comment = String::from_utf8_lossy(&block[pos..pos + len]).into_owned();
        pos += len;
        if let Some(eq) = comment.find('=')
            && eq > 0
            && LYRIC_KEYS.contains(&comment[..eq].trim().to_uppercase().as_str())
        {
            found.push(comment[eq + 1..].to_string());
        }
    }
    pick(found)
}

// ---- Ogg (Opus, Vorbis) -----------------------------------------------------------------------

fn ogg(source: &dyn Source) -> Option<String> {
    let packet = ogg_packet(source, 1)?;
    let start = if packet.starts_with(b"OpusTags") {
        8
    } else if packet.len() > 7 && packet[0] == 3 && &packet[1..7] == b"vorbis" {
        7
    } else {
        return None;
    };
    vorbis_comments(&packet[start..])
}

/// One whole packet, read page by page (a comment packet with a cover runs across many pages).
fn ogg_packet(source: &dyn Source, index: usize) -> Option<Vec<u8>> {
    let mut pos = 0u64;
    let mut packet_index = 0usize;
    let mut packet = Vec::new();
    for _ in 0..MAX_OGG_PAGES {
        let h = source.read_at(pos, 27);
        if h.len() < 27 || !h.starts_with(b"OggS") {
            return None;
        }
        let segments = h[26] as usize;
        let lacing = source.read_at(pos + 27, segments);
        if lacing.len() < segments {
            return None;
        }
        let mut body = pos + 27 + segments as u64;
        for &v in &lacing {
            let len = v as usize;
            if packet_index == index {
                if packet.len() + len > MAX_TAG_BYTES {
                    return None;
                }
                packet.extend(source.read_at(body, len));
            }
            body += len as u64;
            // a segment shorter than 255 bytes closes its packet
            if len < 255 {
                if packet_index == index {
                    return Some(packet);
                }
                packet_index += 1;
            }
        }
        pos = body;
    }
    None
}

// ---- MP4 / M4A --------------------------------------------------------------------------------

fn mp4(source: &dyn Source) -> Option<String> {
    let moov = atom(source, 0, source.size(), b"moov")?;
    let udta = atom(source, moov.0, moov.1, b"udta")?;
    let meta = atom(source, udta.0, udta.1, b"meta")?;
    // meta is a full box (version and flags before its children), except in QuickTime's files
    let ilst = atom(source, meta.0 + 4, meta.1, b"ilst").or_else(|| atom(source, meta.0, meta.1, b"ilst"))?;
    let lyr = atom(source, ilst.0, ilst.1, b"\xA9lyr")?;
    let data = atom(source, lyr.0, lyr.1, b"data")?;
    // data: a type and a locale, then the text
    let len = data.1.checked_sub(data.0 + 8)? as usize;
    if len == 0 || len > MAX_TAG_BYTES {
        return None;
    }
    pick(vec![String::from_utf8_lossy(&source.read_at(data.0 + 8, len)).into_owned()])
}

/// The body of the first `kind` atom between `from` and `to`: (start, end).
fn atom(source: &dyn Source, from: u64, to: u64, kind: &[u8; 4]) -> Option<(u64, u64)> {
    let mut pos = from;
    while pos + 8 <= to {
        let h = source.read_at(pos, 16);
        if h.len() < 8 {
            return None;
        }
        let mut size = int32(&h, 0) as u64;
        let mut header = 8u64;
        match size {
            1 => {
                if h.len() < 16 {
                    return None;
                }
                size = ((int32(&h, 8) as u64) << 32) | int32(&h, 12) as u64;
                header = 16;
            }
            0 => size = to - pos,
            _ => {}
        }
        if size < header {
            return None;
        }
        if &h[4..8] == kind {
            return Some((pos + header, (pos + size).min(to)));
        }
        pos += size;
    }
    None
}

// ---- shared -----------------------------------------------------------------------------------

/// Synced over plain; nothing that is only whitespace.
fn pick(found: Vec<String>) -> Option<String> {
    let texts: Vec<String> = found
        .into_iter()
        .map(|t| t.replace("\r\n", "\n").replace('\r', "\n").trim_matches(['\0', ' ', '\n']).to_string())
        .filter(|t| !t.trim().is_empty())
        .collect();
    texts
        .iter()
        .find(|t| LRC_TIMESTAMP.is_match(t))
        .or(texts.first())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syncsafe_bytes(n: usize) -> [u8; 4] {
        [((n >> 21) & 0x7F) as u8, ((n >> 14) & 0x7F) as u8, ((n >> 7) & 0x7F) as u8, (n & 0x7F) as u8]
    }

    fn id3v24(frames: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut body = Vec::new();
        for (id, data) in frames {
            body.extend_from_slice(*id);
            body.extend_from_slice(&syncsafe_bytes(data.len()));
            body.extend_from_slice(&[0, 0]);
            body.extend_from_slice(data);
        }
        body.extend_from_slice(&[0; 16]); // padding
        let mut out = b"ID3\x04\x00\x00".to_vec();
        out.extend_from_slice(&syncsafe_bytes(body.len()));
        out.extend(body);
        out.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]); // audio
        out
    }

    #[test]
    fn id3_uslt_and_sylt_prefers_synced() {
        let mut uslt = vec![3u8];
        uslt.extend_from_slice(b"eng");
        uslt.extend_from_slice(b"desc\0");
        uslt.extend_from_slice("Plain line one\nPlain line two".as_bytes());

        let mut sylt = vec![3u8];
        sylt.extend_from_slice(b"eng");
        sylt.push(2); // ms timestamps
        sylt.push(1); // content type: lyrics
        sylt.extend_from_slice(b"\0"); // empty descriptor
        for (t, ms) in [("Hello", 1_000u32), ("\nWorld", 61_500)] {
            sylt.extend_from_slice(t.as_bytes());
            sylt.push(0);
            sylt.extend_from_slice(&ms.to_be_bytes());
        }
        let file = id3v24(&[(b"USLT", uslt.clone()), (b"SYLT", sylt)]);
        assert_eq!(find(&file).as_deref(), Some("[00:01.00]Hello\n[01:01.50]World"));

        let only_plain = id3v24(&[(b"TIT2", b"\x03Title".to_vec()), (b"USLT", uslt)]);
        assert_eq!(find(&only_plain).as_deref(), Some("Plain line one\nPlain line two"));
    }

    #[test]
    fn id3_utf16_txxx() {
        let mut txxx = vec![1u8];
        let enc16 = |s: &str| -> Vec<u8> {
            let mut v = vec![0xFF, 0xFE];
            for u in s.encode_utf16() {
                v.extend_from_slice(&u.to_le_bytes());
            }
            v
        };
        txxx.extend(enc16("LYRICS"));
        txxx.extend_from_slice(&[0, 0]);
        txxx.extend(enc16("[00:02.00]歌詞"));
        let file = id3v24(&[(b"TXXX", txxx)]);
        assert_eq!(find(&file).as_deref(), Some("[00:02.00]歌詞"));
    }

    #[test]
    fn flac_vorbis_comment() {
        let mut vc = Vec::new();
        let vendor = b"ref";
        vc.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        vc.extend_from_slice(vendor);
        let comments = ["TITLE=x", "LYRICS=[00:01.00]la la"];
        vc.extend_from_slice(&(comments.len() as u32).to_le_bytes());
        for c in comments {
            vc.extend_from_slice(&(c.len() as u32).to_le_bytes());
            vc.extend_from_slice(c.as_bytes());
        }
        let mut file = b"fLaC".to_vec();
        // STREAMINFO (34 bytes), not last
        file.extend_from_slice(&[0, 0, 0, 34]);
        file.extend_from_slice(&[0; 34]);
        file.push(0x80 | VORBIS_COMMENT_BLOCK);
        let n = vc.len();
        file.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8]);
        file.extend(vc);
        assert_eq!(find(&file).as_deref(), Some("[00:01.00]la la"));
    }

    fn mp4_atom(kind: &[u8], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn mp4_lyr_atom() {
        let mut data_body = vec![0, 0, 0, 1, 0, 0, 0, 0];
        data_body.extend_from_slice("Line A\r\nLine B".as_bytes());
        let data = mp4_atom(b"data", &data_body);
        let lyr = mp4_atom(b"\xA9lyr", &data);
        let ilst = mp4_atom(b"ilst", &lyr);
        let mut meta_body = vec![0, 0, 0, 0];
        meta_body.extend(ilst);
        let meta = mp4_atom(b"meta", &meta_body);
        let udta = mp4_atom(b"udta", &meta);
        let moov = mp4_atom(b"moov", &udta);
        let mut file = mp4_atom(b"ftyp", b"M4A \0\0\0\0");
        file.extend(moov);
        assert_eq!(find(&file).as_deref(), Some("Line A\nLine B"));
    }

    #[test]
    fn unknown_and_truncated() {
        assert_eq!(find(&b"RIFF....WAVE".to_vec()), None);
        assert_eq!(find(&b"ID3\x04\x00".to_vec()), None);
        assert_eq!(find(&Vec::new()), None);
    }

    #[test]
    fn sidecar_lrc_beats_plain_tags() {
        let dir = std::env::temp_dir().join(format!("mochy-embedded-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let song = dir.join("song.mp3");
        let mut uslt = vec![3u8];
        uslt.extend_from_slice(b"eng\0");
        uslt.extend_from_slice(b"plain words");
        std::fs::write(&song, id3v24(&[(b"USLT", uslt)])).unwrap();
        assert_eq!(read_file_lyrics(&song).as_deref(), Some("plain words"));
        std::fs::write(dir.join("song.lrc"), "[00:01.00]synced words\n").unwrap();
        assert_eq!(read_file_lyrics(&song).as_deref(), Some("[00:01.00]synced words"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
