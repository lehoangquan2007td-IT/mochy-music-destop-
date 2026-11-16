//! Audio Lab → Headphones: the AutoEq database browser (port of Android
//! `AutoEqTab` + `AudioLabViewModel`'s AutoEq part + `AutoEqRepository`).
//!
//! Pick a headphone from the AutoEq project's index; its measurement and
//! ParametricEQ are downloaded (and cached for good), and the correction plays
//! *under* the user's own curve (`EqSettings::correction`). The fit can be
//! re-tuned (band count, upper limit, bass, tilt) with the core fitter.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use egui::{Align, Align2, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Sense, Shape, Stroke, Ui, pos2, vec2};
use mochy_core::autoeq::{self, AutoEqEntry, AutoEqModel, Fit, Measurement};
use mochy_core::eq::{AutoEqCorrection, AutoEqTuning, EqSettings, HeadphoneForm};

use super::{chip, kit, section_label, slider_row, theme, toggle_row};
use crate::ui::i18n::tr;

const ORIGINAL: Color32 = Color32::from_rgb(0x6B, 0x9B, 0xF2);
const CORRECTED: Color32 = Color32::from_rgb(0xF2, 0x8B, 0xA8);
const TARGET: Color32 = Color32::from_rgb(0x9A, 0x9A, 0xA0);
const ROW_H: f32 = 46.0;

enum Msg {
    Index(Result<Vec<AutoEqModel>, String>),
    Measured { entry: AutoEqEntry, apply: bool, result: Result<(Measurement, Fit), String> },
    Fitted { path: String, tuning: AutoEqTuning, fit: Fit },
}

enum IndexState {
    Idle,
    Loading,
    Ready(Arc<Vec<AutoEqModel>>),
    Failed(String),
}

struct Loaded {
    path: String,
    measurement: Arc<Measurement>,
    own_fit: Fit,
}

/// Cached plot curves for the response graph.
struct Curves {
    key: (String, Vec<(u32, i32, i32)>, (i32, i32)),
    raw: Vec<f32>,
    target: Vec<f32>,
    corrected: Vec<f32>,
    lo: f32,
    hi: f32,
}

pub struct DbState {
    index: IndexState,
    query: String,
    results: Vec<usize>,
    results_for: Option<String>,
    expanded: Option<String>,
    loaded: Option<Loaded>,
    busy: Option<String>,
    error: Option<String>,
    inbox: Arc<Mutex<Vec<Msg>>>,
    tune_at: Option<Instant>,
    curves: Option<Curves>,
    scroll_to_row: Option<usize>,
    opened: bool,
    shots_done: bool,
}

impl Default for DbState {
    fn default() -> Self {
        Self {
            index: IndexState::Idle,
            query: String::new(),
            results: Vec::new(),
            results_for: None,
            expanded: None,
            loaded: None,
            busy: None,
            error: None,
            inbox: Arc::new(Mutex::new(Vec::new())),
            tune_at: None,
            curves: None,
            scroll_to_row: None,
            opened: false,
            shots_done: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Background work
// ---------------------------------------------------------------------------

fn cache_dir() -> PathBuf {
    mochy_storage::data_dir().join("autoeq")
}

fn spawn(ctx: &egui::Context, inbox: &Arc<Mutex<Vec<Msg>>>, job: impl FnOnce() -> Msg + Send + 'static) {
    let inbox = inbox.clone();
    let ctx = ctx.clone();
    let _ = std::thread::Builder::new().name("mochy-autoeq".into()).spawn(move || {
        let msg = job();
        if let Ok(mut q) = inbox.lock() {
            q.push(msg);
        }
        ctx.request_repaint();
    });
}

fn download(url: &str) -> Result<String, String> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    rt.block_on(async {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(12))
            .timeout(Duration::from_secs(30))
            .user_agent("Mochy")
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("AutoEq {} for {url}", resp.status().as_u16()));
        }
        resp.text().await.map_err(|e| e.to_string())
    })
}

/// The index: kept on disk for a week so search works offline.
fn load_index(refresh: bool) -> Result<Vec<AutoEqModel>, String> {
    let dir = cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("INDEX.md");
    let age = std::fs::metadata(&file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .map(|d| d.as_secs());
    let stale = age.is_none_or(|a| a > autoeq::INDEX_MAX_AGE_SECS);
    let text = if refresh || stale {
        match download(&format!("{}/INDEX.md", autoeq::RESULTS_BASE)) {
            Ok(t) => {
                let _ = std::fs::write(&file, &t);
                t
            }
            Err(e) => {
                tracing::warn!("AutoEq index fetch failed: {e}");
                std::fs::read_to_string(&file).map_err(|_| tr("Couldn't reach the AutoEq database").to_string())?
            }
        }
    } else {
        std::fs::read_to_string(&file).map_err(|e| e.to_string())?
    };
    let models = autoeq::parse_index(&text);
    if models.is_empty() {
        return Err(tr("Couldn't reach the AutoEq database").into());
    }
    Ok(models)
}

fn cached(name: &str, url: &str) -> Result<String, String> {
    let file = cache_dir().join(name);
    if let Ok(t) = std::fs::read_to_string(&file)
        && !t.is_empty()
    {
        return Ok(t);
    }
    let t = download(url)?;
    let _ = std::fs::create_dir_all(cache_dir());
    let _ = std::fs::write(&file, &t);
    Ok(t)
}

/// A result's measurement and AutoEq's own fit (its ParametricEQ.txt).
fn load_measurement(entry: &AutoEqEntry) -> Result<(Measurement, Fit), String> {
    let key = entry.cache_key();
    let csv = cached(&format!("{key}.csv"), &entry.csv_url())?;
    let txt = cached(&format!("{key}.txt"), &entry.parametric_url())?;
    let m = autoeq::parse_measurement(&csv)?;
    let parsed = super::autoeq::parse_autoeq(&txt)?;
    Ok((m, Fit { preamp_db: parsed.preamp_db, filters: parsed.nodes }))
}

fn entry_of(c: &AutoEqCorrection) -> AutoEqEntry {
    AutoEqEntry { name: c.model.clone(), source: c.source.clone(), rig: c.rig.clone(), path: c.path.clone(), form: c.form }
}

impl DbState {
    /// Handle finished jobs; call every frame the Audio Lab is shown.
    pub fn poll(&mut self, ctx: &egui::Context, eq: &mut EqSettings) {
        let msgs: Vec<Msg> = self.inbox.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        for msg in msgs {
            match msg {
                Msg::Index(Ok(models)) => {
                    self.index = IndexState::Ready(Arc::new(models));
                    self.results_for = None;
                }
                Msg::Index(Err(e)) => self.index = IndexState::Failed(e),
                Msg::Measured { entry, apply, result } => {
                    if self.busy.as_deref() == Some(entry.path.as_str()) {
                        self.busy = None;
                    }
                    match result {
                        Ok((m, own)) => {
                            if apply {
                                let tuning = AutoEqTuning::default();
                                let fit = autoeq::correction(&m, &tuning, Some(&own));
                                eq.enabled = true;
                                eq.correction_enabled = true;
                                eq.correction = Some(AutoEqCorrection {
                                    model: entry.name.clone(),
                                    source: entry.source.clone(),
                                    rig: entry.rig.clone(),
                                    path: entry.path.clone(),
                                    form: entry.form,
                                    tuning,
                                    filters: fit.filters,
                                    preamp_db: fit.preamp_db,
                                });
                            }
                            if apply || eq.correction.as_ref().is_some_and(|c| c.path == entry.path) {
                                self.loaded = Some(Loaded { path: entry.path, measurement: Arc::new(m), own_fit: own });
                            }
                        }
                        Err(e) => {
                            if apply {
                                tracing::warn!("AutoEq {}: {e}", entry.path);
                                self.error = Some(tr("Couldn't load {name}. Check your connection.").replace("{name}", &entry.name));
                            }
                        }
                    }
                }
                Msg::Fitted { path, tuning, fit } => {
                    if self.busy.as_deref() == Some("fit") {
                        self.busy = None;
                    }
                    if let Some(c) = eq.correction.as_mut()
                        && c.path == path
                        && c.tuning == tuning
                    {
                        c.filters = fit.filters;
                        c.preamp_db = fit.preamp_db;
                    }
                }
            }
        }
        // A tuning change rests for 140 ms before the (CPU-heavy) fit runs.
        if let Some(at) = self.tune_at {
            if at.elapsed() >= Duration::from_millis(140) {
                self.tune_at = None;
                if let (Some(c), Some(l)) = (eq.correction.as_ref(), self.loaded.as_ref())
                    && c.path == l.path
                {
                    let (m, own, tuning, path) = (l.measurement.clone(), l.own_fit.clone(), c.tuning.clone(), c.path.clone());
                    self.busy = Some("fit".into());
                    spawn(ctx, &self.inbox, move || {
                        let fit = autoeq::correction(&m, &tuning, Some(&own));
                        Msg::Fitted { path, tuning, fit }
                    });
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
    }

    /// The tab opened: the list, and the current headphone's curve for the graph.
    fn open(&mut self, ctx: &egui::Context, eq: &EqSettings) {
        if matches!(self.index, IndexState::Idle) {
            self.load_index(ctx, false);
        }
        if self.opened {
            return;
        }
        self.opened = true;
        if let Some(c) = &eq.correction
            && self.loaded.as_ref().is_none_or(|l| l.path != c.path)
        {
            let entry = entry_of(c);
            spawn(ctx, &self.inbox, move || {
                let result = load_measurement(&entry);
                Msg::Measured { entry, apply: false, result }
            });
        }
    }

    /// Screenshot automation only (`MOCHY_SHOTS` + `MOCHY_SHOTS_AUTOEQ=<query>`):
    /// search for the query and pick its first result once.
    fn shots_pick(&mut self, ctx: &egui::Context) {
        if self.shots_done || std::env::var_os("MOCHY_SHOTS").is_none() {
            return;
        }
        let Ok(q) = std::env::var("MOCHY_SHOTS_AUTOEQ") else {
            self.shots_done = true;
            return;
        };
        if let IndexState::Ready(models) = &self.index {
            self.shots_done = true;
            self.query = q.clone();
            if let Some(&i) = autoeq::search(models, &q).first() {
                let e = models[i].entries[0].clone();
                self.select(ctx, &e);
            }
        }
    }

    fn load_index(&mut self, ctx: &egui::Context, refresh: bool) {
        if !matches!(self.index, IndexState::Ready(_)) {
            self.index = IndexState::Loading;
        }
        spawn(ctx, &self.inbox, move || Msg::Index(load_index(refresh)));
    }

    fn select(&mut self, ctx: &egui::Context, entry: &AutoEqEntry) {
        self.busy = Some(entry.path.clone());
        self.error = None;
        let entry = entry.clone();
        spawn(ctx, &self.inbox, move || {
            let result = load_measurement(&entry);
            Msg::Measured { entry, apply: true, result }
        });
    }
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

pub fn page(ui: &mut Ui, eq: &mut EqSettings, st: &mut DbState, accent: Color32) {
    let ctx = ui.ctx().clone();
    st.open(&ctx, eq);
    st.shots_pick(&ctx);
    // Side by side on a wide window: the curve and the picked headphone on the
    // left, the database on the right.
    if ui.available_width() >= 980.0 {
        ui.columns(2, |cols| {
            let (l, r) = cols.split_at_mut(1);
            correction_column(&mut l[0], eq, st, accent);
            database(&mut r[0], eq, st, accent);
        });
    } else {
        correction_column(ui, eq, st, accent);
        database(ui, eq, st, accent);
    }
}

fn correction_column(ui: &mut Ui, eq: &mut EqSettings, st: &mut DbState, accent: Color32) {
    section_label(ui, tr("Headphone correction"));
    super::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("FREQUENCY RESPONSE")).small().strong().color(theme::TEXT_SECONDARY));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                legend(ui, CORRECTED, tr("Corrected"));
                legend(ui, TARGET, tr("Target"));
                legend(ui, ORIGINAL, tr("Original"));
            });
        });
        let showing = eq.correction.as_ref().filter(|c| st.loaded.as_ref().is_some_and(|l| l.path == c.path));
        response_graph(ui, st, showing);
        if showing.is_none() {
            let msg = match &eq.correction {
                None => tr("Pick your headphones below to see their curve.").to_string(),
                Some(c) => tr("Loading {name}…").replace("{name}", &c.model),
            };
            ui.label(RichText::new(msg).small().color(theme::TEXT_FAINT));
        }
    });
    if eq.correction.is_some() {
        selected_card(ui, eq, st, accent);
    }
    if let Some(e) = &st.error {
        ui.label(RichText::new(e).color(Color32::from_rgb(0xFF, 0x8A, 0x80)));
    }
}

fn legend(ui: &mut Ui, col: Color32, label: &str) {
    ui.label(RichText::new(label).small().color(theme::TEXT_SECONDARY));
    let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
    ui.painter().circle_filled(r.center(), 3.5, col);
}

fn curves_for(st: &mut DbState, c: &AutoEqCorrection) {
    let key = (
        c.path.clone(),
        c.filters.iter().map(|f| (f.frequency_hz.to_bits(), (f.gain_db * 100.0) as i32, (f.q * 100.0) as i32)).collect::<Vec<_>>(),
        ((c.tuning.bass_db * 10.0) as i32, (c.tuning.tilt_db_per_octave * 10.0) as i32),
    );
    if st.curves.as_ref().is_some_and(|cv| cv.key == key) {
        return;
    }
    let Some(l) = &st.loaded else { return };
    let m = &l.measurement;
    let f = &m.frequencies;
    let resp = autoeq::response_on(f, &c.filters);
    let target = autoeq::tuned_target(m, &c.tuning);
    let corrected: Vec<f64> = m.raw.iter().zip(&resp).map(|(r, x)| r + x).collect();
    let mid = |a: &[f64]| {
        let v: Vec<f64> = f.iter().zip(a).filter(|(x, _)| (300.0..=3000.0).contains(*x)).map(|(_, y)| *y).collect();
        if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 }
    };
    let off_raw = mid(&target) - mid(&m.raw);
    let off_cor = mid(&target) - mid(&corrected);
    let raw: Vec<f32> = m.raw.iter().map(|v| (v + off_raw) as f32).collect();
    let cor: Vec<f32> = corrected.iter().map(|v| (v + off_cor) as f32).collect();
    let tgt: Vec<f32> = target.iter().map(|v| *v as f32).collect();
    let all = raw.iter().chain(&tgt).chain(&cor).copied().filter(|v| v.is_finite());
    let (mn, mx) = all.fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v), b.max(v)));
    let lo = ((if mn == f32::MAX { -10.0 } else { mn }) / 5.0).floor() * 5.0 - 5.0;
    let hi = ((if mx == f32::MIN { 10.0 } else { mx }) / 5.0).ceil() * 5.0 + 5.0;
    st.curves = Some(Curves { key, raw, target: tgt, corrected: cor, lo, hi });
}

fn response_graph(ui: &mut Ui, st: &mut DbState, showing: Option<&AutoEqCorrection>) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 230.0), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(18), theme::STAGE);
    p.rect_stroke(rect, CornerRadius::same(18), Stroke::new(1.0, theme::STAGE_LINE), egui::StrokeKind::Inside);
    let plot = Rect::from_min_max(rect.min + vec2(34.0, 10.0), rect.max - vec2(10.0, 24.0));
    if let Some(c) = showing {
        curves_for(st, c);
    }
    let (lo, hi) = st.curves.as_ref().filter(|_| showing.is_some()).map(|c| (c.lo, c.hi)).unwrap_or((-15.0, 15.0));
    let x_of = |f: f64| plot.left() + ((f.max(20.0) / 20.0).log10() / 3.0) as f32 * plot.width();
    let y_of = |db: f32| plot.bottom() - (db - lo) / (hi - lo) * plot.height();
    let font = FontId::proportional(10.0);
    let step = if hi - lo > 40.0 { 10.0 } else { 5.0 };
    let mut v = (lo / step).ceil() * step;
    while v <= hi {
        let y = y_of(v);
        p.hline(plot.x_range(), y, Stroke::new(1.0, Color32::from_white_alpha(14)));
        p.text(pos2(plot.left() - 6.0, y), Align2::RIGHT_CENTER, format!("{v:.0}"), font.clone(), theme::TEXT_FAINT);
        v += step;
    }
    for (f, l) in [(20.0, "20"), (100.0, "100"), (1000.0, "1k"), (10_000.0, "10k"), (20_000.0, "20k")] {
        let x = x_of(f);
        p.vline(x, plot.y_range(), Stroke::new(1.0, Color32::from_white_alpha(14)));
        p.text(pos2(x, plot.bottom() + 5.0), Align2::CENTER_TOP, l, font.clone(), theme::TEXT_FAINT);
    }
    let (Some(cv), Some(l)) = (st.curves.as_ref().filter(|_| showing.is_some()), st.loaded.as_ref()) else { return };
    let f = &l.measurement.frequencies;
    let appear = ui.ctx().animate_bool_with_time(ui.scope_id().with(("aeq_appear", &l.path)), true, 0.6);
    let clip = p.with_clip_rect(Rect::from_min_max(plot.min, pos2(plot.left() + plot.width() * appear, plot.bottom())).intersect(rect));
    let pts = |vals: &[f32]| -> Vec<Pos2> { f.iter().zip(vals).map(|(&x, &y)| pos2(x_of(x), y_of(y))).collect() };
    clip.extend(Shape::dashed_line(&pts(&cv.target), Stroke::new(1.6, TARGET), 5.0, 4.0));
    clip.add(Shape::line(pts(&cv.raw), Stroke::new(1.8, ORIGINAL)));
    clip.add(Shape::line(pts(&cv.corrected), Stroke::new(2.2, CORRECTED)));
}

fn form_glyph(ui: &mut Ui, form: HeadphoneForm) {
    let (r, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
    let p = ui.painter();
    p.circle_filled(r.center(), 20.0, Color32::from_rgb(0x2C, 0x2C, 0x2E));
    kit::headphone_sketch(&p.with_clip_rect(r), Rect::from_center_size(r.center() + vec2(13.0, -3.0), vec2(30.0, 30.0)), true);
    if form != HeadphoneForm::OverEar {
        let t = if form == HeadphoneForm::InEar { "IEM" } else { "BUD" };
        p.text(pos2(r.center().x, r.bottom() - 6.0), Align2::CENTER_CENTER, t, FontId::proportional(7.5), Color32::WHITE);
    }
}

fn selected_card(ui: &mut Ui, eq: &mut EqSettings, st: &mut DbState, accent: Color32) {
    let can_tune = eq.correction.as_ref().is_some_and(|c| st.loaded.as_ref().is_some_and(|l| l.path == c.path));
    let fitting = st.busy.as_deref() == Some("fit");
    let mut remove = false;
    super::card(ui, |ui| {
        let Some(c) = eq.correction.clone() else { return };
        ui.horizontal(|ui| {
            form_glyph(ui, c.form);
            ui.vertical(|ui| {
                ui.label(RichText::new(&c.model).font(theme::bold(ui.ctx(), 17.0)).color(Color32::WHITE));
                let mut parts = vec![tr("by {source}").replace("{source}", &c.source)];
                if let Some(r) = &c.rig {
                    parts.push(r.clone());
                }
                parts.push(tr("{n} filters").replace("{n}", &c.filters.len().to_string()));
                ui.label(RichText::new(parts.join(" · ")).small().color(theme::TEXT_SECONDARY));
            });
            if fitting {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.spinner());
            }
        });
        let mut on = eq.correction_enabled;
        if toggle_row(ui, tr("Apply correction"), Some(tr("Plays under your own EQ curve")), &mut on, accent).changed() {
            eq.correction_enabled = on;
            eq.enabled = eq.enabled || on;
        }
        ui.add_space(4.0);
        let mut tuning = c.tuning.clone();
        ui.add_enabled_ui(can_tune, |ui| {
            section_label(ui, tr("Filter bands"));
            ui.horizontal_wrapped(|ui| {
                for b in AutoEqTuning::BAND_CHOICES {
                    if chip(ui, &tr("{n} bands").replace("{n}", &b.to_string()), tuning.bands == b, accent).clicked() {
                        tuning.bands = b;
                    }
                }
            });
            section_label(ui, tr("Correct up to"));
            ui.horizontal_wrapped(|ui| {
                for hz in AutoEqTuning::MAX_HZ_CHOICES {
                    if chip(ui, &format!("{} kHz", hz / 1000), tuning.max_hz == hz, accent).clicked() {
                        tuning.max_hz = hz;
                    }
                }
            });
            let mut bass = tuning.bass_db;
            if slider_row(ui, tr("Bass"), &mut bass, -6.0..=6.0, " dB", 0, false).changed() {
                tuning.bass_db = bass.round();
            }
            let mut tilt = tuning.tilt_db_per_octave;
            if slider_row(ui, tr("Tilt"), &mut tilt, -1.0..=1.0, " dB/oct", 1, false).changed() {
                tuning.tilt_db_per_octave = (tilt * 10.0).round() / 10.0;
            }
        });
        if !can_tune {
            ui.label(RichText::new(tr("Tuning needs the measurement — it is loading.")).small().color(theme::TEXT_FAINT));
        }
        ui.horizontal(|ui| {
            if !tuning.is_default() && ui.add_enabled(can_tune, egui::Button::new(tr("Use AutoEq's fit"))).clicked() {
                tuning = AutoEqTuning::default();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(tr("Remove")).clicked() {
                    remove = true;
                }
            });
        });
        if can_tune && tuning != c.tuning {
            if let Some(cc) = eq.correction.as_mut() {
                cc.tuning = tuning;
            }
            st.tune_at = Some(Instant::now());
            ui.ctx().request_repaint_after(Duration::from_millis(150));
        }
    });
    if remove {
        eq.correction = None;
        st.loaded = None;
        st.curves = None;
    }
}

/// One line in the list: a model (maybe expanded) or one of its results.
enum Row {
    Model(usize),
    Entry(usize, usize),
}

fn database(ui: &mut Ui, eq: &mut EqSettings, st: &mut DbState, accent: Color32) {
    let ctx = ui.ctx().clone();
    let selected_path = eq.correction.as_ref().map(|c| c.path.clone());
    let mut pick: Option<AutoEqEntry> = None;
    super::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(tr("AutoEq database")).font(theme::bold(ui.ctx(), 17.0)).color(Color32::WHITE));
                ui.label(RichText::new(tr("Measured headphones from the AutoEq project — pick yours")).small().color(theme::TEXT_SECONDARY));
            });
            if let IndexState::Ready(m) = &st.index {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(tr("{n} models").replace("{n}", &m.len().to_string())).color(theme::TEXT_SECONDARY));
                });
            }
        });
        // Search field.
        ui.horizontal(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut st.query)
                    .hint_text(tr("Search headphones, e.g. HD 650"))
                    .desired_width(ui.available_width() - 40.0),
            );
            if r.changed() {
                st.scroll_to_row = Some(0);
            }
            if !st.query.is_empty() && ui.small_button("✕").on_hover_text(tr("Clear")).clicked() {
                st.query.clear();
                st.scroll_to_row = Some(0);
            }
        });
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 420.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(18), theme::STAGE);
        ui.painter().rect_stroke(rect, CornerRadius::same(18), Stroke::new(1.0, theme::STAGE_LINE), egui::StrokeKind::Inside);
        let inner = rect.shrink2(vec2(6.0, 8.0));
        match &st.index {
            IndexState::Idle | IndexState::Loading => {
                let p = ui.painter_at(inner);
                for k in 0..7 {
                    let y = inner.top() + 14.0 + k as f32 * 34.0;
                    p.circle_filled(pos2(inner.left() + 20.0, y + 8.0), 9.0, theme::ROW);
                    let w = 120.0 + ((k * 37) % 90) as f32;
                    p.rect_filled(Rect::from_min_size(pos2(inner.left() + 40.0, y), vec2(w, 11.0)), CornerRadius::same(6), theme::ROW);
                    p.rect_filled(Rect::from_min_size(pos2(inner.left() + 40.0, y + 16.0), vec2(70.0, 8.0)), CornerRadius::same(5), theme::PILL_CHIP);
                }
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            IndexState::Failed(e) => {
                let e = e.clone();
                in_rect(ui, inner, Layout::top_down(Align::Center), |ui| {
                    ui.add_space(150.0);
                    ui.label(RichText::new(e).color(theme::TEXT_SECONDARY));
                    if ui.button(tr("Try again")).clicked() {
                        st.load_index(&ctx, true);
                    }
                });
            }
            IndexState::Ready(models) => {
                let models = models.clone();
                if st.results_for.as_deref() != Some(st.query.as_str()) {
                    st.results = autoeq::search(&models, &st.query);
                    st.results_for = Some(st.query.clone());
                }
                let mut rows: Vec<Row> = Vec::with_capacity(st.results.len() + 4);
                for &i in &st.results {
                    rows.push(Row::Model(i));
                    if st.expanded.as_deref() == Some(models[i].name.as_str()) && models[i].entries.len() > 1 {
                        rows.extend((0..models[i].entries.len()).map(|k| Row::Entry(i, k)));
                    }
                }
                let list_rect = Rect::from_min_max(inner.min, pos2(inner.right() - 24.0, inner.bottom()));
                let rail_rect = Rect::from_min_max(pos2(inner.right() - 22.0, inner.top()), inner.max);
                if rows.is_empty() {
                    ui.painter().text(
                        inner.center(),
                        Align2::CENTER_CENTER,
                        tr("No headphones match \"{q}\"").replace("{q}", &st.query),
                        FontId::proportional(14.0),
                        theme::TEXT_SECONDARY,
                    );
                }
                // Alphabet rail.
                if st.query.is_empty() {
                    let letters: Vec<char> = ('A'..='Z').chain(std::iter::once('#')).collect();
                    let resp = ui.interact(rail_rect, ui.scope_id().with("aeq_rail"), Sense::click_and_drag());
                    let lh = rail_rect.height() / letters.len() as f32;
                    for (k, ch) in letters.iter().enumerate() {
                        ui.painter().text(
                            pos2(rail_rect.center().x, rail_rect.top() + lh * (k as f32 + 0.5)),
                            Align2::CENTER_CENTER,
                            ch.to_string(),
                            FontId::proportional(9.5),
                            theme::TEXT_FAINT,
                        );
                    }
                    if (resp.clicked() || resp.dragged())
                        && let Some(pos) = resp.interact_pointer_pos()
                    {
                        let k = (((pos.y - rail_rect.top()) / lh) as usize).min(letters.len() - 1);
                        let letter = letters[k];
                        let target = rows.iter().position(|r| {
                            let Row::Model(i) = r else { return false };
                            let c = models[*i].name.chars().next().map(|c| c.to_ascii_uppercase()).unwrap_or('#');
                            if letter == '#' { !c.is_alphabetic() } else { c >= letter }
                        });
                        st.scroll_to_row = target;
                    }
                }
                in_rect(ui, list_rect, Layout::top_down(Align::Min), |ui| {
                    let mut area = egui::ScrollArea::vertical().id_salt("aeq_list").auto_shrink([false, false]);
                    if let Some(r) = st.scroll_to_row.take() {
                        area = area.vertical_scroll_offset(r as f32 * ROW_H);
                    }
                    area.show_rows(ui, ROW_H, rows.len(), |ui, range| {
                        for row in &rows[range] {
                            match *row {
                                Row::Model(i) => {
                                    let m = &models[i];
                                    let picked = m.entries.iter().any(|e| Some(&e.path) == selected_path.as_ref());
                                    let single = m.entries.len() == 1;
                                    let loading = single && st.busy.as_deref() == Some(m.entries[0].path.as_str());
                                    let sub = if single {
                                        m.entries[0].source_label()
                                    } else {
                                        tr("{n} profiles").replace("{n}", &m.entries.len().to_string())
                                    };
                                    let expanded = st.expanded.as_deref() == Some(m.name.as_str());
                                    let r = model_row(ui, &m.name, &sub, picked && single, loading, !single, expanded);
                                    if r.clicked() {
                                        if single {
                                            pick = Some(m.entries[0].clone());
                                        } else {
                                            st.expanded = if expanded { None } else { Some(m.name.clone()) };
                                        }
                                    }
                                }
                                Row::Entry(i, k) => {
                                    let e = &models[i].entries[k];
                                    let sel = Some(&e.path) == selected_path.as_ref();
                                    let loading = st.busy.as_deref() == Some(e.path.as_str());
                                    if entry_row(ui, &e.source_label(), sel, loading).clicked() {
                                        pick = Some(e.clone());
                                    }
                                }
                            }
                        }
                    });
                });
            }
        }
    });
    if let Some(e) = pick {
        st.select(&ctx, &e);
    }
    let _ = accent;
}

fn model_row(ui: &mut Ui, name: &str, sub: &str, picked: bool, loading: bool, chevron: bool, expanded: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect.shrink(1.0), CornerRadius::same(10), theme::ROW);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    kit::headphone_sketch(&p.with_clip_rect(rect), Rect::from_center_size(pos2(rect.left() + 22.0, rect.center().y), vec2(30.0, 22.0)), false);
    let tx = rect.left() + 42.0;
    let w = rect.right() - tx - 34.0;
    paint_text(ui, pos2(tx, rect.top() + 5.0), name, FontId::proportional(14.5), Color32::WHITE, w);
    paint_text(ui, pos2(tx, rect.top() + 25.0), sub, FontId::proportional(11.5), theme::TEXT_SECONDARY, w);
    let end = pos2(rect.right() - 16.0, rect.center().y);
    if loading {
        egui::Spinner::new().size(14.0).paint_at(ui, Rect::from_center_size(end, vec2(14.0, 14.0)));
    } else if picked {
        check(ui.painter(), end, Color32::WHITE);
    } else if chevron {
        let a = ui.ctx().animate_bool_with_time(resp.id.with("chev"), expanded, 0.2) * std::f32::consts::FRAC_PI_2;
        let pts = [vec2(-3.0, -5.0), vec2(3.0, 0.0), vec2(-3.0, 5.0)]
            .iter()
            .map(|v| end + vec2(v.x * a.cos() - v.y * a.sin(), v.x * a.sin() + v.y * a.cos()))
            .collect();
        ui.painter().add(Shape::line(pts, Stroke::new(1.6, theme::TEXT_FAINT)));
    }
    resp
}

fn entry_row(ui: &mut Ui, label: &str, selected: bool, loading: bool) -> egui::Response {
    let (full, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    if !ui.is_rect_visible(full) {
        return resp;
    }
    let rect = Rect::from_min_max(pos2(full.left() + 44.0, full.top() + 3.0), pos2(full.right() - 10.0, full.bottom() - 3.0));
    let fill = if selected { theme::ROW_SELECTED } else if resp.hovered() { theme::PILL_SELECTED } else { theme::ROW };
    ui.painter().rect_filled(rect, CornerRadius::same(12), fill);
    paint_text(ui, pos2(rect.left() + 12.0, rect.center().y - 8.0), label, FontId::proportional(13.0), Color32::WHITE, rect.width() - 50.0);
    let end = pos2(rect.right() - 16.0, rect.center().y);
    if loading {
        egui::Spinner::new().size(13.0).paint_at(ui, Rect::from_center_size(end, vec2(13.0, 13.0)));
    } else if selected {
        check(ui.painter(), end, Color32::WHITE);
    }
    resp
}

fn check(p: &egui::Painter, c: Pos2, col: Color32) {
    p.add(Shape::line(vec![c + vec2(-6.0, 0.0), c + vec2(-2.0, 4.0), c + vec2(6.0, -5.0)], Stroke::new(2.0, col)));
}

fn in_rect<R>(ui: &mut Ui, rect: Rect, layout: Layout, f: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(layout), f).inner
}

/// One line of text, cut with an ellipsis to `max_w`.
fn paint_text(ui: &Ui, pos: Pos2, text: &str, font: FontId, color: Color32, max_w: f32) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: max_w.max(10.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let g = ui.painter().layout_job(job);
    ui.painter().galley(pos, g, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_draws_headless_without_network() {
        let ctx = egui::Context::default();
        let mut st = DbState { index: IndexState::Ready(Arc::new(autoeq::parse_index(
            "- [Sennheiser HD 650](./oratory1990/over-ear/Sennheiser%20HD%20650) by oratory1990\n\
             - [Sennheiser HD 650](./crinacle/GRAS%2043AG-7%20over-ear/Sennheiser%20HD%20650) by crinacle on GRAS 43AG-7\n",
        ))), opened: true, ..Default::default() };
        let mut eq = EqSettings::default();
        st.expanded = Some("Sennheiser HD 650".into());
        for _ in 0..2 {
            let mut out = ctx.run_ui(Default::default(), |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| page(ui, &mut eq, &mut st, Color32::RED));
            });
            out.textures_delta.clear();
        }
        assert_eq!(st.results.len(), 1);
        assert!(eq.correction.is_none());
    }

    #[test]
    fn measured_message_applies_correction() {
        let ctx = egui::Context::default();
        let mut st = DbState::default();
        let mut eq = EqSettings::default();
        let entry = AutoEqEntry {
            name: "HP".into(),
            source: "src".into(),
            rig: None,
            path: "src/over-ear/HP".into(),
            form: HeadphoneForm::OverEar,
        };
        let m = Measurement { frequencies: vec![20.0, 1000.0], raw: vec![0.0, 0.0], target: vec![0.0, 0.0] };
        let own = Fit { preamp_db: -2.0, filters: vec![mochy_core::eq::EqNode::peaking(100.0, 2.0)] };
        st.busy = Some(entry.path.clone());
        st.inbox.lock().unwrap().push(Msg::Measured { entry, apply: true, result: Ok((m, own.clone())) });
        st.poll(&ctx, &mut eq);
        let c = eq.correction.as_ref().unwrap();
        assert!(eq.enabled && eq.correction_enabled);
        assert_eq!(c.filters, own.filters);
        assert_eq!(c.preamp_db, -2.0);
        assert!(st.busy.is_none());
        assert!(st.loaded.is_some());
    }
}
