//! Apple Music style TTML → lines (port of BetterLyrics `TTMLParser.kt`).
//!
//! `<p begin end ttm:agent>` is a line; its direct `<span begin end>` children are timed
//! pieces, merged into words unless a space separates them; `<span ttm:role="x-bg">` holds
//! background vocals (emitted as a separate `bg` line right after its parent);
//! `x-translation` / `x-roman` spans are skipped.

use crate::script::unspaced_boundary;
use crate::xml::{self, XElement, XNode};
use crate::{LyricLine, LyricWord, LyricsError, LyricsResult};

struct SpanInfo {
    text: String,
    start: f64,
    end: f64,
    trailing_space: bool,
}

/// TTML clock values: `1:02.345`, `00:01:02.345`, `62.345`, `62.345s`, `1500ms`, `1.5m`, `1h`.
pub(crate) fn parse_time(s: &str) -> f64 {
    let s = s.trim();
    if s.contains(':') {
        let parts: Vec<f64> = s
            .split(':')
            .map(|p| p.trim().parse::<f64>().unwrap_or(0.0))
            .collect();
        return match parts.as_slice() {
            [m, sec] => m * 60.0 + sec,
            [h, m, sec] => h * 3600.0 + m * 60.0 + sec,
            _ => 0.0,
        };
    }
    let (num, mul) = if let Some(n) = s.strip_suffix("ms") {
        (n, 0.001)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1.0)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60.0)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3600.0)
    } else {
        (s, 1.0)
    };
    num.trim().parse::<f64>().map(|v| v * mul).unwrap_or(0.0)
}

fn role(el: &XElement) -> &str {
    el.attr_local("role").unwrap_or("")
}

fn is_span(node: &XNode) -> Option<&XElement> {
    match node {
        XNode::El(e) if e.local_name().eq_ignore_ascii_case("span") => Some(e),
        _ => None,
    }
}

/// A word boundary is a same-line whitespace text node right after the span.
fn is_word_boundary(node: Option<&XNode>) -> bool {
    match node {
        Some(XNode::Text(t)) => {
            !t.is_empty()
                && !t.contains(['\n', '\r'])
                && t.chars().any(|c| c == ' ' || c == '\t' || c == '\u{00A0}')
        }
        _ => false,
    }
}

fn timed_span(span: &XElement, trailing_space: bool) -> Option<SpanInfo> {
    let begin = span.attr("begin").filter(|b| !b.is_empty())?;
    let end = span.attr("end").filter(|e| !e.is_empty())?;
    let text = span.text_content().trim().to_string();
    if text.is_empty() {
        return None;
    }
    Some(SpanInfo {
        text,
        start: parse_time(begin),
        end: parse_time(end),
        trailing_space,
    })
}

/// Pieces with no space between them form one word, except between two CJK/Thai characters
/// (deviation from Android, which glued a whole Japanese line into one "word" and so lost its
/// word timing).
fn merge_spans(spans: &[SpanInfo]) -> Vec<LyricWord> {
    let mut words: Vec<LyricWord> = Vec::new();
    for (i, span) in spans.iter().enumerate() {
        if i > 0
            && !spans[i - 1].trailing_space
            && !unspaced_boundary(&spans[i - 1].text, &span.text)
        {
            let w = words.last_mut().expect("first span pushed");
            w.text.push_str(&span.text);
            w.end = span.end;
        } else {
            words.push(LyricWord {
                start: span.start,
                end: span.end,
                text: span.text.clone(),
            });
        }
    }
    for w in &mut words {
        w.text = w.text.trim().to_string();
    }
    words
}

/// Text of direct text nodes plus non-bg/translation spans (fallback for untimed lines).
fn direct_text(el: &XElement) -> String {
    let mut out = String::new();
    for c in &el.children {
        match c {
            XNode::Text(t) => out.push_str(t),
            XNode::El(e) => {
                if !matches!(role(e), "x-bg" | "x-translation" | "x-roman")
                    && e.local_name().eq_ignore_ascii_case("span")
                {
                    out.push_str(&e.text_content());
                }
            }
        }
    }
    out
}

fn line_from(
    words: Vec<LyricWord>,
    el: &XElement,
    time: f64,
    agent: Option<String>,
    bg: bool,
) -> Option<LyricLine> {
    let mut joined = String::new();
    for (i, w) in words.iter().enumerate() {
        if i > 0 && !unspaced_boundary(&words[i - 1].text, &w.text) {
            joined.push(' ');
        }
        joined.push_str(&w.text);
    }
    let text = if joined.is_empty() {
        direct_text(el).trim().to_string()
    } else {
        joined
    };
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(LyricLine {
        time,
        text,
        words,
        bg,
        agent,
    })
}

fn parse_background(span: &XElement, parent_start: f64) -> Option<LyricLine> {
    let start = span
        .attr("begin")
        .filter(|b| !b.is_empty())
        .map(parse_time)
        .unwrap_or(parent_start);
    let mut infos = Vec::new();
    for (j, node) in span.children.iter().enumerate() {
        let Some(inner) = is_span(node) else { continue };
        if matches!(role(inner), "x-translation" | "x-roman") {
            continue;
        }
        let trailing = matches!(span.children.get(j + 1), Some(XNode::Text(t)) if t.chars().any(char::is_whitespace));
        if let Some(info) = timed_span(inner, trailing) {
            infos.push(info);
        }
    }
    line_from(merge_spans(&infos), span, start, None, true)
}

pub(crate) fn parse_ttml_impl(src: &str) -> LyricsResult<Vec<LyricLine>> {
    let root = xml::parse(src).map_err(LyricsError::Parse)?;
    if !root.local_name().eq_ignore_ascii_case("tt") {
        return Err(LyricsError::Parse(format!(
            "not TTML (root <{}>)",
            root.name
        )));
    }
    let mut ps = Vec::new();
    root.descendants("p", &mut ps);
    let mut lines = Vec::new();
    for p in ps {
        let Some(begin) = p.attr("begin").filter(|b| !b.is_empty()) else {
            continue;
        };
        let start = parse_time(begin);
        let agent = p
            .attr_local("agent")
            .filter(|a| !a.is_empty())
            .map(str::to_string);
        let mut infos = Vec::new();
        let mut bgs = Vec::new();
        for (j, node) in p.children.iter().enumerate() {
            let Some(span) = is_span(node) else { continue };
            match role(span) {
                "x-bg" => bgs.extend(parse_background(span, start)),
                "x-translation" | "x-roman" => {}
                _ => infos.extend(timed_span(span, is_word_boundary(p.children.get(j + 1)))),
            }
        }
        if let Some(line) = line_from(merge_spans(&infos), p, start, agent, false) {
            lines.push(line);
            lines.extend(bgs);
        }
    }
    Ok(lines)
}
