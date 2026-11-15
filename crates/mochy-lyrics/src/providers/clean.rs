//! Title / artist cleanup shared by LrcLib and Paxsenix (`cleanTitle`, `cleanArtist`).

use std::sync::LazyLock;

use regex::Regex;

const TAGS: &str = "official|video|audio|lyrics|lyric|visualizer|hd|hq|4k|remaster|remix|live|acoustic|version|edit|extended|radio|clean|explicit";

static TITLE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        format!(r"(?i)\s*\(.*?({TAGS}).*?\)"),
        format!(r"(?i)\s*\[.*?({TAGS}).*?\]"),
        r"\s*【.*?】".to_string(),
        r"\s*\|.*$".to_string(),
        r"(?i)\s*-\s*(official|video|audio|lyrics|lyric|visualizer).*$".to_string(),
        r"(?i)\s*\(feat\..*?\)".to_string(),
        r"(?i)\s*\(ft\..*?\)".to_string(),
        r"(?i)\s*feat\..*$".to_string(),
        r"(?i)\s*ft\..*$".to_string(),
    ]
    .iter()
    .map(|p| Regex::new(p).expect("regex"))
    .collect()
});

/// Paxsenix additionally drops parentheses holding a year ("(2011 Remaster)").
static YEAR_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s*\([^)]*\d{4}[^)]*\)").expect("regex"));

const ARTIST_SEPARATORS: [&str; 11] = [
    " & ",
    " and ",
    ", ",
    " x ",
    " X ",
    " feat. ",
    " feat ",
    " ft. ",
    " ft ",
    " featuring ",
    " with ",
];

/// LrcLib `cleanTitle`.
pub(crate) fn clean_title(title: &str) -> String {
    let mut s = title.trim().to_string();
    for p in TITLE_PATTERNS.iter() {
        s = p.replace_all(&s, "").into_owned();
    }
    s.trim().to_string()
}

/// Paxsenix `cleanTitle` (LrcLib's plus the year pattern).
pub(crate) fn clean_title_paxsenix(title: &str) -> String {
    let s = clean_title(title);
    YEAR_PATTERN.replace_all(&s, "").trim().to_string()
}

fn find_ignore_ascii_case(hay: &str, needle: &str) -> Option<usize> {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

/// First artist before the first separator (in list order) that occurs.
pub(crate) fn clean_artist(artist: &str) -> String {
    let s = artist.trim();
    for sep in ARTIST_SEPARATORS {
        if let Some(pos) = find_ignore_ascii_case(s, sep) {
            return s[..pos].trim().to_string();
        }
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_titles_and_artists() {
        assert_eq!(
            clean_title("Blinding Lights (Official Video)"),
            "Blinding Lights"
        );
        assert_eq!(clean_title("Song [Lyric Video] | Topic"), "Song");
        assert_eq!(clean_title("Stay (feat. Justin Bieber)"), "Stay");
        assert_eq!(clean_title_paxsenix("Yesterday (2009 Mix)"), "Yesterday");
        assert_eq!(
            clean_artist("The Kid LAROI, Justin Bieber"),
            "The Kid LAROI"
        );
        assert_eq!(clean_artist("Simon AND Garfunkel"), "Simon");
        assert_eq!(clean_artist("Ed Sheeran"), "Ed Sheeran");
    }
}
