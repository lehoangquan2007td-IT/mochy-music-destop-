//! UI modules. Ownership (parallel development):
//! - `audio_lab`, `lyrics_view`, `theme` — reusable widgets (Audio Lab / lyrics / palette).
//! - everything else in mochy-app — app shell, state, pages, player controller.

pub mod audio_lab;
pub mod i18n;
pub mod lyrics_view;
pub mod theme;
pub mod transitions_ui;
