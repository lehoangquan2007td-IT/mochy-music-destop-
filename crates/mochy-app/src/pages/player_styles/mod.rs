//! Now Playing styles and look (port of the Android player selector's
//! styles, `PlayerBackgroundStyle`, `PlayerCoverStyle` and the customisation
//! switches), adapted to a desktop window.
//!
//! The flagship view in `now_playing.rs` calls the small hooks here
//! ([`show_alternate`], [`paint_backdrop`], [`paint_cover`], [`art_side`],
//! [`badge`], [`stream_info_button`], [`style_switcher`]…); the settings are pushed once per frame by
//! [`begin_frame`] (like the UI language), so the views need no new arguments.

pub mod bedside;
pub mod click_wheel;
mod clock;
mod settings_ui;
mod switcher;
pub mod turntable;

use std::sync::{Mutex, RwLock};

use egui::{Color32, CornerRadius, Mesh, Pos2, Rect, Shape, Stroke, TextureHandle, Ui, Vec2, pos2, vec2};
use mochy_core::models::Song;
use mochy_core::player_look::{CoverShape, PlayerBackground, PlayerLook, PlayerStyle};
use mochy_core::settings::AppSettings;

pub use settings_ui::{appearance_section, now_playing_section};

use super::now_playing::NpUi;
use super::{NowInfo, SideTab};
use crate::actions::{Action, Ctx};
use crate::pages::now_playing::SideData;
use crate::stream_facts;
use crate::ui::i18n::tr;
use crate::ui::lyrics_view::LyricsView;
use crate::widgets;

/// What the player is drawn with this frame.
#[derive(Debug, Clone, Default)]
pub struct Look {
    pub player: PlayerLook,
    /// Frosted / translucent effects allowed (Appearance → glass).
    pub glass: bool,
}

static LOOK: RwLock<Option<Look>> = RwLock::new(None);

/// The look edited inside the player ([`style_switcher`]); applied to the
/// settings by the next [`begin_frame`], so it is saved like any Settings edit.
static REQUEST: Mutex<Option<PlayerLook>> = Mutex::new(None);

fn request(look: PlayerLook) {
    if let Ok(mut r) = REQUEST.lock() {
        *r = Some(look);
    }
}

fn take_request() -> Option<PlayerLook> {
    REQUEST.lock().ok().and_then(|mut r| r.take())
}

/// egui's own default animation time (s).
const BASE_ANIMATION_TIME: f32 = 1.0 / 12.0;

/// Push this frame's look and Audio Lab requests (call before drawing).
pub fn begin_frame(ctx: &egui::Context, s: &mut AppSettings, song: Option<&Song>, volume: f32) {
    if let Some(p) = take_request() {
        s.player = p;
    }
    s.player.sanitize();
    let (norm, quality) = stream_facts::take_requests();
    if let Some(n) = norm {
        s.playback.audio_normalization = n;
    }
    if let Some(q) = quality {
        s.playback.audio_quality = q;
    }
    stream_facts::note_playback(stream_facts::PlaybackFacts {
        quality: s.playback.audio_quality,
        youtube_normalization: s.playback.audio_normalization,
    });
    stream_facts::note_now(song, volume);
    let mut player = s.player.clone();
    if let Some(st) = shots_style() {
        player.style = st;
    }
    let look = Look { player, glass: !s.appearance.disable_blur };
    if let Ok(mut l) = LOOK.write() {
        *l = Some(look);
    }
    // Motion: the master switch and the speed multiplier (Android
    // `EnableElementBlurAnimatorKey` / `AnimationSpeedKey`).
    let speed = if s.appearance.animation_speed.is_finite() { s.appearance.animation_speed.clamp(0.25, 4.0) } else { 1.0 };
    let want = if s.player.animations { BASE_ANIMATION_TIME / speed } else { 0.0 };
    if (ctx.global_style().animation_time - want).abs() > 1e-4 {
        ctx.all_styles_mut(|st| st.animation_time = want);
    }
}

/// `MOCHY_SHOTS_NP_STYLE=turntable|wheel|bedside` (screenshot automation only).
fn shots_style() -> Option<PlayerStyle> {
    std::env::var_os("MOCHY_SHOTS")?;
    match std::env::var("MOCHY_SHOTS_NP_STYLE").ok()?.as_str() {
        "turntable" => Some(PlayerStyle::Turntable),
        "wheel" | "clickwheel" => Some(PlayerStyle::ClickWheel),
        "bedside" => Some(PlayerStyle::Bedside),
        "flagship" => Some(PlayerStyle::Flagship),
        _ => None,
    }
}

pub fn look() -> Look {
    LOOK.read().ok().and_then(|l| l.clone()).unwrap_or_else(|| Look { glass: true, ..Default::default() })
}

/// Motion allowed (respect the animations switch for continuous motion).
pub fn animations() -> bool {
    look().player.animations
}

// ---------------------------------------------------------------------------
// Hooks for the flagship view
// ---------------------------------------------------------------------------

/// Draws the non-flagship styles over the whole window; `true` when it did.
#[allow(clippy::too_many_arguments)]
pub fn show_alternate(
    ui: &mut Ui,
    cx: &mut Ctx,
    now: &NowInfo,
    d: &SideData,
    tab: &mut SideTab,
    np: &mut NpUi,
    lv: &mut LyricsView,
    grad: (Color32, Color32),
) -> bool {
    match look().player.style {
        PlayerStyle::ClickWheel => {
            click_wheel::show(ui, cx, now, d, tab, np, lv, grad);
            true
        }
        PlayerStyle::Bedside => {
            bedside::show(ui, cx, now, grad);
            true
        }
        PlayerStyle::Flagship | PlayerStyle::Turntable => false,
    }
}

/// Paint the chosen background over the flagship's default one (which is the
/// cover gradient — then nothing is drawn).
pub fn paint_backdrop(ui: &Ui, cx: &mut Ctx, now: &NowInfo, rect: Rect, grad: (Color32, Color32)) {
    let l = look();
    // Without glass the default gradient is repainted flat (no cover wash).
    let force = !l.glass && l.player.background == PlayerBackground::CoverGradient;
    paint_background(ui, cx, now, rect, grad, l.player.background, l.glass, force);
}

/// Any background style into `rect`; `force` paints the cover gradient too.
#[allow(clippy::too_many_arguments)]
pub fn paint_background(ui: &Ui, cx: &mut Ctx, now: &NowInfo, rect: Rect, grad: (Color32, Color32), bg: PlayerBackground, glass: bool, force: bool) {
    let dark = |c: Color32, t: f32| widgets::lerp_color(c, Color32::BLACK, t);
    let (g1, g2) = grad;
    let p = ui.painter();
    match bg {
        PlayerBackground::CoverGradient => {
            if force {
                super::now_playing::gradient(ui, rect, dark(g1, 0.25), dark(widgets::lerp_color(g1, g2, 0.5), 0.4), dark(widgets::lerp_color(g1, g2, 0.6), 0.55), dark(g2, 0.6));
                if glass {
                    cover_wash(ui, cx, now, rect, 34);
                }
                super::now_playing::gradient(ui, rect, Color32::from_black_alpha(40), Color32::from_black_alpha(70), Color32::from_black_alpha(150), Color32::from_black_alpha(170));
            }
        }
        PlayerBackground::Solid => {
            p.rect_filled(rect, CornerRadius::ZERO, dark(g1, 0.55));
        }
        PlayerBackground::DarkGradient => {
            super::now_playing::gradient(ui, rect, dark(g1, 0.55), dark(g1, 0.7), Color32::BLACK, Color32::BLACK);
        }
        PlayerBackground::BlurredCover => {
            p.rect_filled(rect, CornerRadius::ZERO, dark(g2, 0.5));
            // A cheap frost: the cover blown up several times, slightly offset.
            if let Some(tex) = cover_tex(cx, now) {
                let side = rect.width().max(rect.height()) * 1.35;
                let n = if glass { 6 } else { 1 };
                for k in 0..n {
                    let a = k as f32 / n as f32 * std::f32::consts::TAU;
                    let off = vec2(a.cos(), a.sin()) * 18.0 * (k > 0) as u8 as f32;
                    let r = Rect::from_center_size(rect.center() + off, vec2(side, side));
                    let alpha = if k == 0 { 150 } else { 40 };
                    p.image(tex.id(), r, widgets::cover_uv(tex.size_vec2(), r.size()), Color32::from_white_alpha(alpha));
                }
            }
            super::now_playing::gradient(ui, rect, Color32::from_black_alpha(110), Color32::from_black_alpha(120), Color32::from_black_alpha(170), Color32::from_black_alpha(190));
        }
        PlayerBackground::Ambient => {
            // The cover's colours drifting slowly (Android `AmbientBackdrop`).
            p.rect_filled(rect, CornerRadius::ZERO, dark(g2, 0.7));
            let t = if animations() { ui.input(|i| i.time) as f32 } else { 0.0 };
            let blobs = [
                (g1, 0.30, 0.30, 0.55, 0.11, 0.0),
                (g2, 0.72, 0.36, 0.50, 0.08, 1.7),
                (widgets::lerp_color(g1, g2, 0.5), 0.45, 0.78, 0.60, 0.06, 3.1),
                (widgets::lerp_color(g1, Color32::WHITE, 0.15), 0.85, 0.85, 0.40, 0.09, 4.4),
            ];
            for (col, x, y, r, speed, phase) in blobs {
                let cx_ = rect.left() + rect.width() * (x + 0.08 * (t * speed + phase).sin());
                let cy_ = rect.top() + rect.height() * (y + 0.08 * (t * speed * 1.3 + phase).cos());
                radial(p, pos2(cx_, cy_), rect.width().max(rect.height()) * r, dark(col, 0.25).gamma_multiply(0.9));
            }
            super::now_playing::gradient(ui, rect, Color32::from_black_alpha(40), Color32::from_black_alpha(60), Color32::from_black_alpha(140), Color32::from_black_alpha(160));
            if animations() {
                crate::ui::pace::animate_after(ui.ctx(), std::time::Duration::from_millis(50));
            }
        }
    }
}

fn cover_wash(ui: &Ui, cx: &mut Ctx, now: &NowInfo, rect: Rect, alpha: u8) {
    if let Some(tex) = cover_tex(cx, now) {
        let side = rect.width().max(rect.height()) * 1.2;
        let r = Rect::from_center_size(rect.center(), vec2(side, side));
        ui.painter().image(tex.id(), r, widgets::cover_uv(tex.size_vec2(), r.size()), Color32::from_white_alpha(alpha));
    }
}

/// A soft radial blob (mesh fan, transparent edge).
pub fn radial(p: &egui::Painter, c: Pos2, r: f32, col: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(c, col);
    let n = 48;
    for k in 0..=n {
        let a = k as f32 / n as f32 * std::f32::consts::TAU;
        m.colored_vertex(c + vec2(a.cos(), a.sin()) * r, Color32::TRANSPARENT);
        if k > 0 {
            m.add_triangle(0, k as u32, k as u32 + 1);
        }
    }
    p.add(Shape::mesh(m));
}

pub fn cover_tex(cx: &mut Ctx, now: &NowInfo) -> Option<TextureHandle> {
    now.song.as_ref().and_then(crate::images::song_src).and_then(|s| cx.images.get(&s, crate::images::LARGE_PX))
}

/// Cover size after the "artwork size" setting.
pub fn art_side(side: f32) -> f32 {
    (side * look().player.artwork_scale).max(140.0)
}

pub fn show_volume() -> bool {
    look().player.show_volume
}

pub fn show_side_panel() -> bool {
    look().player.show_side_panel
}

/// The cover as the look wants it: rounded/square artwork, a record, or the
/// turntable (whose arm seeks).
pub fn paint_cover(ui: &mut Ui, cx: &mut Ctx, now: &NowInfo, art: Rect, tex: Option<&TextureHandle>) {
    let l = look().player;
    if l.style == PlayerStyle::Turntable {
        turntable::show(ui, cx, now, art, tex, l.spin_records);
        return;
    }
    match l.cover {
        CoverShape::Rounded | CoverShape::Square => {
            let radius = if l.cover == CoverShape::Square { 0.0 } else { l.artwork_radius };
            if l.cover == CoverShape::Rounded {
                ui.painter().rect_filled(art.expand(6.0).translate(vec2(0.0, 8.0)), CornerRadius::same((radius + 6.0) as u8), Color32::from_black_alpha(70));
            }
            widgets::paint_artwork(ui, art, tex, radius);
        }
        CoverShape::Disc | CoverShape::DiscLabel => {
            let angle = spin(ui, "np_disc", now.playing && l.spin_records, 0.9);
            let c = art.center();
            let r = art.width().min(art.height()) / 2.0;
            ui.painter().circle_filled(c + vec2(0.0, 8.0), r + 4.0, Color32::from_black_alpha(80));
            if l.cover == CoverShape::Disc {
                textured_disc(ui.painter(), tex, c, r, angle, Color32::from_rgb(0x1A, 0x1A, 0x1C));
                ui.painter().circle_stroke(c, r * 0.36, Stroke::new(1.0, Color32::from_white_alpha(40)));
                ui.painter().circle_filled(c, r * 0.06, Color32::from_rgb(0x0E, 0x0E, 0x10));
            } else {
                record(ui.painter(), c, r, angle);
                textured_disc(ui.painter(), tex, c, r * 0.36, angle, Color32::from_rgb(0x40, 0x40, 0x44));
                ui.painter().circle_filled(c, r * 0.025, Color32::from_rgb(0xD0, 0xD0, 0xD4));
            }
        }
    }
}

/// A rotating angle kept in egui memory; turns while `running`.
pub fn spin(ui: &Ui, key: &str, running: bool, rad_per_sec: f32) -> f32 {
    let id = egui::Id::unique(("mochy_spin", key));
    let dt = ui.input(|i| i.stable_dt).clamp(0.0, 0.1);
    let running = running && animations();
    let a = ui.ctx().data_mut(|d| {
        let a = d.get_temp_mut_or(id, 0.0f32);
        if running {
            *a = (*a + dt * rad_per_sec).rem_euclid(std::f32::consts::TAU);
        }
        *a
    });
    if running {
        crate::ui::pace::animate(ui.ctx());
    }
    a
}

/// A black record with grooves and a sheen.
pub fn record(p: &egui::Painter, c: Pos2, r: f32, angle: f32) {
    p.circle_filled(c, r, Color32::from_rgb(0x12, 0x12, 0x14));
    let mut k = 0.40;
    while k < 0.97 {
        p.circle_stroke(c, r * k, Stroke::new(1.0, Color32::from_white_alpha(10)));
        k += 0.035;
    }
    p.circle_stroke(c, r * 0.985, Stroke::new(2.0, Color32::from_white_alpha(18)));
    // Two light sheens that turn with the record.
    for base in [0.5f32, 0.5 + std::f32::consts::PI] {
        let a0 = base + angle * 0.0 + angle;
        let mut m = Mesh::default();
        m.colored_vertex(c, Color32::TRANSPARENT);
        let n = 10;
        for i in 0..=n {
            let a = a0 + 0.55 * i as f32 / n as f32;
            let w = (1.0 - ((i as f32 / n as f32) - 0.5).abs() * 2.0).max(0.0);
            m.colored_vertex(c + vec2(a.cos(), a.sin()) * r * 0.97, Color32::from_white_alpha((22.0 * w) as u8));
            if i > 0 {
                m.add_triangle(0, i, i + 1);
            }
        }
        p.add(Shape::mesh(m));
    }
}

/// The cover mapped onto a disc of radius `r`, turned by `angle`.
pub fn textured_disc(p: &egui::Painter, tex: Option<&TextureHandle>, c: Pos2, r: f32, angle: f32, fallback: Color32) {
    let Some(t) = tex else {
        p.circle_filled(c, r, fallback);
        return;
    };
    let uv = widgets::cover_uv(t.size_vec2(), Vec2::splat(2.0 * r));
    let mut m = Mesh::with_texture(t.id());
    let n = 72;
    m.vertices.push(egui::epaint::Vertex { pos: c, uv: uv.center(), color: Color32::WHITE });
    for k in 0..=n {
        let a = k as f32 / n as f32 * std::f32::consts::TAU;
        let (s, co) = a.sin_cos();
        let pos = c + vec2(co, s) * r;
        // Sample the image rotated by -angle so the picture turns with the disc.
        let (sa, ca) = (a - angle).sin_cos();
        let u = uv.center().x + ca * uv.width() / 2.0;
        let v = uv.center().y + sa * uv.height() / 2.0;
        m.vertices.push(egui::epaint::Vertex { pos, uv: pos2(u, v), color: Color32::WHITE });
        if k > 0 {
            m.add_triangle(0, k, k + 1);
        }
    }
    p.add(Shape::mesh(m));
}

/// The format badge text, per the badge settings (None = hidden).
pub fn badge(now: &NowInfo) -> Option<String> {
    let l = look().player;
    if !l.show_format_badge {
        return None;
    }
    stream_facts::badge(&stream_facts::snapshot_now(), l.show_sample_rate).or_else(|| now.format.clone())
}

/// A small "stream info" button; opens a window with the stats for nerds.
pub fn stream_info_button(ui: &mut Ui, cx: &mut Ctx) {
    let id = egui::Id::unique("mochy_stream_info_open");
    let mut open = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let r = ui.add(egui::Button::new(egui::RichText::new("ⓘ").size(13.0)).frame(false)).on_hover_text(tr("Stream info"));
    if r.clicked() {
        open = !open;
    }
    if open {
        let accent = cx.accent;
        egui::Window::new(tr("Stream info"))
            .id(egui::Id::unique("mochy_stream_info"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(460.0)
            .default_pos(ui.max_rect().left_top() + vec2(40.0, 60.0))
            .show(ui.ctx(), |ui| {
                egui::ScrollArea::vertical().max_height(560.0).show(ui, |ui| {
                    let snap = stream_facts::snapshot_now();
                    let cards = stream_facts::cards(&snap);
                    if cards.is_empty() {
                        ui.label(tr("Play a song to see its format, decoder, DSP chain and output."));
                    }
                    for c in &cards {
                        crate::ui::audio_lab::quality::stat_card(ui, c);
                    }
                    if ui.button(tr("Open Audio Lab")).clicked() {
                        cx.act(Action::Navigate(crate::nav::Route::AudioLab));
                        cx.act(Action::SetNowPlaying(false));
                    }
                    let _ = accent;
                });
            });
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, open));
}

/// Shared top-left close button for full-window styles; returns its rect.
pub fn close_button(ui: &mut Ui, cx: &mut Ctx, at: Pos2, color: Color32) {
    let r = Rect::from_min_size(at, vec2(36.0, 36.0));
    let resp = ui.interact(r, egui::Id::unique("mochy_style_close"), egui::Sense::click()).on_hover_text(tr("Close (Esc)"));
    if resp.hovered() {
        ui.painter().circle_filled(r.center(), 18.0, Color32::from_white_alpha(20));
    }
    crate::widgets::icons::paint(ui.painter(), r.shrink(8.0), crate::widgets::icons::Icon::Collapse, color);
    if resp.clicked() {
        cx.act(Action::SetNowPlaying(false));
    }
}

/// Shared "player style" button, right of the close button in every style:
/// opens the look panel ([`switcher::panel`]: styles and backgrounds as small
/// pictures plus the current style's options) without a trip to Settings.
pub fn style_switcher(ui: &mut Ui, cx: &mut Ctx, at: Pos2, color: Color32) {
    let r = Rect::from_min_size(at, vec2(36.0, 36.0));
    let resp = ui.interact(r, egui::Id::unique("mochy_style_switcher"), egui::Sense::click()).on_hover_text(tr("Player style"));
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    if resp.hovered() || open {
        ui.painter().circle_filled(r.center(), 18.0, Color32::from_white_alpha(20));
    }
    paint_style_icon(ui.painter(), r.shrink(9.0), color);
    // Screenshot automation: `MOCHY_SHOTS_NP_PANEL=1` shows the panel open.
    if !open && std::env::var_os("MOCHY_SHOTS").is_some() && std::env::var_os("MOCHY_SHOTS_NP_PANEL").is_some() {
        egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&resp));
    }
    let picked = egui::Popup::menu(&resp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(switcher::WIDTH)
        .show(|ui| {
            let before = look().player;
            let mut edited = before.clone();
            let pick = switcher::panel(ui, &mut edited);
            if edited != before {
                request(edited);
                ui.ctx().request_repaint();
            }
            pick
        });
    if let Some(switcher::PanelPick::OpenSettings) = picked.and_then(|p| p.inner) {
        cx.act(Action::Navigate(crate::nav::Route::Settings));
        cx.act(Action::SetNowPlaying(false));
    }
}

/// A small "layouts" glyph: a frame split into a big pane and two small ones.
fn paint_style_icon(p: &egui::Painter, r: Rect, color: Color32) {
    let s = r.width().min(r.height());
    let r = Rect::from_center_size(r.center(), Vec2::splat(s));
    let st = Stroke::new((s * 0.085).max(1.4), color);
    p.rect_stroke(r.shrink(s * 0.06), CornerRadius::same((s * 0.16) as u8), st, egui::StrokeKind::Inside);
    let x = r.left() + s * 0.48;
    p.line_segment([pos2(x, r.top() + s * 0.1), pos2(x, r.bottom() - s * 0.1)], st);
    let y = r.center().y;
    p.line_segment([pos2(x, y), pos2(r.right() - s * 0.1, y)], st);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_frame_applies_requests_and_motion() {
        let ctx = egui::Context::default();
        let mut s = AppSettings::default();
        s.player.artwork_scale = 3.0;
        s.appearance.animation_speed = 2.0;
        stream_facts::request_quality(mochy_core::settings::AudioQuality::High);
        begin_frame(&ctx, &mut s, None, 0.5);
        assert_eq!(s.player.artwork_scale, 1.0);
        assert_eq!(s.playback.audio_quality, mochy_core::settings::AudioQuality::High);
        assert!((ctx.global_style().animation_time - BASE_ANIMATION_TIME / 2.0).abs() < 1e-5);
        s.player.animations = false;
        begin_frame(&ctx, &mut s, None, 0.5);
        assert_eq!(ctx.global_style().animation_time, 0.0);
        // In-player style / background picks land in the settings once.
        request(PlayerLook { style: PlayerStyle::ClickWheel, background: PlayerBackground::Ambient, ..s.player.clone() });
        begin_frame(&ctx, &mut s, None, 0.5);
        assert_eq!(s.player.style, PlayerStyle::ClickWheel);
        assert_eq!(s.player.background, PlayerBackground::Ambient);
        assert_eq!(look().player.style, PlayerStyle::ClickWheel);
        s.player.style = PlayerStyle::Flagship;
        begin_frame(&ctx, &mut s, None, 0.5);
        assert_eq!(s.player.style, PlayerStyle::Flagship);
    }
}
