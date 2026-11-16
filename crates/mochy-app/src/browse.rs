//! State and background loads of the Explore / browse / library-parity /
//! stats pages.
//!
//! [`BrowseState`] owns the home feed (mood chips, continuation), Explore,
//! moods & genres, charts, generic browse pages ("see all", genre/mood
//! pages), the library entries (albums, artists, history events, recently
//! added, saved ids) and the listening stats, so `app.rs` only forwards
//! [`BrowseAction`]s and [`BrowseMsg`]s and runs the returned [`Effect`]s.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use mochy_core::models::{Album, Artist, ArtistRef, Playlist, ShelfItem, Song};
use mochy_innertube::explore::{BrowseEndpoint, BrowsePage, ContinuationPage, ExplorePage, HomePage, MoodGroup};
use mochy_storage::{AlbumEntry, ArtistEntry, HistoryEntry, ListeningBuckets, RecentItem, StatAlbum, StatArtist, StatSong, StatsPeriod, StatsSummary};

use crate::actions::{Action, LoadState};
use crate::nav::Route;
use crate::pages::Data;
use crate::services::{AppMsg, Services};
use crate::ui::i18n::tr;

const DAY_MS: i64 = 86_400_000;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Mutable async slot (continuations append to `Ready` data in place).
#[derive(Debug, Clone)]
pub enum Slot<T> {
    Loading,
    Ready(T),
    Failed(String),
}

/// What a collection (album / playlist / artist card) should do once its songs are known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionOp {
    Play,
    Shuffle,
    PlayNext,
    AddToQueue,
    Radio,
}

/// A collection whose songs may need fetching first.
#[derive(Debug, Clone)]
pub enum CollectionRef {
    /// YTM album browse id (`MPRE…`).
    Album(String),
    /// YTM playlist (browse id `VL…` or bare id).
    Playlist(String),
    /// YTM artist (top songs).
    Artist(String),
    /// Local DB playlist.
    LocalPlaylist(String),
}

/// Intents of the new pages (`Action::Browse`).
#[derive(Debug, Clone)]
pub enum BrowseAction {
    /// Select (`Some`) / clear (`None`) a home mood chip.
    HomeChip(Option<usize>),
    /// Load the next home page (scrolled to the bottom).
    HomeMore,
    /// Next page of a browse page (`section: None`) or of one shelf.
    PageMore { key: String, section: Option<usize> },
    /// Retry a failed page (`page_key`, `"explore"`, `"moods"`, `"stats"`).
    Retry(String),
    Collection { target: CollectionRef, op: CollectionOp },
    SaveAlbum { album: Album, artists: Vec<ArtistRef>, save: bool },
    SaveArtist { artist: Artist, save: bool },
    SavePlaylist { playlist: Playlist, save: bool },
    SetInLibrary { song: Song, add: bool },
    RemoveHistoryEvent(i64),
    RemoveSongHistory(String),
    ClearHistory,
}

/// Side effects for `app.rs` to run.
pub enum Effect {
    Act(Action),
    /// Insert songs right after the current one, keeping their order.
    PlayNext(Vec<Song>),
    Toast(String),
}

/// Ids the menus need to show "Save" vs "Remove from library".
#[derive(Debug, Clone, Default)]
pub struct Saved {
    pub albums: HashSet<String>,
    pub artists: HashSet<String>,
    pub playlists: HashSet<String>,
    /// Songs in the library (`inLibrary`).
    pub songs: HashSet<String>,
}

/// Library data read from the DB in one job.
#[derive(Default)]
pub struct LibData {
    pub songs: Arc<Vec<Song>>,
    pub albums: Arc<Vec<AlbumEntry>>,
    pub artists: Arc<Vec<ArtistEntry>>,
    pub recent_added: Arc<Vec<RecentItem>>,
    pub history: Arc<Vec<HistoryEntry>>,
    /// Songs of `history`, same order (row playback list).
    pub history_songs: Arc<Vec<Song>>,
    /// Distinct recently played songs.
    pub recent_songs: Arc<Vec<Song>>,
    /// Most played of the last 30 days (Android "Quick picks").
    pub quick_picks: Arc<Vec<Song>>,
    pub forgotten: Arc<Vec<Song>>,
    pub utc_offset_secs: i64,
}

pub struct LibSnapshot {
    songs: Vec<Song>,
    albums: Vec<AlbumEntry>,
    artists: Vec<ArtistEntry>,
    recent_added: Vec<RecentItem>,
    history: Vec<HistoryEntry>,
    recent_songs: Vec<Song>,
    quick_picks: Vec<Song>,
    forgotten: Vec<Song>,
    utc_offset_secs: i64,
    saved: Saved,
}

/// Home feed with mood chips and continuation.
#[derive(Default)]
pub struct HomeFeed {
    pub status: Option<Slot<()>>,
    pub chips: Vec<mochy_innertube::explore::Chip>,
    pub selected: Option<usize>,
    pub sections: Vec<mochy_innertube::explore::Section>,
    pub continuation: Option<String>,
    pub loading_more: bool,
    generation: u64,
    /// Unfiltered feed kept while a chip is selected.
    unfiltered: Option<(Vec<mochy_innertube::explore::Section>, Option<String>)>,
}

/// One generic browse page.
pub struct PageSlot {
    pub status: Slot<()>,
    pub page: BrowsePage,
    /// "See all" of an artist shelf that turned out to be a playlist.
    pub songs: Option<Arc<Vec<Song>>>,
    pub loading_more: bool,
}

impl PageSlot {
    fn loading() -> Self {
        Self { status: Slot::Loading, page: BrowsePage::default(), songs: None, loading_more: false }
    }
}

/// Library list views: search, sort, layout (kept for the session).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SongSort {
    #[default]
    Added,
    Name,
    Artist,
    PlayTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlbumSort {
    #[default]
    Added,
    Name,
    Artist,
    Year,
    SongCount,
    PlayTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArtistSort {
    #[default]
    Added,
    Name,
    SongCount,
    PlayTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaylistSort {
    #[default]
    Added,
    Name,
    SongCount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SongFilter {
    #[default]
    Library,
    Liked,
    Local,
}

#[derive(Debug, Clone, Default)]
pub struct LibView {
    pub query: String,
    pub song_sort: SongSort,
    pub song_desc: bool,
    pub song_filter: SongFilter,
    pub album_sort: AlbumSort,
    pub album_desc: bool,
    pub artist_sort: ArtistSort,
    pub artist_desc: bool,
    pub playlist_sort: PlaylistSort,
    pub playlist_desc: bool,
    pub saved_only: bool,
    pub list_layout: bool,
}

/// Listening stats of one period.
#[derive(Debug, Clone, Default)]
pub struct StatsData {
    pub period: Option<StatsPeriod>,
    pub offset: u32,
    /// Local `YYYY-MM-DD` of the first and last day of the period.
    pub first_day: String,
    pub last_day: String,
    pub summary: StatsSummary,
    pub top_songs: Vec<StatSong>,
    /// `top_songs` as a playable list.
    pub top_song_list: Arc<Vec<Song>>,
    pub top_artists: Vec<StatArtist>,
    pub top_albums: Vec<StatAlbum>,
    pub buckets: ListeningBuckets,
    /// Bars of the "by day" chart: (bucket key, ms) — days (`YYYY-MM-DD`) for
    /// week/month, months (`YYYY-MM`) for year/all time.
    pub timeline: Vec<(String, i64)>,
    /// Local day of the very first recorded play.
    pub since_day: Option<String>,
}

pub struct RecapUi {
    pub page: usize,
    pub page_started: Instant,
}

pub struct StatsUi {
    pub period: StatsPeriod,
    pub offset: u32,
    pub data: Option<Slot<Arc<StatsData>>>,
    loaded_key: Option<(StatsPeriod, u32)>,
    dirty: bool,
    pub recap: Option<RecapUi>,
}

impl Default for StatsUi {
    fn default() -> Self {
        Self { period: StatsPeriod::Month, offset: 0, data: None, loaded_key: None, dirty: false, recap: None }
    }
}

/// Messages from background jobs (`AppMsg::Browse`).
pub enum BrowseMsg {
    Home { generation: u64, result: Result<HomePage, String> },
    HomeMore { generation: u64, result: Result<HomePage, String> },
    Explore(Result<ExplorePage, String>),
    Moods(Result<Vec<MoodGroup>, String>),
    Page { key: String, result: Result<BrowsePage, String> },
    PageSongs { key: String, result: Result<Vec<Song>, String> },
    PageMore { key: String, section: Option<usize>, result: Result<ContinuationPage, String> },
    Library(Box<LibSnapshot>),
    LocalSongs { key: String, songs: Vec<Song> },
    Stats { key: (StatsPeriod, u32), result: Result<Box<StatsData>, String> },
    Collection { op: CollectionOp, result: Result<Vec<Song>, String> },
}

#[derive(Default)]
pub struct BrowseState {
    pub home: HomeFeed,
    pub explore: Option<Slot<ExplorePage>>,
    pub moods: Option<Slot<Vec<MoodGroup>>>,
    pub pages: HashMap<String, PageSlot>,
    pub lib: LibData,
    pub lib_view: LibView,
    pub saved: Arc<Saved>,
    saved_dirty: bool,
    /// Songs of local artist / album pages (`"artist:<id>"`, `"album:<id>"`).
    pub local_songs: HashMap<String, Arc<Vec<Song>>>,
    pub stats: StatsUi,
    /// Filtered/sorted song lists of the library views, keyed by view.
    pub view_cache: HashMap<&'static str, (u64, Arc<Vec<Song>>)>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Page-map key of a route that shows a generic browse page.
pub fn page_key(route: &Route) -> Option<String> {
    match route {
        Route::Charts => Some("charts".into()),
        Route::NewReleases => Some("new_releases".into()),
        Route::Browse { id, params } => Some(format!("b:{id}:{}", params.as_deref().unwrap_or(""))),
        Route::ArtistMore { artist, title } => Some(format!("am:{artist}:{title}")),
        _ => None,
    }
}

/// Route for a "see all" / tile endpoint.
pub fn route_for_endpoint(ep: &BrowseEndpoint) -> Route {
    match ep.browse_id.as_str() {
        "FEmusic_moods_and_genres" => Route::MoodsGenres,
        "FEmusic_charts" => Route::Charts,
        "FEmusic_new_releases" => Route::NewReleases,
        "FEmusic_explore" => Route::Explore,
        id if id.starts_with("VL") => Route::Playlist(id.to_string()),
        id if id.starts_with("MPRE") => Route::Album(id.to_string()),
        id if id.starts_with("UC") && ep.params.is_none() => Route::Artist(id.to_string()),
        id => Route::Browse { id: id.to_string(), params: ep.params.clone() },
    }
}

/// Top songs of an artist page (first all-song shelf), like `detail::artist`.
pub fn artist_top_songs(shelves: &[mochy_core::models::Shelf]) -> Vec<Song> {
    shelves
        .iter()
        .find(|s| !s.items.is_empty() && s.items.iter().all(|i| matches!(i, ShelfItem::Song(_))))
        .map(|s| s.items.iter().filter_map(|i| if let ShelfItem::Song(s) = i { Some(s.clone()) } else { None }).collect())
        .unwrap_or_default()
}

/// Album tracks inherit the album's id/title/art (same as `app.rs`).
fn fill_album(al: &Album, mut songs: Vec<Song>) -> Vec<Song> {
    for s in &mut songs {
        if s.album_id.is_none() {
            s.album_id = Some(al.id.clone());
        }
        if s.album_name.is_none() {
            s.album_name = Some(al.title.clone());
        }
        if s.thumbnail_url.is_none() {
            s.thumbnail_url = al.thumbnail_url.clone();
        }
    }
    songs
}

fn send(svc: &Services, m: BrowseMsg) {
    svc.bus.send(AppMsg::Browse(m));
}

fn finish_collection(op: CollectionOp, songs: Vec<Song>) -> Vec<Effect> {
    let songs: Vec<Song> = songs.into_iter().filter(|s| s.video_id.is_some() || s.local_path.is_some()).collect();
    if songs.is_empty() {
        return vec![Effect::Toast(tr("Nothing to play").into())];
    }
    match op {
        CollectionOp::Play => vec![Effect::Act(Action::PlayList { songs: Arc::new(songs), start: 0, shuffle: false })],
        CollectionOp::Shuffle => vec![Effect::Act(Action::PlayList { songs: Arc::new(songs), start: 0, shuffle: true })],
        CollectionOp::PlayNext => vec![Effect::PlayNext(songs)],
        CollectionOp::AddToQueue => vec![Effect::Act(Action::AddToQueue(songs))],
        CollectionOp::Radio => {
            let seed = songs.into_iter().find(|s| s.video_id.is_some());
            match seed {
                Some(s) => vec![Effect::Act(Action::StartRadio(s))],
                None => vec![Effect::Toast(tr("Nothing to play").into())],
            }
        }
    }
}

// ---------------------------------------------------------------------------
// BrowseState
// ---------------------------------------------------------------------------

impl BrowseState {
    /// Saved ids for the menus (also pushed into egui memory, see `widgets::menus`).
    pub fn publish_saved(&mut self, ctx: &egui::Context) {
        if self.saved_dirty {
            self.saved_dirty = false;
            crate::widgets::menus::set_saved(ctx, self.saved.clone());
        }
    }

    // -- home -------------------------------------------------------------

    pub fn home_failed(&self) -> bool {
        matches!(self.home.status, Some(Slot::Failed(_)))
    }

    /// (Re)load the home feed, keeping the selected chip.
    pub fn load_home(&mut self, svc: &Services) {
        self.home.generation += 1;
        self.home.status = Some(Slot::Loading);
        self.home.loading_more = false;
        let generation = self.home.generation;
        let params = self.home.selected.and_then(|i| self.home.chips.get(i)).and_then(|c| c.params.clone());
        svc.ytm_call(
            "home",
            move |c| async move { c.home_page(params.as_deref()).await },
            move |result| AppMsg::Browse(BrowseMsg::Home { generation, result }),
        );
    }

    fn select_chip(&mut self, chip: Option<usize>, svc: &Services) {
        if chip == self.home.selected || chip.is_none() {
            // Deselect: restore the unfiltered feed.
            self.home.selected = None;
            self.home.generation += 1;
            if let Some((sections, cont)) = self.home.unfiltered.take() {
                self.home.sections = sections;
                self.home.continuation = cont;
                self.home.status = Some(Slot::Ready(()));
                self.home.loading_more = false;
            } else {
                self.load_home(svc);
            }
            return;
        }
        if self.home.selected.is_none() {
            self.home.unfiltered = Some((std::mem::take(&mut self.home.sections), self.home.continuation.take()));
        }
        self.home.selected = chip;
        self.load_home(svc);
    }

    fn home_more(&mut self, svc: &Services) {
        if self.home.loading_more || !matches!(self.home.status, Some(Slot::Ready(_))) {
            return;
        }
        let Some(token) = self.home.continuation.clone() else { return };
        self.home.loading_more = true;
        let generation = self.home.generation;
        svc.ytm_call(
            "home_more",
            move |c| async move { c.home_continuation(&token).await },
            move |result| AppMsg::Browse(BrowseMsg::HomeMore { generation, result }),
        );
    }

    // -- pages ------------------------------------------------------------

    /// Start loads the current route needs (called every frame; cheap).
    pub fn ensure(&mut self, route: &Route, svc: &Services) {
        match route {
            Route::Explore => {
                if self.explore.is_none() {
                    self.load_explore(svc);
                }
                if !self.pages.contains_key("charts") {
                    self.load_page("charts".into(), Route::Charts, svc);
                }
            }
            Route::MoodsGenres if self.moods.is_none() => self.load_moods(svc),
            Route::Charts | Route::NewReleases | Route::Browse { .. } | Route::ArtistMore { .. } => {
                if let Some(key) = page_key(route)
                    && !self.pages.contains_key(&key)
                {
                    self.load_page(key, route.clone(), svc);
                }
            }
            Route::Stats => {
                let key = (self.stats.period, self.stats.offset);
                let loading = matches!(self.stats.data, Some(Slot::Loading));
                if (self.stats.loaded_key != Some(key) || self.stats.dirty) && !loading {
                    self.load_stats(svc);
                }
            }
            Route::LibraryArtist(id) => self.ensure_local(format!("artist:{id}"), svc),
            Route::LibraryAlbum(id) => self.ensure_local(format!("album:{id}"), svc),
            _ => {}
        }
    }

    fn load_explore(&mut self, svc: &Services) {
        self.explore = Some(Slot::Loading);
        svc.ytm_call("explore", |c| async move { c.explore().await }, |r| AppMsg::Browse(BrowseMsg::Explore(r)));
    }

    fn load_moods(&mut self, svc: &Services) {
        self.moods = Some(Slot::Loading);
        svc.ytm_call("moods", |c| async move { c.moods_and_genres().await }, |r| AppMsg::Browse(BrowseMsg::Moods(r)));
    }

    fn load_page(&mut self, key: String, route: Route, svc: &Services) {
        self.pages.insert(key.clone(), PageSlot::loading());
        match route {
            Route::Charts => {
                svc.ytm_call("charts", |c| async move { c.charts().await }, move |result| AppMsg::Browse(BrowseMsg::Page { key, result }))
            }
            Route::NewReleases => svc.ytm_call(
                "new_releases",
                |c| async move { c.new_releases().await },
                move |result| AppMsg::Browse(BrowseMsg::Page { key, result }),
            ),
            Route::Browse { id, params } => {
                let ep = BrowseEndpoint::new(id, params);
                svc.ytm_call("browse", move |c| async move { c.browse_page(&ep).await }, move |result| AppMsg::Browse(BrowseMsg::Page { key, result }))
            }
            Route::ArtistMore { artist, title } => {
                let svc2 = svc.clone();
                svc.spawn(async move {
                    let client = match svc2.ytm().await {
                        Ok(c) => c,
                        Err(e) => return send(&svc2, BrowseMsg::Page { key, result: Err(format!("offline: {e}")) }),
                    };
                    let ep = match client.artist_shelf_endpoint(&artist, &title).await {
                        Ok(Some(ep)) => ep,
                        Ok(None) => return send(&svc2, BrowseMsg::Page { key, result: Err(tr("This shelf has no full list.").into()) }),
                        Err(e) => return send(&svc2, BrowseMsg::Page { key, result: Err(e.to_string()) }),
                    };
                    if ep.is_playlist() {
                        let r = client.playlist(&ep.browse_id).await.map(|(_, s)| s).map_err(|e| e.to_string());
                        send(&svc2, BrowseMsg::PageSongs { key, result: r });
                    } else {
                        let r = client.browse_page(&ep).await.map_err(|e| e.to_string());
                        send(&svc2, BrowseMsg::Page { key, result: r });
                    }
                });
            }
            _ => {}
        }
    }

    fn page_more(&mut self, key: String, section: Option<usize>, svc: &Services) {
        let Some(slot) = self.pages.get_mut(&key) else { return };
        if slot.loading_more {
            return;
        }
        let token = match section {
            None => slot.page.continuation.clone(),
            Some(i) => slot.page.sections.get(i).and_then(|s| s.continuation.clone()),
        };
        let Some(token) = token else { return };
        slot.loading_more = true;
        svc.ytm_call(
            "browse_more",
            move |c| async move { c.browse_continuation(&token).await },
            move |result| AppMsg::Browse(BrowseMsg::PageMore { key, section, result }),
        );
    }

    fn ensure_local(&mut self, key: String, svc: &Services) {
        if self.local_songs.contains_key(&key) {
            return;
        }
        self.local_songs.insert(key.clone(), Arc::new(Vec::new()));
        let bus = svc.bus.clone();
        svc.db.run(move |db| {
            let songs = if let Some(id) = key.strip_prefix("artist:") {
                crate::db::log_err("artist_songs", db.artist_songs(id))
            } else if let Some(id) = key.strip_prefix("album:") {
                crate::db::log_err("album_songs", db.album_songs(id))
            } else {
                None
            };
            bus.send(AppMsg::Browse(BrowseMsg::LocalSongs { key, songs: songs.unwrap_or_default() }));
        });
    }

    // -- library ------------------------------------------------------------

    /// Re-read library entries, history, saved ids and home's local shelves.
    pub fn refresh_library(&self, svc: &Services) {
        let bus = svc.bus.clone();
        svc.db.run(move |db| {
            use crate::db::log_err;
            let now = now_ms();
            let history = log_err("history_events", db.history_events(500)).unwrap_or_default();
            let ids = log_err("saved_ids", db.saved_ids()).unwrap_or_default();
            let songs = log_err("all_songs", db.all_songs()).unwrap_or_default();
            let saved = Saved {
                albums: ids.albums,
                artists: ids.artists,
                playlists: ids.playlists,
                songs: songs.iter().map(|s| s.id.clone()).collect(),
            };
            let snap = LibSnapshot {
                albums: log_err("library_album_entries", db.library_album_entries()).unwrap_or_default(),
                artists: log_err("library_artist_entries", db.library_artist_entries()).unwrap_or_default(),
                recent_added: log_err("recently_added", db.recently_added(120)).unwrap_or_default(),
                recent_songs: log_err("recent_history", db.recent_history(30)).unwrap_or_default(),
                quick_picks: log_err("quick_picks", db.top_songs_between(now - 30 * DAY_MS, now + 1, 20))
                    .unwrap_or_default()
                    .into_iter()
                    .map(|s| s.song)
                    .collect(),
                forgotten: log_err("forgotten_favorites", db.forgotten_favorites(now - 14 * DAY_MS, 20)).unwrap_or_default(),
                utc_offset_secs: log_err("utc_offset", db.local_utc_offset_secs()).unwrap_or(0),
                songs,
                history,
                saved,
            };
            bus.send(AppMsg::Browse(BrowseMsg::Library(Box::new(snap))));
        });
    }

    fn db_then_refresh(&mut self, svc: &Services, what: &'static str, f: impl FnOnce(&mochy_storage::Db) -> mochy_storage::StorageResult<()> + Send + 'static) {
        svc.db.run(move |db| {
            crate::db::log_err(what, f(db));
        });
        self.stats.dirty = true;
        self.local_songs.clear();
        self.refresh_library(svc);
    }

    // -- stats ----------------------------------------------------------------

    fn load_stats(&mut self, svc: &Services) {
        let key = (self.stats.period, self.stats.offset);
        self.stats.loaded_key = Some(key);
        self.stats.dirty = false;
        if self.stats.data.is_none() || matches!(self.stats.data, Some(Slot::Failed(_))) {
            self.stats.data = Some(Slot::Loading);
        }
        let bus = svc.bus.clone();
        svc.db.run(move |db| {
            let r = compute_stats(db, key.0, key.1, now_ms()).map(Box::new).map_err(|e| e.to_string());
            bus.send(AppMsg::Browse(BrowseMsg::Stats { key, result: r }));
        });
    }

    /// Change the stats period / offset (the page reloads on the next frame).
    pub fn set_stats_period(&mut self, period: StatsPeriod, offset: u32) {
        self.stats.period = period;
        self.stats.offset = offset;
    }

    /// A play was recorded: stats and history are stale.
    pub fn mark_stale(&mut self) {
        self.stats.dirty = true;
    }

    // -- actions / messages -------------------------------------------------

    pub fn apply(&mut self, a: BrowseAction, svc: &Services, data: &Data) -> Vec<Effect> {
        match a {
            BrowseAction::HomeChip(c) => self.select_chip(c, svc),
            BrowseAction::HomeMore => self.home_more(svc),
            BrowseAction::PageMore { key, section } => self.page_more(key, section, svc),
            BrowseAction::Retry(key) => match key.as_str() {
                "explore" => {
                    self.load_explore(svc);
                    self.pages.remove("charts");
                }
                "moods" => self.load_moods(svc),
                "stats" => self.load_stats(svc),
                _ => {
                    self.pages.remove(&key);
                }
            },
            BrowseAction::Collection { target, op } => return self.collection(target, op, svc, data),
            BrowseAction::SaveAlbum { album, artists, save } => {
                let toast = if save { tr("Saved to library") } else { tr("Removed from library") };
                self.optimistic_saved(|s| toggle(&mut s.albums, &album.id, save));
                self.db_then_refresh(svc, "set_album_saved", move |db| db.set_album_saved(&album, &artists, save));
                return vec![Effect::Toast(toast.into())];
            }
            BrowseAction::SaveArtist { artist, save } => {
                let toast = if save { tr("Saved to library") } else { tr("Removed from library") };
                self.optimistic_saved(|s| toggle(&mut s.artists, &artist.id, save));
                self.db_then_refresh(svc, "set_artist_saved", move |db| db.set_artist_saved(&artist, save));
                return vec![Effect::Toast(toast.into())];
            }
            BrowseAction::SavePlaylist { playlist, save } => {
                let toast = if save { tr("Saved to library") } else { tr("Removed from library") };
                self.optimistic_saved(|s| {
                    toggle(&mut s.playlists, &playlist.id, save);
                    if let Some(b) = &playlist.browse_id {
                        toggle(&mut s.playlists, b, save);
                    }
                });
                self.db_then_refresh(svc, "set_playlist_saved", move |db| db.set_playlist_saved(&playlist, save));
                // The sidebar / playlists tab list DB playlists.
                let bus = svc.bus.clone();
                svc.db.run(move |db| {
                    if let Some(v) = crate::db::log_err("playlists", db.playlists()) {
                        bus.send(AppMsg::Library(crate::services::LibraryMsg::Playlists(v)));
                    }
                });
                return vec![Effect::Toast(toast.into())];
            }
            BrowseAction::SetInLibrary { song, add } => {
                let toast = if add { tr("Added to library") } else { tr("Removed from library") };
                self.optimistic_saved(|s| toggle(&mut s.songs, &song.id, add));
                self.db_then_refresh(svc, "set_in_library", move |db| {
                    db.upsert_song(&song)?;
                    db.set_in_library(&song.id, add)
                });
                return vec![Effect::Toast(toast.into())];
            }
            BrowseAction::RemoveHistoryEvent(id) => {
                self.remove_history_local(|e| e.event_id == id);
                self.db_then_refresh(svc, "delete_event", move |db| db.delete_event(id));
                return vec![Effect::Toast(tr("Removed from history").into())];
            }
            BrowseAction::RemoveSongHistory(song_id) => {
                let sid = song_id.clone();
                self.remove_history_local(move |e| e.song.id == sid);
                self.db_then_refresh(svc, "delete_song_history", move |db| db.delete_song_history(&song_id));
                return vec![Effect::Toast(tr("Removed from history").into())];
            }
            BrowseAction::ClearHistory => {
                self.remove_history_local(|_| true);
                self.db_then_refresh(svc, "clear_history", |db| db.clear_history());
                return vec![Effect::Toast(tr("History cleared").into())];
            }
        }
        Vec::new()
    }

    fn optimistic_saved(&mut self, f: impl FnOnce(&mut Saved)) {
        let mut s = (*self.saved).clone();
        f(&mut s);
        self.saved = Arc::new(s);
        self.saved_dirty = true;
    }

    fn remove_history_local(&mut self, pred: impl Fn(&HistoryEntry) -> bool) {
        let kept: Vec<HistoryEntry> = self.lib.history.iter().filter(|e| !pred(e)).cloned().collect();
        self.lib.history_songs = Arc::new(kept.iter().map(|e| e.song.clone()).collect());
        self.lib.history = Arc::new(kept);
    }

    fn collection(&mut self, target: CollectionRef, op: CollectionOp, svc: &Services, data: &Data) -> Vec<Effect> {
        let done = move |r: Result<Vec<Song>, String>| AppMsg::Browse(BrowseMsg::Collection { op, result: r });
        match target {
            CollectionRef::Album(id) => {
                if let Some(LoadState::Ready(v)) = data.albums.get(&id) {
                    return finish_collection(op, v.1.clone());
                }
                svc.ytm_call("album_songs", move |c| async move { c.album(&id).await.map(|(al, s)| fill_album(&al, s)) }, done);
            }
            CollectionRef::Playlist(id) => {
                if let Some(LoadState::Ready(v)) = data.playlists_remote.get(&id) {
                    return finish_collection(op, v.1.clone());
                }
                svc.ytm_call("playlist_songs", move |c| async move { c.playlist(&id).await.map(|(_, s)| s) }, done);
            }
            CollectionRef::Artist(id) => {
                if let Some(LoadState::Ready(v)) = data.artists.get(&id) {
                    return finish_collection(op, artist_top_songs(&v.1));
                }
                svc.ytm_call("artist_songs", move |c| async move { c.artist(&id).await.map(|(_, sh)| artist_top_songs(&sh)) }, done);
            }
            CollectionRef::LocalPlaylist(id) => {
                if let Some(songs) = data.playlist_songs.get(&id) {
                    return finish_collection(op, songs.as_ref().clone());
                }
                let bus = svc.bus.clone();
                svc.db.run(move |db| {
                    let r = db.playlist_songs(&id).map_err(|e| e.to_string());
                    bus.send(AppMsg::Browse(BrowseMsg::Collection { op, result: r }));
                });
            }
        }
        Vec::new()
    }

    pub fn on_msg(&mut self, ctx: &egui::Context, msg: BrowseMsg, _svc: &Services) -> Vec<Effect> {
        match msg {
            BrowseMsg::Home { generation, result } => {
                if generation != self.home.generation {
                    return Vec::new();
                }
                match result {
                    Ok(page) => {
                        tracing::info!(sections = page.sections.len(), chips = page.chips.len(), "home loaded");
                        if !page.chips.is_empty() {
                            self.home.chips = page.chips;
                        }
                        self.home.sections = page.sections;
                        self.home.continuation = page.continuation;
                        self.home.status = Some(Slot::Ready(()));
                    }
                    Err(e) => self.home.status = Some(Slot::Failed(e)),
                }
            }
            BrowseMsg::HomeMore { generation, result } => {
                if generation != self.home.generation {
                    return Vec::new();
                }
                self.home.loading_more = false;
                match result {
                    Ok(page) => {
                        for s in page.sections {
                            if !self.home.sections.iter().any(|x| x.title == s.title && x.items.len() == s.items.len()) {
                                self.home.sections.push(s);
                            }
                        }
                        self.home.continuation = page.continuation;
                    }
                    Err(e) => {
                        tracing::warn!("home continuation failed: {e}");
                        self.home.continuation = None;
                    }
                }
            }
            BrowseMsg::Explore(r) => {
                self.explore = Some(match r {
                    Ok(p) => Slot::Ready(p),
                    Err(e) => Slot::Failed(e),
                })
            }
            BrowseMsg::Moods(r) => {
                self.moods = Some(match r {
                    Ok(p) => Slot::Ready(p),
                    Err(e) => Slot::Failed(e),
                })
            }
            BrowseMsg::Page { key, result } => {
                let slot = self.pages.entry(key).or_insert_with(PageSlot::loading);
                match result {
                    Ok(p) => {
                        slot.page = p;
                        slot.status = Slot::Ready(());
                    }
                    Err(e) => slot.status = Slot::Failed(e),
                }
            }
            BrowseMsg::PageSongs { key, result } => {
                let slot = self.pages.entry(key).or_insert_with(PageSlot::loading);
                match result {
                    Ok(s) => {
                        slot.songs = Some(Arc::new(s));
                        slot.status = Slot::Ready(());
                    }
                    Err(e) => slot.status = Slot::Failed(e),
                }
            }
            BrowseMsg::PageMore { key, section, result } => {
                let Some(slot) = self.pages.get_mut(&key) else { return Vec::new() };
                slot.loading_more = false;
                match (result, section) {
                    (Ok(more), None) => {
                        slot.page.sections.extend(more.sections);
                        slot.page.continuation = more.continuation;
                    }
                    (Ok(more), Some(i)) => {
                        if let Some(sec) = slot.page.sections.get_mut(i) {
                            for (item, sub) in more.items.into_iter().zip(more.subtitles) {
                                sec.items.push(item);
                                sec.subtitles.push(sub);
                            }
                            sec.continuation = more.continuation;
                        }
                    }
                    (Err(e), None) => {
                        tracing::warn!("browse continuation failed: {e}");
                        slot.page.continuation = None;
                    }
                    (Err(e), Some(i)) => {
                        tracing::warn!("shelf continuation failed: {e}");
                        if let Some(sec) = slot.page.sections.get_mut(i) {
                            sec.continuation = None;
                        }
                    }
                }
            }
            BrowseMsg::Library(snap) => {
                let snap = *snap;
                self.lib = LibData {
                    history_songs: Arc::new(snap.history.iter().map(|e| e.song.clone()).collect()),
                    songs: Arc::new(snap.songs),
                    albums: Arc::new(snap.albums),
                    artists: Arc::new(snap.artists),
                    recent_added: Arc::new(snap.recent_added),
                    history: Arc::new(snap.history),
                    recent_songs: Arc::new(snap.recent_songs),
                    quick_picks: Arc::new(snap.quick_picks),
                    forgotten: Arc::new(snap.forgotten),
                    utc_offset_secs: snap.utc_offset_secs,
                };
                self.saved = Arc::new(snap.saved);
                self.saved_dirty = true;
                self.publish_saved(ctx);
            }
            BrowseMsg::LocalSongs { key, songs } => {
                self.local_songs.insert(key, Arc::new(songs));
            }
            BrowseMsg::Stats { key, result } => {
                if key != (self.stats.period, self.stats.offset) {
                    return Vec::new();
                }
                self.stats.data = Some(match result {
                    Ok(d) => Slot::Ready(Arc::new(*d)),
                    Err(e) => Slot::Failed(e),
                });
            }
            BrowseMsg::Collection { op, result } => {
                return match result {
                    Ok(songs) => finish_collection(op, songs),
                    Err(e) => vec![Effect::Toast(format!("{}: {e}", tr("Couldn't load songs")))],
                };
            }
        }
        Vec::new()
    }
}

fn toggle(set: &mut HashSet<String>, id: &str, on: bool) {
    if on {
        set.insert(id.to_string());
    } else {
        set.remove(id);
    }
}

// ---------------------------------------------------------------------------
// Stats computation (DB thread)
// ---------------------------------------------------------------------------

/// Aggregate one period. Runs on the DB worker.
pub fn compute_stats(db: &mochy_storage::Db, period: StatsPeriod, offset: u32, now: i64) -> mochy_storage::StorageResult<StatsData> {
    let first_event = db.first_event_time()?;
    let (mut from, to) = db.stats_period_bounds(period, offset, now)?;
    if period == StatsPeriod::All {
        from = first_event.unwrap_or(now);
    }
    let summary = db.stats_summary(from, to)?;
    let top_songs = db.top_songs_between(from, to, 50)?;
    let top_artists = db.top_artists_between(from, to, 25)?;
    let top_albums = db.top_albums_between(from, to, 20)?;
    let buckets = db.listening_buckets(from, to)?;
    let first_day = db.local_day(from + DAY_MS / 2)?;
    let last_day = db.local_day((to - 1).max(from))?;
    let per_day: HashMap<&str, i64> = buckets.by_day.iter().map(|(d, ms)| (d.as_str(), *ms)).collect();
    let timeline: Vec<(String, i64)> = match period {
        StatsPeriod::Week | StatsPeriod::Month => {
            let days = (((to - from) as f64) / DAY_MS as f64).round().max(1.0) as i64;
            let mut v = Vec::with_capacity(days as usize);
            for i in 0..days {
                let d = db.local_day(from + i * DAY_MS + DAY_MS / 2)?;
                let ms = per_day.get(d.as_str()).copied().unwrap_or(0);
                v.push((d, ms));
            }
            v
        }
        StatsPeriod::Year => {
            let year = first_day.get(..4).unwrap_or("0000").to_string();
            (1..=12)
                .map(|m| {
                    let key = format!("{year}-{m:02}");
                    let ms = buckets.by_day.iter().filter(|(d, _)| d.starts_with(&key)).map(|(_, ms)| ms).sum();
                    (key, ms)
                })
                .collect()
        }
        StatsPeriod::All => {
            let mut months: Vec<(String, i64)> = Vec::new();
            for (d, ms) in &buckets.by_day {
                let key = d.get(..7).unwrap_or("").to_string();
                match months.last_mut() {
                    Some((k, total)) if *k == key => *total += ms,
                    _ => months.push((key, *ms)),
                }
            }
            // Fill gaps so the bars are evenly spaced in time.
            let mut filled: Vec<(String, i64)> = Vec::new();
            for (k, ms) in months {
                if let Some((last, _)) = filled.last() {
                    let mut next = next_month(last);
                    while next < k && filled.len() < 600 {
                        filled.push((next.clone(), 0));
                        next = next_month(&next);
                    }
                }
                filled.push((k, ms));
            }
            let n = filled.len();
            filled.into_iter().skip(n.saturating_sub(36)).collect()
        }
    };
    Ok(StatsData {
        period: Some(period),
        offset,
        first_day,
        last_day,
        summary,
        top_song_list: Arc::new(top_songs.iter().map(|s| s.song.clone()).collect()),
        top_songs,
        top_artists,
        top_albums,
        buckets,
        timeline,
        since_day: first_event.map(|t| db.local_day(t)).transpose()?,
    })
}

/// `"YYYY-MM"` → the following month.
fn next_month(key: &str) -> String {
    let y: i32 = key.get(..4).and_then(|s| s.parse().ok()).unwrap_or(1970);
    let m: u32 = key.get(5..7).and_then(|s| s.parse().ok()).unwrap_or(1);
    if m >= 12 { format!("{:04}-01", y + 1) } else { format!("{y:04}-{:02}", m + 1) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mochy_core::models::ArtistRef;

    #[test]
    fn endpoint_routes() {
        let r = route_for_endpoint(&BrowseEndpoint::new("FEmusic_moods_and_genres", None));
        assert_eq!(r, Route::MoodsGenres);
        assert_eq!(route_for_endpoint(&BrowseEndpoint::new("VLPL1", None)), Route::Playlist("VLPL1".into()));
        let g = route_for_endpoint(&BrowseEndpoint::new("FEmusic_moods_and_genres_category", Some("p".into())));
        assert_eq!(g, Route::Browse { id: "FEmusic_moods_and_genres_category".into(), params: Some("p".into()) });
        assert_eq!(page_key(&g).as_deref(), Some("b:FEmusic_moods_and_genres_category:p"));
    }

    #[test]
    fn months_roll_over() {
        assert_eq!(next_month("2026-11"), "2026-12");
        assert_eq!(next_month("2026-12"), "2027-01");
    }

    #[test]
    fn collection_ops_map_to_actions() {
        let songs = vec![Song::remote("a", "A"), Song { video_id: None, ..Song::remote("b", "B") }];
        let e = finish_collection(CollectionOp::Shuffle, songs.clone());
        assert!(matches!(&e[..], [Effect::Act(Action::PlayList { shuffle: true, songs, .. })] if songs.len() == 1));
        let e = finish_collection(CollectionOp::PlayNext, songs.clone());
        assert!(matches!(&e[..], [Effect::PlayNext(v)] if v.len() == 1));
        assert!(matches!(&finish_collection(CollectionOp::Play, Vec::new())[..], [Effect::Toast(_)]));
        let _ = ArtistRef::new("x");
    }

    #[test]
    fn stats_on_temp_db() {
        let db = mochy_storage::Db::open_in_memory().unwrap();
        let s = Song { artists: vec![ArtistRef::with_id("UC1", "One")], ..Song::remote("v1", "Song 1") };
        db.upsert_song(&s).unwrap();
        let now = now_ms();
        db.record_play_at("v1", 120.0, now - 1000).unwrap();
        db.record_play_at("v1", 60.0, now - 2000).unwrap();
        for p in [StatsPeriod::Week, StatsPeriod::Month, StatsPeriod::Year, StatsPeriod::All] {
            let d = compute_stats(&db, p, 0, now).unwrap();
            assert_eq!(d.summary.plays, 2, "{p:?}");
            assert_eq!(d.summary.total_ms, 180_000);
            assert_eq!(d.timeline.iter().map(|t| t.1).sum::<i64>(), 180_000, "{p:?} timeline");
            assert_eq!(d.top_songs[0].song.id, "v1");
        }
        let w = compute_stats(&db, StatsPeriod::Week, 0, now).unwrap();
        assert_eq!(w.timeline.len(), 7);
        let y = compute_stats(&db, StatsPeriod::Year, 0, now).unwrap();
        assert_eq!(y.timeline.len(), 12);
        let prev = compute_stats(&db, StatsPeriod::Week, 1, now).unwrap();
        assert_eq!(prev.summary.plays, 0);
    }
}
