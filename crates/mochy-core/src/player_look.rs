//! Now Playing styles and appearance extras (port of the Android player
//! selector `PlayerStyle`, `PlayerBackgroundStyle`, `PlayerCoverStyle`, the
//! Now Playing settings and the customisation screen's motion switches).
//! Stored as `settings.player`; every field has a default so old settings load.

use serde::{Deserialize, Serialize};

/// Whole-player presets that fit a desktop window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PlayerStyle {
    /// Big cover, controls under it, lyrics/queue beside it.
    #[default]
    Flagship,
    /// The flagship around a record player whose arm is the seek bar.
    Turntable,
    /// A pocket music player with a click wheel (original drawing).
    ClickWheel,
    /// A big quiet clock for the nightstand with the song beside it.
    Bedside,
}

impl PlayerStyle {
    pub const ALL: [PlayerStyle; 4] = [PlayerStyle::Flagship, PlayerStyle::Turntable, PlayerStyle::ClickWheel, PlayerStyle::Bedside];
}

/// What lies behind the player (Android `PlayerBackgroundStyle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PlayerBackground {
    /// Gradient from the cover's colours.
    #[default]
    CoverGradient,
    /// The cover blown up and frosted behind everything.
    BlurredCover,
    /// The cover's colours drifting slowly.
    Ambient,
    /// Black fading into the cover's colour.
    DarkGradient,
    /// One flat colour from the cover.
    Solid,
}

impl PlayerBackground {
    pub const ALL: [PlayerBackground; 5] = [
        PlayerBackground::CoverGradient,
        PlayerBackground::BlurredCover,
        PlayerBackground::Ambient,
        PlayerBackground::DarkGradient,
        PlayerBackground::Solid,
    ];
}

/// How the cover is drawn on the flagship (Android `PlayerCoverStyle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CoverShape {
    /// Rounded square.
    #[default]
    Rounded,
    /// Square corners, no shadow.
    Square,
    /// The whole cover printed on a record.
    Disc,
    /// The cover as a record's centre label.
    DiscLabel,
}

impl CoverShape {
    pub const ALL: [CoverShape; 4] = [CoverShape::Rounded, CoverShape::Square, CoverShape::Disc, CoverShape::DiscLabel];
}

/// Bedside clock faces (Android `BedsideClockFace`, minus the cover face).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BedsideFace {
    #[default]
    Big,
    Stacked,
    Analog,
}

impl BedsideFace {
    pub const ALL: [BedsideFace; 3] = [BedsideFace::Big, BedsideFace::Stacked, BedsideFace::Analog];
}

/// Click wheel player colours (Android `IpodTheme`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WheelTheme {
    #[default]
    Silver,
    Black,
    TrueDark,
    Red,
}

impl WheelTheme {
    pub const ALL: [WheelTheme; 4] = [WheelTheme::Silver, WheelTheme::Black, WheelTheme::TrueDark, WheelTheme::Red];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerLook {
    pub style: PlayerStyle,
    pub background: PlayerBackground,
    pub cover: CoverShape,
    /// Cover size relative to the room it has, 0.6–1.0.
    pub artwork_scale: f32,
    /// Corner radius of the big cover, px.
    pub artwork_radius: f32,
    /// Records (disc covers, turntable) turn while the music plays.
    pub spin_records: bool,
    /// Audio format badge in the top bar (Android `ShowFormatBadgeKey`).
    pub show_format_badge: bool,
    /// Sample rate inside the badge (Android `ShowFormatSampleRateKey`).
    pub show_sample_rate: bool,
    pub show_volume: bool,
    /// Lyrics / queue / related column beside the player.
    pub show_side_panel: bool,
    pub bedside_face: BedsideFace,
    pub bedside_24h: bool,
    /// Dim the bedside clock after this many idle seconds (0 = never).
    pub bedside_dim_secs: u32,
    /// Warm, red-shifted ink once the bedside clock has dimmed.
    pub bedside_warm: bool,
    pub wheel_theme: WheelTheme,
    /// Master motion switch (Android "animations"); off = nothing animates.
    pub animations: bool,
}

impl Default for PlayerLook {
    fn default() -> Self {
        Self {
            style: PlayerStyle::Flagship,
            background: PlayerBackground::CoverGradient,
            cover: CoverShape::Rounded,
            artwork_scale: 1.0,
            artwork_radius: 22.0,
            spin_records: true,
            show_format_badge: true,
            show_sample_rate: true,
            show_volume: true,
            show_side_panel: true,
            bedside_face: BedsideFace::Big,
            bedside_24h: true,
            bedside_dim_secs: 30,
            bedside_warm: true,
            wheel_theme: WheelTheme::Silver,
            animations: true,
        }
    }
}

impl PlayerLook {
    /// Clamp numeric fields into their documented ranges.
    pub fn sanitize(&mut self) {
        if !self.artwork_scale.is_finite() {
            self.artwork_scale = 1.0;
        }
        self.artwork_scale = self.artwork_scale.clamp(0.6, 1.0);
        if !self.artwork_radius.is_finite() {
            self.artwork_radius = 22.0;
        }
        self.artwork_radius = self.artwork_radius.clamp(0.0, 64.0);
        self.bedside_dim_secs = self.bedside_dim_secs.min(3600);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::AppSettings;

    #[test]
    fn old_settings_get_the_default_look() {
        let s: AppSettings = serde_json::from_str(r#"{"appearance":{"pure_black":true}}"#).unwrap();
        assert_eq!(s.player, PlayerLook::default());
        assert!(s.appearance.pure_black);
        let partial: PlayerLook = serde_json::from_str(r#"{"style":"Bedside","artwork_scale":0.2}"#).unwrap();
        assert_eq!(partial.style, PlayerStyle::Bedside);
        assert!(partial.animations);
        let mut p = partial;
        p.sanitize();
        assert_eq!(p.artwork_scale, 0.6);
    }

    #[test]
    fn round_trips() {
        let p = PlayerLook { style: PlayerStyle::ClickWheel, wheel_theme: WheelTheme::Red, ..Default::default() };
        let back: PlayerLook = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
    }
}
