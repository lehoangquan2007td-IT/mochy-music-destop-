//! Audio Lab page widget (spec §4.2–§4.7, §7.5 "Audio Lab"): 10-band / parametric
//! EQ editor with live response curve, built-in presets, preamp + auto-preamp,
//! loudness, ReplayGain normalization, exponential volume curve, speed.
//!
//! CONTRACT (used by the app shell — keep the signature stable):
//! the widget edits `lab` in place and returns `true` when anything changed.
//! The caller then pushes the new values to the audio engine
//! (`Command::SetEq` / `SetLoudness` / `SetVolumeCurve` / `SetReplayGain` /
//! `SetSpeed`) and persists settings.

use egui::{Color32, Ui};
use mochy_core::settings::AudioLab;

/// Persistent UI-only state for the Audio Lab page (selected node, etc.).
#[derive(Default)]
pub struct AudioLabView {}

impl AudioLabView {
    /// Draw the full Audio Lab page. Returns `true` if `lab` was modified.
    pub fn show(&mut self, ui: &mut Ui, lab: &mut AudioLab, accent: Color32) -> bool {
        let _ = (lab, accent);
        ui.label("Audio Lab — TODO");
        false
    }
}
