//! Explore / browse pages: home with mood chips and continuation, Explore,
//! moods & genres, charts, new releases, generic browse pages ("see all",
//! genre/mood pages) and their continuations.
//!
//! Ports `YouTube.home(continuation, params)`, `explore()`, `moodAndGenres()`,
//! `getChartsPage()`, `browse(browseId, params)`, `artistItems()` and
//! `artistItemsContinuation()` from the Kotlin client, plus the page models
//! (`HomePage`, `ExplorePage`, `MoodAndGenres`, `BrowseResult`, `ChartsPage`).
//! Unlike the Kotlin pages, sections keep *every* item kind (cards and rows)
//! and the raw subtitle line so the desktop UI can show what YTM shows.

use mochy_core::models::{Album, ShelfItem, Song};
use serde_json::Value;

use crate::{YtmClient, YtmResult, parse};

/// Charts browse params used by the Android client (`getChartsPage`).
pub const CHARTS_PARAMS: &str = "ggMGCgQIgAQ%3D";

/// A `browseEndpoint` (`browseId` + optional `params`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BrowseEndpoint {
    pub browse_id: String,
    pub params: Option<String>,
}

impl BrowseEndpoint {
    pub fn new(browse_id: impl Into<String>, params: Option<String>) -> Self {
        Self { browse_id: browse_id.into(), params }
    }

    fn from_json(v: &Value) -> Option<Self> {
        let id = v["browseId"].as_str()?.to_string();
        Some(Self { browse_id: id, params: v["params"].as_str().map(str::to_string) })
    }

    /// Page type declared by the endpoint (`MUSIC_PAGE_TYPE_PLAYLIST`, …).
    pub fn is_playlist(&self) -> bool {
        self.browse_id.starts_with("VL")
    }
}

/// A mood/genre tile (`musicNavigationButtonRenderer`).
#[derive(Debug, Clone, PartialEq)]
pub struct MoodItem {
    pub title: String,
    /// ARGB stripe colour (`solid.leftStripeColor`); 0 when the tile has an icon instead.
    pub color: u32,
    /// Icon type for shortcut tiles (`MUSIC_NEW_RELEASE`, `TRENDING_UP`, …).
    pub icon: Option<String>,
    pub endpoint: BrowseEndpoint,
}

/// A titled group of mood/genre tiles ("Moods & moments", "Genres").
#[derive(Debug, Clone, PartialEq)]
pub struct MoodGroup {
    pub title: String,
    pub items: Vec<MoodItem>,
}

/// One shelf of a browse page.
#[derive(Debug, Clone, Default)]
pub struct Section {
    pub title: String,
    /// Small line above the title (e.g. "Similar to").
    pub strapline: Option<String>,
    /// Header thumbnail (artist avatar for "Similar to …" shelves).
    pub thumbnail: Option<String>,
    /// "See all" / "More" endpoint.
    pub more: Option<BrowseEndpoint>,
    pub items: Vec<ShelfItem>,
    /// The item's subtitle line as YTM shows it, parallel to `items`.
    pub subtitles: Vec<String>,
    /// Items came as list rows (`musicResponsiveListItemRenderer`: Quick picks,
    /// Trending, top songs) rather than cards.
    pub rows: bool,
    /// Grid (`gridRenderer`) rather than a horizontal carousel.
    pub grid: bool,
    /// Mood/genre tiles in this section.
    pub moods: Vec<MoodItem>,
    /// Continuation of this shelf (grid / music shelf pages).
    pub continuation: Option<String>,
}

impl Section {
    /// Songs of this section, in order.
    pub fn songs(&self) -> Vec<Song> {
        self.items
            .iter()
            .filter_map(|i| match i {
                ShelfItem::Song(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    fn push(&mut self, item: ShelfItem, subtitle: String) {
        let id = item_id(&item).to_string();
        if self.items.iter().any(|i| item_id(i) == id) {
            return;
        }
        self.items.push(item);
        self.subtitles.push(subtitle);
    }
}

/// A home-feed mood chip ("Workout", "Relax", …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chip {
    pub title: String,
    /// `params` of the chip's `FEmusic_home` endpoint.
    pub params: Option<String>,
}

/// Home feed page (Kotlin `HomePage`).
#[derive(Debug, Clone, Default)]
pub struct HomePage {
    pub chips: Vec<Chip>,
    pub sections: Vec<Section>,
    pub continuation: Option<String>,
}

/// Explore tab (`FEmusic_explore`).
#[derive(Debug, Clone, Default)]
pub struct ExplorePage {
    /// Shortcut tiles (New releases, Charts, Moods & genres).
    pub shortcuts: Vec<MoodItem>,
    /// Mood & genre tiles of the "Moods & genres" carousel.
    pub moods: Vec<MoodItem>,
    /// New release albums (when YTM still lists them on Explore).
    pub new_release_albums: Vec<Album>,
    /// Every other shelf (Trending, New music videos, …).
    pub sections: Vec<Section>,
}

/// Generic browse page (`BrowseResult` / `ArtistItemsPage`).
#[derive(Debug, Clone, Default)]
pub struct BrowsePage {
    pub title: String,
    pub sections: Vec<Section>,
    /// Next page of sections.
    pub continuation: Option<String>,
}

/// Result of a browse continuation: either more sections (section list
/// continuation) or more items for the shelf that asked.
#[derive(Debug, Clone, Default)]
pub struct ContinuationPage {
    pub sections: Vec<Section>,
    pub items: Vec<ShelfItem>,
    pub subtitles: Vec<String>,
    pub continuation: Option<String>,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

fn item_id(item: &ShelfItem) -> &str {
    match item {
        ShelfItem::Song(s) => &s.id,
        ShelfItem::Album(a) => &a.id,
        ShelfItem::Artist(a) => &a.id,
        ShelfItem::Playlist(p) => &p.id,
    }
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map(|a| a.as_slice()).unwrap_or(&[])
}

fn opt_text(v: &Value) -> Option<String> {
    let t = parse::nav_text(v).trim().to_string();
    (!t.is_empty()).then_some(t)
}

/// Subtitle of a list row: every flex column after the title, joined with " • ".
fn list_subtitle(r: &Value) -> String {
    let r = r.get("musicResponsiveListItemRenderer").unwrap_or(r);
    arr(&r["flexColumns"])
        .iter()
        .skip(1)
        .filter_map(|c| opt_text(&c["musicResponsiveListItemFlexColumnRenderer"]["text"]))
        .collect::<Vec<_>>()
        .join(" • ")
}

/// Parse one mood/genre tile.
pub fn mood_item(r: &Value) -> Option<MoodItem> {
    let r = r.get("musicNavigationButtonRenderer").unwrap_or(r);
    let title = opt_text(&r["buttonText"])?;
    let endpoint = BrowseEndpoint::from_json(&r["clickCommand"]["browseEndpoint"])?;
    let color = r["solid"]["leftStripeColor"].as_u64().unwrap_or(0) as u32;
    let icon = r["iconStyle"]["icon"]["iconType"].as_str().map(str::to_string);
    Some(MoodItem { title, color, icon, endpoint })
}

/// Add one shelf entry (card, row or tile) to `section`.
fn push_entry(section: &mut Section, c: &Value) {
    if let Some(r) = c.get("musicTwoRowItemRenderer") {
        if let Some(item) = parse::two_row_to_shelf_item(r) {
            section.push(item, parse::nav_text(&r["subtitle"]));
        }
    } else if let Some(r) = c.get("musicResponsiveListItemRenderer") {
        if let Some(item) = parse::list_item(r) {
            section.rows = true;
            section.push(item, list_subtitle(r));
        }
    } else if let Some(r) = c.get("musicNavigationButtonRenderer")
        && let Some(m) = mood_item(r)
    {
        section.moods.push(m);
    }
}

fn next_continuation(v: &Value) -> Option<String> {
    v["continuations"][0]["nextContinuationData"]["continuation"]
        .as_str()
        .or_else(|| v["continuations"][0]["reloadContinuationData"]["continuation"].as_str())
        .map(str::to_string)
}

/// Token of a trailing `continuationItemRenderer` in an item list.
fn continuation_item_token(items: &[Value]) -> Option<String> {
    items.iter().rev().find_map(|c| {
        let r = &c["continuationItemRenderer"]["continuationEndpoint"];
        r["continuationCommand"]["token"].as_str().map(str::to_string)
    })
}

/// One `sectionListRenderer` entry → section (`None` for unknown/empty shelves).
pub fn section_from_json(s: &Value) -> Option<Section> {
    let mut sec = Section::default();
    if let Some(r) = s.get("musicCarouselShelfRenderer").or_else(|| s.get("musicImmersiveCarouselShelfRenderer")) {
        let h = r["header"]
            .get("musicCarouselShelfBasicHeaderRenderer")
            .or_else(|| r["header"].get("musicImmersiveCarouselShelfBasicHeaderRenderer"))
            .unwrap_or(&Value::Null);
        sec.title = parse::nav_text(&h["title"]);
        sec.strapline = opt_text(&h["strapline"]);
        sec.thumbnail = parse::thumbnail_url(&h["thumbnail"]["musicThumbnailRenderer"]["thumbnail"]);
        sec.more = BrowseEndpoint::from_json(&h["moreContentButton"]["buttonRenderer"]["navigationEndpoint"]["browseEndpoint"])
            .or_else(|| BrowseEndpoint::from_json(&h["title"]["runs"][0]["navigationEndpoint"]["browseEndpoint"]));
        for c in arr(&r["contents"]) {
            push_entry(&mut sec, c);
        }
    } else if let Some(r) = s.get("gridRenderer") {
        sec.grid = true;
        sec.title = parse::nav_text(&r["header"]["gridHeaderRenderer"]["title"]);
        for c in arr(&r["items"]) {
            push_entry(&mut sec, c);
        }
        sec.continuation = next_continuation(r);
    } else if let Some(r) = s.get("musicShelfRenderer").or_else(|| s.get("musicPlaylistShelfRenderer")) {
        sec.title = parse::nav_text(&r["title"]);
        sec.more = BrowseEndpoint::from_json(&r["bottomEndpoint"]["browseEndpoint"])
            .or_else(|| BrowseEndpoint::from_json(&r["title"]["runs"][0]["navigationEndpoint"]["browseEndpoint"]));
        for c in arr(&r["contents"]) {
            push_entry(&mut sec, c);
        }
        sec.rows = true;
        sec.continuation = next_continuation(r).or_else(|| continuation_item_token(arr(&r["contents"])));
    } else {
        return None;
    }
    (!sec.items.is_empty() || !sec.moods.is_empty()).then_some(sec)
}

fn first_tab_section_list(json: &Value) -> &Value {
    let single = &json["contents"]["singleColumnBrowseResultsRenderer"]["tabs"][0]["tabRenderer"]["content"]["sectionListRenderer"];
    if single.is_object() {
        return single;
    }
    &json["contents"]["twoColumnBrowseResultsRenderer"]["secondaryContents"]["sectionListRenderer"]
}

fn header_title(json: &Value) -> String {
    let h = &json["header"];
    ["musicHeaderRenderer", "musicImmersiveHeaderRenderer", "musicVisualHeaderRenderer", "musicDetailHeaderRenderer"]
        .iter()
        .find_map(|k| opt_text(&h[*k]["title"]))
        .unwrap_or_default()
}

/// Parse any browse page: header title, sections, section-list continuation.
pub fn browse_page_response(json: &Value) -> BrowsePage {
    let list = first_tab_section_list(json);
    BrowsePage {
        title: header_title(json),
        sections: arr(&list["contents"]).iter().filter_map(section_from_json).collect(),
        continuation: next_continuation(list),
    }
}

/// Home page: chips + sections + continuation (first page or continuation).
pub fn home_page_response(json: &Value) -> HomePage {
    let first = first_tab_section_list(json);
    if first.is_object() {
        let chips = arr(&first["header"]["chipCloudRenderer"]["chips"])
            .iter()
            .filter_map(|c| {
                let r = &c["chipCloudChipRenderer"];
                Some(Chip {
                    title: opt_text(&r["text"])?,
                    params: r["navigationEndpoint"]["browseEndpoint"]["params"].as_str().map(str::to_string),
                })
            })
            .collect();
        return HomePage {
            chips,
            sections: arr(&first["contents"]).iter().filter_map(section_from_json).collect(),
            continuation: next_continuation(first),
        };
    }
    let page = continuation_response(json);
    HomePage { chips: Vec::new(), sections: page.sections, continuation: page.continuation }
}

/// Explore page (`FEmusic_explore`).
pub fn explore_response(json: &Value) -> ExplorePage {
    let list = first_tab_section_list(json);
    let mut page = ExplorePage::default();
    for s in arr(&list["contents"]) {
        let Some(sec) = section_from_json(s) else { continue };
        let more_id = sec.more.as_ref().map(|m| m.browse_id.as_str());
        if !sec.moods.is_empty() && more_id == Some("FEmusic_moods_and_genres") {
            page.moods = sec.moods;
        } else if !sec.moods.is_empty() && sec.items.is_empty() {
            page.shortcuts.extend(sec.moods);
        } else if more_id == Some("FEmusic_new_releases_albums") {
            page.new_release_albums = sec
                .items
                .iter()
                .filter_map(|i| if let ShelfItem::Album(a) = i { Some(a.clone()) } else { None })
                .collect();
        } else {
            page.sections.push(sec);
        }
    }
    page
}

/// Moods & genres page (`FEmusic_moods_and_genres`): one group per grid.
pub fn moods_and_genres_response(json: &Value) -> Vec<MoodGroup> {
    let list = first_tab_section_list(json);
    arr(&list["contents"])
        .iter()
        .filter_map(|s| {
            let g = s.get("gridRenderer")?;
            let title = parse::nav_text(&g["header"]["gridHeaderRenderer"]["title"]);
            let items: Vec<MoodItem> = arr(&g["items"]).iter().filter_map(mood_item).collect();
            (!items.is_empty()).then_some(MoodGroup { title, items })
        })
        .collect()
}

/// Any browse continuation response.
pub fn continuation_response(json: &Value) -> ContinuationPage {
    let mut page = ContinuationPage::default();
    let cc = &json["continuationContents"];
    fn collect(items: &[Value], page: &mut ContinuationPage) {
        let mut tmp = Section::default();
        for c in items {
            push_entry(&mut tmp, c);
        }
        page.items = tmp.items;
        page.subtitles = tmp.subtitles;
    }
    if let Some(sl) = cc.get("sectionListContinuation") {
        page.sections = arr(&sl["contents"]).iter().filter_map(section_from_json).collect();
        page.continuation = next_continuation(sl);
    } else if let Some(g) = cc.get("gridContinuation") {
        collect(arr(&g["items"]), &mut page);
        page.continuation = next_continuation(g);
    } else if let Some(m) = cc.get("musicShelfContinuation").or_else(|| cc.get("musicPlaylistShelfContinuation")) {
        collect(arr(&m["contents"]), &mut page);
        page.continuation = next_continuation(m).or_else(|| continuation_item_token(arr(&m["contents"])));
    } else {
        let items = arr(&json["onResponseReceivedActions"][0]["appendContinuationItemsAction"]["continuationItems"]);
        collect(items, &mut page);
        page.continuation = continuation_item_token(items);
    }
    if page.items.is_empty() && page.sections.is_empty() {
        // Nothing new: stop paging even if a token was sent.
        page.continuation = None;
    }
    page
}

// ---------------------------------------------------------------------------
// Client calls
// ---------------------------------------------------------------------------

impl YtmClient {
    /// Raw `browse` response (fixture recording, diagnostics).
    pub async fn browse_raw(&self, browse_id: Option<&str>, params: Option<&str>, continuation: Option<&str>) -> YtmResult<Value> {
        self.browse(browse_id, params, continuation).await
    }

    /// Home feed, optionally filtered by a mood chip's `params`.
    pub async fn home_page(&self, params: Option<&str>) -> YtmResult<HomePage> {
        let json = self.browse(Some("FEmusic_home"), params, None).await?;
        Ok(home_page_response(&json))
    }

    /// Next page of the home feed.
    pub async fn home_continuation(&self, token: &str) -> YtmResult<HomePage> {
        let json = self.browse(None, None, Some(token)).await?;
        Ok(home_page_response(&json))
    }

    /// Explore tab.
    pub async fn explore(&self) -> YtmResult<ExplorePage> {
        let json = self.browse(Some("FEmusic_explore"), None, None).await?;
        Ok(explore_response(&json))
    }

    /// Moods & genres tiles.
    pub async fn moods_and_genres(&self) -> YtmResult<Vec<MoodGroup>> {
        let json = self.browse(Some("FEmusic_moods_and_genres"), None, None).await?;
        Ok(moods_and_genres_response(&json))
    }

    /// Charts (`FEmusic_charts`).
    pub async fn charts(&self) -> YtmResult<BrowsePage> {
        self.browse_page(&BrowseEndpoint::new("FEmusic_charts", Some(CHARTS_PARAMS.to_string()))).await
    }

    /// New releases (`FEmusic_new_releases`).
    pub async fn new_releases(&self) -> YtmResult<BrowsePage> {
        self.browse_page(&BrowseEndpoint::new("FEmusic_new_releases", None)).await
    }

    /// Any browse endpoint as a generic page (genre/mood pages, "see all").
    pub async fn browse_page(&self, ep: &BrowseEndpoint) -> YtmResult<BrowsePage> {
        let json = self.browse(Some(&ep.browse_id), ep.params.as_deref(), None).await?;
        Ok(browse_page_response(&json))
    }

    /// Next page of a browse page or of one of its shelves.
    pub async fn browse_continuation(&self, token: &str) -> YtmResult<ContinuationPage> {
        let json = self.browse(None, None, Some(token)).await?;
        Ok(continuation_response(&json))
    }

    /// The "see all" endpoint of an artist page shelf titled `shelf_title`
    /// (Kotlin `ArtistItemsScreen` entry), or `None` when the shelf has none.
    pub async fn artist_shelf_endpoint(&self, artist_id: &str, shelf_title: &str) -> YtmResult<Option<BrowseEndpoint>> {
        let json = self.browse(Some(artist_id), None, None).await?;
        let page = browse_page_response(&json);
        Ok(page.sections.into_iter().find(|s| s.title == shelf_title).and_then(|s| s.more))
    }
}
