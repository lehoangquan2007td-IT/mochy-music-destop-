//! A lyrics file someone picked, turned into the text the `lyrics` table
//! keeps (port of Android `songedit/LyricsFileReader` +
//! `LyricsFileFormats`): LRC (loose stamps and `[offset:]` folded in) and
//! plain text stay as they are; TTML, SRT/VTT, ASS karaoke, NetEase YRC,
//! QQ QRC and KuGou KRC (text or the encrypted `.krc`) become LRC lines in
//! the Mochy dialect the lyrics parser reads.

use std::io::Read;
use std::sync::LazyLock;

use mochy_lyrics::SyncKind;
use regex::{Captures, Regex};

/// A lyrics file is a few kilobytes; anything past this is the wrong file.
pub const MAX_BYTES: usize = 1_000_000;
const MAX_TEXT_BYTES: usize = 4_000_000;
const KRC_KEY: [u8; 16] = [64, 71, 97, 119, 94, 50, 116, 71, 81, 54, 49, 45, 206, 210, 110, 105];

fn re(p: &str) -> Regex {
    Regex::new(p).expect("static regex")
}

static SUBTITLE_TIME: LazyLock<Regex> = LazyLock::new(|| re(r"(?:(\d+):)?(\d{1,2}):(\d{2})[,.](\d{1,3})\s*-->"));
static TAG: LazyLock<Regex> = LazyLock::new(|| re(r"<[^>]+>"));
static ASS_DIALOGUE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?m)^Dialogue:\s*[^,]*,(\d+):(\d{2}):(\d{2})[.:](\d{1,3}),[^,]*,(?:[^,]*,){6}(.*)$"));
static ASS_BREAK: LazyLock<Regex> = LazyLock::new(|| re(r"\\[Nnh]"));
static ASS_OVERRIDE: LazyLock<Regex> = LazyLock::new(|| re(r"\{[^}]*\}"));
static ASS_KARAOKE: LazyLock<Regex> = LazyLock::new(|| re(r"\\[kK][fo]?(\d+)"));
static YRC_LINE: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^\[(\d+),\d+\](\(\d+,\d+,-?\d+\).*)$"));
static YRC_WORD: LazyLock<Regex> = LazyLock::new(|| re(r"\((\d+),\d+,-?\d+\)([^(]*)"));
static KRC_LINE: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^\[(\d+),\d+\](<\d+,\d+,-?\d+>.*)$"));
static KRC_WORD: LazyLock<Regex> = LazyLock::new(|| re(r"<(\d+),\d+,-?\d+>([^<]*)"));
static QRC_LINE: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^\[(\d+),\d+\](.*\(\d+,\d+\).*)$"));
static QRC_WORD: LazyLock<Regex> = LazyLock::new(|| re(r"(.*?)\((\d+),\d+\)"));
static QRC_XML: LazyLock<Regex> = LazyLock::new(|| re(r#"LyricContent="([^"]*)""#));
static LRC_STAMP: LazyLock<Regex> = LazyLock::new(|| re(r"([\[<])(\d{1,3}):(\d{1,2})(?:[.:](\d{1,3}))?([\]>])"));
static CANONICAL_STAMP: LazyLock<Regex> = LazyLock::new(|| re(r"^\d{2}:\d{2}\.\d{2,3}$"));
static LRC_OFFSET: LazyLock<Regex> = LazyLock::new(|| re(r"(?mi)^\[offset:\s*([+-]?\d+)\s*\]\s*$"));
static DEC_ENTITY: LazyLock<Regex> = LazyLock::new(|| re(r"&#(\d+);"));
static HEX_ENTITY: LazyLock<Regex> = LazyLock::new(|| re(r"&#x([0-9a-fA-F]+);"));

/// What the picker shows after reading a file.
#[derive(Debug, Clone, PartialEq)]
pub struct PickedLyrics {
    pub text: String,
    pub lines: usize,
    pub synced: bool,
}

/// Read and convert a lyrics file; `None` when nothing in it reads as lyrics.
pub fn read_file(path: &std::path::Path) -> Option<PickedLyrics> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    f.by_ref().take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > MAX_BYTES {
        return None;
    }
    let text = parse(&bytes)?;
    let (lines, sync) = mochy_lyrics::parse_lrc(&text);
    Some(PickedLyrics { lines: lines.len(), synced: !matches!(sync, SyncKind::Plain | SyncKind::None), text })
}

/// A lyrics file's bytes as the text the lyrics table keeps.
pub fn parse(bytes: &[u8]) -> Option<String> {
    let raw = decode_krc(bytes).unwrap_or_else(|| decode_text(bytes));
    let lyrics = if raw.to_ascii_lowercase().contains("<tt") {
        from_ttml(&raw)?
    } else if SUBTITLE_TIME.is_match(&raw) {
        from_srt(&raw)
    } else {
        convert(&raw)
    };
    let lyrics = lyrics.trim().to_string();
    let (lines, sync) = mochy_lyrics::parse_lrc(&lyrics);
    (sync != SyncKind::None && !lines.is_empty()).then_some(lyrics)
}

/// Text files come as UTF-8, UTF-16 (Windows) or a legacy 8-bit code page.
pub fn decode_text(bytes: &[u8]) -> String {
    let text = if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(rest).into_owned()
    } else if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        utf16(rest, u16::from_le_bytes)
    } else if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        utf16(rest, u16::from_be_bytes)
    } else {
        match std::str::from_utf8(bytes) {
            Ok(s) => s.to_string(),
            // windows-1252 / latin-1 fallback (GB18030 isn't ported).
            Err(_) => bytes.iter().map(|&b| cp1252(b)).collect(),
        }
    };
    text.trim_start_matches('\u{FEFF}').to_string()
}

fn utf16(bytes: &[u8], f: fn([u8; 2]) -> u16) -> String {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| f([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
}

fn cp1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž', '\u{8F}', '\u{90}', '‘', '’', '“',
        '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}', 'ž', 'Ÿ',
    ];
    if (0x80..0xA0).contains(&b) { HIGH[(b - 0x80) as usize] } else { b as char }
}

/// The text inside a KuGou `.krc` (zlib behind a fixed xor key).
pub fn decode_krc(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 5 || &bytes[..4] != b"krc1" {
        return None;
    }
    let body: Vec<u8> = bytes[4..].iter().enumerate().map(|(i, b)| b ^ KRC_KEY[i % KRC_KEY.len()]).collect();
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(&body[..]).take(MAX_TEXT_BYTES as u64 + 1).read_to_end(&mut out).ok()?;
    if out.len() > MAX_TEXT_BYTES {
        return None;
    }
    Some(decode_text(&out))
}

pub fn lrc_time(ms: i64) -> String {
    let ms = ms.max(0);
    format!("[{:02}:{:02}.{:02}]", ms / 60_000, (ms / 1000) % 60, (ms % 1000) / 10)
}

pub fn word_time(ms: i64) -> String {
    let ms = ms.max(0);
    format!("{:02}:{:02}.{:03}", ms / 60_000, (ms / 1000) % 60, ms % 1000)
}

/// "5" is half a second, "05" is 50 ms, "050" is 50 ms.
fn fraction(digits: &str) -> i64 {
    let mut d: String = digits.chars().take(3).collect();
    while d.len() < 3 {
        d.push('0');
    }
    d.parse().unwrap_or(0)
}

/// TTML goes through the app's own TTML parser and comes out in the
/// LRC dialect it stores (word timing, agents and background parts kept).
fn from_ttml(raw: &str) -> Option<String> {
    let lines = mochy_lyrics::parse_ttml(raw).ok()?;
    if lines.is_empty() {
        return None;
    }
    Some(mochy_lyrics::lines_to_lrc(&lines))
}

fn from_srt(raw: &str) -> String {
    let text = raw.replace('\r', "");
    let blocks = re(r"\n\s*\n");
    blocks
        .split(&text)
        .filter_map(|block| {
            let lines: Vec<&str> = block.lines().filter(|l| !l.trim().is_empty()).collect();
            let idx = lines.iter().position(|l| SUBTITLE_TIME.is_match(l))?;
            let c = SUBTITLE_TIME.captures(lines[idx])?;
            let h: i64 = c.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
            let m: i64 = c[2].parse().ok()?;
            let s: i64 = c[3].parse().ok()?;
            let ms = ((h * 60 + m) * 60 + s) * 1000 + fraction(&c[4]);
            let body = TAG.replace_all(&lines[idx + 1..].join(" "), "").trim().to_string();
            (!body.is_empty()).then(|| format!("{}{body}", lrc_time(ms)))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Loose LRC, karaoke subtitles and word-timed files → LRC lines.
pub fn convert(raw: &str) -> String {
    let text = raw.replace("\r\n", "\n").replace('\r', "\n");
    let qrc = QRC_XML.captures(&text).map(|c| unescape_xml(&c[1]));
    let t = qrc.as_deref().unwrap_or(&text);
    if ASS_DIALOGUE.is_match(t) {
        from_ass(t)
    } else if YRC_LINE.is_match(t) {
        rich_lines(&YRC_LINE, t, |line_ms, words| {
            YRC_WORD.captures_iter(words).map(|w| (w[2].to_string(), w[1].parse().unwrap_or(line_ms))).collect()
        })
    } else if KRC_LINE.is_match(t) {
        rich_lines(&KRC_LINE, t, |line_ms, words| {
            KRC_WORD.captures_iter(words).map(|w| (w[2].to_string(), line_ms + w[1].parse::<i64>().unwrap_or(0))).collect()
        })
    } else if QRC_LINE.is_match(t) {
        rich_lines(&QRC_LINE, t, |line_ms, words| {
            QRC_WORD.captures_iter(words).map(|w| (w[1].to_string(), w[2].parse().unwrap_or(line_ms))).collect()
        })
    } else {
        normalize_lrc(t)
    }
}

fn rich_lines(line_re: &Regex, text: &str, words: impl Fn(i64, &str) -> Vec<(String, i64)>) -> String {
    line_re
        .captures_iter(text)
        .filter_map(|c| {
            let start: i64 = c[1].parse().ok()?;
            rich_line(start, words(start, &c[2]))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Words keep their own spaces; a blank timed gap joins the word before it.
fn rich_line(line_ms: i64, pieces: Vec<(String, i64)>) -> Option<String> {
    let mut words: Vec<(String, i64)> = Vec::new();
    for (text, start) in pieces {
        if text.is_empty() {
            continue;
        }
        if !text.trim().is_empty() {
            words.push((text, start));
        } else if let Some(last) = words.last_mut() {
            last.0.push_str(&text);
        }
    }
    if words.is_empty() {
        return None;
    }
    let body: String = words.iter().map(|(t, s)| format!("<{}>{t}", word_time(*s))).collect();
    Some(format!("{}{body}", lrc_time(line_ms)))
}

fn from_ass(raw: &str) -> String {
    ASS_DIALOGUE
        .captures_iter(raw)
        .filter_map(|c| {
            let start = ((c[1].parse::<i64>().ok()? * 60 + c[2].parse::<i64>().ok()?) * 60 + c[3].parse::<i64>().ok()?) * 1000
                + fraction(&c[4]);
            let clean = ASS_BREAK.replace_all(&c[5], " ").into_owned();
            let mut pieces: Vec<(String, i64)> = Vec::new();
            let (mut cursor, mut syllable, mut karaoke, mut last) = (start, start, false, 0usize);
            for block in ASS_OVERRIDE.find_iter(&clean) {
                pieces.push((clean[last..block.start()].to_string(), syllable));
                last = block.end();
                if let Some(k) = ASS_KARAOKE.captures(block.as_str()) {
                    karaoke = true;
                    syllable = cursor;
                    cursor += k[1].parse::<i64>().unwrap_or(0) * 10;
                }
            }
            pieces.push((clean[last..].to_string(), syllable));
            if karaoke {
                rich_line(start, pieces)
            } else {
                let text: String = pieces.into_iter().map(|p| p.0).collect();
                let text = text.trim();
                (!text.is_empty()).then(|| format!("{}{text}", lrc_time(start)))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Rewrite only the stamps the parser would skip, and fold an `[offset:]`
/// tag into every time (a positive offset shows the lyrics sooner).
fn normalize_lrc(raw: &str) -> String {
    let offset: i64 = LRC_OFFSET.captures(raw).and_then(|c| c[1].parse().ok()).unwrap_or(0);
    let text = if offset != 0 { LRC_OFFSET.replace_all(raw, "").into_owned() } else { raw.to_string() };
    LRC_STAMP
        .replace_all(&text, |c: &Captures| {
            let whole = &c[0];
            let (open, close) = (&c[1], &c[5]);
            let body = &whole[1..whole.len() - 1];
            if (open == "[") != (close == "]") || (offset == 0 && CANONICAL_STAMP.is_match(body)) {
                return whole.to_string();
            }
            let ms = c[2].parse::<i64>().unwrap_or(0) * 60_000
                + c[3].parse::<i64>().unwrap_or(0) * 1000
                + c.get(4).map(|m| fraction(m.as_str())).unwrap_or(0)
                - offset;
            let ms = ms.max(0);
            if open == "[" { lrc_time(ms) } else { format!("<{}>", word_time(ms)) }
        })
        .into_owned()
}

fn unescape_xml(text: &str) -> String {
    let t = DEC_ENTITY.replace_all(text, |c: &Captures| {
        c[1].parse::<u32>().ok().and_then(char::from_u32).map(String::from).unwrap_or_else(|| c[0].to_string())
    });
    let t = HEX_ENTITY.replace_all(&t, |c: &Captures| {
        u32::from_str_radix(&c[1], 16).ok().and_then(char::from_u32).map(String::from).unwrap_or_else(|| c[0].to_string())
    });
    t.replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// Stored lyrics → standard LRC other players read (`[mm:ss.xx]text` per
/// line, word timing dropped); untimed lyrics stay plain text. Used for the
/// LYRICS tag and the `.lrc` sidecar of a download.
pub fn to_standard_lrc(stored: &str) -> Option<String> {
    let (lines, sync) = mochy_lyrics::parse_lrc(stored);
    if lines.is_empty() || sync == SyncKind::None {
        return None;
    }
    if sync == SyncKind::Plain {
        return Some(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n"));
    }
    Some(lines.iter().filter(|l| !l.text.trim().is_empty()).map(|l| format!("{}{}", lrc_time((l.time * 1000.0).round() as i64), l.text)).collect::<Vec<_>>().join("\n"))
}

/// The file extension a stored lyrics text exports as (Android export:
/// timed → .lrc, plain → .txt).
pub fn sidecar_extension(stored: &str) -> &'static str {
    match mochy_lyrics::parse_lrc(stored).1 {
        SyncKind::Plain | SyncKind::None => "txt",
        _ => "lrc",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrc_stays_and_loose_stamps_are_normalized() {
        let src = "[ti:Song]\n[00:12.50]Hello\n[0:15]World\n[01:02:50]Again";
        let out = parse(src.as_bytes()).unwrap();
        assert!(out.contains("[00:12.50]Hello"));
        assert!(out.contains("[00:15.00]World"));
        assert!(out.contains("[01:02.50]Again"));
    }

    #[test]
    fn offset_tag_is_folded_in() {
        let out = parse(b"[offset:+500]\n[00:10.00]A\n[00:20.00]B").unwrap();
        assert!(out.contains("[00:09.50]A"));
        assert!(out.contains("[00:19.50]B"));
        assert!(!out.contains("offset"));
    }

    #[test]
    fn plain_text_is_kept() {
        let out = parse("Line one\nLine two".as_bytes()).unwrap();
        assert_eq!(out, "Line one\nLine two");
        assert!(parse(b"   \n ").is_none());
    }

    #[test]
    fn srt_and_vtt() {
        let srt = "1\r\n00:00:01,000 --> 00:00:02,000\r\nHello <i>there</i>\r\n\r\n2\r\n00:00:03,500 --> 00:00:04,000\r\nSecond\r\n";
        assert_eq!(parse(srt.as_bytes()).unwrap(), "[00:01.00]Hello there\n[00:03.50]Second");
        let vtt = "WEBVTT\n\n00:01.000 --> 00:02.000\nOne\n\n00:02.500 --> 00:03.000\nTwo\n";
        assert_eq!(parse(vtt.as_bytes()).unwrap(), "[00:01.00]One\n[00:02.50]Two");
    }

    #[test]
    fn ttml_becomes_synced_lines() {
        let ttml = r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><div>
            <p begin="00:01.000" end="00:02.000">First line</p>
            <p begin="00:03.000" end="00:04.000">Second line</p>
        </div></body></tt>"#;
        let out = parse(ttml.as_bytes()).unwrap();
        let (lines, sync) = mochy_lyrics::parse_lrc(&out);
        assert_eq!(lines.len(), 2);
        assert_ne!(sync, SyncKind::Plain);
        assert_eq!(lines[1].text, "Second line");
        assert!((lines[1].time - 3.0).abs() < 0.01);
    }

    #[test]
    fn word_timed_formats() {
        let yrc = "[1000,2000](1000,500,0)Hello (1500,500,0)world";
        assert_eq!(convert(yrc), "[00:01.00]<00:01.000>Hello <00:01.500>world");
        let krc = "[2000,1000]<0,300,0>A<300,300,0>B";
        assert_eq!(convert(krc), "[00:02.00]<00:02.000>A<00:02.300>B");
        let qrc = "[3000,1000]Hi (3000,200)there(3200,300)";
        assert_eq!(convert(qrc), "[00:03.00]<00:03.000>Hi <00:03.200>there");
        let ass = "[Events]\nDialogue: 0,0:00:05.00,0:00:06.00,Default,,0,0,0,,{\\k50}Ka{\\k50}ra";
        assert_eq!(convert(ass), "[00:05.00]<00:05.000>Ka<00:05.500>ra");
    }

    #[test]
    fn text_encodings() {
        let mut utf16 = vec![0xFF, 0xFE];
        for u in "Xin chào".encode_utf16() {
            utf16.extend(u.to_le_bytes());
        }
        assert_eq!(decode_text(&utf16), "Xin chào");
        assert_eq!(decode_text(&[0xEF, 0xBB, 0xBF, b'a']), "a");
        assert_eq!(decode_text(&[b'c', 0xE9]), "cé");
    }

    #[test]
    fn krc_roundtrip() {
        use std::io::Write;
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(b"[1000,500]<0,250,0>La<250,250,0>la").unwrap();
        let plain = z.finish().unwrap();
        let mut krc = b"krc1".to_vec();
        krc.extend(plain.iter().enumerate().map(|(i, b)| b ^ KRC_KEY[i % KRC_KEY.len()]));
        assert_eq!(parse(&krc).unwrap(), "[00:01.00]<00:01.000>La<00:01.250>la");
    }

    #[test]
    fn standard_lrc_export() {
        let stored = "[00:01.00]<00:01.000>Hello <00:01.500>world\n[00:03.00]Next";
        let out = to_standard_lrc(stored).unwrap();
        assert_eq!(out, "[00:01.00]Hello world\n[00:03.00]Next");
        assert_eq!(sidecar_extension(stored), "lrc");
        assert_eq!(to_standard_lrc("just words").unwrap(), "just words");
        assert_eq!(sidecar_extension("just words"), "txt");
    }
}
