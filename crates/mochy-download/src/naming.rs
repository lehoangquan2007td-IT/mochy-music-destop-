//! File names for downloaded songs (port of Android `DocumentTree.safeName`
//! + the export's "artist - title" base name), plus a desktop filename
//! template: `{artist} - {title}` by default; tokens `{title}`, `{artist}`,
//! `{album}`, `{album_artist}`, `{year}`, `{id}`; `/` makes sub-folders.

use std::path::{Path, PathBuf};

pub const DEFAULT_TEMPLATE: &str = "{artist} - {title}";

/// Longest file-name segment kept (Android keeps 120 characters).
const MAX_SEGMENT_CHARS: usize = 120;

/// Windows device names that can't be used as a file name (any extension).
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// `raw` as a file-name-safe segment: `\ / : * ? " < > |` and control
/// characters become spaces, whitespace collapses, trailing dots/spaces go,
/// at most 120 characters; `fallback` when nothing is left.
pub fn safe_name(raw: &str, fallback: &str) -> String {
    let replaced: String =
        raw.chars().map(|c| if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { ' ' } else { c }).collect();
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim().trim_end_matches(['.', ' ']);
    let cut: String = trimmed.chars().take(MAX_SEGMENT_CHARS).collect();
    let mut out = cut.trim().trim_end_matches(['.', ' ']).to_string();
    if out.is_empty() {
        out = fallback.to_string();
    }
    let stem = out.split('.').next().unwrap_or("").trim().to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        out = format!("_{out}");
    }
    out
}

/// What a template can refer to.
#[derive(Debug, Clone, Default)]
pub struct NameFields {
    pub title: String,
    /// Artists joined with ", ".
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub year: Option<i32>,
    pub id: String,
}

/// Render `template` into a relative path (no extension). Each `/`- or
/// `\`-separated segment is sanitized; separators left dangling by an empty
/// token (" - ", "_", …) are trimmed, and empty segments dropped. Falls back
/// to the default template when the result would be empty.
pub fn render_template(template: &str, f: &NameFields) -> PathBuf {
    let tpl = if template.trim().is_empty() { DEFAULT_TEMPLATE } else { template };
    let year = f.year.map(|y| y.to_string()).unwrap_or_default();
    let mut segments: Vec<String> = Vec::new();
    let raw_segments: Vec<&str> = tpl.split(['/', '\\']).collect();
    let last = raw_segments.len().saturating_sub(1);
    for (i, seg) in raw_segments.iter().enumerate() {
        let filled = seg
            .replace("{title}", &f.title)
            .replace("{artist}", &f.artist)
            .replace("{album_artist}", &f.album_artist)
            .replace("{album}", &f.album)
            .replace("{year}", &year)
            .replace("{id}", &f.id);
        let tidy = tidy_separators(&filled);
        if tidy.is_empty() {
            if i == last {
                // the file name itself can't be empty
                segments.push(safe_name(&f.title, &f.id));
            }
            continue;
        }
        segments.push(safe_name(&tidy, &f.id));
    }
    if segments.is_empty() {
        segments.push(safe_name(&f.title, &f.id));
    }
    segments.iter().collect()
}

/// Trim separators an empty token leaves behind: "- Title" → "Title",
/// "Artist - " → "Artist", "A -  - B" → "A - B", "()" / "[]" removed.
fn tidy_separators(s: &str) -> String {
    let mut t = s.replace("()", "").replace("[]", "");
    loop {
        let before = t.clone();
        t = t.replace(" -  - ", " - ").replace(" - - ", " - ").replace("  ", " ");
        if t == before {
            break;
        }
    }
    t.trim_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '_' | '.' | ',' | '·')).to_string()
}

/// `dir/base.ext`, or `dir/base (2).ext`, `(3)`, … when taken. A path for
/// which `is_own(path)` says yes (the same song's earlier file) is reused.
pub fn unique_path(dir: &Path, base: &str, ext: &str, is_own: impl Fn(&Path) -> bool) -> PathBuf {
    let make = |name: String| if ext.is_empty() { dir.join(name) } else { dir.join(format!("{name}.{ext}")) };
    let first = make(base.to_string());
    if !first.exists() || is_own(&first) {
        return first;
    }
    for n in 2..10_000 {
        let p = make(format!("{base} ({n})"));
        if !p.exists() || is_own(&p) {
            return p;
        }
    }
    first
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_name_matches_android_rules() {
        assert_eq!(safe_name("AC/DC: Back in Black?", "x"), "AC DC Back in Black");
        assert_eq!(safe_name("  a\t\tb\nc  ", "x"), "a b c");
        assert_eq!(safe_name("Dots...", "x"), "Dots");
        assert_eq!(safe_name("<>|*", "fallback"), "fallback");
        assert_eq!(safe_name("", "id123"), "id123");
        assert_eq!(safe_name(&"é".repeat(200), "x").chars().count(), 120);
        assert_eq!(safe_name("Sơn Tùng M-TP - Lạc Trôi", "x"), "Sơn Tùng M-TP - Lạc Trôi");
    }

    #[test]
    fn windows_reserved_names_are_escaped() {
        assert_eq!(safe_name("CON", "x"), "_CON");
        assert_eq!(safe_name("nul.txt", "x"), "_nul.txt");
        assert_eq!(safe_name("Console", "x"), "Console");
    }

    fn fields() -> NameFields {
        NameFields {
            title: "Lạc Trôi".into(),
            artist: "Sơn Tùng M-TP".into(),
            album: "m-tp M-TP".into(),
            album_artist: "Sơn Tùng M-TP".into(),
            year: Some(2017),
            id: "Llw9Q6akRo4".into(),
        }
    }

    #[test]
    fn default_template_is_artist_dash_title() {
        assert_eq!(render_template(DEFAULT_TEMPLATE, &fields()), PathBuf::from("Sơn Tùng M-TP - Lạc Trôi"));
        let mut f = fields();
        f.artist.clear();
        assert_eq!(render_template(DEFAULT_TEMPLATE, &f), PathBuf::from("Lạc Trôi"));
        assert_eq!(render_template("", &f), PathBuf::from("Lạc Trôi"));
    }

    #[test]
    fn folders_and_empty_tokens() {
        let p = render_template("{album_artist}/{album}/{artist} - {title}", &fields());
        assert_eq!(p, PathBuf::from("Sơn Tùng M-TP").join("m-tp M-TP").join("Sơn Tùng M-TP - Lạc Trôi"));
        let mut f = fields();
        f.album.clear();
        f.year = None;
        let p = render_template("{album}/{title} ({year})", &f);
        assert_eq!(p, PathBuf::from("Lạc Trôi"));
        let p = render_template("{artist}/{title}: part 2", &fields());
        assert_eq!(p, PathBuf::from("Sơn Tùng M-TP").join("Lạc Trôi part 2"));
        // ".." can't climb out of the folder
        let mut f = fields();
        f.artist = "..".into();
        let p = render_template("{artist}/{title}", &f);
        assert!(p.components().all(|c| matches!(c, std::path::Component::Normal(_))));
    }

    #[test]
    fn unique_path_adds_counter() {
        let dir = std::env::temp_dir().join(format!("mochy-naming-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("A.opus"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "A", "opus", |_| false), dir.join("A (2).opus"));
        assert_eq!(unique_path(&dir, "A", "opus", |p| p.ends_with("A.opus")), dir.join("A.opus"));
        assert_eq!(unique_path(&dir, "B", "m4a", |_| false), dir.join("B.m4a"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
