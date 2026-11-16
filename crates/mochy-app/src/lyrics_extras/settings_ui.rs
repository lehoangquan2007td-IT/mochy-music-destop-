//! Settings → Lyrics: display, romanization and translation options (lyrics extras).

use egui::{RichText, Ui};
use mochy_core::settings::{AppSettings, LyricsFontWeight, LyricsTextAlign};
use mochy_lyrics::romanize::RomanizeLang;
use mochy_lyrics::translate::LANGUAGES;

use crate::ui::i18n::tr;
use crate::widgets::{TEXT, TEXT_DIM};

fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(TEXT_DIM));
}

fn subheading(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).strong().color(TEXT));
}

fn language_name(code: &str) -> String {
    LANGUAGES
        .iter()
        .find(|(c, _)| c.eq_ignore_ascii_case(code))
        .map(|(_, n)| tr(n).to_string())
        .unwrap_or_else(|| code.to_string())
}

fn language_combo(ui: &mut Ui, id: &str, value: &mut String, with_auto: bool) {
    let shown = if with_auto && (value.is_empty() || value == "auto") { tr("Detect automatically").to_string() } else { language_name(value) };
    egui::ComboBox::from_id_salt(id).selected_text(shown).width(200.0).show_ui(ui, |ui| {
        if with_auto {
            ui.selectable_value(value, "auto".to_string(), tr("Detect automatically"));
        }
        for (code, name) in LANGUAGES {
            ui.selectable_value(value, (*code).to_string(), format!("{} ({code})", tr(name)));
        }
    });
}

/// The lyrics extras part of Settings → Lyrics.
pub fn lyrics_extras_section(ui: &mut Ui, s: &mut AppSettings) {
    let x = &mut s.lyrics.extras;

    // ---- display ----
    subheading(ui, tr("Lyrics display"));
    ui.horizontal(|ui| {
        ui.label(tr("Alignment"));
        for (v, name) in [(LyricsTextAlign::Left, "Left"), (LyricsTextAlign::Center, "Center"), (LyricsTextAlign::Right, "Right")] {
            ui.selectable_value(&mut x.align, v, tr(name));
        }
    });
    ui.horizontal(|ui| {
        ui.label(tr("Font weight"));
        for (v, name) in [
            (LyricsFontWeight::Regular, "Regular"),
            (LyricsFontWeight::Medium, "Medium"),
            (LyricsFontWeight::Semibold, "Semibold"),
            (LyricsFontWeight::Bold, "Bold"),
        ] {
            ui.selectable_value(&mut x.font_weight, v, tr(name));
        }
    });
    ui.horizontal(|ui| {
        ui.label(tr("Line spacing"));
        ui.add(egui::Slider::new(&mut s.lyrics.line_spacing, 0.5..=2.0).step_by(0.05).suffix("×"));
    });
    let x = &mut s.lyrics.extras;
    ui.horizontal(|ui| {
        ui.label(tr("Dim inactive lines"));
        ui.add(egui::Slider::new(&mut x.dim_inactive, 0.0..=1.0).step_by(0.05));
    });
    ui.checkbox(&mut x.blur_inactive, tr("Blur lines away from the current one"));
    ui.checkbox(&mut x.click_to_seek, tr("Click a line to seek to it"));
    ui.horizontal(|ui| {
        ui.label(tr("Translation / romanization size"));
        ui.add(egui::Slider::new(&mut x.sub_line_scale, 0.35..=0.9).step_by(0.05).suffix("×"));
    });

    // ---- romanization ----
    subheading(ui, tr("Romanization"));
    ui.checkbox(&mut x.romanize, tr("Show romanization under lyrics"));
    hint(ui, tr("Japanese (romaji), Korean, Chinese (pinyin), Russian and other Cyrillic languages, Hindi and Punjabi."));
    if x.romanize {
        ui.checkbox(&mut x.romanize_as_main, tr("Show the romanization as the line, original underneath"));
        ui.horizontal_wrapped(|ui| {
            for lang in RomanizeLang::ALL {
                let mut on = !x.romanize_disabled.iter().any(|d| d.eq_ignore_ascii_case(lang.id()));
                if ui.checkbox(&mut on, tr(lang.name())).changed() {
                    x.romanize_disabled.retain(|d| !d.eq_ignore_ascii_case(lang.id()));
                    if !on {
                        x.romanize_disabled.push(lang.id().to_string());
                    }
                }
            }
        });
    }

    // ---- translation ----
    subheading(ui, tr("Translation"));
    ui.horizontal(|ui| {
        ui.label(tr("Translate into"));
        language_combo(ui, "lyrics_tl_target", &mut x.translate_language, false);
    });
    ui.horizontal(|ui| {
        ui.label(tr("Lyrics language"));
        language_combo(ui, "lyrics_tl_source", &mut x.translate_source, true);
    });
    ui.checkbox(&mut x.translate_auto, tr("Translate every song automatically"));
    hint(ui, tr("Uses Google Translate (no account needed). Press 文A above the lyrics to translate one song. Translations are cached."));
}
