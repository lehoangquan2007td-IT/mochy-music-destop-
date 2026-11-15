//! Pure parsing helpers — public so tests can pin response shapes without
//! network (fixtures under `tests/fixtures/`).
//!
//! The logic mirrors the Kotlin pages (`SearchSummaryPage.kt`, `SearchPage.kt`,
//! `AlbumPage.kt`, `PlaylistPage.kt`, `NextPage.kt`, `RelatedPage.kt`,
//! `HomePage.kt`) but is written against `serde_json::Value` so a missing
//! field degrades to "unknown" instead of failing the whole page.

use mochy_core::models::{
    Album, Artist, ArtistRef, Playlist, QueueItem, SearchResults, Shelf, ShelfItem, Song,
};
use mochy_core::settings::AudioQuality;
use serde_json::Value;

use crate::{AudioFormat, CodecPreference, StreamCandidate, StreamResult};

/// Size requested for `googleusercontent`/`ggpht` artwork (the API hands out
/// 60–226 px variants; the URL suffix can ask for any size).
pub const THUMBNAIL_SIZE: u32 = 544;

const MVT_ATV: &str = "MUSIC_VIDEO_TYPE_ATV";
const MVT_PRIVATE: &str = "MUSIC_VIDEO_TYPE_PRIVATELY_OWNED_TRACK";
const MVT_PODCAST: &str = "MUSIC_VIDEO_TYPE_PODCAST_EPISODE";

// ---------------------------------------------------------------------------
// Text / runs helpers
// ---------------------------------------------------------------------------

/// Extract text from a runs array, `simpleText`, or a view-model `content`.
pub fn nav_text(val: &Value) -> String {
    if let Some(s) = val["simpleText"].as_str() {
        return s.to_string();
    }
    if let Some(runs) = val["runs"].as_array() {
        return runs.iter().filter_map(|r| r["text"].as_str()).collect::<String>();
    }
    if let Some(s) = val["content"].as_str() {
        return s.to_string();
    }
    String::new()
}

fn runs(val: &Value) -> &[Value] {
    val["runs"].as_array().map(|a| a.as_slice()).unwrap_or(&[])
}

fn text(run: &Value) -> &str {
    run["text"].as_str().unwrap_or("")
}

fn browse_id(run: &Value) -> Option<&str> {
    run["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str()
}

fn endpoint_page_type(endpoint: &Value) -> Option<&str> {
    endpoint["browseEndpoint"]["browseEndpointContextSupportedConfigs"]
        ["browseEndpointContextMusicConfig"]["pageType"]
        .as_str()
}

fn page_type(run: &Value) -> Option<&str> {
    endpoint_page_type(&run["navigationEndpoint"])
}

fn watch_music_video_type(watch_endpoint: &Value) -> Option<&str> {
    watch_endpoint["watchEndpointMusicSupportedConfigs"]["watchEndpointMusicConfig"]["musicVideoType"]
        .as_str()
}

/// Split runs on the `" • "` separator (Kotlin `splitBySeparator`).
pub fn split_runs(runs: &[Value]) -> Vec<Vec<&Value>> {
    let mut out = vec![Vec::new()];
    for r in runs {
        if text(r).trim() == "•" {
            out.push(Vec::new());
        } else {
            out.last_mut().expect("non-empty").push(r);
        }
    }
    out.retain(|seg| !seg.is_empty());
    out
}

fn is_separator(t: &str) -> bool {
    matches!(t.trim(), "" | "&" | "," | "•" | "·" | "|")
}

fn is_artist_run(run: &Value) -> bool {
    match page_type(run) {
        Some(
            "MUSIC_PAGE_TYPE_ARTIST" | "MUSIC_PAGE_TYPE_USER_CHANNEL" | "MUSIC_PAGE_TYPE_LIBRARY_ARTIST",
        ) => true,
        Some(_) => false,
        None => browse_id(run).is_some_and(|b| b.starts_with("UC")),
    }
}

fn is_album_run(run: &Value) -> bool {
    matches!(page_type(run), Some("MUSIC_PAGE_TYPE_ALBUM" | "MUSIC_PAGE_TYPE_AUDIOBOOK"))
        || browse_id(run).is_some_and(|b| b.starts_with("MPRE"))
}

/// Parse a duration string like "3:45" or "1:02:30" into seconds.
pub fn parse_duration(text: &str) -> Option<f64> {
    let t = text.trim();
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit() || c == ':') {
        return None;
    }
    let parts: Vec<&str> = t.split(':').collect();
    let nums: Option<Vec<f64>> = parts.iter().map(|p| p.parse::<f64>().ok()).collect();
    let nums = nums?;
    match nums.as_slice() {
        [m, s] => Some(m * 60.0 + s),
        [h, m, s] => Some(h * 3600.0 + m * 60.0 + s),
        _ => None,
    }
}

/// "43 minutes", "2 hours, 28 minutes", "1 hour 5 min" → seconds (English only).
pub fn parse_long_duration(text: &str) -> Option<f64> {
    let mut total = 0.0;
    let mut found = false;
    let lower = text.to_lowercase();
    let tokens: Vec<&str> = lower
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .collect();
    for pair in tokens.windows(2) {
        let Ok(n) = pair[0].replace('+', "").parse::<f64>() else { continue };
        let unit = pair[1];
        let mult = if unit.starts_with("hour") || unit == "hr" || unit == "hrs" {
            3600.0
        } else if unit.starts_with("min") {
            60.0
        } else if unit.starts_with("sec") {
            1.0
        } else {
            continue;
        };
        total += n * mult;
        found = true;
    }
    found.then_some(total)
}

fn parse_year(text: &str) -> Option<i32> {
    let t = text.trim();
    if t.len() == 4 && t.chars().all(|c| c.is_ascii_digit()) {
        let y: i32 = t.parse().ok()?;
        (1900..=2100).contains(&y).then_some(y)
    } else {
        None
    }
}

/// First integer in a text ("40 songs", "1,234 songs").
fn leading_count(text: &str) -> Option<u32> {
    let digits: String = text
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
        .filter(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Counters such as "7B plays", "3B views", "11.3M subscribers".
fn is_counter_text(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    let starts_numeric = t.chars().next().is_some_and(|c| c.is_ascii_digit());
    starts_numeric
        && ["play", "view", "subscriber", "monthly", "listener", "song", "track", "episode", "like"]
            .iter()
            .any(|w| t.contains(w))
}

/// Item-type labels YTM prefixes subtitles with in mixed result lists.
fn is_type_label(text: &str) -> bool {
    matches!(
        text.trim(),
        "Song" | "Video" | "Episode" | "Podcast" | "Single" | "EP" | "Album" | "Playlist" | "Artist"
            | "Profile" | "Station" | "Audiobook" | "Live"
    )
}

// ---------------------------------------------------------------------------
// Thumbnails
// ---------------------------------------------------------------------------

/// Upscale a `googleusercontent`/`ggpht` artwork URL to `size`×`size` by
/// rewriting its `=w…-h…` suffix. Other hosts (i.ytimg.com) are returned as-is.
pub fn hi_res_thumbnail(url: &str, size: u32) -> String {
    let is_resizable = url.contains("googleusercontent.com") || url.contains("ggpht.com");
    if !is_resizable {
        return url.to_string();
    }
    match url.rfind('=') {
        Some(pos) if url[pos + 1..].starts_with('w') || url[pos + 1..].starts_with('s') => {
            format!("{}=w{size}-h{size}-l90-rj", &url[..pos])
        }
        _ => url.to_string(),
    }
}

/// Largest thumbnail from a `{thumbnails: [...]}` object (upscaled when possible).
pub fn thumbnail_url(thumbnails: &Value) -> Option<String> {
    let arr = thumbnails["thumbnails"].as_array()?;
    let best = arr
        .iter()
        .max_by_key(|t| t["width"].as_u64().unwrap_or(0))
        .or_else(|| arr.last())?;
    let url = best["url"].as_str()?;
    let url = if url.starts_with("//") { format!("https:{url}") } else { url.to_string() };
    Some(hi_res_thumbnail(&url, THUMBNAIL_SIZE))
}

/// Thumbnail of any renderer: tries the usual wrapper shapes.
fn renderer_thumbnail(r: &Value) -> Option<String> {
    thumbnail_url(&r["thumbnail"]["musicThumbnailRenderer"]["thumbnail"])
        .or_else(|| thumbnail_url(&r["thumbnailRenderer"]["musicThumbnailRenderer"]["thumbnail"]))
        .or_else(|| thumbnail_url(&r["thumbnail"]["croppedSquareThumbnailRenderer"]["thumbnail"]))
        .or_else(|| thumbnail_url(&r["thumbnail"]))
}

fn has_explicit_badge(badges: &Value) -> bool {
    badges.as_array().is_some_and(|b| {
        b.iter().any(|x| x["musicInlineBadgeRenderer"]["icon"]["iconType"].as_str() == Some("MUSIC_EXPLICIT_BADGE"))
    })
}

// ---------------------------------------------------------------------------
// Subtitle line analysis
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct LineInfo {
    artists: Vec<ArtistRef>,
    album: Option<(String, String)>,
    duration: Option<f64>,
    year: Option<i32>,
}

fn artists_from_segment(seg: &[&Value]) -> Vec<ArtistRef> {
    seg.iter()
        .filter(|r| !is_separator(text(r)) && !is_album_run(r))
        .filter(|r| parse_duration(text(r)).is_none() && !is_counter_text(text(r)))
        .map(|r| {
            let name = text(r).trim().to_string();
            match browse_id(r) {
                Some(id) => ArtistRef::with_id(id, name),
                None => ArtistRef::new(name),
            }
        })
        .filter(|a| !a.name.is_empty())
        .collect()
}

/// Analyze subtitle segments (already split on " • ").
fn analyze_segments(segs: &[Vec<&Value>]) -> LineInfo {
    let mut info = LineInfo::default();
    for seg in segs {
        for r in seg {
            if info.album.is_none() && is_album_run(r) {
                info.album = Some((browse_id(r).unwrap_or_default().to_string(), text(r).to_string()));
            }
        }
        if seg.len() == 1 {
            let t = text(seg[0]);
            if let Some(d) = parse_duration(t) {
                info.duration = Some(d);
            } else if info.year.is_none() && seg[0]["navigationEndpoint"].is_null() {
                info.year = parse_year(t);
            }
        }
    }
    // Artists: the first segment that links to an artist/channel …
    if let Some(seg) = segs.iter().find(|s| s.iter().any(|r| is_artist_run(r))) {
        info.artists = artists_from_segment(seg);
    } else {
        // … or the first plain segment that is not a label/duration/year/counter/album.
        let plain = segs.iter().find(|seg| {
            let first = text(seg[0]);
            !(seg.len() == 1
                && (is_type_label(first)
                    || parse_duration(first).is_some()
                    || parse_year(first).is_some()
                    || is_counter_text(first)))
                && !seg.iter().any(|r| is_album_run(r))
        });
        if let Some(seg) = plain {
            info.artists = artists_from_segment(seg);
        }
    }
    info
}

// ---------------------------------------------------------------------------
// musicResponsiveListItemRenderer
// ---------------------------------------------------------------------------

fn unwrap_list_item(r: &Value) -> &Value {
    match r.get("musicResponsiveListItemRenderer") {
        Some(inner) => inner,
        None => r,
    }
}

fn flex_column(r: &Value, i: usize) -> &Value {
    let col = &r["flexColumns"][i];
    if col["musicResponsiveListItemFlexColumnRenderer"].is_null() {
        &col["musicResponsiveListItemFixedColumnRenderer"]["text"]
    } else {
        &col["musicResponsiveListItemFlexColumnRenderer"]["text"]
    }
}

fn fixed_column(r: &Value, i: usize) -> &Value {
    &r["fixedColumns"][i]["musicResponsiveListItemFixedColumnRenderer"]["text"]
}

fn list_item_video_id(r: &Value) -> Option<String> {
    r["playlistItemData"]["videoId"]
        .as_str()
        .or_else(|| {
            r["overlay"]["musicItemThumbnailOverlayRenderer"]["content"]["musicPlayButtonRenderer"]
                ["playNavigationEndpoint"]["watchEndpoint"]["videoId"]
                .as_str()
        })
        .or_else(|| {
            runs(flex_column(r, 0))
                .iter()
                .find_map(|run| run["navigationEndpoint"]["watchEndpoint"]["videoId"].as_str())
        })
        .or_else(|| r["navigationEndpoint"]["watchEndpoint"]["videoId"].as_str())
        .map(str::to_string)
}

fn list_item_video_type(r: &Value) -> Option<&str> {
    watch_music_video_type(
        &r["overlay"]["musicItemThumbnailOverlayRenderer"]["content"]["musicPlayButtonRenderer"]
            ["playNavigationEndpoint"]["watchEndpoint"],
    )
    .or_else(|| watch_music_video_type(&r["navigationEndpoint"]["watchEndpoint"]))
    .or_else(|| {
        runs(flex_column(r, 0))
            .iter()
            .find_map(|run| watch_music_video_type(&run["navigationEndpoint"]["watchEndpoint"]))
    })
}

fn is_video_type(mvt: Option<&str>) -> bool {
    mvt.is_some_and(|t| t != MVT_ATV && t != MVT_PRIVATE)
}

fn list_item_title(r: &Value) -> String {
    nav_text(flex_column(r, 0)).trim().to_string()
}

/// All subtitle segments of a list item: flex columns 1.. split on " • ".
fn list_item_segments(r: &Value) -> Vec<Vec<&Value>> {
    let n = r["flexColumns"].as_array().map_or(0, |a| a.len());
    let mut segs = Vec::new();
    for i in 1..n {
        segs.extend(split_runs(runs(flex_column(r, i))));
    }
    segs
}

/// Parse one track row (`musicResponsiveListItemRenderer`) into a [`Song`].
/// Accepts either the wrapper object or the renderer itself.
pub fn list_item_to_song(r: &Value) -> Option<Song> {
    let r = unwrap_list_item(r);
    let video_id = list_item_video_id(r)?;
    let title = list_item_title(r);
    if title.is_empty() {
        return None;
    }
    let segs = list_item_segments(r);
    let info = analyze_segments(&segs);
    let duration = info.duration.or_else(|| parse_duration(&nav_text(fixed_column(r, 0))));
    let mvt = list_item_video_type(r);

    Some(Song {
        id: video_id.clone(),
        video_id: Some(video_id),
        title,
        artists: info.artists,
        album_id: info.album.as_ref().map(|a| a.0.clone()),
        album_name: info.album.map(|a| a.1),
        duration_secs: duration,
        thumbnail_url: renderer_thumbnail(r),
        explicit: has_explicit_badge(&r["badges"]),
        is_video: is_video_type(mvt),
        year: info.year,
        ..Default::default()
    })
}

fn list_item_to_artist(r: &Value) -> Option<Artist> {
    let id = r["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str()?.to_string();
    let name = list_item_title(r);
    if name.is_empty() {
        return None;
    }
    Some(Artist {
        channel_id: id.starts_with("UC").then(|| id.clone()),
        id,
        name,
        thumbnail_url: renderer_thumbnail(r),
    })
}

fn album_playlist_id_from_overlay(overlay: &Value) -> Option<String> {
    let ep = &overlay["musicItemThumbnailOverlayRenderer"]["content"]["musicPlayButtonRenderer"]["playNavigationEndpoint"];
    ep["watchPlaylistEndpoint"]["playlistId"]
        .as_str()
        .or_else(|| ep["watchEndpoint"]["playlistId"].as_str())
        .map(str::to_string)
}

fn list_item_to_album(r: &Value) -> Option<Album> {
    let id = r["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str()?.to_string();
    let title = list_item_title(r);
    if title.is_empty() {
        return None;
    }
    let info = analyze_segments(&list_item_segments(r));
    Some(Album {
        id,
        playlist_id: album_playlist_id_from_overlay(&r["overlay"]),
        title,
        year: info.year,
        thumbnail_url: renderer_thumbnail(r),
        ..Default::default()
    })
}

/// Strip a `VL` browse prefix: `VLPL…` → `PL…`.
pub fn playlist_id_from_browse(browse_id: &str) -> String {
    browse_id.strip_prefix("VL").unwrap_or(browse_id).to_string()
}

fn song_count_from_segments(segs: &[Vec<&Value>]) -> u32 {
    segs.iter()
        .filter(|s| s.len() == 1)
        .map(|s| text(s[0]).to_lowercase())
        .find(|t| t.contains("song") || t.contains("track") || t.contains("episode"))
        .and_then(|t| leading_count(&t))
        .unwrap_or(0)
}

fn list_item_to_playlist(r: &Value) -> Option<Playlist> {
    let bid = r["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str()?;
    let name = list_item_title(r);
    if name.is_empty() {
        return None;
    }
    let segs = list_item_segments(r);
    Some(Playlist {
        id: playlist_id_from_browse(bid),
        name,
        browse_id: Some(bid.to_string()),
        thumbnail_url: renderer_thumbnail(r),
        song_count: song_count_from_segments(&segs),
        is_editable: false,
        ..Default::default()
    })
}

/// Classify and parse any `musicResponsiveListItemRenderer` (song/video,
/// album, artist or playlist). Podcast episodes / user profiles → `None`.
pub fn list_item(r: &Value) -> Option<ShelfItem> {
    let r = unwrap_list_item(r);
    let nav = &r["navigationEndpoint"];
    if nav["browseEndpoint"].is_object() {
        return match endpoint_page_type(nav) {
            Some("MUSIC_PAGE_TYPE_ARTIST" | "MUSIC_PAGE_TYPE_LIBRARY_ARTIST") => {
                list_item_to_artist(r).map(ShelfItem::Artist)
            }
            Some("MUSIC_PAGE_TYPE_ALBUM" | "MUSIC_PAGE_TYPE_AUDIOBOOK") => list_item_to_album(r).map(ShelfItem::Album),
            Some("MUSIC_PAGE_TYPE_PLAYLIST") => list_item_to_playlist(r).map(ShelfItem::Playlist),
            Some(_) => None,
            None => {
                let bid = nav["browseEndpoint"]["browseId"].as_str().unwrap_or("");
                if bid.starts_with("MPRE") {
                    list_item_to_album(r).map(ShelfItem::Album)
                } else if bid.starts_with("VL") || bid.starts_with("PL") || bid.starts_with("RD") {
                    list_item_to_playlist(r).map(ShelfItem::Playlist)
                } else {
                    None
                }
            }
        };
    }
    if list_item_video_type(r) == Some(MVT_PODCAST) {
        return None;
    }
    list_item_to_song(r).map(ShelfItem::Song)
}

// ---------------------------------------------------------------------------
// musicTwoRowItemRenderer (carousels: home, artist page, related)
// ---------------------------------------------------------------------------

/// Parse a `musicTwoRowItemRenderer` into a ShelfItem.
pub fn two_row_to_shelf_item(r: &Value) -> Option<ShelfItem> {
    let r = match r.get("musicTwoRowItemRenderer") {
        Some(inner) => inner,
        None => r,
    };
    let title = nav_text(&r["title"]).trim().to_string();
    if title.is_empty() {
        return None;
    }
    let thumb = renderer_thumbnail(r);
    let nav = &r["navigationEndpoint"];
    let segs = split_runs(runs(&r["subtitle"]));
    let info = analyze_segments(&segs);

    if let Some(vid) = nav["watchEndpoint"]["videoId"].as_str() {
        let mvt = watch_music_video_type(&nav["watchEndpoint"]);
        if mvt == Some(MVT_PODCAST) {
            return None;
        }
        // Kotlin HomePage: artists are the linked runs that are not albums.
        let mut artists: Vec<ArtistRef> = runs(&r["subtitle"])
            .iter()
            .filter(|run| browse_id(run).is_some() && !is_album_run(run))
            .map(|run| ArtistRef::with_id(browse_id(run).unwrap_or_default(), text(run).trim()))
            .collect();
        if artists.is_empty() {
            artists = info.artists;
        }
        return Some(ShelfItem::Song(Song {
            id: vid.to_string(),
            video_id: Some(vid.to_string()),
            title,
            artists,
            album_id: info.album.as_ref().map(|a| a.0.clone()),
            album_name: info.album.map(|a| a.1),
            thumbnail_url: thumb,
            explicit: has_explicit_badge(&r["subtitleBadges"]),
            is_video: is_video_type(mvt),
            ..Default::default()
        }));
    }

    let bid = nav["browseEndpoint"]["browseId"].as_str()?;
    let pt = endpoint_page_type(nav);
    let kind = match pt {
        Some("MUSIC_PAGE_TYPE_ALBUM" | "MUSIC_PAGE_TYPE_AUDIOBOOK") => "album",
        Some("MUSIC_PAGE_TYPE_ARTIST" | "MUSIC_PAGE_TYPE_LIBRARY_ARTIST") => "artist",
        Some("MUSIC_PAGE_TYPE_PLAYLIST") => "playlist",
        Some(_) => return None,
        None if bid.starts_with("MPRE") => "album",
        None if bid.starts_with("UC") => "artist",
        None => "playlist",
    };
    match kind {
        "album" => Some(ShelfItem::Album(Album {
            id: bid.to_string(),
            playlist_id: album_playlist_id_from_overlay(&r["thumbnailOverlay"]),
            title,
            year: info.year,
            thumbnail_url: thumb,
            ..Default::default()
        })),
        "artist" => Some(ShelfItem::Artist(Artist {
            id: bid.to_string(),
            name: title,
            thumbnail_url: thumb,
            channel_id: bid.starts_with("UC").then(|| bid.to_string()),
        })),
        _ => Some(ShelfItem::Playlist(Playlist {
            id: playlist_id_from_browse(bid),
            name: title,
            browse_id: Some(bid.to_string()),
            thumbnail_url: thumb,
            song_count: song_count_from_segments(&segs),
            ..Default::default()
        })),
    }
}

/// Any carousel/shelf content entry → item.
fn content_item(c: &Value) -> Option<ShelfItem> {
    if let Some(r) = c.get("musicResponsiveListItemRenderer") {
        list_item(r)
    } else if let Some(r) = c.get("musicTwoRowItemRenderer") {
        two_row_to_shelf_item(r)
    } else {
        None
    }
}

fn shelf_item_id(item: &ShelfItem) -> &str {
    match item {
        ShelfItem::Song(s) => &s.id,
        ShelfItem::Album(a) => &a.id,
        ShelfItem::Artist(a) => &a.id,
        ShelfItem::Playlist(p) => &p.id,
    }
}

/// Shelves from `sectionListRenderer.contents` (carousels and list shelves).
pub fn shelves_from_sections(sections: &[Value]) -> Vec<Shelf> {
    let mut shelves = Vec::new();
    for section in sections {
        let (title, contents) = if let Some(s) = section.get("musicCarouselShelfRenderer") {
            (nav_text(&s["header"]["musicCarouselShelfBasicHeaderRenderer"]["title"]), &s["contents"])
        } else if let Some(s) = section.get("musicImmersiveCarouselShelfRenderer") {
            (nav_text(&s["header"]["musicImmersiveCarouselShelfBasicHeaderRenderer"]["title"]), &s["contents"])
        } else if let Some(s) = section.get("musicShelfRenderer") {
            (nav_text(&s["title"]), &s["contents"])
        } else if let Some(s) = section.get("gridRenderer") {
            (nav_text(&s["header"]["gridHeaderRenderer"]["title"]), &s["items"])
        } else {
            continue;
        };
        let mut items: Vec<ShelfItem> = Vec::new();
        for c in contents.as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            if let Some(item) = content_item(c)
                && !items.iter().any(|i| shelf_item_id(i) == shelf_item_id(&item))
            {
                items.push(item);
            }
        }
        if !items.is_empty() {
            shelves.push(Shelf { title, items });
        }
    }
    shelves
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

fn card_item(card: &Value) -> Option<ShelfItem> {
    let title = nav_text(&card["title"]).trim().to_string();
    if title.is_empty() {
        return None;
    }
    let thumb = renderer_thumbnail(card);
    let on_tap = &card["onTap"];
    let segs = split_runs(runs(&card["subtitle"]));
    let info = analyze_segments(&segs);
    if let Some(vid) = on_tap["watchEndpoint"]["videoId"].as_str() {
        let mvt = watch_music_video_type(&on_tap["watchEndpoint"]);
        if mvt == Some(MVT_PODCAST) {
            return None;
        }
        return Some(ShelfItem::Song(Song {
            id: vid.to_string(),
            video_id: Some(vid.to_string()),
            title,
            artists: info.artists,
            album_id: info.album.as_ref().map(|a| a.0.clone()),
            album_name: info.album.map(|a| a.1),
            duration_secs: info.duration,
            thumbnail_url: thumb,
            explicit: has_explicit_badge(&card["subtitleBadges"]),
            is_video: is_video_type(mvt),
            ..Default::default()
        }));
    }
    let bid = on_tap["browseEndpoint"]["browseId"].as_str()?;
    match endpoint_page_type(on_tap) {
        Some("MUSIC_PAGE_TYPE_ARTIST") => Some(ShelfItem::Artist(Artist {
            id: bid.to_string(),
            name: title,
            thumbnail_url: thumb,
            channel_id: bid.starts_with("UC").then(|| bid.to_string()),
        })),
        Some("MUSIC_PAGE_TYPE_ALBUM" | "MUSIC_PAGE_TYPE_AUDIOBOOK") => Some(ShelfItem::Album(Album {
            id: bid.to_string(),
            title,
            year: info.year,
            thumbnail_url: thumb,
            playlist_id: card["buttons"][0]["buttonRenderer"]["command"]["watchPlaylistEndpoint"]["playlistId"]
                .as_str()
                .map(str::to_string),
            ..Default::default()
        })),
        Some("MUSIC_PAGE_TYPE_PLAYLIST") => Some(ShelfItem::Playlist(Playlist {
            id: playlist_id_from_browse(bid),
            name: title,
            browse_id: Some(bid.to_string()),
            thumbnail_url: thumb,
            ..Default::default()
        })),
        _ => None,
    }
}

fn push_result(results: &mut SearchResults, item: ShelfItem) {
    match item {
        ShelfItem::Song(s) => {
            let list = if s.is_video { &mut results.videos } else { &mut results.songs };
            if !list.iter().any(|x| x.id == s.id) {
                list.push(s);
            }
        }
        ShelfItem::Album(a) => {
            if !results.albums.iter().any(|x| x.id == a.id) {
                results.albums.push(a);
            }
        }
        ShelfItem::Artist(a) => {
            if !results.artists.iter().any(|x| x.id == a.id) {
                results.artists.push(a);
            }
        }
        ShelfItem::Playlist(p) => {
            if !results.playlists.iter().any(|x| x.id == p.id) {
                results.playlists.push(p);
            }
        }
    }
}

/// Sections of a search response (tabbed, plain, or continuation).
fn search_sections(json: &Value) -> &[Value] {
    let tabbed = &json["contents"]["tabbedSearchResultsRenderer"]["tabs"][0]["tabRenderer"]["content"]
        ["sectionListRenderer"]["contents"];
    if let Some(a) = tabbed.as_array() {
        return a;
    }
    if let Some(a) = json["contents"]["sectionListRenderer"]["contents"].as_array() {
        return a;
    }
    &[]
}

/// Parse a search response — both the unfiltered "summary" layout (top-result
/// card + one `itemSectionRenderer` per row, type taken from the endpoint)
/// and filtered layouts (`musicShelfRenderer`). Songs vs videos are split by
/// `musicVideoType` (ATV = song).
pub fn search_response(json: &Value) -> SearchResults {
    let mut results = SearchResults::default();
    let mut rows: Vec<&Value> = Vec::new();
    for section in search_sections(json) {
        if let Some(card) = section.get("musicCardShelfRenderer") {
            if let Some(item) = card_item(card) {
                push_result(&mut results, item);
            }
            rows.extend(card["contents"].as_array().map(|a| a.iter()).into_iter().flatten());
        }
        if let Some(shelf) = section.get("musicShelfRenderer") {
            rows.extend(shelf["contents"].as_array().map(|a| a.iter()).into_iter().flatten());
        }
        if let Some(sec) = section.get("itemSectionRenderer") {
            rows.extend(sec["contents"].as_array().map(|a| a.iter()).into_iter().flatten());
        }
    }
    // Continuation pages of a filtered search.
    if let Some(a) = json["continuationContents"]["musicShelfContinuation"]["contents"].as_array() {
        rows.extend(a.iter());
    }
    for row in rows {
        if let Some(r) = row.get("musicResponsiveListItemRenderer")
            && let Some(item) = list_item(r)
        {
            push_result(&mut results, item);
        }
    }
    results
}

/// Continuation token of a filtered search's song shelf.
pub fn search_continuation(json: &Value) -> Option<String> {
    for section in search_sections(json) {
        if let Some(c) = section["musicShelfRenderer"]["continuations"][0]["nextContinuationData"]["continuation"].as_str() {
            return Some(c.to_string());
        }
    }
    json["continuationContents"]["musicShelfContinuation"]["continuations"][0]["nextContinuationData"]["continuation"]
        .as_str()
        .map(str::to_string)
}

/// `music/get_search_suggestions` → query strings (section 0).
pub fn suggestions_response(json: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for section in json["contents"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        for item in section["searchSuggestionsSectionRenderer"]["contents"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or(&[])
        {
            let r = &item["searchSuggestionRenderer"];
            if r.is_null() {
                continue;
            }
            let t = nav_text(&r["suggestion"]);
            let t = if t.is_empty() {
                r["navigationEndpoint"]["searchEndpoint"]["query"].as_str().unwrap_or("").to_string()
            } else {
                t
            };
            if !t.is_empty() && !out.contains(&t) {
                out.push(t);
            }
        }
    }
    out
}

/// Recommended items shown under the suggestions (artists, songs…).
pub fn suggestion_items(json: &Value) -> Vec<ShelfItem> {
    let mut out = Vec::new();
    for section in json["contents"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        for item in section["searchSuggestionsSectionRenderer"]["contents"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or(&[])
        {
            if let Some(i) = content_item(item) {
                out.push(i);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Browse pages
// ---------------------------------------------------------------------------

fn service_browse_id(json: &Value) -> Option<String> {
    json["responseContext"]["serviceTrackingParams"].as_array()?.iter().find_map(|p| {
        p["params"].as_array()?.iter().find_map(|kv| {
            (kv["key"].as_str() == Some("browse_id"))
                .then(|| kv["value"].as_str().map(str::to_string))
                .flatten()
        })
    })
}

fn first_tab_sections(json: &Value) -> &[Value] {
    let two = &json["contents"]["twoColumnBrowseResultsRenderer"]["tabs"][0]["tabRenderer"]["content"]
        ["sectionListRenderer"]["contents"];
    if let Some(a) = two.as_array() {
        return a;
    }
    json["contents"]["singleColumnBrowseResultsRenderer"]["tabs"][0]["tabRenderer"]["content"]
        ["sectionListRenderer"]["contents"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}

fn secondary_sections(json: &Value) -> &[Value] {
    json["contents"]["twoColumnBrowseResultsRenderer"]["secondaryContents"]["sectionListRenderer"]["contents"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}

/// The detail header of an album/playlist page in any known layout.
/// Returns (header, editable).
fn detail_header(json: &Value) -> (&Value, bool) {
    for section in first_tab_sections(json) {
        if section["musicResponsiveHeaderRenderer"].is_object() {
            return (&section["musicResponsiveHeaderRenderer"], false);
        }
        let ed = &section["musicEditablePlaylistDetailHeaderRenderer"]["header"];
        if ed["musicResponsiveHeaderRenderer"].is_object() {
            return (&ed["musicResponsiveHeaderRenderer"], true);
        }
    }
    let h = &json["header"];
    if h["musicDetailHeaderRenderer"].is_object() {
        return (&h["musicDetailHeaderRenderer"], false);
    }
    let ed = &h["musicEditablePlaylistDetailHeaderRenderer"]["header"];
    for k in ["musicDetailHeaderRenderer", "musicResponsiveHeaderRenderer"] {
        if ed[k].is_object() {
            return (&ed[k], true);
        }
    }
    if h["musicImmersiveHeaderRenderer"].is_object() {
        return (&h["musicImmersiveHeaderRenderer"], false);
    }
    (&Value::Null, false)
}

fn header_description(h: &Value) -> Option<String> {
    let d = &h["description"];
    let t = nav_text(&d["musicDescriptionShelfRenderer"]["description"]);
    let t = if t.is_empty() { nav_text(d) } else { t };
    let t = t.trim().to_string();
    (!t.is_empty()).then_some(t)
}

/// Rows of every track shelf on a browse page (album / playlist / legacy layouts).
fn track_rows(json: &Value) -> Vec<&Value> {
    let mut rows = Vec::new();
    for section in secondary_sections(json).iter().chain(first_tab_sections(json)) {
        for key in ["musicShelfRenderer", "musicPlaylistShelfRenderer"] {
            if let Some(a) = section[key]["contents"].as_array() {
                rows.extend(a.iter());
            }
        }
    }
    rows
}

fn query_param<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then_some(v)
    })
}

/// Parse an album browse response. Track rows carry no artist/thumbnail of
/// their own; they inherit the album's (`straplineTextOne`, header art).
pub fn album_response(json: &Value) -> (Album, Vec<Song>) {
    let (h, _) = detail_header(json);
    let title = nav_text(&h["title"]);
    let thumbnail = renderer_thumbnail(h)
        .or_else(|| thumbnail_url(&json["background"]["musicThumbnailRenderer"]["thumbnail"]));

    let sub_segs = split_runs(runs(&h["subtitle"]));
    let year = sub_segs.iter().filter(|s| s.len() == 1).find_map(|s| parse_year(text(s[0])));

    let mut album_artists: Vec<ArtistRef> = runs(&h["straplineTextOne"])
        .iter()
        .filter(|r| !is_separator(text(r)))
        .map(|r| match browse_id(r) {
            Some(id) => ArtistRef::with_id(id, text(r).trim()),
            None => ArtistRef::new(text(r).trim()),
        })
        .collect();
    if album_artists.is_empty() {
        album_artists = analyze_segments(&sub_segs).artists;
    }

    let second = split_runs(runs(&h["secondSubtitle"]));
    let song_count = second.iter().find_map(|s| {
        let t = text(s[0]).to_lowercase();
        (t.contains("song") || t.contains("track")).then(|| leading_count(&t)).flatten()
    });
    let text_duration = second.iter().find_map(|s| parse_long_duration(text(s[0])));

    let playlist_id = h["buttons"]
        .as_array()
        .and_then(|b| {
            b.iter().find_map(|x| {
                x["musicPlayButtonRenderer"]["playNavigationEndpoint"]["watchPlaylistEndpoint"]["playlistId"].as_str()
            })
        })
        .map(str::to_string)
        .or_else(|| {
            json["microformat"]["microformatDataRenderer"]["urlCanonical"]
                .as_str()
                .and_then(|u| query_param(u, "list"))
                .map(str::to_string)
        });

    let id = service_browse_id(json).unwrap_or_default();

    let mut songs: Vec<Song> = Vec::new();
    for row in track_rows(json) {
        if let Some(mut song) = row.get("musicResponsiveListItemRenderer").and_then(list_item_to_song) {
            if song.artists.is_empty() {
                song.artists = album_artists.clone();
            }
            if song.thumbnail_url.is_none() {
                song.thumbnail_url = thumbnail.clone();
            }
            song.album_id = Some(id.clone());
            song.album_name = Some(title.clone());
            if song.year.is_none() {
                song.year = year;
            }
            if !songs.iter().any(|s| s.id == song.id) {
                songs.push(song);
            }
        }
    }

    let summed: f64 = songs.iter().filter_map(|s| s.duration_secs).sum();
    let album = Album {
        id,
        playlist_id,
        title,
        year,
        thumbnail_url: thumbnail,
        song_count: song_count.or((!songs.is_empty()).then_some(songs.len() as u32)),
        duration_secs: text_duration.or((summed > 0.0).then_some(summed)),
        description: header_description(h),
    };
    (album, songs)
}

/// Album artists from an album page header (Kotlin `AlbumPage.artists`).
/// `Album` in mochy-core has no artist field, so this is a separate helper.
pub fn album_artists(json: &Value) -> Vec<ArtistRef> {
    let (h, _) = detail_header(json);
    runs(&h["straplineTextOne"])
        .iter()
        .filter(|r| !is_separator(text(r)))
        .map(|r| match browse_id(r) {
            Some(id) => ArtistRef::with_id(id, text(r).trim()),
            None => ArtistRef::new(text(r).trim()),
        })
        .collect()
}

/// Parse an artist browse response: header + shelves (top songs, albums,
/// singles, videos, playlists, similar artists…).
pub fn artist_response(json: &Value) -> (Artist, Vec<Shelf>) {
    let h = ["musicImmersiveHeaderRenderer", "musicVisualHeaderRenderer", "musicHeaderRenderer"]
        .iter()
        .map(|k| &json["header"][*k])
        .find(|v| v.is_object())
        .unwrap_or(&Value::Null);

    let name = nav_text(&h["title"]);
    let thumbnail = renderer_thumbnail(h).or_else(|| thumbnail_url(&h["foregroundThumbnail"]["musicThumbnailRenderer"]["thumbnail"]));
    let channel_id = h["subscriptionButton"]["subscribeButtonRenderer"]["channelId"]
        .as_str()
        .map(str::to_string);
    let id = service_browse_id(json).unwrap_or_default();

    let mut shelves = shelves_from_sections(first_tab_sections(json));
    // Top songs rows omit the artist when it is the page's artist.
    for shelf in &mut shelves {
        for item in &mut shelf.items {
            if let ShelfItem::Song(s) = item
                && s.artists.is_empty()
                && !name.is_empty()
            {
                s.artists.push(ArtistRef::with_id(id.clone(), name.clone()));
            }
        }
    }
    let artist = Artist { channel_id: channel_id.or_else(|| id.starts_with("UC").then(|| id.clone())), id, name, thumbnail_url: thumbnail };
    (artist, shelves)
}

/// Parse a playlist browse response (first page of tracks).
pub fn playlist_response(json: &Value) -> (Playlist, Vec<Song>) {
    let (h, editable) = detail_header(json);
    let name = nav_text(&h["title"]);
    let thumbnail = renderer_thumbnail(h)
        .or_else(|| thumbnail_url(&json["background"]["musicThumbnailRenderer"]["thumbnail"]));
    let second = split_runs(runs(&h["secondSubtitle"]));
    let sub = split_runs(runs(&h["subtitle"]));
    let song_count = song_count_from_segments(&second).max(song_count_from_segments(&sub));

    let browse = service_browse_id(json);
    let id = browse.as_deref().map(playlist_id_from_browse).unwrap_or_default();

    let mut songs = Vec::new();
    for row in track_rows(json) {
        if let Some(song) = row.get("musicResponsiveListItemRenderer").and_then(list_item_to_song)
            && !songs.iter().any(|s: &Song| s.id == song.id)
        {
            songs.push(song);
        }
    }

    let playlist = Playlist {
        id,
        name,
        browse_id: browse,
        thumbnail_url: thumbnail,
        song_count: if song_count > 0 { song_count } else { songs.len() as u32 },
        description: header_description(h),
        is_editable: editable,
        created_at: 0,
    };
    (playlist, songs)
}

fn continuation_item_token(items: &[Value]) -> Option<String> {
    items.iter().rev().find_map(|c| {
        c["continuationItemRenderer"]["continuationEndpoint"]["continuationCommand"]["token"]
            .as_str()
            .map(str::to_string)
    })
}

/// Continuation token for the next page of playlist tracks, if any.
pub fn playlist_continuation_token(json: &Value) -> Option<String> {
    for section in secondary_sections(json).iter().chain(first_tab_sections(json)) {
        let shelf = &section["musicPlaylistShelfRenderer"];
        if let Some(t) = shelf["contents"].as_array().and_then(|a| continuation_item_token(a)) {
            return Some(t);
        }
        if let Some(t) = shelf["continuations"][0]["nextContinuationData"]["continuation"].as_str() {
            return Some(t.to_string());
        }
    }
    None
}

/// Parse a playlist continuation response → (songs, next token).
pub fn playlist_continuation(json: &Value) -> (Vec<Song>, Option<String>) {
    let mut items: Vec<&Value> = Vec::new();
    let mut next = None;
    for action in json["onResponseReceivedActions"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        if let Some(a) = action["appendContinuationItemsAction"]["continuationItems"].as_array() {
            items.extend(a.iter());
            next = next.or_else(|| continuation_item_token(a));
        }
    }
    let shelf = &json["continuationContents"]["musicPlaylistShelfContinuation"];
    if let Some(a) = shelf["contents"].as_array() {
        items.extend(a.iter());
        next = next
            .or_else(|| continuation_item_token(a))
            .or_else(|| shelf["continuations"][0]["nextContinuationData"]["continuation"].as_str().map(str::to_string));
    }
    let songs: Vec<Song> = items
        .into_iter()
        .filter_map(|r| r.get("musicResponsiveListItemRenderer").and_then(list_item_to_song))
        .collect();
    if songs.is_empty() {
        next = None;
    }
    (songs, next)
}

/// Home feed page: (shelves, continuation token). Works for the first page
/// (`FEmusic_home`) and for `sectionListContinuation` pages.
pub fn home_response(json: &Value) -> (Vec<Shelf>, Option<String>) {
    let first = &json["contents"]["singleColumnBrowseResultsRenderer"]["tabs"][0]["tabRenderer"]["content"]["sectionListRenderer"];
    let list = if first.is_object() { first } else { &json["continuationContents"]["sectionListContinuation"] };
    let sections = list["contents"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    let cont = list["continuations"][0]["nextContinuationData"]["continuation"]
        .as_str()
        .map(str::to_string);
    (shelves_from_sections(sections), cont)
}

// ---------------------------------------------------------------------------
// next (queue / radio / related / lyrics)
// ---------------------------------------------------------------------------

fn watch_next_tabs(json: &Value) -> &[Value] {
    json["contents"]["singleColumnMusicWatchNextResultsRenderer"]["tabbedRenderer"]["watchNextTabbedResultsRenderer"]["tabs"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}

fn playlist_panel(json: &Value) -> &Value {
    let cont = &json["continuationContents"]["playlistPanelContinuation"];
    if cont.is_object() {
        return cont;
    }
    watch_next_tabs(json)
        .first()
        .map(|t| &t["tabRenderer"]["content"]["musicQueueRenderer"]["content"]["playlistPanelRenderer"])
        .unwrap_or(&Value::Null)
}

/// `playlistPanelVideoRenderer`s of a `next` response, unwrapping the
/// song/video counterpart wrapper (`playlistPanelVideoWrapperRenderer`).
pub fn panel_videos(json: &Value) -> Vec<&Value> {
    playlist_panel(json)["contents"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or(&[])
        .iter()
        .filter_map(|c| {
            c.get("playlistPanelVideoRenderer").or_else(|| {
                c["playlistPanelVideoWrapperRenderer"]["primaryRenderer"].get("playlistPanelVideoRenderer")
            })
        })
        .collect()
}

/// The automix (radio) endpoint attached to a `next` panel: (playlistId, params).
pub fn next_automix_endpoint(json: &Value) -> Option<(String, Option<String>)> {
    let contents = playlist_panel(json)["contents"].as_array()?;
    contents.iter().rev().find_map(|c| {
        let ep = &c["automixPreviewVideoRenderer"]["content"]["automixPlaylistVideoRenderer"]["navigationEndpoint"]
            ["watchPlaylistEndpoint"];
        let pid = ep["playlistId"].as_str()?;
        Some((pid.to_string(), ep["params"].as_str().map(str::to_string)))
    })
}

fn tab_browse_id<'a>(json: &'a Value, prefix: &str, fallback_index: usize) -> Option<&'a str> {
    let tabs = watch_next_tabs(json);
    tabs.iter()
        .filter_map(|t| t["tabRenderer"]["endpoint"]["browseEndpoint"]["browseId"].as_str())
        .find(|b| b.starts_with(prefix))
        .or_else(|| tabs.get(fallback_index)?["tabRenderer"]["endpoint"]["browseEndpoint"]["browseId"].as_str())
}

/// Browse id of the "Related" tab (`MPTR…`).
pub fn next_related_browse_id(json: &Value) -> Option<String> {
    tab_browse_id(json, "MPTR", 2).map(str::to_string)
}

/// Browse id of the "Lyrics" tab (`MPLY…`), when the track has YTM lyrics.
pub fn next_lyrics_browse_id(json: &Value) -> Option<String> {
    tab_browse_id(json, "MPLY", 1).filter(|b| b.starts_with("MPLY")).map(str::to_string)
}

/// Plain lyrics text of a lyrics browse response (`musicDescriptionShelfRenderer`).
pub fn lyrics_response(json: &Value) -> Option<String> {
    let sections = json["contents"]["sectionListRenderer"]["contents"].as_array()?;
    sections.iter().find_map(|s| {
        let t = nav_text(&s["musicDescriptionShelfRenderer"]["description"]);
        (!t.is_empty()).then_some(t)
    })
}

struct PanelInfo {
    video_id: String,
    title: String,
    artists: Vec<ArtistRef>,
    album: Option<(String, String)>,
    year: Option<i32>,
    duration: Option<f64>,
    thumbnail: Option<String>,
    explicit: bool,
    is_video: bool,
}

fn panel_info(r: &Value) -> Option<PanelInfo> {
    let video_id = r["videoId"]
        .as_str()
        .or_else(|| r["navigationEndpoint"]["watchEndpoint"]["videoId"].as_str())?
        .to_string();
    let title = nav_text(&r["title"]).trim().to_string();
    if title.is_empty() {
        return None;
    }
    let byline = if r["longBylineText"].is_object() { &r["longBylineText"] } else { &r["shortBylineText"] };
    let segs = split_runs(runs(byline));
    // NextPage.kt: segment 0 = artists, 1 = album (when linked), 2 = year.
    let artists = segs.first().map(|s| artists_from_segment(s)).unwrap_or_default();
    let mut info = analyze_segments(&segs);
    if !artists.is_empty() {
        info.artists = artists;
    }
    let duration = parse_duration(&nav_text(&r["lengthText"]));
    Some(PanelInfo {
        video_id,
        title,
        artists: info.artists,
        album: info.album,
        year: info.year,
        duration,
        thumbnail: thumbnail_url(&r["thumbnail"]),
        explicit: has_explicit_badge(&r["badges"]),
        is_video: is_video_type(watch_music_video_type(&r["navigationEndpoint"]["watchEndpoint"])),
    })
}

/// Convert a playlistPanelVideoRenderer to a Song.
pub fn panel_video_to_song(r: &Value) -> Option<Song> {
    let p = panel_info(r)?;
    Some(Song {
        id: p.video_id.clone(),
        video_id: Some(p.video_id),
        title: p.title,
        artists: p.artists,
        album_id: p.album.as_ref().map(|a| a.0.clone()),
        album_name: p.album.map(|a| a.1),
        duration_secs: p.duration,
        thumbnail_url: p.thumbnail,
        explicit: p.explicit,
        is_video: p.is_video,
        year: p.year,
        ..Default::default()
    })
}

/// Convert a playlistPanelVideoRenderer to a QueueItem.
pub fn panel_video_to_queue_item(r: &Value) -> Option<QueueItem> {
    let set_video_id = r["playlistSetVideoId"].as_str().map(str::to_string);
    let playlist_id = r["navigationEndpoint"]["watchEndpoint"]["playlistId"].as_str().map(str::to_string);
    let p = panel_info(r)?;
    Some(QueueItem {
        video_id: p.video_id,
        playlist_id,
        set_video_id,
        title: p.title,
        artists: p.artists,
        duration_secs: p.duration,
        thumbnail_url: p.thumbnail,
    })
}

/// Parse a `next` response into its queue items (no network follow-ups).
pub fn queue_from_next(json: &Value) -> Vec<QueueItem> {
    let mut items: Vec<QueueItem> = Vec::new();
    for r in panel_videos(json) {
        if let Some(q) = panel_video_to_queue_item(r)
            && !items.iter().any(|i| i.video_id == q.video_id)
        {
            items.push(q);
        }
    }
    items
}

/// Songs of a `next` response's panel.
pub fn songs_from_next(json: &Value) -> Vec<Song> {
    panel_videos(json).into_iter().filter_map(panel_video_to_song).collect()
}

/// Parse the related-tab browse response (`MPTR…`) into songs. Like
/// `YouTube.related` in Kotlin, audio tracks (ATV) are preferred; videos are
/// returned only when there are no audio tracks at all.
pub fn related_response(json: &Value) -> Vec<Song> {
    let sections = json["contents"]["sectionListRenderer"]["contents"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or(&[]);
    let mut all: Vec<Song> = Vec::new();
    for shelf in shelves_from_sections(sections) {
        for item in shelf.items {
            if let ShelfItem::Song(s) = item
                && !all.iter().any(|x| x.id == s.id)
            {
                all.push(s);
            }
        }
    }
    if all.iter().any(|s| !s.is_video) {
        all.retain(|s| !s.is_video);
    }
    all
}

// ---------------------------------------------------------------------------
// player
// ---------------------------------------------------------------------------

/// `(status, reason)` of a player response's `playabilityStatus`.
pub fn playability(player_json: &Value) -> (String, String) {
    let ps = &player_json["playabilityStatus"];
    (
        ps["status"].as_str().unwrap_or("UNKNOWN").to_string(),
        ps["reason"].as_str().map(str::to_string).unwrap_or_else(|| nav_text(&ps["errorScreen"]["playerErrorMessageRenderer"]["reason"])),
    )
}

/// "Sign in to confirm you're not a bot".
pub fn is_bot_check(player_json: &Value) -> bool {
    let (status, reason) = playability(player_json);
    status == "LOGIN_REQUIRED" && reason.to_lowercase().contains("bot")
}

/// Premium-only refusal ("only available to Music Premium members").
pub fn is_premium_only(player_json: &Value) -> bool {
    let (status, reason) = playability(player_json);
    status != "OK" && reason.to_lowercase().contains("premium")
}

/// `expire=` (unix seconds) of a googlevideo URL.
pub fn expire_from_url(url: &str) -> Option<u64> {
    query_param(url, "expire")?.parse().ok()
}

fn codecs_of(mime: &str) -> Option<String> {
    let start = mime.find("codecs=\"")? + 8;
    let end = mime[start..].find('"')? + start;
    Some(mime[start..end].to_string())
}

fn audio_format_of(mime: &str) -> AudioFormat {
    let m = mime.to_lowercase();
    if m.contains("mp4a") || m.starts_with("audio/mp4") {
        AudioFormat::AacM4a
    } else if m.contains("opus") {
        AudioFormat::OpusWebm
    } else if m.contains("mpeg") || m.contains("mp3") {
        AudioFormat::Mp3
    } else if m.contains("flac") {
        AudioFormat::Flac
    } else {
        AudioFormat::Unknown
    }
}

/// Loudness (dB above the −14 LKFS reference) for one format: per-format
/// `loudnessDb`, else `audioConfig.loudnessDb`, else derived from the
/// track's absolute/perceptual loudness. Normalization gain = `-loudness_db`.
fn loudness_for(player_json: &Value, fmt: &Value) -> Option<f32> {
    let ac = &player_json["playerConfig"]["audioConfig"];
    let target = ac["loudnessTargetLkfs"].as_f64().unwrap_or(-14.0);
    fmt["loudnessDb"]
        .as_f64()
        .or_else(|| ac["loudnessDb"].as_f64())
        .or_else(|| fmt["trackAbsoluteLoudnessLkfs"].as_f64().map(|a| a - target))
        .or_else(|| ac["trackAbsoluteLoudnessLkfs"].as_f64().map(|a| a - target))
        .or_else(|| ac["perceptualLoudnessDb"].as_f64().map(|a| a - target))
        .map(|v| v as f32)
}

#[derive(Clone)]
struct RawFormat {
    cand: StreamCandidate,
    quality_rank: u8,
    channels: u32,
    bitrate: u64,
    codec_rank: u8,
    is_original: bool,
}

/// Every audio format of a player response that has a **direct** `url`
/// (ciphered ones need the JS player and are skipped), ordered best-first
/// for `quality` and `pref`.
///
/// Port of `YTPlayerUtils.selectAudioFormatCandidates`: original (non
/// auto-dubbed) tracks first; Auto/Lossless/… ⇒ High; Low targets ≤70 kbps,
/// Medium ≤135 kbps (best at/below target, then cheapest above), High = best.
/// Unlike Kotlin (opus > mp4a) the codec family is chosen first by `pref`:
/// `PreferAac` puts every AAC rung before any Opus one.
pub fn audio_candidates(player_json: &Value, quality: AudioQuality, pref: CodecPreference) -> Vec<StreamCandidate> {
    let Some(formats) = player_json["streamingData"]["adaptiveFormats"].as_array() else {
        return Vec::new();
    };
    let mut raw: Vec<RawFormat> = Vec::new();
    for fmt in formats {
        let mime = fmt["mimeType"].as_str().unwrap_or("");
        let bitrate = fmt["bitrate"].as_u64().unwrap_or(0);
        if !mime.starts_with("audio/") || bitrate == 0 {
            continue;
        }
        let Some(url) = fmt["url"].as_str().filter(|u| !u.is_empty()) else { continue };
        let is_original = fmt["audioTrack"]["isAutoDubbed"].is_null();
        let format = audio_format_of(mime);
        let avg = fmt["averageBitrate"].as_u64().unwrap_or(bitrate);
        let quality_rank = match fmt["audioQuality"].as_str() {
            Some("AUDIO_QUALITY_HIGH") => 3,
            Some("AUDIO_QUALITY_MEDIUM") => 2,
            Some("AUDIO_QUALITY_LOW") => 1,
            _ => 0,
        };
        let codecs = codecs_of(mime);
        let codec_rank = match codecs.as_deref() {
            Some(c) if c.contains("opus") => 3,
            Some(c) if c.contains("mp4a") => 2,
            Some(_) => 1,
            None => 0,
        };
        raw.push(RawFormat {
            cand: StreamCandidate {
                url: url.to_string(),
                format,
                itag: fmt["itag"].as_u64().map(|v| v as u32),
                mime_type: mime.to_string(),
                codecs,
                bitrate_kbps: Some((avg / 1000) as u32).filter(|v| *v > 0),
                sample_rate_hz: fmt["audioSampleRate"].as_str().and_then(|s| s.parse().ok()),
                channels: fmt["audioChannels"].as_u64().map(|v| v as u32),
                content_length: fmt["contentLength"].as_str().and_then(|s| s.parse().ok()),
                approx_duration_ms: fmt["approxDurationMs"].as_str().and_then(|s| s.parse().ok()),
                loudness_db: loudness_for(player_json, fmt),
                audio_quality: fmt["audioQuality"].as_str().map(str::to_string),
            },
            quality_rank,
            channels: fmt["audioChannels"].as_u64().unwrap_or(2) as u32,
            bitrate,
            codec_rank,
            is_original,
        });
    }
    if raw.iter().any(|r| r.is_original) {
        raw.retain(|r| r.is_original);
    }

    let target: Option<u64> = match quality {
        AudioQuality::Low => Some(70_000),
        AudioQuality::Medium => Some(135_000),
        _ => None,
    };
    let higher = |a: &RawFormat, b: &RawFormat| {
        b.quality_rank
            .cmp(&a.quality_rank)
            .then(b.channels.cmp(&a.channels))
            .then(b.bitrate.cmp(&a.bitrate))
            .then(b.codec_rank.cmp(&a.codec_rank))
            .then(b.cand.sample_rate_hz.cmp(&a.cand.sample_rate_hz))
    };
    let lower_above = |a: &RawFormat, b: &RawFormat| {
        a.bitrate
            .cmp(&b.bitrate)
            .then(b.codec_rank.cmp(&a.codec_rank))
            .then(b.cand.sample_rate_hz.cmp(&a.cand.sample_rate_hz))
    };
    let order = |mut group: Vec<RawFormat>| -> Vec<StreamCandidate> {
        match target {
            None => {
                group.sort_by(higher);
                group.into_iter().map(|r| r.cand).collect()
            }
            Some(t) => {
                let (mut below, mut above): (Vec<_>, Vec<_>) = group.into_iter().partition(|r| r.bitrate <= t);
                below.sort_by(higher);
                above.sort_by(lower_above);
                below.into_iter().chain(above).map(|r| r.cand).collect()
            }
        }
    };

    let family = |f: AudioFormat| -> u8 {
        match (pref, f) {
            (CodecPreference::PreferOpus, AudioFormat::OpusWebm) => 0,
            (CodecPreference::PreferOpus, AudioFormat::AacM4a) => 1,
            (_, AudioFormat::AacM4a) => 0,
            (_, AudioFormat::OpusWebm) => 1,
            (_, AudioFormat::Mp3 | AudioFormat::Flac) => 2,
            (_, AudioFormat::Unknown) => 3,
        }
    };
    let mut out = Vec::new();
    for fam in 0..=3u8 {
        let group: Vec<RawFormat> = raw.iter().filter(|r| family(r.cand.format) == fam).cloned().collect();
        out.extend(order(group));
    }
    if pref == CodecPreference::AacOnly {
        out.retain(|c| c.format == AudioFormat::AacM4a);
    }
    out
}

/// Expiry of a player response's URLs (unix seconds): `expire=` of the URL,
/// else now + `streamingData.expiresInSeconds`.
pub fn stream_expiry(player_json: &Value, url: &str) -> Option<u64> {
    expire_from_url(url).or_else(|| {
        let secs: u64 = player_json["streamingData"]["expiresInSeconds"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .or_else(|| player_json["streamingData"]["expiresInSeconds"].as_u64())?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
        Some(now + secs)
    })
}

/// Pick the best playable `adaptiveFormats` entry (Auto quality, AAC first).
/// Unvalidated — `YtmClient::resolve_stream` probes the URL before use.
pub fn pick_audio_format(player_json: &Value) -> Option<StreamResult> {
    let mut cands = audio_candidates(player_json, AudioQuality::Auto, CodecPreference::PreferAac);
    if cands.is_empty() {
        return None;
    }
    let best = cands.remove(0);
    let expires = stream_expiry(player_json, &best.url);
    Some(StreamResult::from_candidate(best, cands, Vec::new(), String::new(), expires))
}
