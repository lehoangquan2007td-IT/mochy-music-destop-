//! The signed-in account: who it is and what its YouTube Music library holds
//! (port of Kotlin `YouTube.accountInfo()` / `library(browseId)` /
//! `libraryContinuation()`). Every call here needs [`YtmClient::set_auth`];
//! without a session YouTube answers with an empty or sign-in page.

use mochy_core::models::{Album, Artist, Playlist, ShelfItem, Song};
use serde_json::{Value, json};

use crate::explore::{self, Section};
use crate::{YtmClient, YtmError, YtmResult, clients, parse};

/// Library browse ids (`FEmusic_*`).
pub mod browse_ids {
    pub const LIKED_PLAYLISTS: &str = "FEmusic_liked_playlists";
    pub const LIKED_ALBUMS: &str = "FEmusic_liked_albums";
    /// Artists the account subscribed to.
    pub const SUBSCRIBED_ARTISTS: &str = "FEmusic_library_corpus_artists";
    pub const HISTORY: &str = "FEmusic_history";
    /// "Liked music" auto playlist.
    pub const LIKED_MUSIC_PLAYLIST: &str = "LM";
}

/// Upper bound of library pages followed per list (each page ≈ 25–100 items).
const MAX_LIBRARY_PAGES: usize = 40;
/// Liked songs cap (larger than the shared 1000 queue cap).
pub const MAX_LIKED_SONGS: usize = 5000;

/// The account's display identity (Kotlin `AccountInfo`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountInfo {
    pub name: String,
    pub email: Option<String>,
    pub channel_handle: Option<String>,
    pub thumbnail_url: Option<String>,
}

/// Parse an `account/account_menu` response.
pub fn account_menu_response(json: &Value) -> Option<AccountInfo> {
    let actions = json["actions"].as_array()?;
    let h = actions.iter().find_map(|a| {
        let h = &a["openPopupAction"]["popup"]["multiPageMenuRenderer"]["header"]["activeAccountHeaderRenderer"];
        h.is_object().then_some(h)
    })?;
    let first = |v: &Value| {
        let t = parse::nav_text(v).trim().to_string();
        (!t.is_empty()).then_some(t)
    };
    let name = first(&h["accountName"])?;
    let thumbnail_url = h["accountPhoto"]["thumbnails"].as_array().and_then(|t| t.last()).and_then(|t| t["url"].as_str()).map(|u| {
        if u.starts_with("//") { format!("https:{u}") } else { u.to_string() }
    });
    Some(AccountInfo { name, email: first(&h["email"]), channel_handle: first(&h["channelHandle"]), thumbnail_url })
}

/// Everything synced from the account in one go.
#[derive(Debug, Clone, Default)]
pub struct LibrarySnapshot {
    pub liked_songs: Vec<Song>,
    pub playlists: Vec<Playlist>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    /// Most recent first.
    pub history: Vec<Song>,
    /// Lists that failed to load (name → error); the rest are still valid.
    pub errors: Vec<(String, String)>,
}

/// Auto playlists that are not user playlists ("Liked music", "Episodes for
/// later") and the "New playlist" tile.
fn is_library_playlist(p: &Playlist) -> bool {
    !p.id.is_empty() && !matches!(p.id.as_str(), "LM" | "SE") && p.browse_id.as_deref() != Some("VLLM") && p.browse_id.as_deref() != Some("VLSE")
}

fn items_of(sections: &[Section]) -> Vec<ShelfItem> {
    sections.iter().flat_map(|s| s.items.iter().cloned()).collect()
}

impl YtmClient {
    /// Name, email, handle and avatar of the signed-in account.
    pub async fn account_info(&self) -> YtmResult<AccountInfo> {
        if !self.is_signed_in() {
            return Err(YtmError::Other("not signed in".into()));
        }
        let json = self.remix("account/account_menu", json!({})).await?;
        account_menu_response(&json).ok_or_else(|| {
            if self.session_expired() {
                YtmError::Other("YouTube session expired — sign in again".into())
            } else {
                YtmError::Parse("account menu: no active account".into())
            }
        })
    }

    /// Every item of a library browse page, following its continuations.
    pub async fn library_items(&self, browse_id: &str) -> YtmResult<Vec<ShelfItem>> {
        let json = self.browse(Some(browse_id), None, None).await?;
        let page = explore::browse_page_response(&json);
        let mut items = items_of(&page.sections);
        // Library pages put one grid/shelf first; its own continuation pages it.
        let mut token = page.sections.iter().find_map(|s| s.continuation.clone()).or(page.continuation);
        let mut pages = 0;
        while let Some(t) = token.take() {
            pages += 1;
            if pages > MAX_LIBRARY_PAGES {
                break;
            }
            let more = explore::continuation_response(&self.browse(None, None, Some(&t)).await?);
            let before = items.len();
            items.extend(more.items);
            items.extend(items_of(&more.sections));
            if items.len() == before {
                break;
            }
            token = more.continuation;
        }
        Ok(items)
    }

    /// Playlists in the account's library (own and saved), without the
    /// "Liked music" / "Episodes for later" auto playlists.
    pub async fn library_playlists(&self) -> YtmResult<Vec<Playlist>> {
        let mut out: Vec<Playlist> = Vec::new();
        for item in self.library_items(browse_ids::LIKED_PLAYLISTS).await? {
            if let ShelfItem::Playlist(p) = item
                && is_library_playlist(&p)
                && !out.iter().any(|x| x.id == p.id)
            {
                out.push(p);
            }
        }
        Ok(out)
    }

    /// Albums saved to the library.
    pub async fn library_albums(&self) -> YtmResult<Vec<Album>> {
        let mut out: Vec<Album> = Vec::new();
        for item in self.library_items(browse_ids::LIKED_ALBUMS).await? {
            if let ShelfItem::Album(a) = item
                && !a.id.is_empty()
                && !out.iter().any(|x| x.id == a.id)
            {
                out.push(a);
            }
        }
        Ok(out)
    }

    /// Artists the account subscribed to.
    pub async fn library_artists(&self) -> YtmResult<Vec<Artist>> {
        let mut out: Vec<Artist> = Vec::new();
        for item in self.library_items(browse_ids::SUBSCRIBED_ARTISTS).await? {
            if let ShelfItem::Artist(a) = item
                && !a.id.is_empty()
                && !out.iter().any(|x| x.id == a.id)
            {
                out.push(a);
            }
        }
        Ok(out)
    }

    /// "Liked music" (`LM`), newest like first, up to [`MAX_LIKED_SONGS`].
    pub async fn liked_songs(&self) -> YtmResult<Vec<Song>> {
        let json = self.browse(Some("VLLM"), None, None).await?;
        let (_, mut songs) = parse::playlist_response(&json);
        let mut token = parse::playlist_continuation_token(&json);
        while let Some(t) = token.take() {
            if songs.len() >= MAX_LIKED_SONGS {
                break;
            }
            let page = self.browse(None, None, Some(&t)).await?;
            let (more, next) = parse::playlist_continuation(&page);
            let before = songs.len();
            for s in more {
                if !songs.iter().any(|x| x.id == s.id) {
                    songs.push(s);
                }
            }
            if songs.len() == before {
                break;
            }
            token = next;
        }
        songs.truncate(MAX_LIKED_SONGS);
        Ok(songs)
    }

    /// Listening history as YouTube Music shows it (Today, Yesterday, …),
    /// most recent first, each song once.
    pub async fn history(&self) -> YtmResult<Vec<Song>> {
        let mut out: Vec<Song> = Vec::new();
        for item in self.library_items(browse_ids::HISTORY).await? {
            if let ShelfItem::Song(s) = item
                && !out.iter().any(|x| x.id == s.id)
            {
                out.push(s);
            }
        }
        out.truncate(clients::MAX_GET_QUEUE_SIZE);
        Ok(out)
    }

    /// Fetch the whole library. Individual lists that fail are reported in
    /// [`LibrarySnapshot::errors`]; only a dead session fails the call.
    pub async fn library_snapshot(&self) -> YtmResult<LibrarySnapshot> {
        if !self.is_signed_in() {
            return Err(YtmError::Other("not signed in".into()));
        }
        let (liked, playlists, albums, artists, history) = tokio::join!(
            self.liked_songs(),
            self.library_playlists(),
            self.library_albums(),
            self.library_artists(),
            self.history(),
        );
        if self.session_expired() {
            return Err(YtmError::Other("YouTube session expired — sign in again".into()));
        }
        let mut snap = LibrarySnapshot::default();
        let mut note = |what: &str, e: YtmError| snap_err(&mut snap.errors, what, e);
        match liked {
            Ok(v) => snap.liked_songs = v,
            Err(e) => note("liked songs", e),
        }
        match playlists {
            Ok(v) => snap.playlists = v,
            Err(e) => note("playlists", e),
        }
        match albums {
            Ok(v) => snap.albums = v,
            Err(e) => note("albums", e),
        }
        match artists {
            Ok(v) => snap.artists = v,
            Err(e) => note("artists", e),
        }
        match history {
            Ok(v) => snap.history = v,
            Err(e) => note("history", e),
        }
        Ok(snap)
    }
}

fn snap_err(errors: &mut Vec<(String, String)>, what: &str, e: YtmError) {
    errors.push((what.to_string(), e.to_string()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_account_menu() {
        let json = json!({
            "actions": [{
                "openPopupAction": { "popup": { "multiPageMenuRenderer": { "header": { "activeAccountHeaderRenderer": {
                    "accountName": { "runs": [{ "text": "Quân" }] },
                    "email": { "runs": [{ "text": "q@example.com" }] },
                    "channelHandle": { "runs": [{ "text": "@quan" }] },
                    "accountPhoto": { "thumbnails": [
                        { "url": "//yt3.ggpht.com/a=s48", "width": 48 },
                        { "url": "//yt3.ggpht.com/a=s88", "width": 88 }
                    ] }
                } } } } }
            }]
        });
        let a = account_menu_response(&json).unwrap();
        assert_eq!(a.name, "Quân");
        assert_eq!(a.email.as_deref(), Some("q@example.com"));
        assert_eq!(a.channel_handle.as_deref(), Some("@quan"));
        assert_eq!(a.thumbnail_url.as_deref(), Some("https://yt3.ggpht.com/a=s88"));
        assert!(account_menu_response(&json!({ "actions": [] })).is_none());
    }

    #[test]
    fn filters_auto_playlists() {
        let p = |id: &str| Playlist { id: id.into(), browse_id: Some(format!("VL{id}")), ..Default::default() };
        assert!(!is_library_playlist(&p("LM")));
        assert!(!is_library_playlist(&p("SE")));
        assert!(!is_library_playlist(&p("")));
        assert!(is_library_playlist(&p("PLabc")));
    }

    #[test]
    fn library_grid_parses_playlists() {
        // Shape of FEmusic_liked_playlists: one grid with two-row cards and a continuation.
        let json = json!({
            "contents": { "singleColumnBrowseResultsRenderer": { "tabs": [{ "tabRenderer": { "content": { "sectionListRenderer": { "contents": [{
                "gridRenderer": {
                    "items": [
                        { "musicTwoRowItemRenderer": {
                            "title": { "runs": [{ "text": "Chill", "navigationEndpoint": { "browseEndpoint": {
                                "browseId": "VLPLchill",
                                "browseEndpointContextSupportedConfigs": { "browseEndpointContextMusicConfig": { "pageType": "MUSIC_PAGE_TYPE_PLAYLIST" } }
                            } } }] },
                            "subtitle": { "runs": [{ "text": "Quân" }, { "text": " • " }, { "text": "12 songs" }] },
                            "navigationEndpoint": { "browseEndpoint": {
                                "browseId": "VLPLchill",
                                "browseEndpointContextSupportedConfigs": { "browseEndpointContextMusicConfig": { "pageType": "MUSIC_PAGE_TYPE_PLAYLIST" } }
                            } },
                            "thumbnailRenderer": { "musicThumbnailRenderer": { "thumbnail": { "thumbnails": [{ "url": "https://i.ytimg.com/x.jpg", "width": 226, "height": 226 }] } } }
                        } }
                    ],
                    "continuations": [{ "nextContinuationData": { "continuation": "TOKEN2" } }]
                }
            }] } } } }] } }
        });
        let page = explore::browse_page_response(&json);
        let items = items_of(&page.sections);
        assert_eq!(items.len(), 1);
        match &items[0] {
            ShelfItem::Playlist(p) => {
                assert_eq!(p.name, "Chill");
                assert!(is_library_playlist(p));
            }
            other => panic!("expected playlist, got {other:?}"),
        }
        assert_eq!(page.sections[0].continuation.as_deref(), Some("TOKEN2"));
    }
}
