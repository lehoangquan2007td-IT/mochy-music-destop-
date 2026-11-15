//! The Mochy LRC dialect (spec §3.4) — a port of Android `LyricsUtils.parseLyrics`,
//! `syncKind`, `fixWordSpacing` and `parsePlainLyrics`.
//!
//! Supported:
//! - `[mm:ss.mmm]` / `[mm:ss.xx]` line stamps (also `[m:ss]`, `[mm:ss:xx]`, up to 3 minute
//!   digits), several stamps per line (`[00:10.00][01:10.00]chorus`);
//! - metadata tags (`[ti:…]`, `[ar:…]`, `[offset:…]`, …) are ignored;
//! - word timings on the following line: `<word:start:end|word:start:end>` (seconds);
//! - rich / enhanced LRC: `[mm:ss.xx]<mm:ss.xx>word <mm:ss.xx>word`;
//! - `{agent:X}` voice tags and `{bg}` background-vocal lines;
//! - every line on one stamp ⇒ [`SyncKind::Plain`]; untimed text ⇒ `Plain` lines at 0 s.

use std::cmp::Ordering;
use std::sync::LazyLock;

use regex::Regex;

use crate::script::{Script, dominant_script, script_of};
use crate::{LYRICS_NOT_FOUND, LyricLine, LyricWord, SyncKind};

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static regex")
}

/// One or more line stamps followed by the line's text.
static LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"^((?:\[\d{1,3}:\d{2}(?:[.:]\d{1,3})?\] ?)+)(.+)$"));
static TIME_RE: LazyLock<Regex> = LazyLock::new(|| re(r"\[(\d{1,3}):(\d{2})(?:[.:](\d{1,3}))?\]"));
static RICH_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"^\[(\d{1,3}):(\d{2})(?:[.:](\d{1,3}))?\](.+)$"));
/// Inline rich-sync word stamp `<mm:ss.cc>`.
static RICH_TAG_RE: LazyLock<Regex> = LazyLock::new(|| re(r"<(\d{1,3}):(\d{2})[.:](\d{1,3})>"));
/// A rich-sync stamp followed by text (what marks a payload as rich-synced).
static RICH_WORD_RE: LazyLock<Regex> = LazyLock::new(|| re(r"<\d{1,3}:\d{2}[.:]\d{1,3}>\s*[^<]+"));
static AGENT_RE: LazyLock<Regex> = LazyLock::new(|| re(r"\{agent:([^}]+)\}"));
static META_LINE_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^\[[a-zA-Z#]+:.*\]$"));
static ANY_TIME_TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"\[\d{1,3}:\d{2}(?:[.:]\d{1,3})?\]"));
static HEX_ENTITY_RE: LazyLock<Regex> = LazyLock::new(|| re(r"&#x([0-9a-fA-F]+);"));
static DEC_ENTITY_RE: LazyLock<Regex> = LazyLock::new(|| re(r"&#(\d+);"));
static CONTRACTION_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^['’](s|t|re|ll|m|ve|d)$"));
static TABS_RE: LazyLock<Regex> = LazyLock::new(|| re(r"[\t\u{00A0}]+"));
static SPACES_RE: LazyLock<Regex> = LazyLock::new(|| re(r" +"));
static MULTI_SPACE_RE: LazyLock<Regex> = LazyLock::new(|| re(r" {2,}"));

const BG_TAG: &str = "{bg}";

/// Decode the HTML entities some providers leave in their payloads.
pub(crate) fn decode_html_entities(text: &str) -> String {
    let from_code = |code: Option<u32>, whole: &str| -> String {
        code.and_then(char::from_u32)
            .map(String::from)
            .unwrap_or_else(|| whole.to_string())
    };
    let s = HEX_ENTITY_RE.replace_all(text, |c: &regex::Captures| {
        from_code(u32::from_str_radix(&c[1], 16).ok(), &c[0])
    });
    let s = DEC_ENTITY_RE.replace_all(&s, |c: &regex::Captures| {
        from_code(c[1].parse().ok(), &c[0])
    });
    s.replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

/// Payloads sometimes arrive JSON-escaped and quoted; undo that like Android does.
fn unescape(src: &str) -> String {
    let s = src.trim();
    let s = s.strip_prefix('"').unwrap_or(s);
    let s = s.strip_suffix('"').unwrap_or(s);
    s.replace("\\\\", "\\")
        .replace("\\n", "\n")
        .replace("\\r", "\r")
        .replace("\\t", "\t")
}

fn fraction_ms(frac: Option<&str>) -> i64 {
    let Some(f) = frac else { return 0 };
    let v: i64 = f.parse().unwrap_or(0);
    match f.len() {
        1 => v * 100,
        2 => v * 10,
        _ => v,
    }
}

fn stamp_secs(min: &str, sec: &str, frac: Option<&str>) -> f64 {
    let ms = min.parse::<i64>().unwrap_or(0) * 60_000
        + sec.parse::<i64>().unwrap_or(0) * 1000
        + fraction_ms(frac);
    ms as f64 / 1000.0
}

/// Strip `{agent:X}` (anywhere) and a leading `{bg}`; returns (text, agent, bg).
fn split_markers(text: &str) -> (String, Option<String>, bool) {
    let mut text = text.to_string();
    let mut agent = None;
    if let Some(c) = AGENT_RE.captures(&text) {
        agent = Some(c[1].to_string());
        let range = c.get(0).expect("whole match").range();
        text.replace_range(range, "");
    }
    let trimmed = text.trim_start();
    let bg = trimmed.starts_with(BG_TAG);
    if bg {
        text = trimmed[BG_TAG.len()..].to_string();
    }
    (text, agent, bg)
}

/// A parsed line plus how many timed pieces it arrived with (before syllables are merged).
type Entry = (LyricLine, usize);

/// Android `parseLyrics` up to (not including) `fixWordSpacing`: timed lines only.
fn parse_entries(src: &str) -> Vec<Entry> {
    let decoded = decode_html_entities(&unescape(src));
    let lines: Vec<&str> = decoded
        .split(['\n', '\r'])
        .filter(|l| !l.trim().is_empty() && !l.trim().starts_with("[offset:"))
        .collect();
    let rich = lines
        .iter()
        .any(|l| RICH_LINE_RE.is_match(l.trim()) && RICH_WORD_RE.is_match(l));
    let mut out = if rich {
        parse_rich(&lines)
    } else {
        parse_standard(&lines)
    };
    out.sort_by(|a, b| a.0.time.partial_cmp(&b.0.time).unwrap_or(Ordering::Equal));
    out
}

fn parse_standard(lines: &[&str]) -> Vec<Entry> {
    let is_word_line = |l: &str| {
        let t = l.trim();
        t.starts_with('<') && t.ends_with('>')
    };
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if is_word_line(line) {
            continue;
        }
        let Some(entries) = parse_line(line.trim()) else {
            continue;
        };
        let words = lines
            .get(i + 1)
            .filter(|n| is_word_line(n))
            .and_then(|n| {
                let t = n.trim();
                parse_word_timestamps(&t[1..t.len() - 1])
            })
            .unwrap_or_default();
        for mut e in entries {
            e.words = words.clone();
            out.push((e, words.len()));
        }
    }
    out
}

fn parse_line(line: &str) -> Option<Vec<LyricLine>> {
    let caps = LINE_RE.captures(line)?;
    let stamps = caps.get(1)?.as_str();
    let (text, agent, bg) = split_markers(&caps[2]);
    let text = text.trim().to_string();
    Some(
        TIME_RE
            .captures_iter(stamps)
            .map(|t| LyricLine {
                time: stamp_secs(&t[1], &t[2], t.get(3).map(|m| m.as_str())),
                text: text.clone(),
                words: Vec::new(),
                bg,
                agent: agent.clone(),
            })
            .collect(),
    )
}

/// `word:start:end|word:start:end` (seconds). Parsed from the right so a word may contain `:`.
fn parse_word_timestamps(data: &str) -> Option<Vec<LyricWord>> {
    if data.trim().is_empty() {
        return None;
    }
    Some(
        data.split('|')
            .filter_map(|item| {
                let mut parts = item.rsplitn(3, ':');
                let end = parts.next()?.trim().parse::<f64>().ok()?;
                let start = parts.next()?.trim().parse::<f64>().ok()?;
                let text = parts.next()?;
                Some(LyricWord {
                    start,
                    end,
                    text: text.to_string(),
                })
            })
            .collect(),
    )
}

fn parse_rich(lines: &[&str]) -> Vec<Entry> {
    let mut out = Vec::new();
    for (idx, raw) in lines.iter().enumerate() {
        let Some(c) = RICH_LINE_RE.captures(raw.trim()) else {
            continue;
        };
        let time = stamp_secs(&c[1], &c[2], c.get(3).map(|m| m.as_str()));
        let (content, agent, bg) = split_markers(c[4].trim_start());
        let next_line = lines.get(idx + 1).and_then(|n| {
            RICH_LINE_RE
                .captures(n.trim())
                .map(|c| stamp_secs(&c[1], &c[2], c.get(3).map(|m| m.as_str())))
        });
        let words = parse_rich_words(&content, next_line);
        let text = repair_rich_sync_plain_text(RICH_TAG_RE.replace_all(&content, "").trim());
        if !text.trim().is_empty() {
            let n = words.len();
            out.push((
                LyricLine {
                    time,
                    text,
                    words,
                    bg,
                    agent,
                },
                n,
            ));
        }
    }
    out
}

fn parse_rich_words(content: &str, next_line_start: Option<f64>) -> Vec<LyricWord> {
    let tags: Vec<(usize, usize, f64)> = RICH_TAG_RE
        .captures_iter(content)
        .map(|c| {
            let m = c.get(0).expect("whole match");
            (
                m.start(),
                m.end(),
                stamp_secs(&c[1], &c[2], c.get(3).map(|m| m.as_str())),
            )
        })
        .collect();
    let mut words = Vec::new();
    for (i, &(_, tag_end, start)) in tags.iter().enumerate() {
        let seg_end = tags.get(i + 1).map(|t| t.0).unwrap_or(content.len());
        let seg = &content[tag_end..seg_end];
        let seg = seg.split('<').next().unwrap_or("");
        let text = seg.trim();
        if text.is_empty() {
            continue;
        }
        // a stamp with nothing after it (enhanced LRC end marker) still ends the word before it
        let end = tags
            .get(i + 1)
            .map(|t| t.2)
            .or(next_line_start)
            .unwrap_or(start + 0.5);
        words.push(LyricWord {
            start,
            end,
            text: text.to_string(),
        });
    }
    words
}

/// Undo the old syllable-split cache format ("wa nt  yo u") and otherwise normalise whitespace.
pub(crate) fn repair_rich_sync_plain_text(text: &str) -> String {
    let compacted = TABS_RE.replace_all(text, " ");
    let compacted = compacted.trim();
    if compacted.is_empty() {
        return String::new();
    }
    let collapse = |s: &str| SPACES_RE.replace_all(s, " ").into_owned();
    if !MULTI_SPACE_RE.is_match(compacted) {
        return collapse(compacted);
    }
    let groups: Vec<&str> = MULTI_SPACE_RE
        .split(compacted)
        .filter(|g| !g.trim().is_empty())
        .collect();
    let multi_piece = groups.iter().filter(|g| g.trim().contains(' ')).count();
    if multi_piece * 2 <= groups.len() {
        return collapse(compacted);
    }
    groups
        .iter()
        .map(|group| {
            let parts: Vec<&str> = group.split(' ').filter(|p| !p.trim().is_empty()).collect();
            if parts.len() > 1
                && parts
                    .iter()
                    .all(|p| p.chars().count() <= 4 && p.chars().any(char::is_alphabetic))
            {
                parts.concat()
            } else {
                collapse(group.trim())
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// fixWordSpacing — syllables merged into words, dropped spaces restored.
// ---------------------------------------------------------------------------

fn is_unspaced(s: Script) -> bool {
    matches!(s, Script::Han | Script::Kana | Script::Thai)
}

fn spaced_script(text: &str) -> bool {
    let s = dominant_script(text);
    !is_unspaced(s) && s != Script::Unknown
}

fn nonblank_count(words: &[LyricWord]) -> usize {
    words.iter().filter(|w| !w.text.trim().is_empty()).count()
}

fn is_glued_line(text: &str, words: &[LyricWord]) -> bool {
    nonblank_count(words) >= 2 && !text.chars().any(char::is_whitespace) && spaced_script(text)
}

fn spaces_dropped(lines: &[LyricLine]) -> bool {
    let candidates: Vec<&LyricLine> = lines
        .iter()
        .filter(|l| nonblank_count(&l.words) >= 2 && spaced_script(&l.text))
        .collect();
    if candidates.len() < 3 {
        return false;
    }
    let glued: Vec<&&LyricLine> = candidates
        .iter()
        .filter(|l| !l.text.chars().any(char::is_whitespace))
        .collect();
    if glued.len() * 5 < candidates.len() * 4 {
        return false;
    }
    let pieces: usize = glued.iter().map(|l| nonblank_count(&l.words)).sum();
    pieces >= glued.len() * 3
}

const NO_SPACE_BEFORE: &str = ",.!?;:)]}…";
const NO_SPACE_AFTER: &str = "([{";

fn spaced_text(words: &[LyricWord]) -> String {
    let mut out = String::new();
    for piece in words
        .iter()
        .map(|w| w.text.trim())
        .filter(|p| !p.is_empty())
    {
        let first = piece.chars().next().unwrap_or(' ');
        let attached = NO_SPACE_BEFORE.contains(first) || CONTRACTION_RE.is_match(piece);
        if let Some(last) = out.chars().last()
            && !attached
            && !NO_SPACE_AFTER.contains(last)
        {
            out.push(' ');
        }
        out.push_str(piece);
    }
    out
}

/// Byte range each timed word occupies inside `text` (Android `wordCharRanges`).
pub(crate) fn word_char_ranges(text: &str, words: &[&LyricWord]) -> Vec<Option<(usize, usize)>> {
    let mut from = 0;
    words
        .iter()
        .map(|w| {
            let t = w.text.trim();
            if t.is_empty() {
                return None;
            }
            let start = text[from..].find(t)? + from;
            from = start + t.len();
            Some((start, from))
        })
        .collect()
}

fn merge_word_pieces(text: &str, words: &[LyricWord]) -> Vec<LyricWord> {
    let pieces: Vec<&LyricWord> = words.iter().filter(|w| !w.text.trim().is_empty()).collect();
    let ranges = word_char_ranges(text, &pieces);
    let mut merged: Vec<LyricWord> = Vec::with_capacity(pieces.len());
    let mut last_end: Option<usize> = None;
    let spaced_char = |c: Option<char>| c.and_then(script_of).is_none_or(|s| !is_unspaced(s));
    for (piece, range) in pieces.iter().zip(ranges) {
        let glued = match (range, last_end) {
            (Some((start, _)), Some(le)) if start == le => {
                spaced_char(text[..le].chars().next_back())
                    && spaced_char(text[start..].chars().next())
            }
            _ => false,
        };
        match merged.last_mut() {
            Some(prev) if glued => {
                prev.text.push_str(piece.text.trim());
                prev.end = prev.end.max(piece.end);
            }
            _ => merged.push(LyricWord {
                start: piece.start,
                end: piece.end,
                text: piece.text.trim().to_string(),
            }),
        }
        last_end = range.map(|(_, e)| e);
    }
    merged
}

fn fix_word_spacing(lines: &mut [LyricLine]) {
    if !lines.iter().any(|l| l.words.len() > 1) {
        return;
    }
    let dropped = spaces_dropped(lines);
    for line in lines.iter_mut().filter(|l| l.words.len() > 1) {
        if dropped && is_glued_line(&line.text, &line.words) {
            line.text = spaced_text(&line.words);
        }
        line.words = merge_word_pieces(&line.text, &line.words);
    }
}

// ---------------------------------------------------------------------------
// Public-ish API
// ---------------------------------------------------------------------------

/// Plain lyrics: one line per non-blank text line, all at 0 s (Android `parsePlainLyrics`).
pub(crate) fn parse_plain(src: &str) -> Vec<LyricLine> {
    let decoded = decode_html_entities(&src.trim().replace("\\n", "\n").replace("\\r", "\r"));
    decoded
        .split(['\n', '\r'])
        .map(str::trim)
        .filter(|l| !l.is_empty() && !META_LINE_RE.is_match(l))
        .map(|l| {
            let l = ANY_TIME_TAG_RE.replace_all(l, "");
            RICH_TAG_RE.replace_all(&l, "").trim().to_string()
        })
        .filter(|l| !l.is_empty())
        .map(|text| LyricLine {
            time: 0.0,
            text,
            ..Default::default()
        })
        .collect()
}

fn is_unsynced(entries: &[Entry]) -> bool {
    if entries.len() < 2 || entries.iter().any(|e| !e.0.words.is_empty()) {
        return false;
    }
    let first = entries[0].0.time;
    entries.iter().all(|e| e.0.time == first)
}

fn sync_of(entries: &[Entry]) -> SyncKind {
    if is_unsynced(entries) {
        return SyncKind::Plain;
    }
    let word_synced = entries.iter().filter(|e| e.1 > 1).count();
    if word_synced > 0 && word_synced * 2 >= entries.len() {
        SyncKind::Word
    } else {
        SyncKind::Line
    }
}

/// Timed lines exactly as Android `parseLyrics` returns them (no plain fallback).
pub(crate) fn parse_timed(src: &str) -> Vec<LyricLine> {
    let mut lines: Vec<LyricLine> = parse_entries(src).into_iter().map(|e| e.0).collect();
    fix_word_spacing(&mut lines);
    lines
}

pub(crate) fn parse_lrc_impl(src: &str) -> (Vec<LyricLine>, SyncKind) {
    if src.trim().is_empty() || src.trim() == LYRICS_NOT_FOUND {
        return (Vec::new(), SyncKind::None);
    }
    let entries = parse_entries(src);
    if entries.is_empty() {
        let plain = parse_plain(src);
        let sync = if plain
            .iter()
            .any(|l| l.text.chars().any(char::is_alphabetic))
        {
            SyncKind::Plain
        } else {
            SyncKind::None
        };
        return (
            if sync == SyncKind::None {
                Vec::new()
            } else {
                plain
            },
            sync,
        );
    }
    let sync = sync_of(&entries);
    let mut lines: Vec<LyricLine> = entries.into_iter().map(|e| e.0).collect();
    fix_word_spacing(&mut lines);
    (lines, sync)
}

fn stamp(secs: f64) -> String {
    let ms = (secs.max(0.0) * 1000.0).round() as i64;
    format!(
        "[{:02}:{:02}.{:03}]",
        ms / 60_000,
        ms / 1000 % 60,
        ms % 1000
    )
}

/// Seconds rounded to the millisecond, shortest form ("12.345", "3.5", "7").
fn secs(x: f64) -> String {
    let r = (x.max(0.0) * 1000.0).round() / 1000.0;
    format!("{r}")
}

fn one_line(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

pub(crate) fn lines_to_lrc_impl(lines: &[LyricLine]) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str(&stamp(line.time));
        if let Some(agent) = line.agent.as_deref().filter(|a| !a.is_empty()) {
            out.push_str("{agent:");
            out.push_str(&agent.replace(['}', '\r', '\n'], ""));
            out.push('}');
        }
        if line.bg {
            out.push_str(BG_TAG);
        }
        out.push_str(&one_line(&line.text));
        out.push('\n');
        let words: Vec<String> = line
            .words
            .iter()
            .filter_map(|w| {
                let text = one_line(&w.text).replace('|', "");
                (!text.trim().is_empty())
                    .then(|| format!("{text}:{}:{}", secs(w.start), secs(w.end)))
            })
            .collect();
        if !words.is_empty() {
            out.push('<');
            out.push_str(&words.join("|"));
            out.push_str(">\n");
        }
    }
    out.truncate(out.trim_end().len());
    out
}
