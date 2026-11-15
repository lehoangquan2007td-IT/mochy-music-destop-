//! Shared helpers of the "direct" providers (NetEase, QQ Music, Deezer, lrc.red):
//! `TrackMatch.kt` (search-result matching) and `TimedLyrics.kt` (timed lines → LRC).

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;

/// Ktor `lyricsHttp`: connect 5 s, request 10 s.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(10);

/// A search result cut down to what tells whether it is the song being played.
#[derive(Debug, Clone)]
pub(crate) struct TrackCandidate<T> {
    pub titles: Vec<String>,
    pub artists: Vec<String>,
    pub duration_sec: i32,
    pub r#ref: T,
}

/// Seconds a result's length may differ from the song's and still be the same recording.
const DURATION_TOLERANCE: i32 = 8;

static DECORATIONS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*[(（\[【「『《].*?[)）\]】」』》]").expect("regex"));

pub(crate) fn normalize(text: &str) -> String {
    DECORATIONS
        .replace_all(text, "")
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn title_score<T>(c: &TrackCandidate<T>, wanted: &str) -> i32 {
    c.titles
        .iter()
        .map(|t| normalize(t))
        .filter(|t| !t.is_empty())
        .map(|t| {
            if t == wanted {
                2
            } else if t.contains(wanted) || wanted.contains(&t) {
                1
            } else {
                0
            }
        })
        .max()
        .unwrap_or(0)
}

fn artist_score<T>(c: &TrackCandidate<T>, wanted: &str) -> i32 {
    if wanted.is_empty() {
        return 0;
    }
    let hit = c
        .artists
        .iter()
        .map(|a| normalize(a))
        .any(|n| !n.is_empty() && (wanted.contains(&n) || n.contains(wanted)));
    if hit { 2 } else { 0 }
}

/// The result that is this song: about the same length, same title, same artist when one
/// matches. First best wins ties (Kotlin `maxByOrNull`).
pub(crate) fn best_match<T: Clone>(
    cands: &[TrackCandidate<T>],
    title: &str,
    artist: &str,
    duration_sec: i32,
) -> Option<T> {
    let wanted_title = normalize(title);
    let wanted_artist = normalize(artist);
    if wanted_title.is_empty() {
        return None;
    }
    let mut best: Option<(&TrackCandidate<T>, i32)> = None;
    for c in cands {
        if duration_sec > 0 && (c.duration_sec - duration_sec).abs() > DURATION_TOLERANCE {
            continue;
        }
        let ts = title_score(c, &wanted_title);
        if ts <= 0 {
            continue;
        }
        let total = ts + artist_score(c, &wanted_artist);
        if best.is_none_or(|(_, s)| total > s) {
            best = Some((c, total));
        }
    }
    best.map(|(c, _)| c.r#ref.clone())
}

/// One timed word (ms from song start).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TimedWord {
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

/// One line; `words` empty when the source only times whole lines.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TimedLine {
    pub start_ms: i64,
    pub text: String,
    pub words: Vec<TimedWord>,
}

static LRC_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[(\d{1,3}):(\d{2})(?:[.:](\d{1,3}))?](.*)$").expect("regex"));

/// Standard `[mm:ss.xx]text` lines; tag lines and untimed text dropped.
pub(crate) fn parse_lrc(lrc: &str) -> Vec<TimedLine> {
    lrc.lines()
        .filter_map(|raw| {
            let c = LRC_LINE.captures(raw.trim())?;
            let frac = c.get(3).map(|m| m.as_str()).unwrap_or("");
            let millis: i64 = format!("{frac:0<3}")[..3].parse().unwrap_or(0);
            let ms = c[1].parse::<i64>().ok()? * 60_000 + c[2].parse::<i64>().ok()? * 1000 + millis;
            Some(TimedLine {
                start_ms: ms,
                text: c[4].trim().to_string(),
                words: Vec::new(),
            })
        })
        .collect()
}

// credit lines like "作词 : 方文山", "lyrics by：max martin": a short label, then a colon
static CREDIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[^:：]{1,24}[:：].+$").expect("regex"));
const HEAD_CREDIT_WINDOW: usize = 15;
const TAIL_CREDIT_WINDOW: usize = 6;

fn is_credit(line: &TimedLine) -> bool {
    if !CREDIT.is_match(&line.text) {
        return false;
    }
    let label = line.text.split(':').next().unwrap_or("");
    let label = label.split('：').next().unwrap_or("").trim();
    label.split(' ').count() <= 4
}

/// Drop the credits Chinese services put around the lyrics.
pub(crate) fn without_credits(lines: Vec<TimedLine>) -> Vec<TimedLine> {
    let head = lines
        .iter()
        .take(HEAD_CREDIT_WINDOW)
        .rposition(is_credit)
        .map(|i| i + 1)
        .unwrap_or(0);
    let mut body: Vec<TimedLine> = lines.into_iter().skip(head).collect();
    let tail_start = body.len().saturating_sub(TAIL_CREDIT_WINDOW);
    if let Some(cut) = body[tail_start..].iter().position(is_credit) {
        body.truncate(tail_start + cut);
    }
    body
}

/// Anything to sing along to, rather than nothing or an "instrumental" note (纯音乐).
pub(crate) fn has_lyrics(lines: &[TimedLine]) -> bool {
    let sung: Vec<&TimedLine> = lines.iter().filter(|l| !l.text.trim().is_empty()).collect();
    !sung.is_empty() && !sung.iter().any(|l| l.text.contains("纯音乐"))
}

fn stamp(ms: i64) -> String {
    let ms = ms.max(0);
    format!(
        "[{:02}:{:02}.{:03}]",
        ms / 60_000,
        ms / 1000 % 60,
        ms % 1000
    )
}

/// Lines in the app's LRC dialect; word-timed lines are followed by `<word:start:end|…>`.
pub(crate) fn to_lrc(lines: &[TimedLine]) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str(&stamp(line.start_ms));
        out.push_str(&line.text);
        out.push('\n');
        let words: Vec<String> = line
            .words
            .iter()
            .map(|w| {
                (
                    w.text
                        .trim()
                        .chars()
                        .filter(|c| !":|<>".contains(*c))
                        .collect::<String>(),
                    w,
                )
            })
            .filter(|(t, _)| !t.is_empty())
            .map(|(t, w)| {
                format!(
                    "{t}:{}:{}",
                    w.start_ms as f64 / 1000.0,
                    w.end_ms as f64 / 1000.0
                )
            })
            .collect();
        if !words.is_empty() {
            out.push('<');
            out.push_str(&words.join("|"));
            out.push_str(">\n");
        }
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ms: i64, text: &str) -> TimedLine {
        TimedLine {
            start_ms: ms,
            text: text.into(),
            words: Vec::new(),
        }
    }

    #[test]
    fn matches_by_duration_title_artist() {
        let c = vec![
            TrackCandidate {
                titles: vec!["Blinding Lights (Live)".into()],
                artists: vec!["Cover Band".into()],
                duration_sec: 201,
                r#ref: 1,
            },
            TrackCandidate {
                titles: vec!["Blinding Lights".into()],
                artists: vec!["The Weeknd".into()],
                duration_sec: 200,
                r#ref: 2,
            },
            TrackCandidate {
                titles: vec!["Blinding Lights".into()],
                artists: vec!["The Weeknd".into()],
                duration_sec: 260,
                r#ref: 3,
            },
        ];
        assert_eq!(
            best_match(&c, "Blinding Lights", "The Weeknd", 200),
            Some(2)
        );
        assert_eq!(best_match(&c, "Other", "The Weeknd", 200), None);
    }

    #[test]
    fn strips_credits_and_formats() {
        let lines = parse_lrc(
            "[ti:x]\n[00:00.00]作词 : 方文山\n[00:01.50]作曲 : 周杰伦\n[00:05.1]hello\n[00:07.123]world",
        );
        let body = without_credits(lines);
        assert_eq!(body, vec![line(5100, "hello"), line(7123, "world")]);
        assert!(has_lyrics(&body));
        // tail credits: the first one in the last 6 lines and everything after it go
        let mut long: Vec<TimedLine> = vec![line(0, "Lyrics by: Someone")];
        long.extend((1..=14).map(|i| line(i * 1000, "sung line")));
        long.push(line(20_000, "制作人 : 某人"));
        long.push(line(21_000, "混音 : 某人"));
        let tail = without_credits(long);
        assert_eq!(tail.len(), 14);
        assert!(tail.iter().all(|l| l.text == "sung line"));
        assert!(!has_lyrics(&[line(0, "纯音乐，请欣赏")]));
        assert_eq!(to_lrc(&body), "[00:05.100]hello\n[00:07.123]world");
    }
}
