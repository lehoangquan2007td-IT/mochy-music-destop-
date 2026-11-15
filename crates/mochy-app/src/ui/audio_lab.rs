//! Audio Lab page widget (spec §4.2–§4.7, §7.5 "Audio Lab"): 10-band / parametric
//! EQ editor with live response curve, built-in presets, preamp + auto-preamp,
//! loudness, ReplayGain normalization, exponential volume curve, speed.
//!
//! CONTRACT (used by the app shell — keep the signature stable):
//! the widget edits `lab` in place and returns `true` when anything changed.
//! The caller then pushes the new values to the audio engine
//! (`Command::SetEq` / `SetLoudness` / `SetVolumeCurve` / `SetReplayGain` /
//! `SetSpeed`) and persists settings.
//!
//! Extra API: [`AudioLabView::set_spectrum`] feeds the faint live spectrum
//! drawn behind the EQ curve (64 log-spaced bands, dBFS).

#[path = "audio_lab_autoeq.rs"]
pub mod autoeq;

use std::ops::RangeInclusive;

use egui::{
    Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Layout, Mesh, Pos2, Rect, Response,
    RichText, Sense, Shape, Stroke, Ui, Vec2, pos2, vec2,
};
use mochy_core::eq::{
    BAND_FREQS_HZ, BUILT_IN_PRESETS, BiquadCoeffs, EqNode, EqSettings, FilterType, MAX_BANDS,
    MAX_GAIN_DB, UserEqPreset, auto_preamp_db, effective_preamp_db, filters_response_db, log_freqs,
    preset_by_id, settings_from_preset,
};
use mochy_core::settings::{AudioLab, REVERB_PRESETS, ReverbSettings};

use super::theme;

/// Sample rate used for plotting (same as the auto-preamp design).
const PLOT_SR: f32 = 48_000.0;
/// Number of plotted curve points.
const CURVE_POINTS: usize = 400;
const F_MIN: f32 = 20.0;
const F_MAX: f32 = 20_000.0;
/// Visible dB range of the graph (±).
const GRAPH_DB: f32 = MAX_GAIN_DB;

/// Android `LabColors.Nodes` — one color per EQ node.
const NODE_COLORS: [Color32; 10] = [
    Color32::from_rgb(0xF2, 0x8B, 0xA8),
    Color32::from_rgb(0xF2, 0xA4, 0x6B),
    Color32::from_rgb(0xF2, 0xD4, 0x6B),
    Color32::from_rgb(0x8F, 0xD9, 0x8B),
    Color32::from_rgb(0x6B, 0xD9, 0xC4),
    Color32::from_rgb(0x6B, 0xBA, 0xF2),
    Color32::from_rgb(0x8C, 0x9E, 0xF2),
    Color32::from_rgb(0xB9, 0x8C, 0xF2),
    Color32::from_rgb(0xE0, 0x8C, 0xF2),
    Color32::from_rgb(0xF2, 0x6B, 0x6B),
];

fn node_color(i: usize) -> Color32 {
    NODE_COLORS[i % NODE_COLORS.len()]
}

const FILTER_TYPES: [(FilterType, &str, &str); 5] = [
    (FilterType::Peaking, "PK", "Peak"),
    (FilterType::LowShelf, "LSC", "Low shelf"),
    (FilterType::HighShelf, "HSC", "High shelf"),
    (FilterType::LowPass, "LPQ", "Low pass"),
    (FilterType::HighPass, "HPQ", "High pass"),
];

fn is_pass(t: FilterType) -> bool {
    matches!(t, FilterType::LowPass | FilterType::HighPass)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum EqViewMode {
    #[default]
    Graph,
    Bands,
}

/// Cached plot data, rebuilt only when the EQ changes.
struct CurveCache {
    eq: EqSettings,
    freqs: Vec<f32>,
    total: Vec<f32>,
    max_total: f32,
    auto_preamp: f32,
    effective_preamp: f32,
}

/// Persistent UI-only state for the Audio Lab page (selected node, etc.).
#[derive(Default)]
pub struct AudioLabView {
    selected: Option<usize>,
    dragging: Option<usize>,
    drag_offset: Vec2,
    menu_node: Option<usize>,
    menu_point: Option<(f32, f32)>,
    view_mode: EqViewMode,
    save_name: String,
    rename: Option<(String, String)>,
    autoeq_text: String,
    autoeq_status: Option<(bool, String)>,
    spectrum_target: Vec<f32>,
    spectrum_display: Vec<f32>,
    spectrum_stamp: Option<std::time::Instant>,
    curve: Option<CurveCache>,
}

impl AudioLabView {
    /// Feed the live spectrum (dBFS, 64 log-spaced bands over 20 Hz–20 kHz,
    /// roughly −90..0). Drawn faintly behind the EQ curve, smoothed with a
    /// falloff. An empty vector hides it.
    pub fn set_spectrum(&mut self, bands: Vec<f32>) {
        self.spectrum_target = bands;
        self.spectrum_stamp = Some(std::time::Instant::now());
    }

    /// Index of the node selected in the graph, if any.
    pub fn selected_node(&self) -> Option<usize> {
        self.selected
    }

    /// Draw the full Audio Lab page. Returns `true` if `lab` was modified.
    pub fn show(&mut self, ui: &mut Ui, lab: &mut AudioLab, accent: Color32) -> bool {
        let before = lab.clone();
        sanitize(lab);
        let accent = theme::readable_on_black(accent);

        egui::ScrollArea::vertical()
            .id_salt("mochy_audio_lab_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.visuals_mut().selection.bg_fill = accent;
                ui.visuals_mut().slider_trailing_fill = true;
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                let max_w = ui.available_width().min(1100.0);
                let pad = ((ui.available_width() - max_w) / 2.0).max(0.0);
                ui.horizontal(|ui| {
                    ui.add_space(pad);
                    ui.vertical(|ui| {
                        ui.set_width(max_w);
                        self.page(ui, lab, accent);
                    });
                });
            });

        sanitize(lab);
        if self.selected.is_some_and(|i| i >= lab.eq.nodes.len()) {
            self.selected = None;
        }
        *lab != before
    }

    fn page(&mut self, ui: &mut Ui, lab: &mut AudioLab, accent: Color32) {
        ui.add_space(8.0);
        ui.label(RichText::new("Audio Lab").font(theme::bold(ui.ctx(), 30.0)).color(Color32::WHITE));
        ui.label(RichText::new("Shape how everything sounds").color(theme::TEXT_SECONDARY));
        ui.add_space(6.0);

        self.eq_card(ui, &mut lab.eq, &mut lab.user_presets, accent);
        self.autoeq_card(ui, &mut lab.eq, accent);

        let wide = ui.available_width() >= 760.0;
        if wide {
            ui.columns(2, |cols| {
                loudness_card(&mut cols[0], lab, accent);
                normalization_card(&mut cols[0], lab, accent);
                volume_curve_card(&mut cols[0], lab, accent);
                speed_card(&mut cols[1], lab, accent);
                spatial_card(&mut cols[1], lab, accent);
            });
            reverb_card(ui, &mut lab.reverb, accent);
        } else {
            loudness_card(ui, lab, accent);
            normalization_card(ui, lab, accent);
            volume_curve_card(ui, lab, accent);
            speed_card(ui, lab, accent);
            reverb_card(ui, &mut lab.reverb, accent);
            spatial_card(ui, lab, accent);
        }
        ui.add_space(24.0);
    }

    // -----------------------------------------------------------------------
    // Equalizer
    // -----------------------------------------------------------------------

    fn eq_card(&mut self, ui: &mut Ui, eq: &mut EqSettings, user: &mut Vec<UserEqPreset>, accent: Color32) {
        let nodes_before = eq.nodes.clone();
        let preamp_before = eq.preamp_db;
        let mut preset_applied = false;

        card(ui, |ui| {
            card_header(ui, "Equalizer", Some("Parametric, up to 20 bands"), Some(&mut eq.enabled), accent);
            let dim = !eq.enabled;
            if dim {
                ui.multiply_opacity(0.6);
            }

            // ---- presets ------------------------------------------------
            section_label(ui, "Presets");
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for p in BUILT_IN_PRESETS {
                    let sel = eq.preset_id.as_deref() == Some(p.id);
                    if chip(ui, p.name, sel, accent).clicked() {
                        let mut s = settings_from_preset(p);
                        s.auto_preamp = eq.auto_preamp;
                        *eq = s;
                        preset_applied = true;
                        self.selected = None;
                    }
                }
            });
            self.user_presets_ui(ui, eq, user, accent, &mut preset_applied);

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let cur = if self.view_mode == EqViewMode::Graph { 0 } else { 1 };
                if let Some(i) = segmented(ui, &["Graph", "10-band"], cur) {
                    self.view_mode = if i == 0 { EqViewMode::Graph } else { EqViewMode::Bands };
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Reset to flat").clicked() {
                        if let Some(flat) = preset_by_id("flat") {
                            let mut s = settings_from_preset(flat);
                            s.auto_preamp = eq.auto_preamp;
                            s.enabled = eq.enabled;
                            *eq = s;
                            preset_applied = true;
                        }
                        self.selected = None;
                    }
                });
            });

            self.ensure_curve(eq);
            match self.view_mode {
                EqViewMode::Graph => {
                    self.graph(ui, eq, accent);
                    self.node_strip(ui, eq);
                    self.node_editor(ui, eq, accent);
                }
                EqViewMode::Bands => self.bands_view(ui, eq, accent),
            }

            // ---- preamp ------------------------------------------------
            ui.add_space(4.0);
            section_label(ui, "Preamp");
            slider_row(ui, "Preamp", &mut eq.preamp_db, -12.0..=12.0, " dB", 1, false);
            self.ensure_curve(eq);
            let (auto_db, eff, max_total) = self
                .curve
                .as_ref()
                .map(|c| (c.auto_preamp, c.effective_preamp, c.max_total))
                .unwrap_or_default();
            toggle_row(
                ui,
                "Auto preamp",
                Some(&format!("Headroom from the curve: {auto_db:+.1} dB · total preamp {eff:+.1} dB")),
                &mut eq.auto_preamp,
                accent,
            );
            if eq.enabled && !eq.auto_preamp && max_total > 0.05 {
                ui.add_space(2.0);
                egui::Frame::new()
                    .fill(Color32::from_rgb(0x3A, 0x2E, 0x14))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(
                            RichText::new(format!(
                                "⚠ Peaks reach {max_total:+.1} dB — this can clip. Turn on Auto preamp or lower the preamp by {:.1} dB.",
                                max_total
                            ))
                            .color(Color32::from_rgb(0xF2, 0xD4, 0x6B)),
                        );
                    });
            }
        });

        let edited = eq.nodes != nodes_before || eq.preamp_db != preamp_before;
        if edited && !preset_applied {
            eq.preset_id = None;
        }
    }

    fn user_presets_ui(
        &mut self,
        ui: &mut Ui,
        eq: &mut EqSettings,
        user: &mut Vec<UserEqPreset>,
        accent: Color32,
        preset_applied: &mut bool,
    ) {
        ui.add_space(2.0);
        section_label(ui, "Your presets");
        let mut delete: Option<usize> = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            if user.is_empty() {
                ui.label(RichText::new("None yet — save the current curve below.").color(theme::TEXT_FAINT));
            }
            for p in user.iter() {
                let sel = eq.preset_id.as_deref() == Some(p.id.as_str());
                if chip(ui, &p.name, sel, accent).clicked() {
                    eq.nodes = p.nodes.iter().take(MAX_BANDS).cloned().collect();
                    eq.preamp_db = p.preamp_db;
                    eq.preset_id = Some(p.id.clone());
                    eq.enabled = true;
                    *preset_applied = true;
                    self.selected = None;
                }
            }
        });

        ui.horizontal(|ui| {
            let te = egui::TextEdit::singleline(&mut self.save_name)
                .hint_text("Preset name")
                .desired_width(180.0);
            let r = ui.add(te);
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Save as preset").clicked() || enter) && !self.save_name.trim().is_empty() {
                let millis = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0);
                let id = format!("user_{millis}");
                user.push(UserEqPreset {
                    id: id.clone(),
                    name: self.save_name.trim().to_owned(),
                    nodes: eq.nodes.clone(),
                    preamp_db: eq.preamp_db,
                });
                eq.preset_id = Some(id);
                *preset_applied = true;
                self.save_name.clear();
            }

            // Selected user preset: overwrite / rename / delete.
            let sel_idx = eq
                .preset_id
                .as_deref()
                .and_then(|id| user.iter().position(|p| p.id == id));
            if let Some(i) = sel_idx {
                ui.separator();
                match &mut self.rename {
                    Some((id, name)) if *id == user[i].id => {
                        let r = ui.add(egui::TextEdit::singleline(name).desired_width(140.0));
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.button("OK").clicked() || enter {
                            if !name.trim().is_empty() {
                                user[i].name = name.trim().to_owned();
                            }
                            self.rename = None;
                        } else if ui.button("Cancel").clicked() {
                            self.rename = None;
                        }
                    }
                    _ => {
                        if ui.button("Rename").clicked() {
                            self.rename = Some((user[i].id.clone(), user[i].name.clone()));
                        }
                        if ui.button("Delete").clicked() {
                            delete = Some(i);
                        }
                    }
                }
            }
        });
        if let Some(i) = delete {
            user.remove(i);
            eq.preset_id = None;
            *preset_applied = true; // nodes unchanged; keep logic simple
            self.rename = None;
        }
    }

    fn ensure_curve(&mut self, eq: &EqSettings) {
        if self.curve.as_ref().is_some_and(|c| c.eq == *eq) {
            return;
        }
        let freqs = log_freqs(CURVE_POINTS);
        let auto = auto_preamp_db(eq);
        let eff = effective_preamp_db(eq);
        // Identical to `total_response_db` (filters + effective preamp, 0 when
        // disabled) without recomputing the auto headroom per point.
        let total: Vec<f32> = freqs
            .iter()
            .map(|&f| if eq.enabled { filters_response_db(eq, f, PLOT_SR) + eff } else { 0.0 })
            .collect();
        let max_total = total.iter().copied().fold(f32::MIN, f32::max);
        self.curve = Some(CurveCache {
            eq: eq.clone(),
            freqs,
            total,
            max_total,
            auto_preamp: auto,
            effective_preamp: eff,
        });
    }

    fn update_spectrum(&mut self, ui: &Ui) {
        if self.spectrum_target.is_empty() {
            self.spectrum_display.clear();
            return;
        }
        let dt = ui.input(|i| i.stable_dt).clamp(0.0, 0.1);
        let stale = self.spectrum_stamp.is_none_or(|t| t.elapsed().as_secs_f32() > 0.5);
        if self.spectrum_display.len() != self.spectrum_target.len() {
            self.spectrum_display = vec![-90.0; self.spectrum_target.len()];
        }
        let mut moving = false;
        for (d, &t) in self.spectrum_display.iter_mut().zip(&self.spectrum_target) {
            let t = if stale { -90.0 } else { t.clamp(-120.0, 6.0) };
            if t > *d {
                *d += (t - *d) * (dt * 30.0).min(1.0); // fast attack
            } else {
                *d = (*d - 36.0 * dt).max(t); // 36 dB/s falloff
            }
            if *d > -89.0 {
                moving = true;
            }
        }
        if moving {
            ui.ctx().request_repaint();
        }
    }

    fn graph(&mut self, ui: &mut Ui, eq: &mut EqSettings, accent: Color32) {
        self.update_spectrum(ui);
        let width = ui.available_width();
        let height = (width * 0.40).clamp(220.0, 340.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
        let plot = Rect::from_min_max(rect.min + vec2(36.0, 12.0), rect.max - vec2(14.0, 26.0));
        let map = PlotMap { plot };

        // ---------------- interaction ----------------
        let hover = resp.hover_pos();
        let hovered_node = hover.and_then(|p| nearest_node(eq, &map, p, 22.0));

        if resp.drag_started() {
            let origin = ui.input(|i| i.pointer.press_origin()).or(hover);
            if let Some(p) = origin
                && let Some(i) = nearest_node(eq, &map, p, 24.0)
            {
                self.dragging = Some(i);
                self.selected = Some(i);
                self.drag_offset = map.node_pos(&eq.nodes[i]) - p;
            }
        }
        if let (Some(i), Some(p)) = (self.dragging, resp.interact_pointer_pos())
            && resp.dragged()
            && i < eq.nodes.len()
        {
            let target = p + self.drag_offset;
            let n = &mut eq.nodes[i];
            n.frequency_hz = round_freq(map.f_of(target.x));
            if !is_pass(n.filter_type) {
                n.gain_db = snap_gain(map.db_of(target.y));
            }
        }
        if resp.drag_stopped() {
            self.dragging = None;
        }
        if resp.clicked() {
            self.selected = resp.interact_pointer_pos().and_then(|p| nearest_node(eq, &map, p, 22.0));
        }
        if resp.double_clicked()
            && let Some(p) = resp.interact_pointer_pos()
            && nearest_node(eq, &map, p, 22.0).is_none()
            && eq.nodes.len() < MAX_BANDS
            && plot.expand(6.0).contains(p)
        {
            eq.nodes.push(EqNode::peaking(round_freq(map.f_of(p.x)), snap_gain(map.db_of(p.y))));
            self.selected = Some(eq.nodes.len() - 1);
        }
        // Mouse wheel over the graph = Q of the hovered (or selected) node.
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            let target = hovered_node.or(self.selected);
            if scroll != 0.0
                && let Some(i) = target
                && i < eq.nodes.len()
            {
                let n = &mut eq.nodes[i];
                n.q = round2((n.q * (scroll * 0.004).exp()).clamp(0.1, 10.0));
                self.selected = Some(i);
                ui.input_mut(|inp| inp.smooth_scroll_delta.y = 0.0);
            }
        }
        if resp.secondary_clicked() {
            let p = resp.interact_pointer_pos();
            self.menu_node = p.and_then(|p| nearest_node(eq, &map, p, 24.0));
            self.menu_point = p.map(|p| (round_freq(map.f_of(p.x)), snap_gain(map.db_of(p.y))));
            if self.menu_node.is_some() {
                self.selected = self.menu_node;
            }
        }
        if self.dragging.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        } else if hovered_node.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        } else if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        }
        let menu_node = self.menu_node;
        let menu_point = self.menu_point;
        let mut new_sel: Option<Option<usize>> = None;
        resp.context_menu(|ui| {
            ui.set_min_width(170.0);
            match menu_node.filter(|&i| i < eq.nodes.len()) {
                Some(i) => {
                    ui.label(RichText::new(format!("Band {}", i + 1)).strong());
                    for (t, code, name) in FILTER_TYPES {
                        let cur = eq.nodes[i].filter_type == t;
                        if ui.selectable_label(cur, format!("{code}  {name}")).clicked() {
                            eq.nodes[i].filter_type = t;
                            ui.close();
                        }
                    }
                    ui.separator();
                    let mut en = eq.nodes[i].enabled;
                    if ui.checkbox(&mut en, "Enabled").changed() {
                        eq.nodes[i].enabled = en;
                    }
                    if ui.button("Reset gain").clicked() {
                        eq.nodes[i].gain_db = 0.0;
                        ui.close();
                    }
                    if ui.button("Delete band").clicked() {
                        eq.nodes.remove(i);
                        new_sel = Some(None);
                        ui.close();
                    }
                }
                None => {
                    let can_add = eq.nodes.len() < MAX_BANDS;
                    if ui.add_enabled(can_add, egui::Button::new("Add band here")).clicked() {
                        let (f, g) = menu_point.unwrap_or((1000.0, 0.0));
                        eq.nodes.push(EqNode::peaking(f, g));
                        new_sel = Some(Some(eq.nodes.len() - 1));
                        ui.close();
                    }
                    if ui.button("Flatten all gains").clicked() {
                        for n in &mut eq.nodes {
                            n.gain_db = 0.0;
                        }
                        ui.close();
                    }
                }
            }
        });
        if let Some(s) = new_sel {
            self.selected = s;
        }

        // ---------------- painting ----------------
        self.ensure_curve(eq);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(18), theme::STAGE);
        painter.rect_stroke(rect, CornerRadius::same(18), Stroke::new(1.0, theme::STAGE_LINE), egui::StrokeKind::Inside);
        draw_grid(&painter, &map);

        let clip = painter.with_clip_rect(plot.expand(1.0));
        if !self.spectrum_display.is_empty() {
            draw_spectrum(&clip, &map, &self.spectrum_display);
        }

        // Selected node's own response, tinted.
        if let Some(i) = self.selected.filter(|&i| i < eq.nodes.len()) {
            let n = &eq.nodes[i];
            if n.enabled {
                let c = BiquadCoeffs::design(n.filter_type, n.frequency_hz, n.gain_db, n.q, PLOT_SR);
                let freqs = &self.curve.as_ref().map(|c| c.freqs.clone()).unwrap_or_default();
                let resp_db: Vec<f32> = freqs.iter().map(|&f| c.magnitude_db(f, PLOT_SR)).collect();
                let col = node_color(i);
                draw_curve_fill(&clip, &map, freqs, &resp_db, col.gamma_multiply(0.20), col.gamma_multiply(0.02));
                clip.add(Shape::line(curve_points(&map, freqs, &resp_db), Stroke::new(1.2, col.gamma_multiply(0.8))));
            }
        }

        if let Some(c) = &self.curve {
            let col = if eq.enabled { accent } else { Color32::from_gray(0x80) };
            draw_curve_fill(&clip, &map, &c.freqs, &c.total, col.gamma_multiply(0.28), col.gamma_multiply(0.0));
            clip.add(Shape::line(curve_points(&map, &c.freqs, &c.total), Stroke::new(2.6, col)));
            if !eq.enabled {
                // What it would sound like when enabled.
                let mut on = eq.clone();
                on.enabled = true;
                let eff = effective_preamp_db(&on);
                let pts: Vec<Pos2> = c
                    .freqs
                    .iter()
                    .step_by(4)
                    .map(|&f| pos2(map.x_of(f), map.y_of(filters_response_db(&on, f, PLOT_SR) + eff)))
                    .collect();
                clip.extend(Shape::dashed_line(&pts, Stroke::new(1.2, Color32::from_white_alpha(70)), 6.0, 5.0));
            }
        }

        // Nodes.
        let t = ui.input(|i| i.time) as f32;
        for (i, n) in eq.nodes.iter().enumerate() {
            let p = map.node_pos(n);
            let selected = self.selected == Some(i);
            let col = if n.enabled { node_color(i) } else { theme::TEXT_FAINT };
            let pulse = if selected { 0.5 + 0.5 * (t * 3.0).sin() } else { 0.0 };
            let r = if selected { 8.5 + 1.5 * pulse } else if hovered_node == Some(i) { 7.5 } else { 6.5 };
            if selected {
                painter.circle_filled(p, r * 2.1, col.gamma_multiply(0.18 + 0.1 * pulse));
            }
            painter.circle_filled(p, r + 2.0, Color32::BLACK);
            painter.circle_filled(p, r, col);
            if selected {
                painter.circle_stroke(p, r + 2.0, Stroke::new(1.5, Color32::WHITE));
            }
            if !n.enabled {
                painter.line_segment([p - vec2(r, -r), p + vec2(r, -r)], Stroke::new(1.5, Color32::BLACK));
            }
        }
        if self.selected.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        }

        // Readout of the active node.
        let readout = self.dragging.or(hovered_node).or(self.selected).filter(|&i| i < eq.nodes.len());
        if let Some(i) = readout {
            let n = &eq.nodes[i];
            let txt = if is_pass(n.filter_type) {
                format!("{} · {} · Q {:.2}", type_code(n.filter_type), fmt_hz(n.frequency_hz), n.q)
            } else {
                format!("{} · {} · {:+.1} dB · Q {:.2}", type_code(n.filter_type), fmt_hz(n.frequency_hz), n.gain_db, n.q)
            };
            let font = FontId::proportional(12.0);
            let g = painter.layout_no_wrap(txt, font, Color32::WHITE);
            let p = map.node_pos(n);
            let mut pos = p + vec2(14.0, -28.0);
            if pos.x + g.size().x + 12.0 > plot.right() {
                pos.x = p.x - g.size().x - 26.0;
            }
            pos.y = pos.y.clamp(plot.top(), plot.bottom() - g.size().y - 6.0);
            let bg = Rect::from_min_size(pos, g.size() + vec2(12.0, 6.0));
            painter.rect_filled(bg, CornerRadius::same(8), Color32::from_black_alpha(200));
            painter.galley(pos + vec2(6.0, 3.0), g, Color32::WHITE);
        }
        if eq.nodes.is_empty() {
            painter.text(
                plot.center(),
                Align2::CENTER_CENTER,
                "Double-click to add a band",
                FontId::proportional(14.0),
                theme::TEXT_SECONDARY,
            );
        }
    }

    fn node_strip(&mut self, ui: &mut Ui, eq: &mut EqSettings) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for (i, n) in eq.nodes.iter().enumerate() {
                let sel = self.selected == Some(i);
                let label = format!("{}  {}", i + 1, fmt_hz(n.frequency_hz));
                if dot_chip(ui, &label, if n.enabled { node_color(i) } else { theme::TEXT_FAINT }, sel).clicked() {
                    self.selected = if sel { None } else { Some(i) };
                }
            }
            let can_add = eq.nodes.len() < MAX_BANDS;
            let add = ui.add_enabled(can_add, egui::Button::new("+ Add band").corner_radius(CornerRadius::same(255)));
            if add.clicked() {
                eq.nodes.push(EqNode::peaking(free_frequency(&eq.nodes), 0.0));
                self.selected = Some(eq.nodes.len() - 1);
            }
            ui.label(
                RichText::new(format!("{}/{}", eq.nodes.len(), MAX_BANDS)).color(theme::TEXT_FAINT).small(),
            );
        });
        ui.label(
            RichText::new("Drag a dot to move it · scroll for Q · double-click to add · right-click for options")
                .small()
                .color(theme::TEXT_FAINT),
        );
    }

    fn node_editor(&mut self, ui: &mut Ui, eq: &mut EqSettings, accent: Color32) {
        let Some(i) = self.selected.filter(|&i| i < eq.nodes.len()) else { return };
        let mut delete = false;
        egui::Frame::new()
            .fill(theme::ROW)
            .corner_radius(CornerRadius::same(14))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let n = &mut eq.nodes[i];
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 5.0, node_color(i));
                    ui.label(RichText::new(format!("Band {}", i + 1)).strong().color(Color32::WHITE));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Delete").clicked() {
                            delete = true;
                        }
                        toggle(ui, &mut n.enabled, accent);
                        ui.label(RichText::new("Enabled").color(theme::TEXT_SECONDARY));
                    });
                });
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for (t, code, name) in FILTER_TYPES {
                        if chip(ui, code, n.filter_type == t, accent).on_hover_text(name).clicked() {
                            n.filter_type = t;
                        }
                    }
                });
                slider_row(ui, "Frequency", &mut n.frequency_hz, F_MIN..=F_MAX, " Hz", 0, true);
                ui.add_enabled_ui(!is_pass(n.filter_type), |ui| {
                    slider_row(ui, "Gain", &mut n.gain_db, -MAX_GAIN_DB..=MAX_GAIN_DB, " dB", 1, false);
                });
                slider_row(ui, "Q", &mut n.q, 0.1..=10.0, "", 2, true);
            });
        if delete {
            eq.nodes.remove(i);
            self.selected = None;
        }
    }

    fn bands_view(&mut self, ui: &mut Ui, eq: &mut EqSettings, accent: Color32) {
        let is_ten = eq.nodes.len() == BAND_FREQS_HZ.len()
            && eq
                .nodes
                .iter()
                .zip(BAND_FREQS_HZ)
                .all(|(n, f)| n.filter_type == FilterType::Peaking && (n.frequency_hz - f).abs() < 0.5);
        if !is_ten {
            egui::Frame::new()
                .fill(theme::ROW)
                .corner_radius(CornerRadius::same(14))
                .inner_margin(egui::Margin::same(14))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label("This EQ uses custom bands. The classic view needs the 10 standard bands (31 Hz – 16 kHz).");
                    if ui.button("Convert to 10 bands").on_hover_text("Fits the current curve onto 10 peaking bands").clicked() {
                        let gains: Vec<f32> = BAND_FREQS_HZ
                            .iter()
                            .map(|&f| snap_gain(filters_response_db(eq, f, PLOT_SR) * 0.8))
                            .collect();
                        eq.nodes = BAND_FREQS_HZ.iter().zip(gains).map(|(&f, g)| EqNode::peaking(f, g)).collect();
                        self.selected = None;
                    }
                });
            return;
        }
        let height = 200.0;
        let n = eq.nodes.len();
        let gap = 6.0;
        let w = ((ui.available_width() - gap * (n as f32 - 1.0)) / n as f32).max(24.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (i, node) in eq.nodes.iter_mut().enumerate() {
                ui.vertical(|ui| {
                    ui.set_width(w);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(format!("{:+.1}", node.gain_db)).small().color(Color32::WHITE));
                    });
                    fader(ui, &mut node.gain_db, vec2(w, height), accent, i);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(fmt_hz_short(node.frequency_hz)).small().color(theme::TEXT_SECONDARY));
                    });
                });
            }
        });
        ui.label(RichText::new("Double-click a fader to reset it").small().color(theme::TEXT_FAINT));
    }

    // -----------------------------------------------------------------------
    // AutoEq
    // -----------------------------------------------------------------------

    fn autoeq_card(&mut self, ui: &mut Ui, eq: &mut EqSettings, accent: Color32) {
        card(ui, |ui| {
            card_header(ui, "AutoEQ", Some("Import or export ParametricEQ.txt"), None, accent);
            ui.add(
                egui::TextEdit::multiline(&mut self.autoeq_text)
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(5)
                    .desired_width(f32::INFINITY)
                    .hint_text("Preamp: -6.2 dB\nFilter 1: ON PK Fc 105 Hz Gain 3.4 dB Q 0.70\n…"),
            );
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!self.autoeq_text.trim().is_empty(), egui::Button::new("Import pasted")).clicked() {
                    let text = self.autoeq_text.clone();
                    self.import_autoeq(eq, &text);
                }
                if ui.button("Open file…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .set_title("Open AutoEq ParametricEQ.txt")
                        .add_filter("ParametricEQ", &["txt"])
                        .pick_file()
                {
                    match std::fs::read_to_string(&path) {
                        Ok(text) => {
                            self.import_autoeq(eq, &text);
                            self.autoeq_text = text;
                        }
                        Err(e) => self.autoeq_status = Some((false, format!("Couldn't read file: {e}"))),
                    }
                }
                ui.separator();
                if ui.button("Copy current EQ").clicked() {
                    let text = autoeq::export_autoeq(eq);
                    ui.ctx().copy_text(text.clone());
                    self.autoeq_text = text;
                    self.autoeq_status = Some((true, "Copied to clipboard".into()));
                }
                if ui.button("Save as file…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .set_title("Export EQ")
                        .set_file_name("ParametricEQ.txt")
                        .add_filter("ParametricEQ", &["txt"])
                        .save_file()
                {
                    self.autoeq_status = Some(match std::fs::write(&path, autoeq::export_autoeq(eq)) {
                        Ok(()) => (true, format!("Saved {}", path.display())),
                        Err(e) => (false, format!("Couldn't save: {e}")),
                    });
                }
            });
            if let Some((ok, msg)) = &self.autoeq_status {
                let col = if *ok { Color32::from_rgb(0x8F, 0xD9, 0x8B) } else { Color32::from_rgb(0xFF, 0x8A, 0x80) };
                ui.label(RichText::new(msg).color(col).small());
            }
        });
    }

    fn import_autoeq(&mut self, eq: &mut EqSettings, text: &str) {
        match autoeq::parse_autoeq(text) {
            Ok(p) => {
                let n = p.nodes.len();
                eq.nodes = p.nodes;
                eq.preamp_db = p.preamp_db;
                eq.auto_preamp = false;
                eq.enabled = true;
                eq.preset_id = None;
                self.selected = None;
                let mut msg = format!("Imported {n} filter(s), preamp {:+.1} dB", eq.preamp_db);
                if !p.warnings.is_empty() {
                    msg.push_str(" — ");
                    msg.push_str(&p.warnings.join("; "));
                }
                self.autoeq_status = Some((true, msg));
            }
            Err(e) => self.autoeq_status = Some((false, e)),
        }
    }
}

// ---------------------------------------------------------------------------
// Other cards
// ---------------------------------------------------------------------------

fn loudness_card(ui: &mut Ui, lab: &mut AudioLab, accent: Color32) {
    card(ui, |ui| {
        card_header(ui, "Loudness", Some("Boost + gentle compression"), Some(&mut lab.loudness_enabled), accent);
        dim_if(ui, !lab.loudness_enabled);
        slider_row(ui, "Boost", &mut lab.loudness_boost_db, 0.0..=12.0, " dB", 1, false);
        percent_row(ui, "Compression", &mut lab.loudness_compression);
    });
}

fn normalization_card(ui: &mut Ui, lab: &mut AudioLab, accent: Color32) {
    card(ui, |ui| {
        card_header(ui, "Normalization", Some("ReplayGain (track)"), Some(&mut lab.normalize_replay_gain), accent);
        dim_if(ui, !lab.normalize_replay_gain);
        slider_row(ui, "Preamp", &mut lab.normalize_preamp_db, -12.0..=12.0, " dB", 1, false);
    });
}

fn volume_curve_card(ui: &mut Ui, lab: &mut AudioLab, accent: Color32) {
    card(ui, |ui| {
        card_header(ui, "Volume curve", Some("gain = limit · volume^exponent"), Some(&mut lab.volume_curve_enabled), accent);
        dim_if(ui, !lab.volume_curve_enabled);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_width((ui.available_width() - 110.0).max(120.0));
                slider_row(ui, "Exponent", &mut lab.volume_curve_exponent, 1.0..=5.0, "", 2, false);
                slider_row(ui, "Limit", &mut lab.volume_curve_limit, 0.05..=1.0, "", 2, false);
            });
            let (rect, _) = ui.allocate_exact_size(vec2(96.0, 64.0), Sense::hover());
            let p = ui.painter_at(rect);
            p.rect_filled(rect, CornerRadius::same(10), theme::STAGE);
            let inner = rect.shrink(8.0);
            let lin: Vec<Pos2> = [0.0, 1.0].iter().map(|&v| pos2(inner.left() + v * inner.width(), inner.bottom() - v * inner.height())).collect();
            p.extend(Shape::dashed_line(&lin, Stroke::new(1.0, Color32::from_white_alpha(50)), 3.0, 3.0));
            let (e, l, on) = (lab.volume_curve_exponent, lab.volume_curve_limit, lab.volume_curve_enabled);
            let pts: Vec<Pos2> = (0..=32)
                .map(|k| {
                    let v = k as f32 / 32.0;
                    let g = if on { l * v.powf(e) } else { v };
                    pos2(inner.left() + v * inner.width(), inner.bottom() - g * inner.height())
                })
                .collect();
            p.add(Shape::line(pts, Stroke::new(2.0, accent)));
        });
    });
}

fn speed_card(ui: &mut Ui, lab: &mut AudioLab, accent: Color32) {
    card(ui, |ui| {
        card_header(ui, "Speed & pitch", None, None, accent);
        slider_row(ui, "Speed", &mut lab.speed, 0.1..=8.0, "×", 2, true);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for s in [0.5f32, 0.75, 1.0, 1.25, 1.5, 2.0] {
                if chip(ui, &format!("{s}×"), (lab.speed - s).abs() < 0.001, accent).clicked() {
                    lab.speed = s;
                }
            }
        });
        let mut pitch = lab.pitch_semitones;
        let r = slider_row(ui, "Pitch", &mut pitch, -12.0..=12.0, " st", 1, false);
        if r.changed() {
            lab.pitch_semitones = (pitch * 2.0).round() / 2.0; // half-semitone steps
        }
        toggle_row(ui, "Preserve pitch", Some("Change speed without changing pitch"), &mut lab.preserve_pitch, accent);
        ui.horizontal(|ui| {
            if ui.button("Reset speed & pitch").clicked() {
                lab.speed = 1.0;
                lab.pitch_semitones = 0.0;
            }
        });
    });
}

fn reverb_card(ui: &mut Ui, rv: &mut ReverbSettings, accent: Color32) {
    card(ui, |ui| {
        card_header(ui, "Reverb", Some("Freeverb-style room"), Some(&mut rv.enabled), accent);
        dim_if(ui, !rv.enabled);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for p in REVERB_PRESETS {
                if chip(ui, p.1, rv.preset_id == p.0, accent).clicked()
                    && let Some(s) = ReverbSettings::from_preset(p.0, rv.enabled)
                {
                    *rv = s;
                }
            }
        });
        let before = rv.clone();
        if ui.available_width() > 640.0 {
            // Two columns of sliders on wide layouts.
            ui.columns(2, |cols| {
                let (a, b) = cols.split_at_mut(1);
                reverb_sliders(&mut a[0], &mut b[0], rv);
            });
        } else {
            reverb_sliders_single(ui, rv);
        }
        if *rv != before && rv.preset_id == before.preset_id {
            rv.preset_id = "custom".into();
        }
    });
}

fn reverb_sliders(left: &mut Ui, right: &mut Ui, rv: &mut ReverbSettings) {
    percent_row(left, "Mix", &mut rv.mix);
    percent_row(left, "Room size", &mut rv.room_size);
    percent_row(left, "Damping", &mut rv.damping);
    percent_row(left, "Wet", &mut rv.wet);
    percent_row(right, "Dry", &mut rv.dry);
    percent_row(right, "Width", &mut rv.width);
    slider_row(right, "Pre-delay", &mut rv.pre_delay_ms, 0.0..=200.0, " ms", 0, false);
    slider_row(right, "Low cut", &mut rv.low_cut_hz, 20.0..=1000.0, " Hz", 0, true);
}

fn reverb_sliders_single(ui: &mut Ui, rv: &mut ReverbSettings) {
    percent_row(ui, "Mix", &mut rv.mix);
    percent_row(ui, "Room size", &mut rv.room_size);
    percent_row(ui, "Damping", &mut rv.damping);
    percent_row(ui, "Wet", &mut rv.wet);
    percent_row(ui, "Dry", &mut rv.dry);
    percent_row(ui, "Width", &mut rv.width);
    slider_row(ui, "Pre-delay", &mut rv.pre_delay_ms, 0.0..=200.0, " ms", 0, false);
    slider_row(ui, "Low cut", &mut rv.low_cut_hz, 20.0..=1000.0, " Hz", 0, true);
}

fn spatial_card(ui: &mut Ui, lab: &mut AudioLab, accent: Color32) {
    let sp = &mut lab.spatial;
    card(ui, |ui| {
        card_header(ui, "Spatial", Some("Crossfeed, virtual speakers, width"), None, accent);
        toggle_row(ui, "Headphone crossfeed", None, &mut sp.crossfeed_enabled, accent);
        ui.add_enabled_ui(sp.crossfeed_enabled, |ui| {
            slider_row(ui, "Cutoff", &mut sp.crossfeed_cutoff_hz, 300.0..=2000.0, " Hz", 0, true);
            slider_row(ui, "Feed", &mut sp.crossfeed_feed_db, 1.0..=15.0, " dB", 1, false);
        });
        toggle_row(ui, "Virtual speakers", None, &mut sp.virtual_speakers, accent);
        ui.add_enabled_ui(sp.virtual_speakers, |ui| {
            slider_row(ui, "Speaker angle", &mut sp.speaker_angle_deg, 5.0..=90.0, "°", 0, false);
        });
        toggle_row(ui, "Surround", None, &mut sp.surround, accent);
        toggle_row(ui, "Mono", Some("Sum both channels"), &mut sp.mono, accent);
        slider_row(ui, "Stereo width", &mut sp.width, 0.0..=2.0, "", 2, false);
    });
}

// ---------------------------------------------------------------------------
// Value sanitizing (documented ranges, spec §4.2/§4.6/§4.7)
// ---------------------------------------------------------------------------

fn fin(v: &mut f32, lo: f32, hi: f32, default: f32) {
    if !v.is_finite() {
        *v = default;
    }
    *v = v.clamp(lo, hi);
}

/// Clamp every Audio Lab value to its documented range.
pub fn sanitize(lab: &mut AudioLab) {
    let eq = &mut lab.eq;
    eq.nodes.truncate(MAX_BANDS);
    for n in &mut eq.nodes {
        fin(&mut n.frequency_hz, F_MIN, F_MAX, 1000.0);
        fin(&mut n.gain_db, -MAX_GAIN_DB, MAX_GAIN_DB, 0.0);
        fin(&mut n.q, 0.1, 10.0, 1.41);
    }
    fin(&mut eq.preamp_db, -12.0, 12.0, 0.0);
    fin(&mut lab.loudness_boost_db, 0.0, 12.0, 4.0);
    fin(&mut lab.loudness_compression, 0.0, 1.0, 0.3);
    fin(&mut lab.volume_curve_exponent, 1.0, 5.0, 2.0);
    fin(&mut lab.volume_curve_limit, 0.05, 1.0, 1.0);
    fin(&mut lab.normalize_preamp_db, -12.0, 12.0, 3.0);
    fin(&mut lab.speed, 0.1, 8.0, 1.0);
    fin(&mut lab.pitch_semitones, -12.0, 12.0, 0.0);
    let rv = &mut lab.reverb;
    fin(&mut rv.mix, 0.0, 1.0, 0.5);
    fin(&mut rv.room_size, 0.0, 1.0, 0.55);
    fin(&mut rv.damping, 0.0, 1.0, 0.45);
    fin(&mut rv.wet, 0.0, 1.0, 0.6);
    fin(&mut rv.dry, 0.0, 1.0, 1.0);
    fin(&mut rv.width, 0.0, 1.0, 1.0);
    fin(&mut rv.pre_delay_ms, 0.0, 200.0, 12.0);
    fin(&mut rv.low_cut_hz, 20.0, 1000.0, 180.0);
    let sp = &mut lab.spatial;
    fin(&mut sp.crossfeed_cutoff_hz, 300.0, 2000.0, 700.0);
    fin(&mut sp.crossfeed_feed_db, 1.0, 15.0, 4.5);
    fin(&mut sp.speaker_angle_deg, 5.0, 90.0, 30.0);
    fin(&mut sp.width, 0.0, 2.0, 1.0);
}

// ---------------------------------------------------------------------------
// Graph helpers
// ---------------------------------------------------------------------------

struct PlotMap {
    plot: Rect,
}

impl PlotMap {
    fn x_of(&self, f: f32) -> f32 {
        let t = (f.max(1.0) / F_MIN).log10() / (F_MAX / F_MIN).log10();
        self.plot.left() + t * self.plot.width()
    }
    fn f_of(&self, x: f32) -> f32 {
        let t = ((x - self.plot.left()) / self.plot.width()).clamp(0.0, 1.0);
        F_MIN * (F_MAX / F_MIN).powf(t)
    }
    fn y_of(&self, db: f32) -> f32 {
        self.plot.center().y - db / GRAPH_DB * self.plot.height() / 2.0
    }
    fn db_of(&self, y: f32) -> f32 {
        (self.plot.center().y - y) / (self.plot.height() / 2.0) * GRAPH_DB
    }
    /// Where a node's handle sits: (freq, gain) — pass filters sit on their
    /// own response at the cutoff.
    fn node_pos(&self, n: &EqNode) -> Pos2 {
        let db = if is_pass(n.filter_type) {
            BiquadCoeffs::design(n.filter_type, n.frequency_hz, 0.0, n.q, PLOT_SR).magnitude_db(n.frequency_hz, PLOT_SR)
        } else {
            n.gain_db
        };
        pos2(self.x_of(n.frequency_hz), self.y_of(db.clamp(-GRAPH_DB, GRAPH_DB)))
    }
}

fn nearest_node(eq: &EqSettings, map: &PlotMap, p: Pos2, radius: f32) -> Option<usize> {
    let mut best = None;
    let mut best_d = radius;
    for (i, n) in eq.nodes.iter().enumerate() {
        let d = map.node_pos(n).distance(p);
        if d < best_d {
            best_d = d;
            best = Some(i);
        }
    }
    best
}

fn round_freq(f: f32) -> f32 {
    let f = f.clamp(F_MIN, F_MAX);
    if f < 100.0 { (f * 10.0).round() / 10.0 } else { f.round() }
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

/// 0.1 dB steps with a small magnetic 0 dB detent (Android `snap`).
fn snap_gain(db: f32) -> f32 {
    let g = (db * 10.0).round() / 10.0;
    if g.abs() < 0.35 { 0.0 } else { g.clamp(-MAX_GAIN_DB, MAX_GAIN_DB) }
}

/// Geometric center of the widest log-frequency gap between existing nodes.
fn free_frequency(nodes: &[EqNode]) -> f32 {
    let mut fs: Vec<f32> = nodes.iter().map(|n| n.frequency_hz.clamp(F_MIN, F_MAX)).collect();
    fs.push(F_MIN);
    fs.push(F_MAX);
    fs.sort_by(f32::total_cmp);
    let mut best = (0.0f32, 1000.0f32);
    for w in fs.windows(2) {
        let gap = (w[1] / w[0]).ln();
        if gap > best.0 {
            best = (gap, (w[0] * w[1]).sqrt());
        }
    }
    round_freq(best.1)
}

fn type_code(t: FilterType) -> &'static str {
    autoeq::type_code(t)
}

fn fmt_hz(f: f32) -> String {
    if f >= 1000.0 {
        let k = f / 1000.0;
        if (k - k.round()).abs() < 0.05 { format!("{:.0} kHz", k) } else { format!("{k:.1} kHz") }
    } else if f < 100.0 {
        if (f - f.round()).abs() < 0.05 { format!("{f:.0} Hz") } else { format!("{f:.1} Hz") }
    } else {
        format!("{f:.0} Hz")
    }
}

fn fmt_hz_short(f: f32) -> String {
    if f >= 1000.0 { format!("{}k", (f / 1000.0).round()) } else { format!("{}", f.round()) }
}

fn curve_points(map: &PlotMap, freqs: &[f32], db: &[f32]) -> Vec<Pos2> {
    freqs
        .iter()
        .zip(db)
        .map(|(&f, &d)| pos2(map.x_of(f), map.y_of(d.clamp(-GRAPH_DB * 1.6, GRAPH_DB * 1.6))))
        .collect()
}

/// Soft fill between the curve and the 0 dB line (vertical gradient).
fn draw_curve_fill(p: &egui::Painter, map: &PlotMap, freqs: &[f32], db: &[f32], at_curve: Color32, at_zero: Color32) {
    let pts = curve_points(map, freqs, db);
    if pts.len() < 2 {
        return;
    }
    let y0 = map.y_of(0.0);
    let mut mesh = Mesh::default();
    mesh.reserve_vertices(pts.len() * 2);
    mesh.reserve_triangles((pts.len() - 1) * 2);
    for (k, q) in pts.iter().enumerate() {
        mesh.colored_vertex(*q, at_curve);
        mesh.colored_vertex(pos2(q.x, y0), at_zero);
        if k > 0 {
            let i = (k * 2) as u32;
            mesh.add_triangle(i - 2, i - 1, i);
            mesh.add_triangle(i - 1, i, i + 1);
        }
    }
    p.add(Shape::mesh(mesh));
}

fn draw_grid(p: &egui::Painter, map: &PlotMap) {
    let plot = map.plot;
    let line = Color32::from_white_alpha(15);
    let font = FontId::proportional(10.5);
    for (db, label) in [(12.0, "+12"), (6.0, "+6"), (0.0, "0"), (-6.0, "-6"), (-12.0, "-12")] {
        let y = map.y_of(db);
        let col = if db == 0.0 { Color32::from_white_alpha(40) } else { line };
        p.hline(plot.x_range(), y, Stroke::new(1.0, col));
        p.text(pos2(plot.left() - 6.0, y), Align2::RIGHT_CENTER, label, font.clone(), theme::TEXT_FAINT);
    }
    for (f, label) in [
        (31.0, "31"),
        (62.0, "62"),
        (125.0, "125"),
        (250.0, "250"),
        (500.0, "500"),
        (1000.0, "1k"),
        (2000.0, "2k"),
        (4000.0, "4k"),
        (8000.0, "8k"),
        (16000.0, "16k"),
    ] {
        let x = map.x_of(f);
        p.vline(x, plot.y_range(), Stroke::new(1.0, line));
        p.text(pos2(x, plot.bottom() + 6.0), Align2::CENTER_TOP, label, font.clone(), theme::TEXT_FAINT);
    }
}

fn draw_spectrum(p: &egui::Painter, map: &PlotMap, levels: &[f32]) {
    let n = levels.len();
    if n < 2 {
        return;
    }
    let plot = map.plot;
    let h = plot.height() * 0.55;
    let pts: Vec<Pos2> = levels
        .iter()
        .enumerate()
        .map(|(i, &db)| {
            let f = F_MIN * (F_MAX / F_MIN).powf(i as f32 / (n - 1) as f32);
            let v = ((db + 78.0) / 66.0).clamp(0.0, 1.0);
            pos2(map.x_of(f), plot.bottom() - v * h)
        })
        .collect();
    if pts.iter().all(|q| q.y >= plot.bottom() - 0.5) {
        return;
    }
    let mut mesh = Mesh::default();
    for (k, q) in pts.iter().enumerate() {
        mesh.colored_vertex(*q, Color32::from_white_alpha(16));
        mesh.colored_vertex(pos2(q.x, plot.bottom()), Color32::from_white_alpha(2));
        if k > 0 {
            let i = (k * 2) as u32;
            mesh.add_triangle(i - 2, i - 1, i);
            mesh.add_triangle(i - 1, i, i + 1);
        }
    }
    p.add(Shape::mesh(mesh));
    p.add(Shape::line(pts, Stroke::new(1.0, Color32::from_white_alpha(24))));
}

// ---------------------------------------------------------------------------
// Small widgets
// ---------------------------------------------------------------------------

fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
    ui.add_space(4.0);
}

fn dim_if(ui: &mut Ui, off: bool) {
    if off {
        ui.multiply_opacity(0.6);
    }
}

fn card_header(ui: &mut Ui, title: &str, subtitle: Option<&str>, toggle_value: Option<&mut bool>, accent: Color32) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(title).font(theme::semibold(ui.ctx(), 18.0)).color(Color32::WHITE));
            if let Some(s) = subtitle {
                ui.label(RichText::new(s).small().color(theme::TEXT_SECONDARY));
            }
        });
        if let Some(v) = toggle_value {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                toggle(ui, v, accent);
            });
        }
    });
    ui.add_space(4.0);
}

fn section_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).small().strong().color(theme::TEXT_FAINT));
}

/// iOS-style switch.
pub fn toggle(ui: &mut Ui, on: &mut bool, accent: Color32) -> Response {
    let size = vec2(42.0, 24.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(resp.id, *on);
        let off_col = if resp.hovered() { Color32::from_gray(0x4A) } else { Color32::from_gray(0x3A) };
        let bg = off_col.lerp_to_gamma(accent, t);
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(12), bg);
        let r = rect.height() / 2.0 - 3.0;
        let cx = egui::lerp((rect.left() + r + 3.0)..=(rect.right() - r - 3.0), t);
        p.circle_filled(pos2(cx, rect.center().y), r, Color32::WHITE);
    }
    resp
}

fn toggle_row(ui: &mut Ui, label: &str, sub: Option<&str>, on: &mut bool, accent: Color32) -> Response {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(label).color(Color32::WHITE));
            if let Some(s) = sub {
                ui.label(RichText::new(s).small().color(theme::TEXT_SECONDARY));
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| toggle(ui, on, accent)).inner
    })
    .inner
}

fn slider_row(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    suffix: &str,
    decimals: usize,
    log: bool,
) -> Response {
    ui.horizontal(|ui| {
        let label_w = 104.0;
        ui.allocate_ui_with_layout(vec2(label_w, 22.0), Layout::left_to_right(Align::Center), |ui| {
            ui.set_width(label_w);
            ui.label(RichText::new(label).color(theme::TEXT_SECONDARY));
        });
        ui.spacing_mut().slider_width = (ui.available_width() - 86.0).max(60.0);
        ui.add(
            egui::Slider::new(value, range)
                .logarithmic(log)
                .fixed_decimals(decimals)
                .suffix(suffix.to_owned())
                .clamping(egui::SliderClamping::Always),
        )
    })
    .inner
}

fn percent_row(ui: &mut Ui, label: &str, value: &mut f32) -> Response {
    let mut pct = *value * 100.0;
    let r = slider_row(ui, label, &mut pct, 0.0..=100.0, "%", 0, false);
    if r.changed() {
        *value = (pct / 100.0).clamp(0.0, 1.0);
    }
    r
}

/// Pill chip; selected chips are filled with the accent.
fn chip(ui: &mut Ui, text: &str, selected: bool, accent: Color32) -> Response {
    let font = FontId::proportional(13.0);
    let fg = if selected { theme::on_color(accent) } else { Color32::from_gray(0xE6) };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, fg);
    let size = galley.size() + vec2(24.0, 12.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if selected {
            accent
        } else if resp.hovered() {
            theme::PILL_SELECTED
        } else {
            theme::PILL_CHIP
        };
        ui.painter().rect_filled(rect, CornerRadius::same((rect.height() / 2.0) as u8), fill);
        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, fg);
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    resp
}

/// Chip with a leading colored dot (EQ node list).
fn dot_chip(ui: &mut Ui, text: &str, dot: Color32, selected: bool) -> Response {
    let font = FontId::proportional(12.5);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, Color32::WHITE);
    let size = galley.size() + vec2(34.0, 10.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if selected {
            theme::ROW_SELECTED
        } else if resp.hovered() {
            theme::PILL_SELECTED
        } else {
            theme::PILL_CHIP
        };
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same((rect.height() / 2.0) as u8), fill);
        if selected {
            p.rect_stroke(rect, CornerRadius::same((rect.height() / 2.0) as u8), Stroke::new(1.0, dot), egui::StrokeKind::Inside);
        }
        p.circle_filled(pos2(rect.left() + 13.0, rect.center().y), 4.5, dot);
        p.galley(pos2(rect.left() + 24.0, rect.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
    }
    resp
}

/// Segmented control; returns the clicked index.
fn segmented(ui: &mut Ui, items: &[&str], selected: usize) -> Option<usize> {
    let font = FontId::proportional(13.0);
    let galleys: Vec<_> =
        items.iter().map(|t| ui.painter().layout_no_wrap((*t).to_owned(), font.clone(), Color32::WHITE)).collect();
    let seg_w = galleys.iter().map(|g| g.size().x).fold(0.0, f32::max) + 28.0;
    let h = 30.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(seg_w * items.len() as f32 + 6.0, h), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(15), theme::PILL_TRACK);
    let anim = ui.ctx().animate_value_with_time(resp.id.with("seg"), selected as f32, 0.15);
    let sel_rect = Rect::from_min_size(pos2(rect.left() + 3.0 + anim * seg_w, rect.top() + 3.0), vec2(seg_w, h - 6.0));
    p.rect_filled(sel_rect, CornerRadius::same(12), theme::PILL_SELECTED);
    for (i, g) in galleys.into_iter().enumerate() {
        let c = pos2(rect.left() + 3.0 + seg_w * (i as f32 + 0.5), rect.center().y);
        let col = if i == selected { Color32::WHITE } else { theme::TEXT_SECONDARY };
        p.galley_with_override_text_color(c - g.size() / 2.0, g, col);
    }
    if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let i = ((pos.x - rect.left() - 3.0) / seg_w).floor().clamp(0.0, items.len() as f32 - 1.0) as usize;
        if i != selected {
            return Some(i);
        }
    }
    None
}

/// Vertical fader for the classic 10-band view (±12 dB, center detent,
/// double-click resets).
fn fader(ui: &mut Ui, gain: &mut f32, size: Vec2, accent: Color32, idx: usize) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), vec2(6.0, rect.height() - 16.0));
    let y_of = |g: f32| track.center().y - g / MAX_GAIN_DB * track.height() / 2.0;
    if (resp.dragged() || resp.clicked())
        && let Some(p) = resp.interact_pointer_pos()
    {
        let g = (track.center().y - p.y) / (track.height() / 2.0) * MAX_GAIN_DB;
        let g = snap_gain(g);
        if g != *gain {
            *gain = g;
            resp.mark_changed();
        }
    }
    if resp.double_clicked() && *gain != 0.0 {
        *gain = 0.0;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(track, CornerRadius::same(3), theme::PILL_TRACK);
        p.hline(track.x_range().expand(4.0), y_of(0.0), Stroke::new(1.0, Color32::from_white_alpha(50)));
        let y = y_of(*gain);
        let fill = Rect::from_x_y_ranges(track.x_range(), y.min(y_of(0.0))..=y.max(y_of(0.0)));
        let col = accent.lerp_to_gamma(node_color(idx), 0.25);
        p.rect_filled(fill, CornerRadius::same(3), col);
        let r = if resp.hovered() || resp.dragged() { 9.0 } else { 8.0 };
        p.circle_filled(pos2(track.center().x, y), r + 1.5, Color32::BLACK);
        p.circle_filled(pos2(track.center().x, y), r, Color32::WHITE);
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use mochy_core::eq::{settings_from_gains, total_response_db};

    #[test]
    fn cached_curve_matches_total_response() {
        let mut eq = settings_from_gains([6.0, 4.0, 0.0, -2.0, 0.0, 3.0, 0.0, -4.0, 5.0, 8.0]);
        eq.auto_preamp = true;
        eq.preamp_db = -1.0;
        let mut v = AudioLabView::default();
        v.ensure_curve(&eq);
        let c = v.curve.as_ref().unwrap();
        for (k, &f) in c.freqs.iter().enumerate().step_by(37) {
            let exact = total_response_db(&eq, f, PLOT_SR);
            assert!((c.total[k] - exact).abs() < 1e-4, "{f}: {} vs {exact}", c.total[k]);
        }
        eq.enabled = false;
        v.ensure_curve(&eq);
        assert!(v.curve.as_ref().unwrap().total.iter().all(|&d| d == 0.0));
    }

    #[test]
    fn sanitize_clamps() {
        let mut lab = AudioLab {
            speed: 20.0,
            pitch_semitones: -30.0,
            volume_curve_exponent: 0.0,
            ..Default::default()
        };
        lab.volume_curve_limit = f32::NAN;
        lab.spatial.width = 3.0;
        lab.eq.nodes = vec![EqNode { frequency_hz: 5.0, gain_db: 40.0, q: 0.0, filter_type: FilterType::Peaking, enabled: true }; 25];
        sanitize(&mut lab);
        assert_eq!(lab.speed, 8.0);
        assert_eq!(lab.pitch_semitones, -12.0);
        assert_eq!(lab.volume_curve_exponent, 1.0);
        assert_eq!(lab.volume_curve_limit, 1.0);
        assert_eq!(lab.spatial.width, 2.0);
        assert_eq!(lab.eq.nodes.len(), MAX_BANDS);
        assert_eq!(lab.eq.nodes[0].frequency_hz, 20.0);
        assert_eq!(lab.eq.nodes[0].gain_db, 12.0);
        assert_eq!(lab.eq.nodes[0].q, 0.1);
        // Defaults are already in range: sanitizing them changes nothing.
        let mut d = AudioLab::default();
        sanitize(&mut d);
        assert_eq!(d, AudioLab::default());
    }

    #[test]
    fn plot_map_round_trips() {
        let m = PlotMap { plot: Rect::from_min_size(pos2(10.0, 10.0), vec2(600.0, 300.0)) };
        for f in [20.0, 100.0, 1000.0, 15000.0] {
            assert!((m.f_of(m.x_of(f)) - f).abs() / f < 1e-3);
        }
        assert!((m.db_of(m.y_of(6.5)) - 6.5).abs() < 1e-3);
    }

    #[test]
    fn free_frequency_fills_gaps() {
        let f = free_frequency(&[]);
        assert!((600.0..=700.0).contains(&f), "{f}");
        let nodes: Vec<EqNode> = BAND_FREQS_HZ.iter().map(|&f| EqNode::peaking(f, 0.0)).collect();
        let f = free_frequency(&nodes);
        assert!(f > 16000.0 || f < 31.0 || BAND_FREQS_HZ.iter().all(|&b| (b - f).abs() > 1.0));
    }

    #[test]
    fn show_reports_changes_only() {
        let ctx = egui::Context::default();
        let mut view = AudioLabView::default();
        let mut lab = AudioLab::default();
        let mut changed = Vec::new();
        for _ in 0..3 {
            let mut out = ctx.run_ui(Default::default(), |ui| {
                changed.push(view.show(ui, &mut lab, Color32::from_rgb(200, 60, 80)));
            });
            out.textures_delta.clear();
        }
        assert!(changed.iter().all(|c| !c), "{changed:?}");
    }
}
