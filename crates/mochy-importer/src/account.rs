//! Reading a signed-in account's library (Android `importer/account/*`).
//!
//! Upstream captures the web player's session tokens in an embedded webview.
//! The desktop app has no webview, so the user pastes the tokens themselves
//! (from the service's web player); they are kept in memory for the import
//! only and sent nowhere but the service they belong to.
//!
//! - Spotify: the Web API (`api.spotify.com/v1`) with a bearer token — the
//!   upstream fallback path. (Upstream's primary path replays the web player's
//!   private GraphQL queries; that is not ported.)
//! - Apple Music: developer token (bearer) + music-user token.
//! - Tidal: the web player's bearer token; user id/country from `/sessions`
//!   or the token's own claims.

use std::time::Duration;

use base64::Engine;
use serde_json::Value;

use crate::json::{array, date_ms, id_string, long, path, string};
use crate::models::{ImportError, ImportResult, ImportSource, ImportedTrack, ParsedImport, PlaylistOrigin};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountService {
    Spotify,
    AppleMusic,
    Tidal,
}

impl AccountService {
    pub const ALL: [AccountService; 3] = [AccountService::Spotify, AccountService::AppleMusic, AccountService::Tidal];

    pub fn label(self) -> &'static str {
        match self {
            AccountService::Spotify => "Spotify",
            AccountService::AppleMusic => "Apple Music",
            AccountService::Tidal => "Tidal",
        }
    }
    pub fn import_source(self) -> ImportSource {
        match self {
            AccountService::Spotify => ImportSource::Spotify,
            AccountService::AppleMusic => ImportSource::AppleMusic,
            AccountService::Tidal => ImportSource::Tidal,
        }
    }
    /// Where the user signs in to get the tokens.
    pub fn web_player(self) -> &'static str {
        match self {
            AccountService::Spotify => "https://open.spotify.com/",
            AccountService::AppleMusic => "https://music.apple.com/",
            AccountService::Tidal => "https://listen.tidal.com/",
        }
    }
}

/// The tokens the user pasted.
#[derive(Debug, Clone, Default)]
pub struct AccountTokens {
    /// Spotify / Tidal bearer, or Apple's developer token.
    pub bearer: String,
    /// Apple's `media-user-token`.
    pub user_token: String,
}

fn clean_bearer(t: &str) -> String {
    let t = t.trim();
    let t = t.strip_prefix("Bearer ").or_else(|| t.strip_prefix("bearer ")).unwrap_or(t);
    t.trim().to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemotePlaylist {
    pub id: String,
    pub name: String,
    pub track_count: Option<i64>,
    pub subtitle: Option<String>,
    /// Goes into Liked songs instead of becoming a playlist.
    pub is_liked: bool,
    /// The account's whole song library: into the library, not a playlist.
    pub is_library: bool,
    pub cover_url: Option<String>,
}

impl RemotePlaylist {
    fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self { id: id.into(), name: name.into(), track_count: None, subtitle: None, is_liked: false, is_library: false, cover_url: None }
    }
}

const SPOTIFY_API: &str = "https://api.spotify.com/v1";
const SPOTIFY_LIKED: &str = "spotify:liked";
const APPLE_HOST: &str = "https://amp-api.music.apple.com";
const APPLE_LIBRARY: &str = "apple:library-songs";
const APPLE_FAVORITES: [&str; 2] = ["favorite songs", "favourite songs"];
const TIDAL_API: &str = "https://api.tidal.com/v1";
const TIDAL_LIKED: &str = "tidal:favorites";
const MAX_PAGES: usize = 400;
const MAX_ATTEMPTS: u32 = 5;
const DESKTOP_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";

fn err(m: impl Into<String>) -> ImportError {
    ImportError(m.into())
}

/// A connected account.
pub struct AccountLibrary {
    pub service: AccountService,
    pub account_name: Option<String>,
    headers: Vec<(String, String)>,
    /// Tidal: user id and country.
    tidal: Option<(String, String)>,
    http: reqwest::Client,
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(DESKTOP_UA)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .gzip(true)
        .build()
        .unwrap_or_default()
}

struct Resp {
    status: u16,
    body: String,
    retry_after: Option<u64>,
}

async fn get(http: &reqwest::Client, url: &str, headers: &[(String, String)], service: &str) -> ImportResult<Resp> {
    let mut rq = http.get(url);
    for (k, v) in headers {
        rq = rq.header(k.as_str(), v.as_str());
    }
    let r = rq.send().await.map_err(|_| err(format!("Couldn't reach {service}. Check your connection.")))?;
    let status = r.status().as_u16();
    let retry_after = r.headers().get("Retry-After").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse().ok());
    let body = r.text().await.unwrap_or_default();
    Ok(Resp { status, body, retry_after })
}

/// A library read: waits out rate limits and turns failures into messages a
/// user can act on. None for a 404 when `not_found_as_none`.
async fn get_json(http: &reqwest::Client, url: &str, headers: &[(String, String)], service: &str, not_found_as_none: bool) -> ImportResult<Option<Value>> {
    for attempt in 0..MAX_ATTEMPTS {
        let r = get(http, url, headers, service).await?;
        match r.status {
            200..=299 => {
                return serde_json::from_str(&r.body).map(Some).map_err(|_| err(format!("{service} sent something we couldn't read.")));
            }
            429 | 502 | 503 => {
                let wait = r.retry_after.unwrap_or(2u64 << attempt).clamp(1, 30);
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
            401 => return Err(err(format!("{service} turned the token down (signed out or expired). Copy a fresh one."))),
            404 if not_found_as_none => return Ok(None),
            403 => return Err(err(format!("{service} won't share this one (403)."))),
            s => return Err(err(format!("{service} answered with an error ({s})."))),
        }
    }
    Err(err(format!("{service} is busy right now. Try again in a minute.")))
}

fn largest_image(sources: &[Value]) -> Option<String> {
    sources.iter().max_by_key(|s| long(s, "width").unwrap_or(0)).and_then(|s| string(s, "url"))
}

fn apple_artwork(t: &str) -> String {
    crate::link::apple_artwork(t)
}

/// The JWT payload's `uid` and `cc` claims (Tidal).
pub fn jwt_user(token: &str) -> Option<(String, String)> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    Some((id_string(&claims, "uid")?, string(&claims, "cc")?))
}

impl AccountLibrary {
    /// Check the pasted tokens against the service and connect.
    pub async fn connect(service: AccountService, tokens: &AccountTokens) -> ImportResult<Self> {
        let http = http();
        let bearer = clean_bearer(&tokens.bearer);
        if bearer.len() < 20 {
            return Err(err("Paste the access token first."));
        }
        match service {
            AccountService::Spotify => {
                let headers = vec![("Authorization".into(), format!("Bearer {bearer}")), ("Accept".into(), "application/json".into())];
                let r = get(&http, &format!("{SPOTIFY_API}/me"), &headers, "Spotify").await?;
                match r.status {
                    200 => {
                        let me: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
                        // the anonymous web-player token has no user behind it
                        let Some(id) = string(&me, "id") else { return Err(err("That Spotify token isn't signed in to an account.")) };
                        Ok(Self { service, account_name: string(&me, "display_name").or(Some(id)), headers, tidal: None, http })
                    }
                    429 => Err(err(format!("Spotify is rate-limiting this token. Try again in {} s.", r.retry_after.unwrap_or(5).clamp(2, 30)))),
                    s => Err(err(format!("Spotify turned the token down ({s})."))),
                }
            }
            AccountService::AppleMusic => {
                let user = tokens.user_token.trim();
                if user.len() < 20 {
                    return Err(err("Apple Music also needs the media-user-token."));
                }
                let headers = vec![
                    ("Authorization".into(), format!("Bearer {bearer}")),
                    ("Media-User-Token".into(), user.to_string()),
                    ("Origin".into(), "https://music.apple.com".into()),
                    ("Referer".into(), "https://music.apple.com/".into()),
                    ("Accept".into(), "application/json".into()),
                ];
                let r = get(&http, &format!("{APPLE_HOST}/v1/me/storefront"), &headers, "Apple Music").await?;
                match r.status {
                    200 => Ok(Self { service, account_name: None, headers, tidal: None, http }),
                    s => Err(err(format!("Apple Music turned the tokens down ({s})."))),
                }
            }
            AccountService::Tidal => {
                let headers = vec![("Authorization".into(), format!("Bearer {bearer}"))];
                let r = get(&http, &format!("{TIDAL_API}/sessions"), &headers, "Tidal").await?;
                if r.status == 200 {
                    let s: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
                    if let (Some(uid), Some(cc)) = (id_string(&s, "userId"), string(&s, "countryCode")) {
                        return Ok(Self { service, account_name: None, headers, tidal: Some((uid, cc)), http });
                    }
                }
                if r.status == 429 {
                    return Err(err("Tidal is rate-limiting. Try again in a few seconds."));
                }
                // some tokens are refused by /sessions but carry the user in the token itself
                if let Some((uid, cc)) = jwt_user(&bearer) {
                    let probe = get(&http, &format!("{TIDAL_API}/users/{uid}/playlists?limit=1&countryCode={cc}"), &headers, "Tidal").await?;
                    if probe.status == 200 {
                        return Ok(Self { service, account_name: None, headers, tidal: Some((uid, cc)), http });
                    }
                }
                Err(err(format!("Tidal turned the token down ({}).", r.status)))
            }
        }
    }

    async fn json(&self, url: &str, not_found_as_none: bool) -> ImportResult<Option<Value>> {
        get_json(&self.http, url, &self.headers, self.service.label(), not_found_as_none).await
    }

    pub async fn playlists(&self) -> ImportResult<Vec<RemotePlaylist>> {
        match self.service {
            AccountService::Spotify => self.spotify_playlists().await,
            AccountService::AppleMusic => self.apple_playlists().await,
            AccountService::Tidal => self.tidal_playlists().await,
        }
    }

    pub async fn tracks(&self, p: &RemotePlaylist) -> ImportResult<Vec<ImportedTrack>> {
        match self.service {
            AccountService::Spotify => self.spotify_tracks(p).await,
            AccountService::AppleMusic => self.apple_tracks(p).await,
            AccountService::Tidal => self.tidal_tracks(p).await,
        }
    }

    /// Read the chosen playlists into one import. `progress(done, total, name)`.
    pub async fn read_selection(&self, chosen: &[RemotePlaylist], mut progress: impl FnMut(usize, usize, &str)) -> ImportResult<ParsedImport> {
        let mut out = ParsedImport { provider: Some(self.service.import_source()), ..Default::default() };
        for (i, p) in chosen.iter().enumerate() {
            progress(i, chosen.len(), &p.name);
            let tracks = self.tracks(p).await?;
            if p.is_liked {
                out.liked_songs.extend(tracks);
            } else if p.is_library {
                out.library_songs.extend(tracks);
            } else {
                // a second playlist with a name already taken becomes "Name (2)"
                let mut name = p.name.clone();
                let mut n = 2;
                while out.playlist(&name).is_some() {
                    name = format!("{} ({n})", p.name);
                    n += 1;
                }
                if let Some(c) = &p.cover_url {
                    out.playlist_covers.insert(name.clone(), c.clone());
                }
                out.playlist_origins.insert(name.clone(), PlaylistOrigin { remote_id: Some(p.id.clone()), remote_url: None });
                out.playlists.push((name, tracks));
            }
        }
        progress(chosen.len(), chosen.len(), "");
        Ok(out)
    }

    // ---- Spotify ------------------------------------------------------------------

    async fn spotify_playlists(&self) -> ImportResult<Vec<RemotePlaylist>> {
        let liked_total = self.json(&format!("{SPOTIFY_API}/me/tracks?limit=1"), false).await?.and_then(|v| long(&v, "total"));
        let mut out = vec![RemotePlaylist { track_count: liked_total, is_liked: true, ..RemotePlaylist::new(SPOTIFY_LIKED, "Liked Songs") }];
        let mut next = Some(format!("{SPOTIFY_API}/me/playlists?limit=50"));
        let mut pages = 0;
        while let Some(u) = next.take() {
            pages += 1;
            if pages > MAX_PAGES {
                break;
            }
            let Some(page) = self.json(&u, false).await? else { break };
            out.extend(parse_spotify_playlists(&page));
            next = string(&page, "next").filter(|n| n.starts_with(SPOTIFY_API));
        }
        Ok(out)
    }

    async fn spotify_tracks(&self, p: &RemotePlaylist) -> ImportResult<Vec<ImportedTrack>> {
        let mut tracks = Vec::new();
        let mut next = Some(if p.is_liked { format!("{SPOTIFY_API}/me/tracks?limit=50") } else { format!("{SPOTIFY_API}/playlists/{}/tracks?limit=100", p.id) });
        let mut tried_items = p.is_liked;
        let mut pages = 0;
        while let Some(u) = next.take() {
            pages += 1;
            if pages > MAX_PAGES {
                break;
            }
            let page = self.json(&u, !tried_items).await?;
            let Some(page) = page else {
                // newer API versions name the endpoint /items
                tried_items = true;
                next = Some(format!("{SPOTIFY_API}/playlists/{}/items?limit=100", p.id));
                continue;
            };
            tried_items = true;
            tracks.extend(parse_spotify_tracks(&page));
            next = string(&page, "next").filter(|n| n.starts_with(SPOTIFY_API));
        }
        Ok(tracks)
    }

    // ---- Apple Music ----------------------------------------------------------------

    async fn apple_pages(&self, first: String, mut on_item: impl FnMut(&Value)) -> ImportResult<()> {
        let mut next = Some(first);
        let mut pages = 0;
        while let Some(u) = next.take() {
            pages += 1;
            if pages > MAX_PAGES {
                break;
            }
            // an empty playlist answers 404 instead of an empty list
            let Some(page) = self.json(&u, true).await? else { return Ok(()) };
            for item in array(page.get("data")) {
                on_item(item);
            }
            next = page.get("next").and_then(Value::as_str).filter(|n| n.starts_with("/v1/")).map(|path| {
                let extra = if path.contains("limit=") { "" } else if path.contains('?') { "&limit=100" } else { "?limit=100" };
                format!("{APPLE_HOST}{path}{extra}")
            });
        }
        Ok(())
    }

    async fn apple_playlists(&self) -> ImportResult<Vec<RemotePlaylist>> {
        let mut out = Vec::new();
        self.apple_pages(format!("{APPLE_HOST}/v1/me/library/playlists?limit=100"), |item| {
            let Some(id) = string(item, "id") else { return };
            let attrs = item.get("attributes");
            let name = attrs.and_then(|a| string(a, "name")).unwrap_or_else(|| "Untitled playlist".into());
            let fav = APPLE_FAVORITES.contains(&name.to_lowercase().as_str());
            out.push(RemotePlaylist {
                subtitle: fav.then(|| "Imports as Liked songs".to_string()),
                is_liked: fav,
                cover_url: attrs.and_then(|a| a.get("artwork")).and_then(|a| string(a, "url")).map(|u| apple_artwork(&u)),
                ..RemotePlaylist::new(id, name)
            });
        })
        .await?;
        // loved songs first, then the whole library
        out.sort_by_key(|p| if p.is_liked { 0 } else { 1 });
        let at = out.iter().rposition(|p| p.is_liked).map(|i| i + 1).unwrap_or(0);
        out.insert(
            at,
            RemotePlaylist {
                subtitle: Some("Every song in your library, into Library → Songs".into()),
                is_library: true,
                ..RemotePlaylist::new(APPLE_LIBRARY, "Library songs")
            },
        );
        Ok(out)
    }

    async fn apple_tracks(&self, p: &RemotePlaylist) -> ImportResult<Vec<ImportedTrack>> {
        let url = if p.id == APPLE_LIBRARY {
            format!("{APPLE_HOST}/v1/me/library/songs?limit=100")
        } else {
            format!("{APPLE_HOST}/v1/me/library/playlists/{}/tracks?limit=100", p.id)
        };
        let mut tracks = Vec::new();
        self.apple_pages(url, |item| {
            if let Some(t) = parse_apple_track(item) {
                tracks.push(t);
            }
        })
        .await?;
        Ok(tracks)
    }

    // ---- Tidal ------------------------------------------------------------------------

    async fn tidal_pages(&self, base: &str, page_size: usize, mut on_item: impl FnMut(&Value)) -> ImportResult<()> {
        let mut offset = 0usize;
        for _ in 0..MAX_PAGES {
            let Some(page) = self.json(&format!("{base}&limit={page_size}&offset={offset}"), true).await? else { return Ok(()) };
            let items = array(page.get("items"));
            for it in items {
                on_item(it);
            }
            offset += items.len();
            let total = long(&page, "totalNumberOfItems");
            if items.is_empty() || total.is_some_and(|t| offset as i64 >= t) {
                return Ok(());
            }
        }
        Ok(())
    }

    async fn tidal_playlists(&self) -> ImportResult<Vec<RemotePlaylist>> {
        let (uid, cc) = self.tidal.clone().ok_or_else(|| err("Tidal isn't connected."))?;
        let liked_total =
            self.json(&format!("{TIDAL_API}/users/{uid}/favorites/tracks?limit=1&countryCode={cc}"), false).await?.and_then(|v| long(&v, "totalNumberOfItems"));
        let mut out = vec![RemotePlaylist {
            track_count: liked_total,
            subtitle: Some("Your favorite tracks".into()),
            is_liked: true,
            ..RemotePlaylist::new(TIDAL_LIKED, "My Tracks")
        }];
        let mut seen = std::collections::HashSet::new();
        self.tidal_pages(&format!("{TIDAL_API}/users/{uid}/playlists?countryCode={cc}"), 50, |item| {
            if let Some(p) = tidal_playlist(item, None)
                && seen.insert(p.id.clone())
            {
                out.push(p);
            }
        })
        .await?;
        self.tidal_pages(&format!("{TIDAL_API}/users/{uid}/favorites/playlists?countryCode={cc}"), 50, |entry| {
            if let Some(p) = entry.get("item").and_then(|i| tidal_playlist(i, Some("Saved playlist")))
                && seen.insert(p.id.clone())
            {
                out.push(p);
            }
        })
        .await?;
        Ok(out)
    }

    async fn tidal_tracks(&self, p: &RemotePlaylist) -> ImportResult<Vec<ImportedTrack>> {
        let (uid, cc) = self.tidal.clone().ok_or_else(|| err("Tidal isn't connected."))?;
        let url = if p.is_liked {
            format!("{TIDAL_API}/users/{uid}/favorites/tracks?countryCode={cc}&order=DATE&orderDirection=DESC")
        } else {
            format!("{TIDAL_API}/playlists/{}/items?countryCode={cc}", p.id)
        };
        let mut tracks = Vec::new();
        self.tidal_pages(&url, 100, |entry| {
            if let Some(t) = parse_tidal_entry(entry) {
                tracks.push(t);
            }
        })
        .await?;
        Ok(tracks)
    }
}

pub fn parse_spotify_playlists(page: &Value) -> Vec<RemotePlaylist> {
    array(page.get("items"))
        .iter()
        .filter_map(|p| {
            let id = string(p, "id")?;
            // the track count moved between "tracks" and "items" across API versions
            let count = p.get("tracks").or_else(|| p.get("items")).and_then(|t| long(t, "total"));
            Some(RemotePlaylist {
                track_count: count,
                subtitle: p.get("owner").and_then(|o| string(o, "display_name")).map(|n| format!("by {n}")),
                cover_url: largest_image(array(p.get("images"))),
                ..RemotePlaylist::new(id, string(p, "name").unwrap_or_else(|| "Untitled playlist".into()))
            })
        })
        .collect()
}

pub fn parse_spotify_tracks(page: &Value) -> Vec<ImportedTrack> {
    array(page.get("items"))
        .iter()
        .filter_map(|entry| {
            let track = entry.get("track").filter(|t| t.is_object()).or_else(|| entry.get("item").filter(|t| t.is_object()))?;
            // podcast episodes saved into playlists aren't music
            if string(track, "type").as_deref() == Some("episode") {
                return None;
            }
            let title = string(track, "name")?;
            let artists: Vec<String> = array(track.get("artists")).iter().filter_map(|a| string(a, "name")).collect();
            Some(ImportedTrack {
                duration_ms: long(track, "duration_ms"),
                added_at_ms: date_ms(string(entry, "added_at").as_deref()),
                source_id: string(track, "id").map(|id| format!("spotify:track:{id}")),
                ..ImportedTrack::new(title, artists.join(", "), track.get("album").and_then(|a| string(a, "name")))
            })
        })
        .collect()
}

pub fn parse_apple_track(item: &Value) -> Option<ImportedTrack> {
    let a = item.get("attributes")?;
    let title = string(a, "name")?;
    Some(ImportedTrack {
        duration_ms: long(a, "durationInMillis"),
        added_at_ms: date_ms(string(a, "dateAdded").as_deref()),
        ..ImportedTrack::new(title, string(a, "artistName").unwrap_or_default(), string(a, "albumName"))
    })
}

fn tidal_playlist(item: &Value, subtitle: Option<&str>) -> Option<RemotePlaylist> {
    let id = string(item, "uuid")?;
    // Tidal image ids are UUIDs whose dashes become path separators
    let cover = string(item, "squareImage")
        .map(|i| format!("https://resources.tidal.com/images/{}/640x640.jpg", i.replace('-', "/")))
        .or_else(|| string(item, "image").map(|i| format!("https://resources.tidal.com/images/{}/750x500.jpg", i.replace('-', "/"))));
    Some(RemotePlaylist {
        track_count: long(item, "numberOfTracks"),
        subtitle: subtitle.map(str::to_string),
        cover_url: cover,
        ..RemotePlaylist::new(id, string(item, "title").unwrap_or_else(|| "Untitled playlist".into()))
    })
}

pub fn parse_tidal_entry(entry: &Value) -> Option<ImportedTrack> {
    // playlists can hold music videos too; only tracks have an album
    if let Some(t) = string(entry, "type")
        && t != "track"
    {
        return None;
    }
    let track = entry.get("item")?;
    let title = string(track, "title")?;
    let version = string(track, "version");
    let title = match version {
        Some(v) if !title.to_lowercase().contains(&v.to_lowercase()) => format!("{title} ({v})"),
        _ => title,
    };
    let artists: Vec<String> = array(track.get("artists")).iter().filter_map(|a| string(a, "name")).collect();
    let artist = if artists.is_empty() { track.get("artist").and_then(|a| string(a, "name")).unwrap_or_default() } else { artists.join(", ") };
    Some(ImportedTrack {
        duration_ms: long(track, "duration").map(|d| d * 1000),
        // favorites say when they were added; playlist entries don't
        added_at_ms: date_ms(string(entry, "created").as_deref()),
        ..ImportedTrack::new(title, artist, path(track, &["album"]).and_then(|a| string(a, "title")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn spotify_pages() {
        let page = json!({"items":[{"id":"p1","name":"Chill","tracks":{"total":12},"owner":{"display_name":"me"},"images":[{"url":"s","width":60},{"url":"b","width":640}]},{"name":"no id"}]});
        let p = parse_spotify_playlists(&page);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].track_count, Some(12));
        assert_eq!(p[0].subtitle.as_deref(), Some("by me"));
        assert_eq!(p[0].cover_url.as_deref(), Some("b"));
        let tracks = json!({"items":[
            {"added_at":"2024-06-06T22:39:08Z","track":{"id":"abc","name":"Song","duration_ms":1000,"artists":[{"name":"A"},{"name":"B"}],"album":{"name":"LP"}}},
            {"track":{"type":"episode","name":"Pod"}},
            {"item":{"id":"x","name":"New API","artists":[]}}
        ]});
        let t = parse_spotify_tracks(&tracks);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].artist, "A, B");
        assert_eq!(t[0].added_at_ms, Some(1_717_713_548_000));
        assert_eq!(t[0].source_id.as_deref(), Some("spotify:track:abc"));
        assert_eq!(t[1].title, "New API");
    }

    #[test]
    fn apple_and_tidal_items() {
        let a = parse_apple_track(&json!({"attributes":{"name":"Song","artistName":"A","albumName":"LP","durationInMillis":5000,"dateAdded":"2024-06-06T22:39:08Z"}})).unwrap();
        assert_eq!(a.duration_ms, Some(5000));
        assert_eq!(a.album.as_deref(), Some("LP"));
        let t = parse_tidal_entry(&json!({"type":"track","created":"2020-09-14T17:29:43.587+0000","item":{"title":"Song","version":"Live","duration":200,"artists":[{"name":"A"}],"album":{"title":"LP"}}})).unwrap();
        assert_eq!(t.title, "Song (Live)");
        assert_eq!(t.duration_ms, Some(200_000));
        assert_eq!(t.added_at_ms, Some(1_600_104_583_587));
        assert!(parse_tidal_entry(&json!({"type":"video","item":{"title":"V"}})).is_none());
        let same = parse_tidal_entry(&json!({"item":{"title":"Song (Live)","version":"live","artist":{"name":"Solo"}}})).unwrap();
        assert_eq!(same.title, "Song (Live)");
        assert_eq!(same.artist, "Solo");
        let p = tidal_playlist(&json!({"uuid":"u1","title":"Mix","squareImage":"ab-cd"}), None).unwrap();
        assert_eq!(p.cover_url.as_deref(), Some("https://resources.tidal.com/images/ab/cd/640x640.jpg"));
    }

    #[test]
    fn jwt_claims_and_bearer_cleanup() {
        // {"uid":12345,"cc":"US"}
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"uid":12345,"cc":"US"}"#);
        let token = format!("h.{payload}.s");
        assert_eq!(jwt_user(&token), Some(("12345".into(), "US".into())));
        assert_eq!(jwt_user("garbage"), None);
        assert_eq!(clean_bearer("  Bearer abc "), "abc");
    }
}
