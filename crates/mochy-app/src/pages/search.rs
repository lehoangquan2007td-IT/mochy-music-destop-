//! Search: debounced suggestions, history, sectioned results.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, FontId, Id, Key, RichText, Sense, Ui, vec2};
use mochy_core::models::{SearchResults, Song};

use crate::actions::{Ctx, LoadState};
use crate::ui::i18n::tr;
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
        tr(match self {
            Filter::All => "All",
            Filter::Songs => "Songs",
            Filter::Videos => "Videos",
            Filter::Albums => "Albums",
            Filter::Artists => "Artists",
            Filter::Playlists => "Playlists",
        })
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
    /// Suggestion/history list is open. Kept as explicit state (not derived
    /// from the text box focus): pressing the mouse on a suggestion takes
    /// focus away from the box on *press*, while the click only registers on
    /// *release* — deriving visibility from focus made the list vanish in
    /// between, so clicks on suggestions were lost.
    pub suggest_open: bool,
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
            suggest_open: false,
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
    let (mut ev, show_results) = search_header(ui, cx.accent, st, history);
    if !show_results {
        return ev;
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
        None | Some(LoadState::Loading) => widgets::loading(ui, tr("Searching…")),
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

/// Search box + suggestion/history list. Returns the event (if any) and
/// whether the results section should be drawn below.
fn search_header(ui: &mut Ui, accent: Color32, st: &mut SearchState, history: &[String]) -> (Option<SearchEvent>, bool) {
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
                    .hint_text(tr("Search songs, albums, artists, playlists"))
                    .font(FontId::proportional(16.0))
                    .frame(egui::Frame::NONE)
                    .desired_width(w),
            );
            if !st.query.is_empty() && icons::icon_button(ui, Icon::Close, 26.0, false, accent).clicked() {
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
    if te.gained_focus() {
        st.suggest_open = true;
    }
    if te.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
        st.suggest_open = false;
    }
    if te.changed() {
        st.suggest_open = true;
        st.edited_at = Some(Instant::now());
        if st.query.trim().is_empty() {
            ev = Some(SearchEvent::ClearSuggestions);
        }
    }
    let enter = te.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    if enter && !st.query.trim().is_empty() {
        st.suggest_open = false;
        ev = Some(SearchEvent::Submit(st.query.trim().to_string()));
    }
    ui.add_space(10.0);

    // --- suggestions / history ----------------------------------------------
    if st.suggest_open || st.submitted.is_none() {
        let mut pick: Option<String> = None;
        let list_top = ui.cursor().top();
        if !st.query.trim().is_empty() && !st.suggestions.is_empty() {
            for s in st.suggestions.iter().take(8) {
                if suggestion_row(ui, Icon::Search, s).clicked() {
                    pick = Some(s.clone());
                }
            }
            ui.add_space(8.0);
        }
        if st.query.trim().is_empty() && !history.is_empty() {
            widgets::section_title(ui, tr("Recent searches"));
            for h in history.iter().take(12) {
                if suggestion_row(ui, Icon::Back, h).clicked() {
                    pick = Some(h.clone());
                }
            }
        }
        let list_rect = egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), list_top..=ui.cursor().top());
        if let Some(p) = pick {
            st.query = p.clone();
            st.suggest_open = false;
            ev = Some(SearchEvent::Submit(p));
        } else if st.suggest_open
            && !te.has_focus()
            && ui.input(|i| i.pointer.any_click())
            && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
            && !list_rect.contains(pos)
            && !te.rect.expand(16.0).contains(pos)
        {
            // Clicked somewhere else on the page: close the list.
            st.suggest_open = false;
        }
        if st.submitted.is_none() {
            return (ev, false);
        }
        ui.add_space(8.0);
    }
    (ev, true)
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
        ui.label(RichText::new(tr("No results")).color(TEXT_DIM));
        return;
    }
    if (all || st.filter == Filter::Songs) && !st.songs.is_empty() {
        widgets::section_title(ui, tr("Songs"));
        let n = if all { 6.min(st.songs.len()) } else { st.songs.len() };
        let songs = st.songs.clone();
        for i in 0..n {
            widgets::song_row(ui, cx, &songs, i, RowOpts { show_album: true, radio: true, ..Default::default() });
        }
        if all && st.songs.len() > n && ui.link(tr("Show all songs")).clicked() {
            st.filter = Filter::Songs;
        }
        ui.add_space(10.0);
    }
    if (all || st.filter == Filter::Artists) && !res.artists.is_empty() {
        cards(ui, tr("Artists"), all, res.artists.len(), |ui, i| widgets::artist_card(ui, cx, &res.artists[i]));
    }
    if (all || st.filter == Filter::Albums) && !res.albums.is_empty() {
        cards(ui, tr("Albums"), all, res.albums.len(), |ui, i| widgets::album_card(ui, cx, &res.albums[i]));
    }
    if (all || st.filter == Filter::Videos) && !st.videos.is_empty() {
        widgets::section_title(ui, tr("Videos"));
        let n = if all { 4.min(st.videos.len()) } else { st.videos.len() };
        let vids = st.videos.clone();
        for i in 0..n {
            widgets::song_row(ui, cx, &vids, i, RowOpts { radio: true, ..Default::default() });
        }
        ui.add_space(10.0);
    }
    if (all || st.filter == Filter::Playlists) && !res.playlists.is_empty() {
        cards(ui, tr("Playlists"), all, res.playlists.len(), |ui, i| widgets::playlist_card(ui, cx, &res.playlists[i]));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives the real search header through egui frames with synthetic input.
    struct Harness {
        ctx: egui::Context,
        st: SearchState,
        events: Vec<egui::Event>,
        last: Vec<SearchEvent>,
    }

    impl Harness {
        fn new() -> Self {
            Self { ctx: egui::Context::default(), st: SearchState::default(), events: Vec::new(), last: Vec::new() }
        }

        fn frame(&mut self) {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 700.0))),
                events: std::mem::take(&mut self.events),
                ..Default::default()
            };
            let st = &mut self.st;
            let last = &mut self.last;
            let mut out = self.ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    let (ev, _) = search_header(ui, Color32::WHITE, st, &[]);
                    if let Some(e) = ev {
                        last.push(e);
                    }
                });
            });
            out.textures_delta.clear();
        }

        fn click_at(&mut self, pos: egui::Pos2) {
            self.events.push(egui::Event::PointerMoved(pos));
            self.frame();
            self.events.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
            self.frame(); // press: the text box loses focus here
            self.events.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
            self.frame(); // release: the click registers here
        }

        fn submitted(&self) -> Vec<String> {
            self.last.iter().filter_map(|e| if let SearchEvent::Submit(q) = e { Some(q.clone()) } else { None }).collect()
        }
    }

    /// y of the n-th suggestion row: search box (~40 px) + 10 px gap, rows 36 px.
    fn row_y(h: &Harness, n: usize) -> f32 {
        let _ = h;
        8.0 + 40.0 + 10.0 + 36.0 * n as f32 + 18.0
    }

    fn type_and_get_suggestions(h: &mut Harness, text: &str, sugg: &[&str]) {
        h.st.focus_request = true;
        h.frame();
        h.frame();
        h.events.push(egui::Event::Text(text.into()));
        h.frame();
        // Suggestions arrive from the network.
        h.st.suggestions_for = text.into();
        h.st.suggestions = sugg.iter().map(|s| s.to_string()).collect();
        h.frame();
    }

    #[test]
    fn clicking_a_suggestion_submits_it_first_search() {
        let mut h = Harness::new();
        type_and_get_suggestions(&mut h, "son tung", &["sơn tùng m-tp", "son tung mtp"]);
        h.click_at(egui::pos2(200.0, row_y(&h, 1)));
        assert_eq!(h.submitted(), vec!["son tung mtp".to_string()]);
    }

    #[test]
    fn clicking_a_suggestion_submits_it_after_a_previous_search() {
        // The reported bug: with results of an earlier search on screen,
        // the list vanished on mouse-press and the click was lost.
        let mut h = Harness::new();
        h.st.submitted = Some("believer".into());
        h.st.results = Some(LoadState::Loading);
        type_and_get_suggestions(&mut h, "lac troi", &["lạc trôi", "lạc trôi sơn tùng"]);
        h.click_at(egui::pos2(200.0, row_y(&h, 0)));
        assert_eq!(h.submitted(), vec!["lạc trôi".to_string()]);
        assert!(!h.st.suggest_open, "list closes after picking");
    }

    #[test]
    fn clicking_elsewhere_closes_the_list() {
        let mut h = Harness::new();
        h.st.submitted = Some("believer".into());
        h.st.results = Some(LoadState::Loading);
        type_and_get_suggestions(&mut h, "abc", &["abc def"]);
        assert!(h.st.suggest_open);
        h.click_at(egui::pos2(200.0, 650.0));
        assert!(h.submitted().is_empty());
        assert!(!h.st.suggest_open);
    }
}
