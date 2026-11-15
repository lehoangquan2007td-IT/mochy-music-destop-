//! Theme & color (spec §7.4): dark-only Mochy style, dynamic accent from artwork.
//!
//! CONTRACT (used by the app shell — keep signatures stable).

use egui::{Color32, Context};

/// Default accent `0xFF8E8E8E`.
pub const DEFAULT_ACCENT: Color32 = Color32::from_rgb(0x8E, 0x8E, 0x8E);

/// Install fonts + dark visuals. Call once at startup and whenever accent /
/// pure-black changes.
pub fn apply(ctx: &Context, accent: Color32, pure_black: bool) {
    let _ = (accent, pure_black);
    ctx.set_visuals(egui::Visuals::dark());
}

/// Extract the dominant "theme" accent from RGBA8 artwork pixels
/// (Palette maximumColorCount=8 + Score equivalent).
pub fn extract_accent(rgba: &[u8], width: u32, height: u32) -> Color32 {
    let _ = (rgba, width, height);
    DEFAULT_ACCENT
}

/// Two gradient colors for the frosted player background
/// (fallback `#595959` / `#0D0D0D`).
pub fn extract_gradient(rgba: &[u8], width: u32, height: u32) -> (Color32, Color32) {
    let _ = (rgba, width, height);
    (Color32::from_rgb(0x59, 0x59, 0x59), Color32::from_rgb(0x0D, 0x0D, 0x0D))
}
