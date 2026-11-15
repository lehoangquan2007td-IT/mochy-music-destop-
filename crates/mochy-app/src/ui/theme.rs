//! Theme & color (spec §7.4): dark-only Mochy style, dynamic accent from artwork.
//!
//! CONTRACT (used by the app shell — keep signatures stable).
//!
//! * [`apply`] installs the Inter fonts (+ system CJK/Korean/Thai/emoji
//!   fallbacks, once per context) and the Mochy dark visuals tinted with the
//!   accent. Calling it every frame is cheap: it returns early when nothing
//!   changed.
//! * [`extract_accent`] / [`extract_gradient`] are the desktop equivalents of
//!   Android's `Bitmap.extractThemeColor()` / `extractGradientColors()`
//!   (AndroidX Palette median-cut + Material `Score`).

use std::sync::Arc;

use egui::{
    Color32, Context, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Id,
    Margin, Shadow, Stroke, TextStyle, Vec2,
};

/// Default accent `0xFF8E8E8E`.
pub const DEFAULT_ACCENT: Color32 = Color32::from_rgb(0x8E, 0x8E, 0x8E);

// ---------------------------------------------------------------------------
// Palette constants (spec §7.4 + Android `LabColors`)
// ---------------------------------------------------------------------------

/// Pure black background/surface.
pub const BLACK: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);
/// Near-black background used when "pure black" is off.
pub const NEAR_BLACK: Color32 = Color32::from_rgb(0x0A, 0x0A, 0x0A);
/// Material `primary`.
pub const PRIMARY: Color32 = Color32::from_rgb(0xE0, 0xE0, 0xE0);
/// Material `secondary`.
pub const SECONDARY: Color32 = Color32::from_rgb(0xB0, 0xB0, 0xB0);
/// Material `primaryContainer`.
pub const PRIMARY_CONTAINER: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);
/// Material `secondaryContainer`.
pub const SECONDARY_CONTAINER: Color32 = Color32::from_rgb(0x2C, 0x2C, 0x2C);
/// Material `surfaceVariant`.
pub const SURFACE_VARIANT: Color32 = Color32::from_rgb(0x1A, 0x1A, 0x1A);
/// Library tab pills: track.
pub const PILL_TRACK: Color32 = Color32::from_rgb(0x16, 0x16, 0x18);
/// Library tab pills: chip.
pub const PILL_CHIP: Color32 = Color32::from_rgb(0x23, 0x23, 0x26);
/// Library tab pills: selected chip.
pub const PILL_SELECTED: Color32 = Color32::from_rgb(0x3D, 0x3D, 0x41);
/// Settings search bubble.
pub const BUBBLE: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x1E);
/// Settings search bubble (selected).
pub const BUBBLE_SELECTED: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x3E);
/// Card background (Android `LabColors.Card`).
pub const CARD: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x1E);
/// Inset "stage" (graphs) background (Android `LabColors.Stage`).
pub const STAGE: Color32 = Color32::from_rgb(0x0E, 0x0E, 0x10);
/// Hairline on stages/cards (Android `LabColors.StageLine`).
pub const STAGE_LINE: Color32 = Color32::from_rgb(0x2E, 0x2E, 0x32);
/// List row inside a card (Android `LabColors.Row`).
pub const ROW: Color32 = Color32::from_rgb(0x26, 0x26, 0x2A);
/// Selected list row (Android `LabColors.RowSelected`).
pub const ROW_SELECTED: Color32 = Color32::from_rgb(0x34, 0x34, 0x3A);
/// Secondary text (Android `LabColors.Secondary`).
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0x9A, 0x9A, 0x9F);
/// Faint text / disabled glyphs (Android `LabColors.Faint`).
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x5C, 0x5C, 0x63);

/// Inter weights embedded in the binary (SIL OFL 1.1, see `assets/fonts`).
pub const FONT_REGULAR: &str = "Inter-Regular";
pub const FONT_MEDIUM: &str = "Inter-Medium";
pub const FONT_SEMIBOLD: &str = "Inter-SemiBold";
pub const FONT_BOLD: &str = "Inter-Bold";

static INTER_REGULAR: &[u8] = include_bytes!("../../assets/fonts/inter_regular.ttf");
static INTER_MEDIUM: &[u8] = include_bytes!("../../assets/fonts/inter_medium.ttf");
static INTER_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/inter_semibold.ttf");
static INTER_BOLD: &[u8] = include_bytes!("../../assets/fonts/inter_bold.ttf");

/// Card corner radius (px).
pub const CARD_RADIUS: u8 = 16;

// ---------------------------------------------------------------------------
// apply
// ---------------------------------------------------------------------------

/// Install fonts + dark visuals. Call once at startup and whenever accent /
/// pure-black changes.
pub fn apply(ctx: &Context, accent: Color32, pure_black: bool) {
    install_fonts_once(ctx);

    // Fonts set this pass may only become active next pass; re-style once
    // they are, so the heading style can point at Inter SemiBold.
    let fonts_ready = ctx.fonts(|f| {
        f.definitions().families.contains_key(&FontFamily::Name(FONT_SEMIBOLD.into()))
    });
    let key = Id::unique("mochy_theme_applied");
    let current = (accent, pure_black, fonts_ready);
    if ctx.data(|d| d.get_temp::<(Color32, bool, bool)>(key)) == Some(current) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(key, current));

    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(|style| configure_style(style, accent, pure_black, fonts_ready));
}

/// Font family by name, or `Proportional` when it isn't installed (e.g. before
/// [`apply`] ran) — using an unknown family would panic in egui.
pub fn family(ctx: &Context, name: &str) -> FontFamily {
    let fam = FontFamily::Name(name.into());
    if ctx.fonts(|f| f.definitions().families.contains_key(&fam)) {
        fam
    } else {
        FontFamily::Proportional
    }
}

/// Inter Bold at `size` (falls back to the default proportional font).
pub fn bold(ctx: &Context, size: f32) -> FontId {
    FontId::new(size, family(ctx, FONT_BOLD))
}

/// Inter SemiBold at `size` (Android's global weight).
pub fn semibold(ctx: &Context, size: f32) -> FontId {
    FontId::new(size, family(ctx, FONT_SEMIBOLD))
}

/// The standard Mochy card: `#1C1C1E`, 16 px corners, 16 px padding.
pub fn card_frame() -> Frame {
    Frame::new()
        .fill(CARD)
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::same(16))
}

/// Background color for the chosen black mode.
pub fn background(pure_black: bool) -> Color32 {
    if pure_black { BLACK } else { NEAR_BLACK }
}

/// Black or white, whichever reads better on `bg`.
pub fn on_color(bg: Color32) -> Color32 {
    // WCAG contrast against black vs white.
    let l = relative_luminance(bg);
    let vs_white = 1.05 / (l + 0.05);
    let vs_black = (l + 0.05) / 0.05;
    if vs_white >= vs_black { Color32::WHITE } else { Color32::BLACK }
}

/// The accent lifted (if needed) so it reads on a black background.
pub fn readable_on_black(c: Color32) -> Color32 {
    lift_lightness(c, 58.0)
}

fn configure_style(style: &mut egui::Style, accent: Color32, pure_black: bool, fonts_ready: bool) {
    use egui::style::{HandleShape, WidgetVisuals};

    let bg = background(pure_black);
    let accent_text = readable_on_black(accent);
    let on_accent = on_color(accent);

    let mut v = egui::Visuals::dark();
    v.dark_mode = true;
    v.override_text_color = None;
    v.panel_fill = bg;
    v.window_fill = SURFACE_VARIANT;
    v.window_stroke = Stroke::new(1.0, STAGE_LINE);
    v.window_corner_radius = CornerRadius::same(16);
    v.menu_corner_radius = CornerRadius::same(12);
    v.window_shadow = Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(140) };
    v.popup_shadow = Shadow { offset: [0, 6], blur: 16, spread: 0, color: Color32::from_black_alpha(120) };
    v.faint_bg_color = PILL_TRACK;
    v.extreme_bg_color = STAGE;
    v.text_edit_bg_color = Some(PILL_TRACK);
    v.code_bg_color = SURFACE_VARIANT;
    v.hyperlink_color = accent_text;
    v.warn_fg_color = Color32::from_rgb(0xF2, 0xD4, 0x6B);
    v.error_fg_color = Color32::from_rgb(0xFF, 0x8A, 0x80);
    v.selection.bg_fill = accent;
    v.selection.stroke = Stroke::new(1.0, on_accent);
    v.slider_trailing_fill = true;
    v.handle_shape = HandleShape::Circle;
    v.text_cursor.stroke = Stroke::new(2.0, accent_text);
    v.striped = false;

    let r = CornerRadius::same(10);
    let mk = |bg_fill: Color32, stroke: Stroke, fg: Color32, expansion: f32| WidgetVisuals {
        bg_fill,
        weak_bg_fill: bg_fill,
        bg_stroke: stroke,
        corner_radius: r,
        fg_stroke: Stroke::new(1.0, fg),
        expansion,
    };
    v.widgets.noninteractive = mk(SURFACE_VARIANT, Stroke::new(1.0, STAGE_LINE), PRIMARY, 0.0);
    v.widgets.inactive = mk(PILL_CHIP, Stroke::NONE, Color32::from_gray(0xEA), 0.0);
    v.widgets.hovered = mk(PILL_SELECTED, Stroke::new(1.0, Color32::from_gray(0x55)), Color32::WHITE, 1.0);
    v.widgets.active = mk(PILL_SELECTED, Stroke::new(1.0, accent_text), Color32::WHITE, 1.0);
    v.widgets.open = mk(SECONDARY_CONTAINER, Stroke::new(1.0, STAGE_LINE), Color32::WHITE, 0.0);
    style.visuals = v;

    let prop = FontFamily::Proportional;
    let heading_family = if fonts_ready {
        FontFamily::Name(FONT_SEMIBOLD.into())
    } else {
        prop.clone()
    };
    style.text_styles = [
        (TextStyle::Small, FontId::new(11.5, prop.clone())),
        (TextStyle::Body, FontId::new(14.0, prop.clone())),
        (TextStyle::Button, FontId::new(14.0, prop.clone())),
        (TextStyle::Heading, FontId::new(22.0, heading_family.clone())),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        (TextStyle::Name("Display".into()), FontId::new(32.0, heading_family.clone())),
        (TextStyle::Name("Title".into()), FontId::new(18.0, heading_family)),
    ]
    .into();

    let s = &mut style.spacing;
    s.item_spacing = Vec2::new(8.0, 8.0);
    s.button_padding = Vec2::new(12.0, 6.0);
    s.window_margin = Margin::same(16);
    s.menu_margin = Margin::same(8);
    s.interact_size = Vec2::new(40.0, 26.0);
    s.slider_width = 200.0;
    s.slider_rail_height = 6.0;
    s.combo_width = 140.0;
    s.text_edit_width = 260.0;
    s.icon_width = 16.0;
    s.icon_spacing = 6.0;
    s.scroll = egui::style::ScrollStyle::floating();

    style.animation_time = 0.18;
}

fn install_fonts_once(ctx: &Context) {
    let key = Id::unique("mochy_theme_fonts_installed");
    if ctx.data(|d| d.get_temp::<bool>(key)).unwrap_or(false) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(key, true));
    ctx.set_fonts(font_definitions());
}

/// Inter (embedded) + egui defaults + whatever system fallback fonts exist.
fn font_definitions() -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    for (name, bytes) in [
        (FONT_REGULAR, INTER_REGULAR),
        (FONT_MEDIUM, INTER_MEDIUM),
        (FONT_SEMIBOLD, INTER_SEMIBOLD),
        (FONT_BOLD, INTER_BOLD),
    ] {
        defs.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    }

    let mut fallbacks: Vec<String> = Vec::new();
    for (i, path) in system_fallback_paths().into_iter().enumerate() {
        if let Ok(bytes) = std::fs::read(&path) {
            let name = format!("system-fallback-{i}");
            defs.font_data.insert(name.clone(), Arc::new(FontData::from_owned(bytes)));
            fallbacks.push(name);
        }
    }

    // Proportional: Inter Medium first, then egui's defaults (emoji icons…), then system.
    let default_prop = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let chain = |primary: &str| -> Vec<String> {
        let mut v = vec![primary.to_owned()];
        v.extend(default_prop.iter().cloned());
        v.extend(fallbacks.iter().cloned());
        v
    };
    let prop = chain(FONT_MEDIUM);
    let regular = chain(FONT_REGULAR);
    let medium = chain(FONT_MEDIUM);
    let semibold = chain(FONT_SEMIBOLD);
    let bold = chain(FONT_BOLD);
    defs.families.insert(FontFamily::Proportional, prop);
    defs.families.insert(FontFamily::Name(FONT_REGULAR.into()), regular);
    defs.families.insert(FontFamily::Name(FONT_MEDIUM.into()), medium);
    defs.families.insert(FontFamily::Name(FONT_SEMIBOLD.into()), semibold);
    defs.families.insert(FontFamily::Name(FONT_BOLD.into()), bold);
    if let Some(mono) = defs.families.get_mut(&FontFamily::Monospace) {
        mono.extend(fallbacks.iter().cloned());
    }
    defs
}

/// First existing font per script group (CJK SC, Japanese, Korean, Thai,
/// Indic, emoji, Segoe UI catch-all) for the current platform.
fn system_fallback_paths() -> Vec<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    let groups: &[&[&str]] = &[
        &[r"C:\Windows\Fonts\segoeui.ttf"],
        &[r"C:\Windows\Fonts\msyh.ttc", r"C:\Windows\Fonts\simsun.ttc"],
        &[r"C:\Windows\Fonts\YuGothM.ttc", r"C:\Windows\Fonts\meiryo.ttc", r"C:\Windows\Fonts\msgothic.ttc"],
        &[r"C:\Windows\Fonts\malgun.ttf", r"C:\Windows\Fonts\gulim.ttc"],
        &[r"C:\Windows\Fonts\LeelawUI.ttf", r"C:\Windows\Fonts\LEELAWAD.TTF", r"C:\Windows\Fonts\tahoma.ttf"],
        &[r"C:\Windows\Fonts\Nirmala.ttc", r"C:\Windows\Fonts\Nirmala.ttf"],
        &[r"C:\Windows\Fonts\seguiemj.ttf", r"C:\Windows\Fonts\seguisym.ttf"],
    ];
    #[cfg(target_os = "macos")]
    let groups: &[&[&str]] = &[
        &["/System/Library/Fonts/PingFang.ttc", "/System/Library/Fonts/STHeiti Medium.ttc"],
        &["/System/Library/Fonts/Hiragino Sans GB.ttc", "/System/Library/Fonts/ヒラギノ角ゴシック W4.ttc"],
        &["/System/Library/Fonts/AppleSDGothicNeo.ttc"],
        &["/System/Library/Fonts/Supplemental/Thonburi.ttc", "/System/Library/Fonts/Thonburi.ttc"],
        &["/System/Library/Fonts/Apple Color Emoji.ttc"],
    ];
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let groups: &[&[&str]] = &[
        &[
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        ],
        &[
            "/usr/share/fonts/truetype/noto/NotoSansThai-Regular.ttf",
            "/usr/share/fonts/noto/NotoSansThai-Regular.ttf",
            "/usr/share/fonts/google-noto/NotoSansThai-Regular.ttf",
        ],
        &[
            "/usr/share/fonts/truetype/noto/NotoSansDevanagari-Regular.ttf",
            "/usr/share/fonts/noto/NotoSansDevanagari-Regular.ttf",
        ],
        &[
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
        ],
    ];
    groups
        .iter()
        .filter_map(|g| g.iter().map(std::path::PathBuf::from).find(|p| p.is_file()))
        .collect()
}

// ---------------------------------------------------------------------------
// Color math
// ---------------------------------------------------------------------------

fn srgb_to_linear(c: u8) -> f64 {
    let c = c as f64 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn relative_luminance(c: Color32) -> f64 {
    0.2126 * srgb_to_linear(c.r()) + 0.7152 * srgb_to_linear(c.g()) + 0.0722 * srgb_to_linear(c.b())
}

/// CIE L*a*b* (D65) of an sRGB color.
fn lab(r: u8, g: u8, b: u8) -> [f64; 3] {
    let (r, g, b) = (srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b));
    let x = (0.41233895 * r + 0.35762064 * g + 0.18051042 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.01932141 * r + 0.11916382 * g + 0.95034478 * b) / 1.08883;
    let f = |t: f64| {
        if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// (lightness L*, chroma, hue degrees) — an HCT stand-in (CIE LCh(ab)).
fn lch(c: [u8; 3]) -> (f64, f64, f64) {
    let [l, a, b] = lab(c[0], c[1], c[2]);
    let chroma = (a * a + b * b).sqrt();
    let mut hue = b.atan2(a).to_degrees();
    if hue < 0.0 {
        hue += 360.0;
    }
    (l, chroma, hue)
}

/// RGB (0–255) → HSL (h degrees, s, l in 0..1).
fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (if h < 0.0 { h + 360.0 } else { h }, s.clamp(0.0, 1.0), l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Color32 {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let to = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(to(r1), to(g1), to(b1))
}

/// Raise HSL lightness (keeping hue/saturation) until L* >= `min_lstar`.
fn lift_lightness(c: Color32, min_lstar: f64) -> Color32 {
    if lab(c.r(), c.g(), c.b())[0] >= min_lstar {
        return c;
    }
    let (h, s, mut l) = rgb_to_hsl(c.r(), c.g(), c.b());
    let mut out = c;
    for _ in 0..40 {
        l = (l + 0.02).min(0.9);
        out = hsl_to_rgb(h, s, l);
        if lab(out.r(), out.g(), out.b())[0] >= min_lstar || l >= 0.9 {
            break;
        }
    }
    out
}

/// Keep the hue, scale HSL lightness into `[min, max]` (moody backdrop).
fn darken(c: Color32, factor: f32, min: f32, max: f32) -> Color32 {
    let (h, s, l) = rgb_to_hsl(c.r(), c.g(), c.b());
    hsl_to_rgb(h, (s * 0.9).min(0.7), (l * factor).clamp(min, max))
}

// ---------------------------------------------------------------------------
// Palette (AndroidX `ColorCutQuantizer` median cut)
// ---------------------------------------------------------------------------

/// One quantized color with its pixel population.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Swatch {
    pub rgb: [u8; 3],
    pub population: u32,
}

/// Android Palette default filter: drop near-black, near-white and the
/// "I-line" (skin-tone band) colors.
fn palette_ignores(rgb: [u8; 3]) -> bool {
    let (h, s, l) = rgb_to_hsl(rgb[0], rgb[1], rgb[2]);
    l <= 0.05 || l >= 0.95 || ((10.0..=37.0).contains(&h) && s <= 0.82)
}

/// Median-cut the image (downsampled to ≈112×112 like Palette) into at most
/// `max_colors` swatches.
pub fn palette(rgba: &[u8], width: u32, height: u32, max_colors: usize) -> Vec<Swatch> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || rgba.len() < w * h * 4 || max_colors == 0 {
        return Vec::new();
    }
    // Palette resizes to an area of 112*112; sample with a stride instead.
    let area = (w * h) as f64;
    let step = ((area / (112.0 * 112.0)).sqrt().ceil() as usize).max(1);

    // Per 15-bit bucket: population + the true 8-bit channel sums, so swatch
    // colors aren't skewed by the 5-bit quantization (tiny noise on a gray
    // image must stay gray).
    let mut hist = vec![0u32; 1 << 15];
    let mut sums = vec![[0u64; 3]; 1 << 15];
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            let i = (y * w + x) * 4;
            if rgba[i + 3] < 128 {
                continue;
            }
            let q = ((rgba[i] as usize >> 3) << 10) | ((rgba[i + 1] as usize >> 3) << 5) | (rgba[i + 2] as usize >> 3);
            hist[q] += 1;
            for k in 0..3 {
                sums[q][k] += rgba[i + k] as u64;
            }
        }
    }
    let bucket = |c: &[u8; 3]| ((c[0] as usize) << 10) | ((c[1] as usize) << 5) | c[2] as usize;
    let bucket_avg = |q: usize| -> [u8; 3] {
        let n = hist[q].max(1) as u64;
        [(sums[q][0] / n) as u8, (sums[q][1] / n) as u8, (sums[q][2] / n) as u8]
    };

    // (quantized rgb as 5-bit components, population)
    let mut colors: Vec<([u8; 3], u32)> = hist
        .iter()
        .enumerate()
        .filter(|(q, n)| **n > 0 && !palette_ignores(bucket_avg(*q)))
        .map(|(q, n)| ([((q >> 10) & 31) as u8, ((q >> 5) & 31) as u8, (q & 31) as u8], *n))
        .collect();
    if colors.is_empty() {
        return Vec::new();
    }

    let to_swatch = |slice: &[([u8; 3], u32)]| -> Swatch {
        let mut sum = [0u64; 3];
        let mut pop = 0u64;
        for (c, n) in slice {
            let q = bucket(c);
            for k in 0..3 {
                sum[k] += sums[q][k];
            }
            pop += *n as u64;
        }
        let avg = |k: usize| (sum[k] as f64 / pop.max(1) as f64).round().clamp(0.0, 255.0) as u8;
        Swatch { rgb: [avg(0), avg(1), avg(2)], population: pop as u32 }
    };

    if colors.len() <= max_colors {
        return colors.iter().map(|c| to_swatch(std::slice::from_ref(c))).collect();
    }

    // Boxes are index ranges into `colors`; split the largest-volume box.
    let bounds = |s: &[([u8; 3], u32)]| -> ([u8; 3], [u8; 3]) {
        let mut lo = [31u8; 3];
        let mut hi = [0u8; 3];
        for (c, _) in s {
            for k in 0..3 {
                lo[k] = lo[k].min(c[k]);
                hi[k] = hi[k].max(c[k]);
            }
        }
        (lo, hi)
    };
    let volume = |s: &[([u8; 3], u32)]| -> u32 {
        let (lo, hi) = bounds(s);
        (0..3).map(|k| (hi[k] - lo[k]) as u32 + 1).product()
    };
    let mut boxes: Vec<(usize, usize)> = vec![(0, colors.len())];
    while boxes.len() < max_colors {
        let Some((bi, _)) = boxes
            .iter()
            .enumerate()
            .filter(|(_, (a, b))| b - a > 1)
            .max_by_key(|(_, (a, b))| volume(&colors[*a..*b]))
        else {
            break;
        };
        let (a, b) = boxes[bi];
        let slice = &mut colors[a..b];
        let (lo, hi) = bounds(slice);
        let dim = (0..3).max_by_key(|&k| hi[k] - lo[k]).unwrap_or(0);
        slice.sort_by_key(|(c, _)| c[dim]);
        let total: u64 = slice.iter().map(|(_, n)| *n as u64).sum();
        let mut acc = 0u64;
        let mut split = slice.len() / 2;
        for (i, (_, n)) in slice.iter().enumerate() {
            acc += *n as u64;
            if acc >= total / 2 {
                split = (i + 1).min(slice.len() - 1).max(1);
                break;
            }
        }
        boxes[bi] = (a, a + split);
        boxes.push((a + split, b));
    }
    let mut out: Vec<Swatch> = boxes.iter().map(|(a, b)| to_swatch(&colors[*a..*b])).collect();
    out.retain(|s| !palette_ignores(s.rgb));
    out
}

// ---------------------------------------------------------------------------
// Score (material-color-utilities `Score.score`, LCh in place of HCT)
// ---------------------------------------------------------------------------

const TARGET_CHROMA: f64 = 48.0;
const WEIGHT_PROPORTION: f64 = 0.7;
const WEIGHT_CHROMA_ABOVE: f64 = 0.3;
const WEIGHT_CHROMA_BELOW: f64 = 0.1;
const CUTOFF_CHROMA: f64 = 5.0;
const CUTOFF_EXCITED_PROPORTION: f64 = 0.01;

/// Rank swatches for use as a theme color (vivid + populous first, with hue
/// diversity). Returns up to `desired` colors; empty when everything is gray.
pub fn score(swatches: &[Swatch], desired: usize) -> Vec<[u8; 3]> {
    let total: f64 = swatches.iter().map(|s| s.population as f64).sum();
    if total <= 0.0 {
        return Vec::new();
    }
    let lchs: Vec<(f64, f64, f64)> = swatches.iter().map(|s| lch(s.rgb)).collect();
    let mut hue_pop = [0.0f64; 360];
    for (s, (_, _, h)) in swatches.iter().zip(&lchs) {
        hue_pop[(h.floor() as usize) % 360] += s.population as f64;
    }
    let mut excited = [0.0f64; 360];
    for hue in 0..360i32 {
        let p = hue_pop[hue as usize] / total;
        for i in (hue - 14)..(hue + 16) {
            excited[i.rem_euclid(360) as usize] += p;
        }
    }
    let mut scored: Vec<(f64, [u8; 3], f64)> = Vec::new();
    for (s, &(_, chroma, hue)) in swatches.iter().zip(&lchs) {
        let proportion = excited[(hue.round() as usize) % 360];
        if chroma < CUTOFF_CHROMA || proportion <= CUTOFF_EXCITED_PROPORTION {
            continue;
        }
        let proportion_score = proportion * 100.0 * WEIGHT_PROPORTION;
        let w = if chroma < TARGET_CHROMA { WEIGHT_CHROMA_BELOW } else { WEIGHT_CHROMA_ABOVE };
        scored.push((proportion_score + (chroma - TARGET_CHROMA) * w, s.rgb, hue));
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));

    let mut chosen: Vec<([u8; 3], f64)> = Vec::new();
    for diff in (15..=90).rev() {
        chosen.clear();
        for &(_, rgb, hue) in &scored {
            let far = chosen.iter().all(|(_, h)| {
                let d = (hue - h).abs();
                d.min(360.0 - d) >= diff as f64
            });
            if far {
                chosen.push((rgb, hue));
            }
            if chosen.len() >= desired {
                break;
            }
        }
        if chosen.len() >= desired {
            break;
        }
    }
    chosen.into_iter().map(|(c, _)| c).collect()
}

/// Extract the dominant "theme" accent from RGBA8 artwork pixels
/// (Palette maximumColorCount=8 + Score equivalent).
pub fn extract_accent(rgba: &[u8], width: u32, height: u32) -> Color32 {
    let swatches = palette(rgba, width, height, 8);
    match score(&swatches, 4).first() {
        Some(c) => readable_on_black(Color32::from_rgb(c[0], c[1], c[2])),
        None => DEFAULT_ACCENT,
    }
}

/// Two gradient colors for the frosted player background
/// (fallback `#595959` / `#0D0D0D`).
pub fn extract_gradient(rgba: &[u8], width: u32, height: u32) -> (Color32, Color32) {
    let swatches = palette(rgba, width, height, 64);
    let mut ranked: Vec<Color32> =
        score(&swatches, 2).into_iter().map(|c| Color32::from_rgb(c[0], c[1], c[2])).collect();
    if ranked.len() < 2 {
        return (Color32::from_rgb(0x59, 0x59, 0x59), Color32::from_rgb(0x0D, 0x0D, 0x0D));
    }
    ranked.sort_by(|a, b| relative_luminance(*b).total_cmp(&relative_luminance(*a)));
    (darken(ranked[0], 0.75, 0.16, 0.42), darken(ranked[1], 0.5, 0.07, 0.24))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        (0..w * h).flat_map(|_| [rgb[0], rgb[1], rgb[2], 255]).collect()
    }

    fn halves(w: u32, h: u32, left: [u8; 3], right: [u8; 3]) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for _y in 0..h {
            for x in 0..w {
                let c = if x < w / 2 { left } else { right };
                v.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        v
    }

    #[test]
    fn solid_red_is_reddish() {
        let img = solid(64, 64, [220, 20, 30]);
        let c = extract_accent(&img, 64, 64);
        assert!(c.r() > 150 && c.g() < 110 && c.b() < 110, "{c:?}");
    }

    #[test]
    fn gray_image_falls_back() {
        let img = solid(50, 40, [128, 128, 128]);
        assert_eq!(extract_accent(&img, 50, 40), DEFAULT_ACCENT);
        assert_eq!(
            extract_gradient(&img, 50, 40),
            (Color32::from_rgb(0x59, 0x59, 0x59), Color32::from_rgb(0x0D, 0x0D, 0x0D))
        );
    }

    #[test]
    fn noisy_gray_falls_back() {
        let mut img = Vec::new();
        for i in 0..(80 * 80) {
            let g = 40 + (i * 37 % 160) as u8;
            img.extend_from_slice(&[g, g.saturating_add((i % 2) as u8), g, 255]);
        }
        assert_eq!(extract_accent(&img, 80, 80), DEFAULT_ACCENT);
    }

    #[test]
    fn two_color_image() {
        let img = halves(100, 60, [200, 30, 40], [30, 60, 210]);
        let c = extract_accent(&img, 100, 60);
        let reddish = c.r() > c.g() && c.r() > c.b();
        let bluish = c.b() > c.r() && c.b() > c.g();
        assert!(reddish || bluish, "{c:?}");
        // Lifted so it reads on black.
        assert!(lab(c.r(), c.g(), c.b())[0] >= 57.0, "{c:?}");

        let (a, b) = extract_gradient(&img, 100, 60);
        let is_red = |c: Color32| c.r() > c.b();
        assert_ne!(is_red(a), is_red(b), "{a:?} {b:?}");
        assert!(relative_luminance(a) >= relative_luminance(b));
    }

    #[test]
    fn palette_caps_color_count() {
        let mut img = Vec::new();
        for y in 0..64u32 {
            for x in 0..64u32 {
                img.extend_from_slice(&[(x * 4) as u8, (y * 4) as u8, 128, 255]);
            }
        }
        let p = palette(&img, 64, 64, 8);
        assert!(!p.is_empty() && p.len() <= 8, "{}", p.len());
    }

    #[test]
    fn dark_blue_is_lifted() {
        let c = readable_on_black(Color32::from_rgb(10, 20, 90));
        assert!(lab(c.r(), c.g(), c.b())[0] >= 57.0);
        assert!(c.b() > c.r());
    }

    #[test]
    fn on_color_picks_contrast() {
        assert_eq!(on_color(Color32::WHITE), Color32::BLACK);
        assert_eq!(on_color(Color32::from_rgb(20, 20, 60)), Color32::WHITE);
    }
}
