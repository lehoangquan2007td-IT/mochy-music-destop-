//! Settings: Playback, Appearance, Lyrics, Storage, About.

use std::path::PathBuf;

use egui::{Color32, RichText, Ui};
use mochy_core::settings::{AppSettings, AudioQuality, Language};
use mochy_lyrics::ProviderId;

use crate::ui::i18n::{self, tr};
use crate::widgets::{self, TEXT, TEXT_DIM};

pub enum SettingsEvent {
    RefreshDevices,
    SetOutputDevice(Option<String>),
    ClearArtCache,
    OpenDir(PathBuf),
}

pub struct SettingsCtx<'a> {
    pub devices: Option<&'a Vec<String>>,
    pub log_file: Option<&'a PathBuf>,
    pub ytm_status: Option<&'a Result<(), String>>,
    pub db_error: Option<&'a String>,
}

fn group(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).size(18.0).strong().color(TEXT));
    ui.add_space(6.0);
    egui::Frame::new().fill(widgets::SURFACE).corner_radius(14).inner_margin(16).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 10.0;
        body(ui);
    });
}

fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(TEXT_DIM));
}

fn quality_label(q: AudioQuality) -> &'static str {
    tr(match q {
        AudioQuality::Auto => "Auto",
        AudioQuality::Low => "Low",
        AudioQuality::Medium => "Medium",
        AudioQuality::High => "High",
        AudioQuality::Lossless => "Lossless",
        AudioQuality::HiResLossless => "Hi-Res Lossless",
        AudioQuality::DolbyAtmos => "Dolby Atmos",
    })
}

/// Language picker: options stay in their own language (standard practice).
fn language_picker(ui: &mut Ui, s: &mut AppSettings) {
    ui.horizontal(|ui| {
        ui.label(tr("Language"));
        let current = match s.appearance.language {
            Language::En => "English",
            Language::Vi => "Tiếng Việt",
        };
        egui::ComboBox::from_id_salt("ui_language").selected_text(current).show_ui(ui, |ui| {
            ui.selectable_value(&mut s.appearance.language, Language::En, "English");
            ui.selectable_value(&mut s.appearance.language, Language::Vi, "Tiếng Việt");
        });
    });
}

pub fn show(ui: &mut Ui, s: &mut AppSettings, sc: SettingsCtx) -> Vec<SettingsEvent> {
    let mut ev = Vec::new();
    widgets::page_title(ui, tr("Settings"));

    // ---- Playback ----------------------------------------------------------
    group(ui, tr("Playback"), |ui| {
        ui.horizontal(|ui| {
            ui.label(tr("Audio quality"));
            egui::ComboBox::from_id_salt("quality").selected_text(quality_label(s.playback.audio_quality)).show_ui(ui, |ui| {
                for q in [AudioQuality::Auto, AudioQuality::Low, AudioQuality::Medium, AudioQuality::High] {
                    ui.selectable_value(&mut s.playback.audio_quality, q, quality_label(q));
                }
            });
        });
        ui.checkbox(&mut s.playback.audio_normalization, tr("Normalize volume (YouTube loudness)"));
        ui.checkbox(&mut s.audio_lab.normalize_replay_gain, tr("Use ReplayGain tags for local files"));
        ui.checkbox(&mut s.playback.auto_skip_next_on_error, tr("Skip to the next song when a stream fails"));
        ui.checkbox(&mut s.playback.skip_silence, tr("Skip silence"));
        hint(ui, tr("Skip silence is saved but not yet applied by the desktop audio engine."));
        ui.checkbox(&mut s.misc.persistent_queue, tr("Restore the queue on startup"));
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(tr("Output device"));
            let current = s.playback.output_device.clone();
            let label = current.clone().unwrap_or_else(|| tr("System default").into());
            egui::ComboBox::from_id_salt("outdev").selected_text(label).width(280.0).show_ui(ui, |ui| {
                if ui.selectable_label(current.is_none(), tr("System default")).clicked() && current.is_some() {
                    s.playback.output_device = None;
                    ev.push(SettingsEvent::SetOutputDevice(None));
                }
                match sc.devices {
                    None => {
                        ui.label(tr("Loading devices…"));
                    }
                    Some(list) if list.is_empty() => {
                        ui.label(RichText::new(tr("No devices reported by the engine")).color(TEXT_DIM));
                    }
                    Some(list) => {
                        for d in list {
                            let sel = current.as_deref() == Some(d.as_str());
                            if ui.selectable_label(sel, d).clicked() && !sel {
                                s.playback.output_device = Some(d.clone());
                                ev.push(SettingsEvent::SetOutputDevice(Some(d.clone())));
                            }
                        }
                    }
                }
            });
            if ui.button(tr("Refresh")).clicked() {
                ev.push(SettingsEvent::RefreshDevices);
            }
        });
    });

    // ---- Appearance ----------------------------------------------------------
    group(ui, tr("Appearance"), |ui| {
        language_picker(ui, s);
        ui.checkbox(&mut s.appearance.dynamic_theme, tr("Dynamic theme (accent from artwork)"));
        ui.horizontal(|ui| {
            let mut custom = s.appearance.selected_theme_color.is_some();
            ui.add_enabled_ui(!s.appearance.dynamic_theme, |ui| {
                if ui.checkbox(&mut custom, tr("Custom accent color")).changed() {
                    s.appearance.selected_theme_color = if custom { Some(0x8E8E8E) } else { None };
                }
                if let Some(c) = s.appearance.selected_theme_color {
                    let mut rgb = [(c >> 16) as u8, (c >> 8) as u8, c as u8];
                    if ui.color_edit_button_srgb(&mut rgb).changed() {
                        s.appearance.selected_theme_color = Some(((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | rgb[2] as u32);
                    }
                }
            });
        });
        if s.appearance.dynamic_theme {
            hint(ui, tr("Turn off the dynamic theme to pick a fixed accent color."));
        }
        ui.checkbox(&mut s.appearance.pure_black, tr("Pure black background"));
        ui.horizontal(|ui| {
            ui.label(tr("Cover corner radius"));
            ui.add(egui::Slider::new(&mut s.appearance.thumbnail_corner_radius, 0.0..=32.0).suffix(" px"));
        });
    });

    // ---- Lyrics --------------------------------------------------------------
    group(ui, tr("Lyrics"), |ui| {
        ui.checkbox(&mut s.lyrics.show, tr("Show lyrics"));
        ui.horizontal(|ui| {
            ui.label(tr("Text size"));
            ui.add(egui::Slider::new(&mut s.lyrics.text_size, 0.6..=2.0).step_by(0.05).suffix("×"));
        });
        ui.horizontal(|ui| {
            ui.label(tr("Lyric sync offset (all songs)"));
            if ui.small_button("−0.5 s").clicked() {
                s.lyrics.global_offset_ms = (s.lyrics.global_offset_ms - 500).max(-10_000);
            }
            ui.label(RichText::new(format!("{:+} ms", s.lyrics.global_offset_ms)).color(TEXT));
            if ui.small_button("+0.5 s").clicked() {
                s.lyrics.global_offset_ms = (s.lyrics.global_offset_ms + 500).min(10_000);
            }
            if s.lyrics.global_offset_ms != 0 && ui.small_button(tr("Reset")).clicked() {
                s.lyrics.global_offset_ms = 0;
            }
        });
        hint(ui, tr("Lyrics run ahead of the vocals? Press − to delay them (Bluetooth / TV setups)."));
        ui.label(RichText::new(tr("Providers (top = highest priority)")).color(TEXT));
        provider_editor(ui, s);
    });

    crate::integrations::settings_section(ui, s);

    // ---- Storage ---------------------------------------------------------------
    group(ui, tr("Storage"), |ui| {
        let rows = [
            (tr("Data"), mochy_storage::data_dir()),
            (tr("Database"), mochy_storage::db_path()),
            (tr("Cache"), mochy_storage::cache_dir()),
            (tr("Artwork cache"), mochy_storage::art_cache_dir()),
        ];
        egui::Grid::new("paths").num_columns(3).spacing([12.0, 8.0]).show(ui, |ui| {
            for (name, p) in rows {
                ui.label(RichText::new(name).color(TEXT_DIM));
                ui.label(RichText::new(p.display().to_string()).monospace().size(12.0));
                let dir = if p.is_dir() { p.clone() } else { p.parent().map(|x| x.to_path_buf()).unwrap_or(p.clone()) };
                if ui.small_button(tr("Open")).clicked() {
                    ev.push(SettingsEvent::OpenDir(dir));
                }
                ui.end_row();
            }
            if let Some(l) = sc.log_file {
                ui.label(RichText::new(tr("Log file")).color(TEXT_DIM));
                ui.label(RichText::new(l.display().to_string()).monospace().size(12.0));
                if ui.small_button(tr("Open")).clicked()
                    && let Some(d) = l.parent()
                {
                    ev.push(SettingsEvent::OpenDir(d.to_path_buf()));
                }
                ui.end_row();
            }
        });
        if ui.button(tr("Clear artwork cache")).clicked() {
            ev.push(SettingsEvent::ClearArtCache);
        }
        if let Some(e) = sc.db_error {
            ui.label(RichText::new(format!("Database error (using a temporary in-memory DB): {e}")).color(Color32::from_rgb(0xFF, 0x8A, 0x8A)));
        }
        match sc.ytm_status {
            Some(Ok(())) => hint(ui, tr("YouTube Music: connected")),
            Some(Err(e)) => {
                ui.label(RichText::new(format!("YouTube Music: {e}")).color(Color32::from_rgb(0xFF, 0x8A, 0x8A)));
            }
            None => hint(ui, tr("YouTube Music: connecting…")),
        }
    });

    // ---- About -------------------------------------------------------------------
    group(ui, tr("About"), |ui| {
        ui.label(RichText::new(format!("Mochy Music desktop {}", env!("CARGO_PKG_VERSION"))).strong());
        ui.label(tr("A desktop port of the Mochy Music Android app, written in Rust (egui)."));
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("Based on the original"));
            ui.hyperlink_to("Mochy Music", "https://github.com/realidkroo/Mochy-Music");
            ui.label(tr("project (a Metrolist fork) — all credit for the design and features goes to its authors."));
        });
        ui.label(
            "This program is free software: you can redistribute it and/or modify it under the terms of the GNU General \
             Public License as published by the Free Software Foundation, either version 3 of the License, or (at your \
             option) any later version. It is distributed WITHOUT ANY WARRANTY; see the GNU GPL v3 for details.",
        );
        ui.hyperlink_to("GNU GPL v3", "https://www.gnu.org/licenses/gpl-3.0.html");
        hint(ui, tr("Not affiliated with Google or YouTube."));
    });
    ui.add_space(30.0);
    ev
}

/// Ordered provider list with enable checkboxes and up/down buttons.
/// Stored as the ordered list of *enabled* provider ids (empty = all, default order).
#[allow(clippy::needless_range_loop)]
fn provider_editor(ui: &mut Ui, s: &mut AppSettings) {
    let implemented: Vec<ProviderId> = ProviderId::all().iter().copied().filter(|p| p.implemented()).collect();
    let enabled: Vec<ProviderId> = crate::lyrics::provider_order(&s.lyrics);
    let mut rows: Vec<(ProviderId, bool)> = enabled.iter().map(|p| (*p, true)).collect();
    for p in &implemented {
        if !enabled.contains(p) {
            rows.push((*p, false));
        }
    }
    let mut changed = false;
    let n = rows.len();
    let mut swap: Option<(usize, usize)> = None;
    for i in 0..n {
        ui.horizontal(|ui| {
            let (p, mut on) = rows[i];
            if ui.checkbox(&mut on, p.display_name()).changed() {
                rows[i].1 = on;
                changed = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(i + 1 < n, egui::Button::new("▼").small()).clicked() {
                    swap = Some((i, i + 1));
                }
                if ui.add_enabled(i > 0, egui::Button::new("▲").small()).clicked() {
                    swap = Some((i, i - 1));
                }
            });
        });
    }
    if let Some((a, b)) = swap {
        rows.swap(a, b);
        changed = true;
    }
    let unimpl: Vec<&str> = ProviderId::all().iter().filter(|p| !p.implemented()).map(|p| p.display_name()).collect();
    hint(ui, &format!("Not available on desktop yet: {}. With none selected, all providers are used.", unimpl.join(", ")));
    if changed {
        s.lyrics.provider_order = rows.iter().filter(|(_, on)| *on).map(|(p, _)| p.id().to_string()).collect();
    }
}
