//! Audio Lab → Quality (port of Android `AudioQualityScreen` + the "stats
//! for nerds" of `StreamInfoSheet`): the streaming quality, and what the
//! playing song really is on its whole road to the speaker.

use egui::{Color32, CornerRadius, RichText, Sense, Stroke, Ui, pos2, vec2};
use mochy_core::settings::AudioQuality;

use super::{chip, section_label, theme};
use crate::stream_facts::{self, StatCard};
use crate::ui::i18n::tr;

pub fn quality_name(q: AudioQuality) -> &'static str {
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

/// How fine the hub's waveform sketch is for a quality.
pub fn resolution(q: AudioQuality) -> f32 {
    match q {
        AudioQuality::Low => 0.0,
        AudioQuality::Medium => 0.35,
        _ => 0.65,
    }
}

pub fn page(ui: &mut Ui, accent: Color32) {
    let playback = stream_facts::playback_now();
    super::card(ui, |ui| {
        super::card_header(ui, tr("Streaming quality"), Some(tr("What YouTube Music hands over")), None, accent);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for q in [AudioQuality::Auto, AudioQuality::Low, AudioQuality::Medium, AudioQuality::High] {
                if chip(ui, quality_name(q), playback.quality == q, accent).clicked() && playback.quality != q {
                    stream_facts::request_quality(q);
                }
            }
        });
        let note = match playback.quality {
            AudioQuality::Auto => tr("The best stream the client offers — usually Opus ~160 kbps."),
            AudioQuality::Low => tr("Saves data. HE-AAC: the desktop decoder plays only its core, so it sounds muffled."),
            AudioQuality::Medium => tr("AAC ~128 kbps."),
            _ => tr("The highest bitrate available for the song."),
        };
        ui.label(RichText::new(note).small().color(theme::TEXT_SECONDARY));
        ui.label(RichText::new(tr("Applies from the next song.")).small().color(theme::TEXT_FAINT));
    });

    let snap = stream_facts::snapshot_now();
    section_label(ui, tr("Format & stats for nerds"));
    let cards = stream_facts::cards(&snap);
    if cards.is_empty() {
        super::card(ui, |ui| {
            ui.label(RichText::new(tr("Play a song to see its format, decoder, DSP chain and output.")).color(theme::TEXT_SECONDARY));
        });
    } else {
        stat_cards(ui, &cards);
    }

    super::card(ui, |ui| {
        super::card_header(ui, tr("Exclusive output"), Some(tr("Bit-perfect playback straight to a DAC")), None, accent);
        ui.label(
            RichText::new(tr(
                "The desktop player shares the device with the system mixer (no exclusive mode yet). For the cleanest path, set the device's format in the system sound settings to the song's sample rate.",
            ))
            .small()
            .color(theme::TEXT_SECONDARY),
        );
    });
}

/// The cards, linked by a short connector, two per row when there is room.
pub fn stat_cards(ui: &mut Ui, cards: &[StatCard]) {
    let wide = ui.available_width() >= 700.0;
    if wide {
        for pair in cards.chunks(2) {
            ui.columns(2, |cols| {
                for (k, c) in pair.iter().enumerate() {
                    stat_card(&mut cols[k], c);
                }
            });
        }
    } else {
        for (i, c) in cards.iter().enumerate() {
            if i > 0 {
                connector(ui);
            }
            stat_card(ui, c);
        }
    }
}

fn connector(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
    ui.painter().line_segment([pos2(r.left() + 30.0, r.top()), pos2(r.left() + 30.0, r.bottom())], Stroke::new(2.0, theme::STAGE_LINE));
}

pub fn stat_card(ui: &mut Ui, card: &StatCard) {
    egui::Frame::new().fill(theme::CARD).corner_radius(CornerRadius::same(18)).inner_margin(egui::Margin::same(16)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(&card.title).font(theme::bold(ui.ctx(), 22.0)).color(Color32::WHITE));
        ui.add_space(4.0);
        egui::Grid::new(("stat", &card.title)).num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
            for (label, value) in &card.lines {
                ui.label(RichText::new(label).color(theme::TEXT_SECONDARY));
                ui.label(RichText::new(value).color(Color32::WHITE));
                ui.end_row();
            }
        });
    });
    ui.add_space(4.0);
}
