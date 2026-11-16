//! Transitions UI (ports of `CrossmixSettingsScreen.kt`,
//! `TransitionModeCards.kt`, `TransportFadeSection.kt`, the sleep timer
//! sheet and the speed sheet's "only this song"):
//! - [`transitions_section`]: the Settings group;
//! - [`now_playing_controls`]: sleep timer + this-song speed/pitch buttons
//!   (Now Playing top bar);
//! - [`bar_indicator`]: the sleep countdown under the player bar's transport.
//!
//! Widgets read a per-frame [`TransitionSnapshot`] (set by the app) and
//! answer with [`TransitionsAction`]s.

use std::cell::RefCell;

use egui::{Align, Align2, Color32, FontId, Layout, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use mochy_audio::SongTuning;
use mochy_audio::crossmix::Style;
use mochy_core::settings::{AppSettings, CrossmixLength, CrossmixTempoMatch, TransitionMode};

use crate::actions::{Action, Ctx};
use crate::controller::{PlayerController, TransitionSnapshot};
use crate::ui::i18n::tr;
use crate::widgets::{self, TEXT, TEXT_DIM};

/// What the transition widgets ask for.
#[derive(Debug, Clone, PartialEq)]
pub enum TransitionsAction {
    SleepMinutes(u32),
    SleepEndOfSong,
    SleepAdjust(i64),
    SleepCancel,
    /// This song's own speed / pitch (`None` = back to Audio Lab).
    TuneSong(Option<SongTuning>),
}

thread_local! {
    static SNAP: RefCell<TransitionSnapshot> = RefCell::new(TransitionSnapshot::default());
}

/// Called by the app once per frame before drawing.
pub fn set_snapshot(s: TransitionSnapshot) {
    SNAP.with(|c| *c.borrow_mut() = s);
}

fn snap() -> TransitionSnapshot {
    SNAP.with(|c| c.borrow().clone())
}

/// Apply an action (called from `app.rs`).
pub fn apply(player: &mut PlayerController, settings: &mut AppSettings, a: TransitionsAction) {
    match a {
        TransitionsAction::SleepMinutes(m) => {
            settings.transitions.sleep_last_minutes = m;
            player.sleep_start_minutes(m);
        }
        TransitionsAction::SleepEndOfSong => player.sleep_end_of_song(),
        TransitionsAction::SleepAdjust(d) => player.sleep_adjust(d),
        TransitionsAction::SleepCancel => player.sleep_cancel(),
        TransitionsAction::TuneSong(t) => player.set_song_tuning(t),
    }
}

fn fmt_remaining(secs: f64) -> String {
    let s = secs.max(0.0).ceil() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// A crescent moon (the sleep timer's icon).
fn paint_moon(ui: &Ui, rect: Rect, color: Color32) {
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.32;
    let pts: Vec<egui::Pos2> = (0..=24)
        .map(|i| {
            let a = std::f32::consts::PI * (0.35 + 1.3 * i as f32 / 24.0);
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    ui.painter().add(egui::Shape::line(pts, Stroke::new(2.0, color)));
    let inner: Vec<egui::Pos2> = (0..=24)
        .map(|i| {
            let a = std::f32::consts::PI * (0.55 + 0.9 * i as f32 / 24.0);
            c + vec2(r * 0.35, -r * 0.1) + vec2(a.cos(), a.sin()) * r * 0.72
        })
        .collect();
    ui.painter().add(egui::Shape::line(inner, Stroke::new(1.6, color)));
}

fn round_button(ui: &mut Ui, w: f32, active: bool, accent: Color32, paint: impl FnOnce(&Ui, Rect, Color32)) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 34.0), Sense::click());
    let bg = if active {
        accent.gamma_multiply(0.35)
    } else if resp.hovered() {
        Color32::from_white_alpha(22)
    } else {
        Color32::from_white_alpha(10)
    };
    ui.painter().rect_filled(rect, 17.0, bg);
    paint(ui, rect, if active { widgets::accent_light(accent) } else { TEXT });
    resp
}

/// Sleep timer + this song's speed/pitch (Now Playing top bar, right side).
pub fn now_playing_controls(ui: &mut Ui, cx: &mut Ctx) {
    let s = snap();
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        // ---- speed / pitch for this song --------------------------------------
        let speed = s.tuning.map_or(s.lab_speed, |t| t.speed);
        let label = format!("{speed:.2}×");
        let tuned = s.tuning.is_some();
        let resp = round_button(ui, 64.0, tuned, cx.accent, |ui, r, c| {
            ui.painter().text(r.center(), Align2::CENTER_CENTER, &label, FontId::proportional(13.0), c);
        })
        .on_hover_text(tr("Speed & pitch for this song"));
        egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| tuning_menu(ui, cx, &s));

        // ---- sleep timer ---------------------------------------------------------
        let active = s.sleep_remaining_secs.is_some() || s.sleep_end_of_song;
        let text = match (s.sleep_remaining_secs, s.sleep_end_of_song) {
            (Some(r), _) => Some(fmt_remaining(r)),
            (None, true) => Some(tr("End of song").to_string()),
            _ => None,
        };
        let w = if text.is_some() { 110.0 } else { 40.0 };
        let resp = round_button(ui, w, active, cx.accent, |ui, r, c| {
            let icon = Rect::from_min_size(r.left_top() + vec2(8.0, 5.0), vec2(24.0, 24.0));
            paint_moon(ui, icon, c);
            if let Some(t) = &text {
                ui.painter().text(pos2(icon.right() + 4.0, r.center().y), Align2::LEFT_CENTER, t, FontId::proportional(12.5), c);
            }
        })
        .on_hover_text(tr("Sleep timer"));
        egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| sleep_menu(ui, cx, &s));
        if active {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        }
    });
}

fn sleep_menu(ui: &mut Ui, cx: &mut Ctx, s: &TransitionSnapshot) {
    ui.set_min_width(230.0);
    ui.label(RichText::new(tr("Sleep timer")).strong().color(TEXT));
    if let Some(r) = s.sleep_remaining_secs {
        ui.label(RichText::new(fmt_remaining(r)).size(26.0).color(TEXT));
        ui.horizontal(|ui| {
            if ui.button(tr("−1 min")).clicked() {
                cx.act(Action::Transitions(TransitionsAction::SleepAdjust(-60)));
            }
            if ui.button(tr("+1 min")).clicked() {
                cx.act(Action::Transitions(TransitionsAction::SleepAdjust(60)));
            }
        });
    } else if s.sleep_end_of_song {
        ui.label(RichText::new(tr("Pauses when this song ends")).color(TEXT_DIM));
    }
    ui.separator();
    ui.label(RichText::new(tr("Quick set")).size(12.0).color(TEXT_DIM));
    ui.horizontal_wrapped(|ui| {
        for m in [5u32, 10, 15, 30, 45, 60, 90, 120] {
            let label = if m >= 60 && m % 60 == 0 { format!("{} {}", m / 60, tr("h")) } else { format!("{m} {}", tr("min")) };
            if widgets::pill(ui, &label, false).clicked() {
                cx.act(Action::Transitions(TransitionsAction::SleepMinutes(m)));
                ui.close();
            }
        }
    });
    if ui.selectable_label(s.sleep_end_of_song, tr("End after this song")).clicked() {
        cx.act(Action::Transitions(TransitionsAction::SleepEndOfSong));
        ui.close();
    }
    if (s.sleep_remaining_secs.is_some() || s.sleep_end_of_song) && ui.button(tr("Cancel timer")).clicked() {
        cx.act(Action::Transitions(TransitionsAction::SleepCancel));
        ui.close();
    }
}

fn tuning_menu(ui: &mut Ui, cx: &mut Ctx, s: &TransitionSnapshot) {
    ui.set_min_width(260.0);
    ui.label(RichText::new(tr("Speed & pitch for this song")).strong().color(TEXT));
    if !s.has_song {
        ui.label(RichText::new(tr("Nothing playing")).color(TEXT_DIM));
        return;
    }
    let mut t = s.tuning.unwrap_or(SongTuning { speed: s.lab_speed, semitones: s.lab_semitones });
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(tr("Speed"));
        changed |= ui.add(egui::Slider::new(&mut t.speed, 0.25..=3.0).step_by(0.05).suffix("×")).changed();
    });
    ui.horizontal(|ui| {
        ui.label(tr("Pitch"));
        changed |= ui.add(egui::Slider::new(&mut t.semitones, -12.0..=12.0).step_by(0.5).suffix(" st")).changed();
    });
    ui.horizontal_wrapped(|ui| {
        for v in [0.75f32, 1.0, 1.25, 1.5] {
            if widgets::pill(ui, &format!("{v}×"), (t.speed - v).abs() < 0.01).clicked() {
                t.speed = v;
                changed = true;
            }
        }
    });
    if changed {
        // remembered for this song as soon as it's moved (Android's "only this song")
        cx.act(Action::Transitions(TransitionsAction::TuneSong(Some(t))));
    }
    ui.label(RichText::new(tr("Saved for this song and applied whenever it plays.")).size(11.5).color(TEXT_DIM));
    if s.tuning.is_some() && ui.button(tr("Use Audio Lab settings")).clicked() {
        cx.act(Action::Transitions(TransitionsAction::TuneSong(None)));
    }
}

/// The sleep countdown, painted under the player bar's transport (left of
/// `center`'s bottom edge); clicking it opens Now Playing.
pub fn bar_indicator(ui: &mut Ui, cx: &mut Ctx, center: Rect) {
    let s = snap();
    let text = match (s.sleep_remaining_secs, s.sleep_end_of_song) {
        (Some(r), _) => fmt_remaining(r),
        (None, true) => tr("End of song").to_string(),
        _ => return,
    };
    let color = widgets::accent_light(cx.accent);
    let font = FontId::proportional(10.5);
    let label = format!("{} {}", tr("Sleep"), text);
    let g = ui.painter().layout_no_wrap(label, font, color);
    let r = Rect::from_min_size(pos2(center.left(), center.bottom() + 2.0 - g.size().y), g.size());
    let resp = ui.interact(r, ui.scope_id().with("sleep_indicator"), Sense::click()).on_hover_text(tr("Sleep timer"));
    ui.painter().galley(r.min, g, color);
    if resp.clicked() {
        cx.act(Action::SetNowPlaying(true));
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(TEXT_DIM));
}

fn mode_label(m: TransitionMode) -> &'static str {
    tr(match m {
        TransitionMode::Off => "Off (I hate having fun)",
        TransitionMode::AutoCrossfade => "Auto Crossfade",
        TransitionMode::ManualCrossfade => "Manual Crossfade",
        TransitionMode::Crossmix => "CrossMix",
    })
}

fn mode_desc(m: TransitionMode) -> String {
    match m {
        TransitionMode::Off => tr("Each song ends before the next one starts (gapless)").to_string(),
        TransitionMode::AutoCrossfade => tr("A smooth 5-second blend from every song into the next").to_string(),
        TransitionMode::ManualCrossfade => tr("Pick how long each song blends into the next").to_string(),
        TransitionMode::Crossmix => tr(
            "DJ-style: reads each song's tempo, beats, key, energy and where its voice is, then picks the transition that fits and locks the two on the beat",
        )
        .to_string(),
    }
}

/// Settings → Transitions (one call from `pages/settings.rs`).
pub fn transitions_section(ui: &mut Ui, s: &mut AppSettings) {
    ui.add_space(10.0);
    ui.label(RichText::new(tr("Transitions")).size(18.0).strong().color(TEXT));
    ui.add_space(6.0);
    egui::Frame::new().fill(widgets::SURFACE).corner_radius(14).inner_margin(16).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 10.0;

        // ---- music transitions -------------------------------------------------
        ui.label(RichText::new(tr("Music transitions")).strong().color(TEXT));
        for m in [TransitionMode::Off, TransitionMode::AutoCrossfade, TransitionMode::ManualCrossfade, TransitionMode::Crossmix] {
            let sel = s.playback.transition_mode == m;
            ui.horizontal(|ui| {
                if ui.radio(sel, mode_label(m)).clicked() {
                    s.playback.transition_mode = m;
                }
            });
            if sel {
                ui.indent(("tmode", m as u8), |ui| hint(ui, &mode_desc(m)));
            }
        }
        match s.playback.transition_mode {
            TransitionMode::ManualCrossfade => {
                ui.horizontal(|ui| {
                    ui.label(tr("Length"));
                    ui.add(egui::Slider::new(&mut s.playback.crossfade_duration_secs, 1.0..=30.0).step_by(0.5).suffix(" s"));
                });
            }
            TransitionMode::Crossmix => crossmix_options(ui, s),
            _ => {}
        }
        if s.playback.transition_mode != TransitionMode::Off {
            ui.checkbox(&mut s.transitions.gapless_albums, tr("Keep gapless albums intact"));
            hint(ui, tr("Album tracks that run straight into each other play through unmixed"));
        }

        ui.separator();
        // ---- transport fades -----------------------------------------------------
        ui.label(RichText::new(tr("Pause, play, forward and backward transitions")).strong().color(TEXT));
        ui.checkbox(&mut s.transitions.transport_fade, tr("Enable fade"));
        hint(ui, tr("Eases the music out when you pause, skip or pick another song, and back in when you play"));
        ui.add_enabled_ui(s.transitions.transport_fade, |ui| {
            ui.horizontal(|ui| {
                ui.label(tr("Length"));
                ui.add(egui::Slider::new(&mut s.transitions.transport_fade_secs, 0.5..=10.0).step_by(0.1).suffix(" s"));
            });
        });

        ui.separator();
        // ---- skip silence --------------------------------------------------------
        ui.checkbox(&mut s.playback.skip_silence, tr("Skip silence"));
        hint(ui, tr("Silences longer than 2 seconds are cut down to a moment"));
        ui.add_enabled_ui(s.playback.skip_silence, |ui| {
            ui.checkbox(&mut s.transitions.skip_silence_instant, tr("Instantly skip silence"));
            hint(ui, tr("Jump ahead during silent moments instead of playing through them"));
        });

        ui.separator();
        // ---- sleep timer -----------------------------------------------------------
        ui.horizontal(|ui| {
            ui.label(tr("Sleep timer fade-out"));
            ui.add(egui::Slider::new(&mut s.transitions.sleep_fade_secs, 0.0..=60.0).step_by(1.0).suffix(" s"));
        });
        hint(ui, tr("Start the sleep timer from the moon button in Now Playing."));
    });
}

fn crossmix_options(ui: &mut Ui, s: &mut AppSettings) {
    let t = &mut s.transitions;
    ui.horizontal(|ui| {
        ui.label(tr("Length"));
        for (v, l) in [(CrossmixLength::Short, "Short"), (CrossmixLength::Medium, "Medium"), (CrossmixLength::Long, "Long")] {
            ui.selectable_value(&mut t.crossmix_length, v, tr(l));
        }
    });
    hint(ui, tr("How long the two songs play together. Long blends need two songs that lock on the beat."));
    ui.horizontal(|ui| {
        ui.label(tr("Tempo match"));
        for (v, l) in [(CrossmixTempoMatch::Off, "Off"), (CrossmixTempoMatch::Gentle, "Gentle"), (CrossmixTempoMatch::Full, "Full")] {
            ui.selectable_value(&mut t.crossmix_tempo_match, v, tr(l));
        }
    });
    hint(ui, tr("How far the slower song may be sped up so its beat lands on the other's. Off only locks songs already at one tempo."));
    ui.horizontal(|ui| {
        ui.label(tr("Transition"));
        let current = t.crossmix_style.as_deref().and_then(Style::from_id);
        let text = current.map_or_else(|| tr("Let Mochy choose").to_string(), |st| st.label().to_string());
        egui::ComboBox::from_id_salt("crossmix_style").selected_text(text).show_ui(ui, |ui| {
            if ui.selectable_label(current.is_none(), tr("Let Mochy choose")).clicked() {
                t.crossmix_style = None;
            }
            for st in Style::ALL {
                if ui.selectable_label(current == Some(st), st.label()).clicked() {
                    t.crossmix_style = Some(st.id().to_string());
                }
            }
        });
    });
    hint(ui, tr("Let Mochy pick the one that fits each pair, or hear the same one every time"));
    ui.checkbox(&mut t.crossmix_stunts, tr("Stunts"));
    hint(ui, tr("Drops and lifts when the energy jumps between songs"));
    ui.checkbox(&mut t.crossmix_always_mix, tr("Always mix"));
    hint(ui, tr("Both songs play together on every change, even when their beats don't line up. Off lets Crossmix drop the next song in at the very end, or just fade."));
}
