//! Reads a public playlist or album from a share link without logging in
//! (Android `LinkImporter.kt`): Spotify's embed page, Apple Music's
//! serialized server data, Deezer's public API, and YouTube Music through our
//! own InnerTube client. Private playlists go through the CSV route.

use std::sync::LazyLock;
use std::time::Duration;

use mochy_innertube::YtmClient;
use regex::Regex;
use serde_json::Value;

use crate::json::{array, long, path, string};
use crate::models::{ImportError, ImportResult, ImportSource, ImportedTrack, ParsedImport, PlaylistOrigin};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkSource {
    Spotify,
    AppleMusic,
    Deezer,
    Youtube,
}

impl LinkSource {
    pub fn label(self) -> &'static str {
        match self {
            LinkSource::Spotify => "Spotify",
            LinkSource::AppleMusic => "Apple Music",
            LinkSource::Deezer => "Deezer",
            LinkSource::Youtube => "YouTube Music",
        }
    }
    pub fn example(self) -> &'static str {
        match self {
            LinkSource::Spotify => "https://open.spotify.com/playlist/…",
            LinkSource::AppleMusic => "https://music.apple.com/us/playlist/…",
            LinkSource::Deezer => "https://www.deezer.com/playlist/…",
            LinkSource::Youtube => "https://music.youtube.com/playlist?list=…",
        }
    }
    pub fn import_source(self) -> ImportSource {
        match self {
            LinkSource::Spotify => ImportSource::Spotify,
            LinkSource::AppleMusic => ImportSource::AppleMusic,
            LinkSource::Deezer => ImportSource::Deezer,
            LinkSource::Youtube => ImportSource::YoutubeMusic,
        }
    }
}

const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 13) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36";
/// Spotify only shares the first 100 songs of a playlist through its embed page.
pub const SPOTIFY_EMBED_LIMIT: usize = 100;

static SPOTIFY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:open\.spotify\.com/(?:intl-[a-zA-Z-]+/)?(?:embed/)?|spotify:)(playlist|album)[/:]([A-Za-z0-9]{22})").unwrap()
});
static APPLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"music\.apple\.com/[a-z]{2}/(playlist|album)/(?:[^/?#]+/)?(pl\.[A-Za-z0-9-]+|\d+)").unwrap());
static DEEZER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"deezer\.com/(?:[a-z]{2}/)?(playlist|album)/(\d+)").unwrap());
static YT_LIST_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[?&]list=([A-Za-z0-9_-]+)").unwrap());
static YT_BROWSE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"music\.youtube\.com/(?:browse|playlist)/((?:VL|MPREb_)?[A-Za-z0-9_-]+)").unwrap());
static URL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://\S+").unwrap());
static NEXT_DATA_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<script id="__NEXT_DATA__" type="application/json">(.*?)</script>"#).unwrap());
static APPLE_DATA_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<script[^>]*id="serialized-server-data"[^>]*>(.*?)</script>"#).unwrap());
const SHORT_LINK_HOSTS: [&str; 4] = ["spotify.link", "spotify.app.link", "deezer.page.link", "link.deezer.com"];

/// Which service a pasted link belongs to, or None.
pub fn detect(link: &str) -> Option<LinkSource> {
    let t = link.trim();
    if t.contains("spotify") {
        Some(LinkSource::Spotify)
    } else if t.contains("music.apple.com") {
        Some(LinkSource::AppleMusic)
    } else if t.contains("deezer") {
        Some(LinkSource::Deezer)
    } else if t.contains("youtube.com") || t.contains("youtu.be") {
        Some(LinkSource::Youtube)
    } else {
        None
    }
}

/// Share sheets paste "check out this playlist! https://…" — take just the URL.
pub fn extract_url(text: &str) -> Option<String> {
    let t = text.trim();
    if t.starts_with("spotify:") {
        return Some(t.to_string());
    }
    URL_RE.find(t).map(|m| m.as_str().trim_end_matches(['.', ',', ')', '"', '\'']).to_string())
}

fn err(m: impl Into<String>) -> ImportError {
    ImportError(m.into())
}

pub struct LinkImporter {
    http: reqwest::Client,
}

impl Default for LinkImporter {
    fn default() -> Self {
        Self::new()
    }
}

impl LinkImporter {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30))
            .gzip(true)
            .build()
            .unwrap_or_default();
        Self { http }
    }

    /// Read the playlist/album behind `link`. `ytm` is needed for YouTube links.
    pub async fn fetch(&self, link: &str, ytm: Option<&YtmClient>) -> ImportResult<ParsedImport> {
        let text = extract_url(link).ok_or_else(|| err("That doesn't look like a link."))?;
        let resolved = if SHORT_LINK_HOSTS.iter().any(|h| text.contains(h)) { self.resolve_short_link(&text).await? } else { text.clone() };
        match detect(&resolved).or_else(|| detect(&text)) {
            Some(LinkSource::Spotify) => self.fetch_spotify(&resolved).await,
            Some(LinkSource::AppleMusic) => self.fetch_apple(&resolved).await,
            Some(LinkSource::Deezer) => self.fetch_deezer(&resolved).await,
            Some(LinkSource::Youtube) => {
                let ytm = ytm.ok_or_else(|| err("YouTube Music isn't reachable right now."))?;
                fetch_youtube(ytm, &resolved).await
            }
            None => Err(err("Paste a Spotify, Apple Music, Deezer or YouTube Music link.")),
        }
    }

    async fn get(&self, url: &str) -> ImportResult<reqwest::Response> {
        self.http
            .get(url)
            .header("Accept-Language", "en-US,en;q=0.9")
            .send()
            .await
            .map_err(|_| err("Couldn't reach the server — check your connection."))
    }

    async fn get_ok(&self, url: &str, what: &str) -> ImportResult<String> {
        let r = self.get(url).await?;
        match r.status().as_u16() {
            200..=299 => r.text().await.map_err(|_| err("Couldn't reach the server — check your connection.")),
            404 => Err(err(format!("{what} wasn't found. Is it public?"))),
            429 => Err(err(format!("{what} is rate-limiting us. Try again in a minute."))),
            s => Err(err(format!("{what} answered with an error ({s})."))),
        }
    }

    /// Short links redirect to the real page; some go through an HTML page holding the URL.
    async fn resolve_short_link(&self, url: &str) -> ImportResult<String> {
        let r = self.get(url).await?;
        let final_url = r.url().to_string();
        if SPOTIFY_RE.is_match(&final_url) || DEEZER_RE.is_match(&final_url) || YT_LIST_RE.is_match(&final_url) {
            return Ok(final_url);
        }
        let body = r.text().await.unwrap_or_default();
        if let Some(c) = SPOTIFY_RE.captures(&body) {
            return Ok(format!("https://open.spotify.com/{}/{}", &c[1], &c[2]));
        }
        if let Some(c) = DEEZER_RE.captures(&body) {
            return Ok(format!("https://www.deezer.com/{}/{}", &c[1], &c[2]));
        }
        Ok(final_url)
    }

    async fn fetch_spotify(&self, url: &str) -> ImportResult<ParsedImport> {
        let c = SPOTIFY_RE.captures(url).ok_or_else(|| err("Paste a Spotify playlist or album link."))?;
        let (kind, id) = (c[1].to_string(), c[2].to_string());
        let html = self.get_ok(&format!("https://open.spotify.com/embed/{kind}/{id}"), "Spotify").await?;
        Ok(parse_spotify_embed(&html, &kind)?
            .with_origin(PlaylistOrigin { remote_id: Some(id.clone()), remote_url: Some(format!("https://open.spotify.com/{kind}/{id}")) }))
    }

    async fn fetch_apple(&self, url: &str) -> ImportResult<ParsedImport> {
        let c = APPLE_RE.captures(url).ok_or_else(|| err("Paste an Apple Music playlist or album link."))?;
        let kind = c[1].to_string();
        let remote_id = c[2].to_string();
        let page = url.split('#').next().unwrap_or(url).to_string();
        let html = self.get_ok(&page, "Apple Music").await?;
        Ok(parse_apple_page(&html, &kind)?.with_origin(PlaylistOrigin { remote_id: Some(remote_id), remote_url: Some(page) }))
    }

    async fn deezer_json(&self, url: &str) -> ImportResult<Value> {
        let v: Value = serde_json::from_str(&self.get_ok(url, "Deezer").await?).map_err(|_| err("Deezer sent something we couldn't read."))?;
        if let Some(e) = v.get("error") {
            let msg = string(e, "message");
            return Err(err(if msg.as_deref().is_some_and(|m| m.to_lowercase().contains("no data")) {
                "That Deezer playlist is private or doesn't exist.".to_string()
            } else {
                format!("Deezer: {}", msg.unwrap_or_else(|| "unknown error".into()))
            }));
        }
        Ok(v)
    }

    async fn fetch_deezer(&self, url: &str) -> ImportResult<ParsedImport> {
        let c = DEEZER_RE.captures(url).ok_or_else(|| err("Paste a Deezer playlist or album link."))?;
        let (kind, id) = (c[1].to_string(), c[2].to_string());
        let info = self.deezer_json(&format!("https://api.deezer.com/{kind}/{id}")).await?;
        let name = string(&info, "title").unwrap_or_else(|| format!("Deezer {kind}"));
        let album_name = (kind == "album").then(|| name.clone());
        let mut tracks = Vec::new();
        let mut next = Some(format!("https://api.deezer.com/{kind}/{id}/tracks?limit=100"));
        let mut pages = 0;
        while let Some(u) = next.take() {
            if pages >= 100 {
                break;
            }
            let page = self.deezer_json(&u).await?;
            tracks.extend(parse_deezer_tracks(&page, album_name.as_deref()));
            next = string(&page, "next").filter(|n| n.starts_with("https://api.deezer.com/"));
            pages += 1;
        }
        if tracks.is_empty() {
            return Err(err(format!("That Deezer {kind} has no songs we can read.")));
        }
        let cover = string(&info, "picture_xl").or_else(|| string(&info, "cover_xl"));
        let mut p = ParsedImport { playlists: vec![(name.clone(), tracks)], provider: Some(ImportSource::Deezer), ..Default::default() };
        if let Some(c) = cover {
            p.playlist_covers.insert(name, c);
        }
        Ok(p.with_origin(PlaylistOrigin { remote_id: Some(id.clone()), remote_url: Some(format!("https://www.deezer.com/{kind}/{id}")) }))
    }
}

pub fn parse_deezer_tracks(page: &Value, album_name: Option<&str>) -> Vec<ImportedTrack> {
    array(page.get("data"))
        .iter()
        .filter_map(|o| {
            let title = string(o, "title")?;
            Some(ImportedTrack {
                artist: o.get("artist").and_then(|a| string(a, "name")).unwrap_or_default(),
                album: album_name.map(str::to_string).or_else(|| o.get("album").and_then(|a| string(a, "title"))),
                duration_ms: long(o, "duration").map(|d| d * 1000),
                ..ImportedTrack::new(title, "", None)
            })
        })
        .collect()
}

/// Spotify: the embed player page carries the track list for logged-out visitors.
pub fn parse_spotify_embed(html: &str, kind: &str) -> ImportResult<ParsedImport> {
    let data = NEXT_DATA_RE.captures(html).map(|c| c[1].to_string()).ok_or_else(|| err("Spotify changed its page — use a CSV export for now."))?;
    let root: Value = serde_json::from_str(&data).map_err(|_| err("Spotify changed its page — use a CSV export for now."))?;
    let entity = path(&root, &["props", "pageProps", "state", "data", "entity"])
        .filter(|e| e.is_object())
        .ok_or_else(|| err(format!("That Spotify {kind} is private or doesn't exist.")))?;
    let name = string(entity, "name").or_else(|| string(entity, "title")).unwrap_or_else(|| format!("Spotify {kind}"));
    let album_name = (kind == "album").then(|| name.clone());
    let tracks: Vec<ImportedTrack> = array(entity.get("trackList"))
        .iter()
        .filter_map(|o| {
            let title = string(o, "title")?;
            Some(ImportedTrack {
                album: album_name.clone(),
                duration_ms: long(o, "duration"),
                source_id: string(o, "uri").filter(|u| u.starts_with("spotify:track:")),
                ..ImportedTrack::new(title, string(o, "subtitle").unwrap_or_default().replace('\u{00A0}', " "), None)
            })
        })
        .collect();
    if tracks.is_empty() {
        return Err(err(format!("That Spotify {kind} has no songs we can read.")));
    }
    let mut warnings = Vec::new();
    if kind == "playlist" && tracks.len() >= SPOTIFY_EMBED_LIMIT {
        warnings.push(format!(
            "Spotify only shares the first {SPOTIFY_EMBED_LIMIT} songs of a playlist through links. For the whole playlist, use a CSV export (TuneMyMusic / Exportify) or the account import."
        ));
    }
    let cover = array(path(entity, &["coverArt", "sources"])).iter().max_by_key(|s| long(s, "width").unwrap_or(0)).and_then(|s| string(s, "url"));
    let mut p = ParsedImport { playlists: vec![(name.clone(), tracks)], warnings, provider: Some(ImportSource::Spotify), ..Default::default() };
    if let Some(c) = cover {
        p.playlist_covers.insert(name, c);
    }
    Ok(p)
}

/// Apple artwork URLs are templates: {w}x{h}, {c} (crop) and {f} (format).
pub fn apple_artwork(template: &str) -> String {
    template.replace("{w}", "600").replace("{h}", "600").replace("{c}", "bb").replace("{f}", "jpg")
}

/// Apple Music: the web page embeds its own data as serialized-server-data.
pub fn parse_apple_page(html: &str, kind: &str) -> ImportResult<ParsedImport> {
    let data = APPLE_DATA_RE.captures(html).map(|c| c[1].to_string()).ok_or_else(|| err("Apple Music changed its page — use a CSV export for now."))?;
    let root: Value = serde_json::from_str(&data).map_err(|_| err("Apple Music changed its page — use a CSV export for now."))?;
    let page = root
        .get("data")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|f| f.get("data"))
        .or_else(|| root.as_array().and_then(|a| a.first()).and_then(|f| f.get("data")))
        .ok_or_else(|| err(format!("That Apple Music {kind} is private or doesn't exist.")))?;
    let sections: Vec<&Value> = array(page.get("sections")).iter().filter(|s| s.is_object()).collect();
    let header = sections
        .iter()
        .find(|s| string(s, "itemKind").as_deref() == Some("containerDetailHeaderLockup"))
        .and_then(|s| array(s.get("items")).first());
    let name = header.and_then(|h| string(h, "title")).unwrap_or_else(|| format!("Apple Music {kind}"));
    let album_artist = header.and_then(|h| array(h.get("subtitleLinks")).first()).and_then(|l| l.get("title")).and_then(Value::as_str).map(str::to_string);
    let declared = header.and_then(|h| long(h, "trackCount"));
    let tracks: Vec<ImportedTrack> = sections
        .iter()
        .filter(|s| string(s, "itemKind").as_deref() == Some("trackLockup"))
        .flat_map(|s| array(s.get("items")).iter())
        .filter_map(|o| {
            let title = string(o, "title")?;
            let artists: Vec<&str> = array(o.get("subtitleLinks")).iter().filter_map(|l| l.get("title").and_then(Value::as_str)).collect();
            let joined = artists.join(", ");
            let artist = string(o, "artistName").unwrap_or_else(|| if joined.is_empty() { album_artist.clone().unwrap_or_default() } else { joined });
            let album = if kind == "album" {
                Some(name.clone())
            } else {
                array(o.get("tertiaryLinks")).first().and_then(|l| l.get("title")).and_then(Value::as_str).map(str::to_string)
            };
            Some(ImportedTrack { duration_ms: long(o, "duration"), ..ImportedTrack::new(title, artist, album) })
        })
        .collect();
    if tracks.is_empty() {
        return Err(err(format!("That Apple Music {kind} has no songs we can read.")));
    }
    let mut warnings = Vec::new();
    if let Some(d) = declared
        && d as usize > tracks.len()
    {
        warnings.push(format!("Apple Music's page only listed {} of {d} songs. For the whole playlist, use a CSV export or the account import.", tracks.len()));
    }
    let cover = header.and_then(|h| path(h, &["artwork", "dictionary", "url"])).and_then(Value::as_str).map(apple_artwork);
    let mut p = ParsedImport { playlists: vec![(name.clone(), tracks)], warnings, provider: Some(ImportSource::AppleMusic), ..Default::default() };
    if let Some(c) = cover {
        p.playlist_covers.insert(name, c);
    }
    Ok(p)
}

/// YouTube / YouTube Music: our own client; ids are exact so nothing needs matching.
pub async fn fetch_youtube(ytm: &YtmClient, url: &str) -> ImportResult<ParsedImport> {
    let browse = YT_BROWSE_RE.captures(url).map(|c| c[1].to_string());
    let list = YT_LIST_RE.captures(url).map(|c| c[1].to_string());
    match (browse, list) {
        (Some(b), _) if b.starts_with("MPREb_") => fetch_youtube_album(ytm, &b).await,
        (_, Some(l)) => fetch_youtube_playlist(ytm, &l).await,
        (Some(b), None) => fetch_youtube_playlist(ytm, b.strip_prefix("VL").unwrap_or(&b)).await,
        _ => Err(err("Paste a YouTube playlist or album link.")),
    }
}

pub async fn fetch_youtube_playlist(ytm: &YtmClient, playlist_id: &str) -> ImportResult<ParsedImport> {
    if playlist_id.starts_with("RD") {
        return Err(err("That's a YouTube mix, which changes every time — save it as a playlist first."));
    }
    let (pl, songs) = ytm.playlist(playlist_id).await.map_err(|_| err("Couldn't open that YouTube playlist. Is it public or unlisted?"))?;
    let tracks: Vec<ImportedTrack> = songs.iter().map(ImportedTrack::from_youtube).collect();
    if tracks.is_empty() {
        return Err(err("That YouTube playlist is empty."));
    }
    let mut p = ParsedImport { playlists: vec![(pl.name.clone(), tracks)], provider: Some(ImportSource::YoutubeMusic), ..Default::default() };
    if let Some(c) = pl.thumbnail_url.filter(|c| !c.is_empty()) {
        p.playlist_covers.insert(pl.name, c);
    }
    Ok(p.with_origin(PlaylistOrigin {
        remote_id: Some(playlist_id.to_string()),
        remote_url: Some(format!("https://music.youtube.com/playlist?list={playlist_id}")),
    }))
}

async fn fetch_youtube_album(ytm: &YtmClient, browse_id: &str) -> ImportResult<ParsedImport> {
    let (album, songs) = ytm.album(browse_id).await.map_err(|_| err("Couldn't open that YouTube Music album."))?;
    let tracks: Vec<ImportedTrack> = songs.iter().map(ImportedTrack::from_youtube).collect();
    if tracks.is_empty() {
        return Err(err("That album has no songs."));
    }
    let mut p = ParsedImport { playlists: vec![(album.title.clone(), tracks)], provider: Some(ImportSource::YoutubeMusic), ..Default::default() };
    if let Some(c) = album.thumbnail_url.filter(|c| !c.is_empty()) {
        p.playlist_covers.insert(album.title, c);
    }
    Ok(p.with_origin(PlaylistOrigin { remote_id: Some(browse_id.to_string()), remote_url: Some(format!("https://music.youtube.com/browse/{browse_id}")) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPOTIFY_FIXTURE: &str = r#"<html><head></head><body><script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"state":{"data":{"entity":{"type":"playlist","name":"Road Trip","trackList":[{"uri":"spotify:track:3h5T5JypYU7huFiVYhv1dr","title":"BbY WOW","subtitle":"KAROL G, Judeline","duration":225834},{"uri":"spotify:episode:x","title":"Ep","subtitle":"Pod","duration":1000},{"subtitle":"no title"}],"coverArt":{"sources":[{"url":"https://i.scdn.co/small","width":64},{"url":"https://i.scdn.co/big","width":640}]}}}}}}}</script></body></html>"#;

    const APPLE_FIXTURE: &str = r#"<html><script type="application/json" id="serialized-server-data">{"data":[{"data":{"sections":[{"itemKind":"containerDetailHeaderLockup","items":[{"title":"Today’s Hits","trackCount":3,"subtitleLinks":[{"title":"Apple Music Hits"}],"artwork":{"dictionary":{"url":"https://is1-ssl.mzstatic.com/image/{w}x{h}{c}.{f}"}}}]},{"itemKind":"trackLockup","items":[{"title":"Housewife","artistName":"Tate McRae","duration":208810,"subtitleLinks":[{"title":"Tate McRae"}],"tertiaryLinks":[{"title":"Housewife - Single"}]},{"title":"Duet","subtitleLinks":[{"title":"A"},{"title":"B"}],"duration":1000}]},{"itemKind":"spacer"}]}}]}</script></html>"#;

    #[test]
    fn link_detection_and_extraction() {
        assert_eq!(detect("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"), Some(LinkSource::Spotify));
        assert_eq!(detect("https://music.apple.com/us/playlist/x/pl.abc"), Some(LinkSource::AppleMusic));
        assert_eq!(detect("https://www.deezer.com/en/playlist/123"), Some(LinkSource::Deezer));
        assert_eq!(detect("https://youtu.be/x?list=PL1"), Some(LinkSource::Youtube));
        assert_eq!(detect("https://example.com"), None);
        assert_eq!(
            extract_url("check out this playlist! https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=1).").as_deref(),
            Some("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=1")
        );
        assert_eq!(extract_url("spotify:album:1234567890123456789012").as_deref(), Some("spotify:album:1234567890123456789012"));
        assert_eq!(extract_url("no link here"), None);
        let c = SPOTIFY_RE.captures("https://open.spotify.com/intl-de/album/1234567890123456789012").unwrap();
        assert_eq!((&c[1], &c[2]), ("album", "1234567890123456789012"));
        let c = APPLE_RE.captures("https://music.apple.com/us/album/housewife/6822497770?i=1").unwrap();
        assert_eq!((&c[1], &c[2]), ("album", "6822497770"));
        let c = YT_BROWSE_RE.captures("https://music.youtube.com/browse/MPREb_abc").unwrap();
        assert_eq!(&c[1], "MPREb_abc");
    }

    #[test]
    fn spotify_embed_fixture() {
        let p = parse_spotify_embed(SPOTIFY_FIXTURE, "playlist").unwrap();
        assert_eq!(p.provider, Some(ImportSource::Spotify));
        let t = p.playlist("Road Trip").unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].title, "BbY WOW");
        assert_eq!(t[0].artist, "KAROL G, Judeline");
        assert_eq!(t[0].duration_ms, Some(225_834));
        assert_eq!(t[0].source_id.as_deref(), Some("spotify:track:3h5T5JypYU7huFiVYhv1dr"));
        assert_eq!(t[1].source_id, None);
        assert_eq!(p.playlist_covers.get("Road Trip").map(String::as_str), Some("https://i.scdn.co/big"));
        assert!(p.warnings.is_empty());
        assert!(parse_spotify_embed("<html></html>", "playlist").is_err());
        let a = parse_spotify_embed(&SPOTIFY_FIXTURE.replace("Road Trip", "LP"), "album").unwrap();
        assert_eq!(a.playlist("LP").unwrap()[0].album.as_deref(), Some("LP"));
    }

    #[test]
    fn apple_page_fixture() {
        let p = parse_apple_page(APPLE_FIXTURE, "playlist").unwrap();
        let t = p.playlist("Today’s Hits").unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].artist, "Tate McRae");
        assert_eq!(t[0].album.as_deref(), Some("Housewife - Single"));
        assert_eq!(t[0].duration_ms, Some(208_810));
        assert_eq!(t[1].artist, "A, B");
        assert_eq!(p.warnings.len(), 1, "page listed 2 of 3");
        assert_eq!(p.playlist_covers.values().next().unwrap(), "https://is1-ssl.mzstatic.com/image/600x600bb.jpg");
    }

    #[test]
    fn deezer_tracks() {
        let page = serde_json::json!({"data":[{"title":"One","duration":200,"artist":{"name":"Metallica"},"album":{"title":"And Justice"}},{"duration":1}]});
        let t = parse_deezer_tracks(&page, None);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].duration_ms, Some(200_000));
        assert_eq!(t[0].album.as_deref(), Some("And Justice"));
        assert_eq!(parse_deezer_tracks(&page, Some("LP"))[0].album.as_deref(), Some("LP"));
    }

    /// Live: a public Spotify playlist and Apple Music playlist. Run with
    /// `cargo test -p mochy-importer -- --ignored live_`.
    #[tokio::test]
    #[ignore]
    async fn live_public_links() {
        let li = LinkImporter::new();
        let sp = li.fetch("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M", None).await.unwrap();
        eprintln!("spotify: {} playlists, {} songs, first {:?}", sp.total_playlists(), sp.total_songs(), sp.playlists[0].1.first().map(|t| (&t.title, &t.artist)));
        assert!(sp.total_songs() > 10);
        let am = li.fetch("https://music.apple.com/us/playlist/todays-hits/pl.f4d106fed2bd41149aaacabb233eb5eb", None).await.unwrap();
        eprintln!("apple: {} songs, first {:?}", am.total_songs(), am.playlists[0].1.first().map(|t| (&t.title, &t.artist)));
        assert!(am.total_songs() > 10);
        let dz = li.fetch("https://www.deezer.com/playlist/3155776842", None).await.unwrap();
        eprintln!("deezer: {} songs", dz.total_songs());
        assert!(dz.total_songs() > 10);
    }
}
