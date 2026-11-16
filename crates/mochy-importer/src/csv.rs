//! Playlist CSVs from TuneMyMusic, Exportify and Mochy's own export (Android
//! `CsvImport.kt`). Columns are found by header name so order doesn't matter.
//! Plain text lists ("Artist - Title" per line) are a desktop addition.

use std::sync::LazyLock;

use regex::Regex;

use crate::models::{ImportedTrack, ParsedImport};

static YOUTUBE_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{11}$").unwrap());
static HEADER_WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\s_]+").unwrap());

fn normalize_header(name: &str) -> String {
    let t = name.strip_prefix('\u{FEFF}').unwrap_or(name).trim().to_lowercase();
    HEADER_WS.replace_all(&t, " ").into_owned()
}

/// "3:25", "1:02:03" or plain seconds.
fn parse_duration_ms(text: &str) -> Option<i64> {
    let parts: Option<Vec<i64>> = text.split(':').map(|p| p.trim().parse::<i64>().ok()).collect();
    let parts = parts?;
    let secs = match parts.as_slice() {
        [s] => *s,
        [m, s] => m * 60 + s,
        [h, m, s] => h * 3600 + m * 60 + s,
        _ => return None,
    };
    Some(secs * 1000).filter(|ms| *ms > 0)
}

pub fn parse_csv_rows(text: &str) -> Vec<Vec<String>> {
    let content = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    // excel in some locales saves "csv" with semicolons; pick whichever the header uses more
    let header_line = content.split('\n').next().unwrap_or("");
    let delimiter = if header_line.matches(';').count() > header_line.matches(',').count() { ';' } else { ',' };
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = content.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' {
            in_quotes = true;
        } else if c == delimiter {
            row.push(std::mem::take(&mut field));
        } else if c == '\r' {
        } else if c == '\n' {
            row.push(std::mem::take(&mut field));
            rows.push(std::mem::take(&mut row));
        } else {
            field.push(c);
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

/// Read a playlist CSV. `fallback_playlist_name` names rows without a playlist column.
pub fn parse_import_csv(text: &str, fallback_playlist_name: &str) -> ParsedImport {
    let rows: Vec<Vec<String>> = parse_csv_rows(text).into_iter().filter(|r| r.iter().any(|c| !c.trim().is_empty())).collect();
    let Some(first) = rows.first() else { return ParsedImport::default() };
    let header: Vec<String> = first.iter().map(|h| normalize_header(h)).collect();
    let column = |names: &[&str]| -> Option<usize> { names.iter().find_map(|n| header.iter().position(|h| *h == normalize_header(n))) };

    let title_idx = column(&["Track name", "Track", "Title", "Song", "Song name", "Name"]);
    let artist_idx = column(&["Artist name", "Artist name(s)", "Artist", "Artists", "Artist names"]);
    let album_idx = column(&["Album", "Album name", "Album title"]);
    let playlist_idx = column(&["Playlist name", "Playlist"]);
    let type_idx = column(&["Type"]);
    let duration_ms_idx = column(&["Duration (ms)", "Duration ms", "Track duration (ms)"]);
    let duration_idx = column(&["Duration", "Length", "Time"]);
    let video_id_idx = column(&["Video ID", "YouTube ID", "VideoId"]);
    let thumbnail_idx = column(&["Thumbnail", "Thumbnail URL"]);
    let Some(title_idx) = title_idx else { return ParsedImport::default() };

    let mut out = ParsedImport::default();
    for row in rows.iter().skip(1) {
        let cell = |idx: Option<usize>| -> Option<String> {
            idx.and_then(|i| row.get(i)).map(|c| c.trim().to_string()).filter(|c| !c.is_empty())
        };
        let kind = cell(type_idx).map(|t| t.to_lowercase());
        let is_liked = matches!(kind.as_deref(), Some("favorite" | "favorites" | "liked" | "liked songs"));
        // tunemymusic also lists albums/artists the user follows; only songs are importable
        if let Some(k) = kind.as_deref()
            && !is_liked
            && !matches!(k, "playlist" | "track" | "song")
        {
            continue;
        }
        let playlist_name = cell(playlist_idx).unwrap_or_else(|| fallback_playlist_name.to_string());
        let Some(title) = cell(Some(title_idx)) else {
            // a playlist row with no track keeps an empty playlist from a mochy export
            if !is_liked && playlist_idx.is_some() && cell(playlist_idx).is_some() {
                out.playlist_mut(&playlist_name);
            }
            continue;
        };
        let video_id = cell(video_id_idx).filter(|v| YOUTUBE_ID.is_match(v));
        let artist = cell(artist_idx).unwrap_or_default();
        // without an artist, search is a guess — only keep those rows when the exact id is known
        if artist.is_empty() && video_id.is_none() {
            continue;
        }
        let track = ImportedTrack {
            title,
            artist,
            album: cell(album_idx),
            duration_ms: cell(duration_ms_idx).and_then(|d| d.parse().ok()).or_else(|| cell(duration_idx).and_then(|d| parse_duration_ms(&d))),
            video_id,
            thumbnail_url: cell(thumbnail_idx).filter(|t| t.starts_with("https://")),
            ..Default::default()
        };
        if is_liked {
            out.liked_songs.push(track);
        } else {
            out.playlist_mut(&playlist_name).push(track);
        }
    }
    out
}

/// One row of a Mochy export; `playlist_name` None = liked songs.
#[derive(Debug, Clone, Default)]
pub struct CsvExportRow {
    pub playlist_name: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub video_id: Option<String>,
    pub thumbnail_url: Option<String>,
}

fn push_csv_line(out: &mut String, fields: &[&str]) {
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        if f.contains([',', '"', '\n', '\r']) {
            out.push('"');
            out.push_str(&f.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(f);
        }
    }
    out.push_str("\r\n");
}

pub fn write_mochy_csv(rows: &[CsvExportRow]) -> String {
    let mut out = String::new();
    push_csv_line(&mut out, &["Type", "Playlist name", "Track name", "Artist name", "Album", "Duration (ms)", "Video ID", "Thumbnail"]);
    for r in rows {
        let dur = r.duration_ms.filter(|d| *d > 0).map(|d| d.to_string()).unwrap_or_default();
        push_csv_line(
            &mut out,
            &[
                if r.playlist_name.is_none() { "Favorite" } else { "Playlist" },
                r.playlist_name.as_deref().unwrap_or(""),
                r.title.as_deref().unwrap_or(""),
                r.artist.as_deref().unwrap_or(""),
                r.album.as_deref().unwrap_or(""),
                &dur,
                r.video_id.as_deref().unwrap_or(""),
                r.thumbnail_url.as_deref().unwrap_or(""),
            ],
        );
    }
    out
}

static TEXT_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(?:\d+[.)]\s+)?(.+?)\s+[-–—]\s+(.+?)\s*$").unwrap());

/// A plain list, one song per line as "Artist - Title" (numbering like
/// "1. " allowed). Lines without a dash are skipped. Desktop addition for
/// pasted lists.
pub fn parse_text_list(text: &str, playlist_name: &str) -> ParsedImport {
    let mut out = ParsedImport::default();
    let tracks = out.playlist_mut(playlist_name);
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(c) = TEXT_LINE.captures(line) {
            tracks.push(ImportedTrack::new(c[2].trim(), c[1].trim(), None));
        }
    }
    if out.playlists[0].1.is_empty() {
        out.playlists.clear();
    }
    out
}

/// CSV if it has a header we know, else a text list.
pub fn parse_file(text: &str, file_stem: &str) -> ParsedImport {
    let csv = parse_import_csv(text, file_stem);
    if csv.total_songs() > 0 || csv.total_playlists() > 0 { csv } else { parse_text_list(text, file_stem) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Port of `CsvImportTest`.
    #[test]
    fn tune_my_music_liked_and_playlists() {
        let csv = "\u{FEFF}Track name,Artist name,Album,Playlist name,Type,ISRC\r\n\
            Blinding Lights,The Weeknd,After Hours,,Favorite,USUG11904206\r\n\
            \"Hello, World\",\"Artist A, Artist B\",Album X,Road Trip,Playlist,\r\n\
            Song Two,Artist C,,Road Trip,Playlist,\r\n\
            Some Album,Artist D,,,Album,\r\n\
            No Artist,,,Road Trip,Playlist,\r\n";
        let parsed = parse_import_csv(csv, "Imported playlist");
        assert_eq!(parsed.liked_songs.len(), 1);
        assert_eq!(parsed.liked_songs[0].title, "Blinding Lights");
        assert_eq!(parsed.playlists.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(), vec!["Road Trip"]);
        let road = parsed.playlist("Road Trip").unwrap();
        // the album row is skipped, and so is the row without an artist (nothing to search with)
        assert_eq!(road.len(), 2);
        assert_eq!(road[0].title, "Hello, World");
        assert_eq!(road[0].artist, "Artist A, Artist B");
        assert_eq!(road[1].album, None);
        assert_eq!(road[0].video_id, None);
    }

    #[test]
    fn mochy_export_round_trips() {
        let rows = vec![
            CsvExportRow {
                playlist_name: None,
                title: Some("Liked \"Quoted\" Song".into()),
                artist: Some("Ärtist".into()),
                album: Some("Album".into()),
                duration_ms: Some(215_000),
                video_id: Some("dQw4w9WgXcQ".into()),
                thumbnail_url: Some("https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg".into()),
            },
            CsvExportRow {
                playlist_name: Some("Mix, Vol. 1".into()),
                title: Some("Line\nBreak".into()),
                artist: Some("日本のアーティスト".into()),
                video_id: Some("abcdefghijk".into()),
                ..Default::default()
            },
            CsvExportRow { playlist_name: Some("Empty one".into()), ..Default::default() },
            CsvExportRow {
                playlist_name: Some("Local files".into()),
                title: Some("From phone".into()),
                artist: Some("Someone".into()),
                duration_ms: Some(1000),
                ..Default::default()
            },
        ];
        let parsed = parse_import_csv(&write_mochy_csv(&rows), "Imported playlist");
        assert_eq!(parsed.liked_songs.len(), 1);
        let l = &parsed.liked_songs[0];
        assert_eq!(l.title, "Liked \"Quoted\" Song");
        assert_eq!(l.artist, "Ärtist");
        assert_eq!(l.duration_ms, Some(215_000));
        assert_eq!(l.video_id.as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(l.thumbnail_url.as_deref(), Some("https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"));
        assert_eq!(parsed.playlists.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(), vec!["Mix, Vol. 1", "Empty one", "Local files"]);
        let mix = parsed.playlist("Mix, Vol. 1").unwrap();
        assert_eq!(mix.len(), 1);
        assert_eq!(mix[0].title, "Line\nBreak");
        assert_eq!(mix[0].artist, "日本のアーティスト");
        assert_eq!(mix[0].video_id.as_deref(), Some("abcdefghijk"));
        assert!(parsed.playlist("Empty one").unwrap().is_empty());
        // no YouTube id: falls back to searching by name
        assert_eq!(parsed.playlist("Local files").unwrap()[0].video_id, None);
    }

    #[test]
    fn exportify_style_uses_file_name_as_playlist() {
        let csv = "\"Track URI\",\"Track Name\",\"Artist Name(s)\",\"Album Name\",\"Duration (ms)\"\n\
            \"spotify:track:1\",\"Song\",\"Band\",\"LP\",\"201000\"\n";
        let parsed = parse_import_csv(csv, "My Exportify");
        let t = &parsed.playlist("My Exportify").unwrap()[0];
        assert_eq!(t.title, "Song");
        assert_eq!(t.artist, "Band");
        assert_eq!(t.duration_ms, Some(201_000));
    }

    #[test]
    fn semicolon_csv_and_clock_durations() {
        let csv = "Title;Artist;Duration\nSong;Band;3:25\nOther;Band;1:02:03\n";
        let parsed = parse_import_csv(csv, "Excel");
        let tracks = parsed.playlist("Excel").unwrap();
        assert_eq!(tracks[0].duration_ms, Some(205_000));
        assert_eq!(tracks[1].duration_ms, Some(3_723_000));
    }

    #[test]
    fn garbage_gives_empty_import() {
        assert_eq!(parse_import_csv("", "x").total_songs(), 0);
        assert_eq!(parse_import_csv("just,some,words\n1,2,3", "x").total_songs(), 0);
    }

    #[test]
    fn text_lists() {
        let text = "# my list\n1. Daft Punk - Get Lucky\nThe Weeknd – Blinding Lights\nnot a song line\n\n";
        let p = parse_file(text, "Pasted");
        let t = p.playlist("Pasted").unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].artist, "Daft Punk");
        assert_eq!(t[0].title, "Get Lucky");
        assert_eq!(t[1].title, "Blinding Lights");
        assert_eq!(parse_file("nothing here", "x").total_playlists(), 0);
    }
}
