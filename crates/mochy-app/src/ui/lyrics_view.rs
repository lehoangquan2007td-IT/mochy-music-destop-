//! Synced lyrics view (spec §7.7): auto-scrolling, active line highlight with
//! 300 ms lead-in, word-level karaoke fill when word timings exist,
//! click-a-line-to-seek, per-song offset.
//!
//! CONTRACT (used by the app shell — keep the signature stable).

use egui::{Color32, Ui};
use mochy_lyrics::Lyrics;

/// What the app currently knows about the playing track's lyrics.
#[derive(Debug, Clone, Default)]
pub enum LyricsState {
    #[default]
    Idle,
    Loading,
    NotFound,
    Loaded(Lyrics),
    Error(String),
}

/// Persistent UI state (scroll animation etc.). One instance per app.
#[derive(Default)]
pub struct LyricsView {}

impl LyricsView {
    /// Draw lyrics filling `ui`. `pos_secs` = current playback position,
    /// `offset_ms` = per-song offset (positive shows lines earlier).
    /// Returns `Some(secs)` when the user clicked a line to seek there.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        state: &LyricsState,
        pos_secs: f64,
        offset_ms: i32,
        accent: Color32,
    ) -> Option<f64> {
        let _ = (state, pos_secs, offset_ms, accent);
        ui.label("Lyrics — TODO");
        None
    }
}
