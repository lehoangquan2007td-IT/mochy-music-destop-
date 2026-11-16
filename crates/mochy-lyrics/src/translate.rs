//! Keyless lyrics translation through the Google Translate web widget endpoint
//! (Android `GoogleTranslateEngine`).
//!
//! - Undocumented endpoint (`client=gtx`): every failure degrades to "no translation".
//! - The response is a nested JSON array parsed positionally.
//! - Lines are joined with a marker that survives the round trip and sent in batches (long
//!   requests get truncated); a batch whose markers come back mangled is retried line by line.
//! - Only a result that lines up one-for-one with the input is returned: a misaligned
//!   translation would put the wrong words under the wrong line.

use std::sync::LazyLock;
use std::time::Duration;

use futures::stream::{self, StreamExt};
use regex::Regex;

use crate::providers::url;

/// Joins lines inside one request (a plain newline gets re-flowed by the translator).
const LINE_MARKER: &str = "\n@@@\n";
/// Kept well under the point where the endpoint starts truncating.
const BATCH_CHAR_BUDGET: usize = 1_500;
const LINE_BY_LINE_PARALLELISM: usize = 4;
const ENDPOINT: &str = "https://translate.googleapis.com/translate_a/single";
const TIMEOUT: Duration = Duration::from_secs(15);

static MARKER_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*@@@\s*").expect("regex"));

/// Target languages offered in settings: (code, English name). Codes are what Google takes.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("vi", "Vietnamese"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("zh-CN", "Chinese (Simplified)"),
    ("zh-TW", "Chinese (Traditional)"),
    ("th", "Thai"),
    ("id", "Indonesian"),
    ("ms", "Malay"),
    ("tl", "Filipino"),
    ("hi", "Hindi"),
    ("ar", "Arabic"),
    ("tr", "Turkish"),
    ("ru", "Russian"),
    ("uk", "Ukrainian"),
    ("pl", "Polish"),
    ("de", "German"),
    ("fr", "French"),
    ("es", "Spanish"),
    ("pt", "Portuguese"),
    ("it", "Italian"),
    ("nl", "Dutch"),
    ("sv", "Swedish"),
];

/// Concatenated translation from a `translate_a/single` body:
/// `[[["translated","original",…], …], …]`. The pieces are split arbitrarily, so they are
/// joined back before re-splitting on the marker.
pub fn parse_response(body: &str) -> Option<String> {
    let root: serde_json::Value = serde_json::from_str(body).ok()?;
    let segments = root.get(0)?.as_array()?;
    let mut out = String::new();
    for seg in segments {
        if let Some(piece) = seg.get(0).and_then(|p| p.as_str()) {
            out.push_str(piece);
        }
    }
    Some(out)
}

/// Split a joined translation on the marker; `None` unless it yields exactly `expected` parts.
pub fn split_marked(joined: &str, expected: usize) -> Option<Vec<String>> {
    let parts: Vec<String> = MARKER_SPLIT.split(joined).map(|p| p.trim().to_string()).collect();
    (parts.len() == expected).then_some(parts)
}

/// Group lines into batches under the character budget (marker length counted per line).
pub fn batches<S: AsRef<str>>(lines: &[S]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut batch: Vec<String> = Vec::new();
    let mut budget = 0usize;
    for line in lines {
        let line = line.as_ref();
        let len = line.chars().count();
        if budget + len > BATCH_CHAR_BUDGET && !batch.is_empty() {
            out.push(std::mem::take(&mut batch));
            budget = 0;
        }
        batch.push(line.to_string());
        budget += len + LINE_MARKER.len();
    }
    if !batch.is_empty() {
        out.push(batch);
    }
    out
}

async fn translate_batch(http: &reqwest::Client, lines: &[String], target: &str, source: &str) -> Option<Vec<String>> {
    let payload = lines.join(LINE_MARKER);
    let u = url(
        ENDPOINT,
        &[
            ("client", "gtx".into()),
            ("sl", source.to_string()),
            ("tl", target.to_string()),
            ("dt", "t".into()),
            ("q", payload),
        ],
    );
    let resp = http.get(&u).timeout(TIMEOUT).send().await.ok()?;
    if resp.status().as_u16() != 200 {
        return None;
    }
    let body = resp.text().await.ok()?;
    split_marked(&parse_response(&body)?, lines.len())
}

/// Line by line for one batch whose markers came back mangled. A line that still fails keeps
/// its original text, unless every line failed.
async fn translate_line_by_line(
    http: &reqwest::Client,
    lines: &[String],
    target: &str,
    source: &str,
) -> Option<Vec<String>> {
    let results: Vec<Option<String>> = stream::iter(lines.iter().cloned())
        .map(|line| async move {
            translate_batch(http, std::slice::from_ref(&line), target, source)
                .await
                .and_then(|mut v| v.pop())
        })
        .buffered(LINE_BY_LINE_PARALLELISM)
        .collect()
        .await;
    if results.iter().all(Option::is_none) {
        return None;
    }
    Some(results.into_iter().zip(lines).map(|(r, l)| r.unwrap_or_else(|| l.clone())).collect())
}

/// Translate `lines` into `target` (`source` = `"auto"` to detect), preserving order and count.
/// `None` when anything goes wrong.
pub async fn translate<S: AsRef<str>>(
    http: &reqwest::Client,
    lines: &[S],
    target: &str,
    source: &str,
) -> Option<Vec<String>> {
    if lines.is_empty() || target.trim().is_empty() {
        return None;
    }
    let source = if source.trim().is_empty() { "auto" } else { source.trim() };
    let mut out = Vec::with_capacity(lines.len());
    for batch in batches(lines) {
        let translated = match translate_batch(http, &batch, target, source).await {
            Some(t) => t,
            None => translate_line_by_line(http, &batch, target, source).await?,
        };
        out.extend(translated);
    }
    (out.len() == lines.len()).then_some(out)
}

/// Translate the non-blank lines of a song: one entry per input line, `None` for blank lines
/// and for lines that came back unchanged (a line already in the target language would only
/// show twice — Android `translateWithGoogle`).
pub async fn translate_song<S: AsRef<str>>(
    http: &reqwest::Client,
    lines: &[S],
    target: &str,
    source: &str,
) -> Option<Vec<Option<String>>> {
    let indexed: Vec<(usize, &str)> = lines
        .iter()
        .enumerate()
        .map(|(i, l)| (i, l.as_ref()))
        .filter(|(_, l)| !l.trim().is_empty())
        .collect();
    if indexed.is_empty() {
        return Some(vec![None; lines.len()]);
    }
    let texts: Vec<&str> = indexed.iter().map(|(_, l)| *l).collect();
    let translated = translate(http, &texts, target, source).await?;
    Some(align_translations(lines.len(), &indexed, &translated))
}

/// Put translations of the non-blank lines back at their line indices.
pub fn align_translations(n: usize, indexed: &[(usize, &str)], translated: &[String]) -> Vec<Option<String>> {
    let mut out = vec![None; n];
    for ((i, orig), t) in indexed.iter().zip(translated) {
        let t = t.trim();
        if !t.is_empty() && !t.eq_ignore_ascii_case(orig.trim()) {
            out[*i] = Some(t.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"[[["Hello\n@@@\n","こんにちは\n@@@\n",null,null,10],["I love you\n@@@\n","愛してる\n@@@\n",null,null,10],["goodbye","さようなら",null,null,10]],null,"ja",null,null,null,1,[],[["ja"],null,[1],["ja"]]]"#;

    #[test]
    fn parses_fixture_and_splits() {
        let joined = parse_response(FIXTURE).unwrap();
        assert_eq!(joined, "Hello\n@@@\nI love you\n@@@\ngoodbye");
        let parts = split_marked(&joined, 3).unwrap();
        assert_eq!(parts, vec!["Hello", "I love you", "goodbye"]);
        // a count mismatch is rejected rather than misaligned
        assert!(split_marked(&joined, 4).is_none());
    }

    #[test]
    fn mangled_markers_are_tolerated_when_count_matches() {
        // the translator sometimes eats the newlines around the marker
        let parts = split_marked("Hello @@@ world@@@again", 3).unwrap();
        assert_eq!(parts, vec!["Hello", "world", "again"]);
        // merged lines (marker lost) → None, triggers the line-by-line fallback
        assert!(split_marked("Hello world", 2).is_none());
    }

    #[test]
    fn bad_bodies() {
        assert!(parse_response("").is_none());
        assert!(parse_response("<html>").is_none());
        assert!(parse_response("{}").is_none());
        assert_eq!(parse_response("[[]]").as_deref(), Some(""));
    }

    #[test]
    fn batching_respects_budget() {
        let long = "x".repeat(700);
        let lines = vec![long.clone(), long.clone(), long.clone(), "short".to_string()];
        let b = batches(&lines);
        assert!(b.len() >= 2);
        assert_eq!(b.iter().map(Vec::len).sum::<usize>(), 4);
        assert!(batches::<&str>(&[]).is_empty());
    }

    #[test]
    fn alignment_skips_blank_and_identical() {
        let lines = ["こんにちは", "", "OK"];
        let indexed: Vec<(usize, &str)> = vec![(0, lines[0]), (2, lines[2])];
        let out = align_translations(3, &indexed, &["Hello".into(), "ok".into()]);
        assert_eq!(out, vec![Some("Hello".into()), None, None]);
    }
}
