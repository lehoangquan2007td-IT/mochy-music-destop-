//! Search: debounced suggestions, history, sectioned results.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, FontId, Id, Key, RichText, Sense, Ui, vec2};
use mochy_core::models::{SearchResults, Song};

use crate::actions::{Ctx, LoadState};
use crate::widgets::{self, RowOpts, TEXT, TEXT_DIM, icons, icons::Icon};

pub const SUGGEST_DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Songs,
    Videos,
    Albums,
    Artists,
    Playlists,
}

impl Filter {
    const ALL: [Filter; 6] = [Filter::All, Filter::Songs, Filter::Videos, Filter::Albums, Filter::Artists, Filter::Playlists];
    fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Songs => "Songs",
            Filter::Videos => "Videos",
            Filter::Albums => "Albums",
            Filter::Artists => "Artists",
            Filter::Playlists => "Playlists",
        }
    }
}

pub struct SearchState {
    pub query: String,
    pub edited_at: Option<Instant>,
    /// Query the current suggestions belong to.
    pub suggestions_for: String,
    pub suggestions: Vec<String>,
    pub submitted: Option<String>,
    pub results: Option<LoadState<SearchResults>>,
    /// Arc'd song lists of the current results (for row playback context).
    pub songs: Arc<Vec<Song>>,
    pub videos: Arc<Vec<Song>>,
    pub filter: Filter,
    pub focus_request: bool,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            query: String::new(),
            edited_at: None,
            suggestions_for: String::new(),
            suggestions: Vec::new(),
            submitted: None,
            results: None,
            songs: Arc::default(),
            videos: Arc::default(),
            filter: Filter::All,
            focus_request: false,
        }
    }
}

impl SearchState {
    /// Query that needs suggestions now (debounce elapsed), if any.
    pub fn due_suggestions(&mut self) -> Option<String> {
        let t = self.edited_at?;
        if t.elapsed() < SUGGEST_DEBOUNCE {
            return None;
        }
        self.edited_at = None;
        let q = self.query.trim().to_string();
        if q.is_empty() || q == self.suggestions_for {
            return None;
        }
        Some(q)
    }

    pub fn set_results(&mut self, query: &str, r: Result<SearchResults, String>) {
        if self.submitted.as_deref() != Some(query) {
            return;
        }
        if let Ok(res) = &r {
            self.songs = Arc::new(res.songs.clone());
            self.videos = Arc::new(res.videos.clone());
        }
        self.results = Some(LoadState::from_result(r));
    }
}

/// Events the app must handle (network requests, history).
pub enum SearchEvent {
    Submit(String),
    ClearSuggestions,
}

pub fn show(ui: &mut Ui, cx: &mut Ctx, st: &mut SearchState, history: &[String]) -> Option<SearchEvent> {
    let mut ev = None;

    // --- search box ---------------------------------------------------------
    let edit_id = Id::unique("mochy_search_box");
    let frame = egui::Frame::new().fill(widgets::SURFACE_2).corner_radius(CornerRadius::same(22)).inner_margin(egui::Margin::symmetric(14, 8));
    let mut te_resp = None;
    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
            icons::paint(ui.painter(), r, Icon::Search, TEXT_DIM);
            let w = ui.available_width() - 34.0;
            let resp = ui.add(
                egui::TextEdit::singleline(&mut st.query)
                    .id(edit_id)
                    .hint_text("Search songs, albums, artists, playlists")
                    .font(FontId::proportional(16.0))
                    .frame(egui::Frame::NONE)
                    .desired_width(w),
            );
            if !st.query.is_empty() && icons::icon_button(ui, Icon::Close, 26.0, false, cx.accent).clicked() {
                st.query.clear();
                st.suggestions.clear();
                st.suggestions_for.clear();
                resp.request_focus();
            }
            te_resp = Some(resp);
        });
    });
    let te = te_resp.expect("search box");
    if st.focus_request {
        st.focus_request = false;
        te.request_focus();
    }
    if te.changed() {
        st.edited_at = Some(Instant::now());
        if st.query.trim().is_empty() {
            ev = Some(SearchEvent::ClearSuggestions);
        }
    }
    let enter = te.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    if enter && !st.query.trim().is_empty() {
        ev = Some(SearchEvent::Submit(st.query.trim().to_string()));
    }
    ui.add_space(10.0);

    // --- suggestions / history ----------------------------------------------
    let typing = te.has_focus() && st.submitted.as_deref() != Some(st.query.trim());
    if typing || st.submitted.is_none() {
        let mut pick: Option<String> = None;
        if !st.query.trim().is_empty() && !st.suggestions.is_empty() {
            for s in st.suggestions.iter().take(8) {
                if suggestion_row(ui, Icon::Search, s).clicked() {
                    pick = Some(s.clone());
                }
            }
            ui.add_space(8.0);
        }
        if st.query.trim().is_empty() && !history.is_empty() {
            widgets::section_title(ui, "Recent searches");
            for h in history.iter().take(12) {
                if suggestion_row(ui, Icon::Back, h).clicked() {
                    pick = Some(h.clone());
                }
            }
        }
        if let Some(p) = pick {
            st.query = p.clone();
            ev = Some(SearchEvent::Submit(p));
        }
        if st.submitted.is_none() {
            return ev;
        }
        ui.add_space(8.0);
    }

    // --- results ------------------------------------------------------------
    ui.horizontal(|ui| {
        for f in Filter::ALL {
            if widgets::pill(ui, f.label(), st.filter == f).clicked() {
                st.filter = f;
            }
        }
    });
    ui.add_space(10.0);
    match &st.results {
        None | Some(LoadState::Loading) => widgets::loading(ui, "Searching…"),
        Some(LoadState::Failed(e)) => {
            if widgets::error_box(ui, e)
                && let Some(q) = st.submitted.clone()
            {
                ev = Some(SearchEvent::Submit(q));
            }
        }
        Some(LoadState::Ready(res)) => {
            let res = res.clone();
            results(ui, cx, st, &res);
        }
    }
    ev
}

fn suggestion_row(ui: &mut Ui, icon: Icon, text: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(8), Color32::from_white_alpha(10));
    }
    let ir = egui::Rect::from_center_size(egui::pos2(rect.left() + 20.0, rect.center().y), vec2(16.0, 16.0));
    icons::paint(ui.painter(), ir, icon, TEXT_DIM);
    widgets::paint_text(ui, egui::pos2(rect.left() + 40.0, rect.center().y - 9.0), text, FontId::proportional(15.0), TEXT, rect.width() - 50.0);
    resp
}

fn results(ui: &mut Ui, cx: &mut Ctx, st: &mut SearchState, res: &SearchResults) {
    let all = st.filter == Filter::All;
    let empty = res.songs.is_empty() && res.videos.is_empty() && res.albums.is_empty() && res.artists.is_empty() && res.playlists.is_empty();
    if empty {
        ui.label(RichText::new("No results").color(TEXT_DIM));
        return;
    }
    if (all || st.filter == Filter::Songs) && !st.songs.is_empty() {
        widgets::section_title(ui, "Songs");
        let n = if all { 6.min(st.songs.len()) } else { st.songs.len() };
        let songs = st.songs.clone();
        for i in 0..n {
            widgets::song_row(ui, cx, &songs, i, RowOpts { show_album: true, ..Default::default() });
        }
        if all && st.songs.len() > n && ui.link("Show all songs").clicked() {
            st.filter = Filter::Songs;
        }
        ui.add_space(10.0);
    }
    if (all || st.filter == Filter::Artists) && !res.artists.is_empty() {
        cards(ui, "Artists", all, res.artists.len(), |ui, i| widgets::artist_card(ui, cx, &res.artists[i]));
    }
    if (all || st.filter == Filter::Albums) && !res.albums.is_empty() {
        cards(ui, "Albums", all, res.albums.len(), |ui, i| widgets::album_card(ui, cx, &res.albums[i]));
    }
    if (all || st.filter == Filter::Videos) && !st.videos.is_empty() {
        widgets::section_title(ui, "Videos");
        let n = if all { 4.min(st.videos.len()) } else { st.videos.len() };
        let vids = st.videos.clone();
        for i in 0..n {
            widgets::song_row(ui, cx, &vids, i, RowOpts::default());
        }
        ui.add_space(10.0);
    }
    if (all || st.filter == Filter::Playlists) && !res.playlists.is_empty() {
        cards(ui, "Playlists", all, res.playlists.len(), |ui, i| widgets::playlist_card(ui, cx, &res.playlists[i]));
    }
}

/// Card section: horizontal row in "All", wrapping grid when filtered.
fn cards(ui: &mut Ui, title: &str, row: bool, n: usize, mut f: impl FnMut(&mut Ui, usize)) {
    widgets::section_title(ui, title);
    if row {
        egui::ScrollArea::horizontal().id_salt(("search_cards", title)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                for i in 0..n {
                    f(ui, i);
                }
            });
        });
    } else {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(16.0, 16.0);
            for i in 0..n {
                f(ui, i);
            }
        });
    }
    ui.add_space(12.0);
}
