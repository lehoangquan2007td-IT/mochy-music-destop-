//! Synced lyrics view (spec §7.7): auto-scrolling, active line highlight with
//! 300 ms lead-in, word-level karaoke fill when word timings exist,
//! click-a-line-to-seek, per-song offset.
//!
//! CONTRACT (used by the app shell — keep the signature stable).
//!
//! Look: Apple-Music-like. Big bold lines; the active line is bright and
//! slightly scaled; others dim with distance; the active line sits ~1/3 from
//! the top with a smooth, gently cascading scroll. Word-synced lines fill
//! left-to-right as they're sung and the words rise a little ("Style 1").
//! `{bg}` lines are smaller/dimmer, a second duet agent is right-aligned,
//! instrumental gaps (> 5 s) show animated "• • •". Mouse-wheel / drag
//! scrolling pauses auto-follow for 3 s. Only visible lines are painted; the
//! wrap layout is cached until the lyrics, width or font size change.

use std::hash::{Hash, Hasher};

use egui::emath::TSTransform;
use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Ui, pos2, vec2};
use mochy_lyrics::{LyricLine, Lyrics, SyncKind};

use super::theme;

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

/// Lines highlight this early. 0 = a line lights up exactly when it is
/// heard. The Android spec's 300 ms `LineLeadInMs` read ~0.3 s ahead of the
/// device clock and users reported "lyrics run ahead of the music"; the
/// per-song offset (±0.5 s buttons in Now Playing) does that job on demand.
pub const LEAD_IN_SECS: f64 = 0.0;
/// Gaps longer than this between sung words show the interlude dots.
pub const MIN_INTERLUDE_SECS: f64 = 5.0;
/// Manual scrolling pauses auto-follow this long.
const MANUAL_HOLD_SECS: f64 = 3.0;
/// Active line anchor, as a fraction of the view height from the top.
const ANCHOR: f32 = 0.33;

/// Index of the line active at `pos_secs` (no lead-in — a line activates
/// exactly when it is heard; positive `offset_ms` shows lines earlier).
/// Lines must be sorted by time.
pub fn active_line_index(lines: &[LyricLine], pos_secs: f64, offset_ms: i32) -> Option<usize> {
    let q = pos_secs + offset_ms as f64 / 1000.0 + LEAD_IN_SECS;
    lines.partition_point(|l| l.time <= q).checked_sub(1)
}

/// Text alignment of non-duet lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LyricsAlign {
    #[default]
    Left,
    Center,
}

/// Persistent UI state (scroll animation etc.). One instance per app.
pub struct LyricsView {
    /// Base font size of a lyric line (points).
    pub font_size: f32,
    /// Fake depth-of-field: lines far from the active one get a soft ghost.
    pub blur: bool,
    /// Alignment of primary-voice lines (second duet voice is always right).
    pub align: LyricsAlign,
    scroll: f32,
    manual_until: f64,
    snap_next: bool,
    layout: Option<LayoutCache>,
    last_pos: f64,
}

impl Default for LyricsView {
    fn default() -> Self {
        Self {
            font_size: 30.0,
            blur: true,
            align: LyricsAlign::Left,
            scroll: 0.0,
            manual_until: f64::NEG_INFINITY,
            snap_next: true,
            layout: None,
            last_pos: f64::NAN,
        }
    }
}

#[derive(Debug, Clone)]
struct Token {
    text: String,
    x: f32,
    row: u32,
    w: f32,
    /// Word timing (seconds) when word-synced.
    timing: Option<(f64, f64)>,
}

#[derive(Debug, Clone)]
enum ItemKind {
    Line(usize),
    Gap { start: f64, end: f64 },
}

#[derive(Debug, Clone)]
struct Item {
    kind: ItemKind,
    y: f32,
    h: f32,
    right: bool,
    /// 1.0 for main vocals, smaller for `{bg}`.
    scale: f32,
    tokens: Vec<Token>,
    row_h: f32,
    width: f32,
}

struct LayoutCache {
    key: u64,
    items: Vec<Item>,
    line_item: Vec<usize>,
    total_h: f32,
}

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
        match state {
            LyricsState::Idle => {
                self.reset();
                None
            }
            LyricsState::Loading => {
                self.reset();
                centered_message(ui, None, true, accent);
                None
            }
            LyricsState::NotFound => {
                self.reset();
                centered_message(ui, Some(("No lyrics for this song", "Enjoy the music instead ♪")), false, accent);
                None
            }
            LyricsState::Error(e) => {
                self.reset();
                centered_message(ui, Some(("Couldn't load lyrics", e.as_str())), false, accent);
                None
            }
            LyricsState::Loaded(lyrics) => {
                let synced = matches!(lyrics.sync, SyncKind::Line | SyncKind::Word) && !lyrics.lines.is_empty();
                if synced {
                    self.synced(ui, lyrics, pos_secs, offset_ms, accent)
                } else {
                    self.reset();
                    plain(ui, lyrics, self.font_size);
                    None
                }
            }
        }
    }

    /// Forget scroll state (next lyrics snap into place instead of animating).
    pub fn reset(&mut self) {
        self.snap_next = true;
        self.layout = None;
        self.manual_until = f64::NEG_INFINITY;
    }

    fn synced(&mut self, ui: &mut Ui, lyrics: &Lyrics, pos: f64, offset_ms: i32, accent: Color32) -> Option<f64> {
        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        if rect.width() < 40.0 || rect.height() < 40.0 {
            return None;
        }
        let ctx = ui.ctx().clone();
        let now = ui.input(|i| i.time);
        let dt = ui.input(|i| i.stable_dt).clamp(0.0, 0.1);
        let font_size = self.font_size.clamp(12.0, 96.0);
        let pad = (font_size * 0.8).min(rect.width() * 0.06).max(8.0);
        let content_w = (rect.width() - pad * 2.0).max(40.0);

        let painter = ui.painter_at(rect);
        self.ensure_layout(&painter, lyrics, content_w, font_size);
        let layout = self.layout.as_ref()?;
        if layout.items.is_empty() {
            return None;
        }

        // ---------------- timing ----------------
        let t = pos + offset_ms as f64 / 1000.0; // song time for word fill
        let q = t + LEAD_IN_SECS; // line activation time
        let active_line = active_line_index(&lyrics.lines, pos, offset_ms);
        let active_gap = layout.items.iter().position(|it| match it.kind {
            ItemKind::Gap { start, end } => q >= start && q < end,
            _ => false,
        });
        let follow_item = active_gap
            .or_else(|| active_line.and_then(|i| layout.line_item.get(i).copied()))
            .unwrap_or(0);
        let highlighted = |idx: usize| -> bool {
            if active_gap.is_some() {
                return false;
            }
            let ItemKind::Line(li) = layout.items[idx].kind else { return false };
            if Some(li) == active_line {
                return true;
            }
            // Overlapping word-synced lines (bg vocals, duets) light up while sung.
            let line = &lyrics.lines[li];
            match (line.words.first(), line.words.last()) {
                (Some(a), Some(b)) => q >= a.start && t <= b.end + 0.1 && line.time <= q,
                _ => false,
            }
        };

        // ---------------- scrolling ----------------
        let view_h = rect.height();
        let fi = &layout.items[follow_item];
        let target = fi.y + fi.h / 2.0 - view_h * ANCHOR;
        let min_scroll = -view_h * ANCHOR;
        let max_scroll = (layout.total_h - view_h * ANCHOR).max(min_scroll);

        let mut manual_input = false;
        if resp.hovered() {
            let wheel = ui.input(|i| i.smooth_scroll_delta.y);
            if wheel != 0.0 {
                self.scroll -= wheel;
                manual_input = true;
                ui.input_mut(|i| i.smooth_scroll_delta.y = 0.0);
            }
        }
        if resp.dragged() {
            self.scroll -= resp.drag_delta().y;
            manual_input = true;
        }
        if manual_input {
            self.manual_until = now + MANUAL_HOLD_SECS;
            self.scroll = self.scroll.clamp(min_scroll, max_scroll);
        }
        let manual = now < self.manual_until;
        let prev_scroll = self.scroll;
        if self.snap_next {
            self.scroll = target;
            self.snap_next = false;
        } else if !manual {
            let k = 1.0 - (-dt * 6.5).exp();
            self.scroll += (target - self.scroll) * k;
            if (target - self.scroll).abs() < 0.25 {
                self.scroll = target;
            }
        }
        let remaining = target - self.scroll;
        let animating = (self.scroll - prev_scroll).abs() > 0.01 || remaining.abs() > 0.25;

        // ---------------- painting ----------------
        let pointer = resp.hover_pos();
        let mut clicked_time = None;
        let click_pos = if resp.clicked() { resp.interact_pointer_pos() } else { None };
        let time = now as f32;
        let fade_zone = (view_h * 0.12).clamp(24.0, 80.0);
        let font_bold = |size: f32| theme::bold(&ctx, size);
        // Glows (held notes, interlude dots) are tinted with the accent.
        let glow_tint = theme::readable_on_black(accent).lerp_to_gamma(Color32::WHITE, 0.35);
        let mut needs_repaint = animating;

        // First item that may be visible (items are sorted by y).
        let margin = view_h * 0.5;
        let first = layout.items.partition_point(|it| it.y + it.h < self.scroll - margin);
        for idx in first..layout.items.len() {
            let it = &layout.items[idx];
            let d_items = idx as i64 - follow_item as i64;
            // Cascade: items below the followed one lag a little behind.
            let lag = if manual { 0.0 } else { (d_items.max(0) as f32 * 0.06).min(0.4) };
            let item_scroll = self.scroll - remaining * lag;
            let top = rect.top() + it.y - item_scroll;
            if top > rect.bottom() + margin * 0.2 {
                break;
            }
            if top + it.h < rect.top() - 4.0 {
                continue;
            }
            let item_rect = Rect::from_min_size(pos2(rect.left() + pad, top), vec2(content_w, it.h));
            // Edge fade.
            let edge = ((item_rect.center().y - rect.top()) / fade_zone)
                .min((rect.bottom() - item_rect.center().y) / fade_zone)
                .clamp(0.0, 1.0);
            let edge = edge * edge * (3.0 - 2.0 * edge);
            if edge <= 0.01 {
                continue;
            }

            match it.kind {
                ItemKind::Gap { start, end } => {
                    let active = active_gap == Some(idx);
                    let progress = if active { ((q - start) / (end - start).max(0.01)) as f32 } else { 0.0 };
                    let show = ctx.animate_bool_with_time(resp.id.with(("gap", idx)), active, 0.35);
                    if show > 0.01 {
                        paint_dots(&painter, item_rect, it, progress, show * edge, time, font_size, self.align, glow_tint);
                        needs_repaint |= active || show < 0.99;
                    }
                }
                ItemKind::Line(li) => {
                    let line = &lyrics.lines[li];
                    let hl = highlighted(idx);
                    let h_anim = ctx.animate_bool_with_time(resp.id.with(("hl", idx)), hl, 0.28);
                    if h_anim > 0.0 && h_anim < 1.0 {
                        needs_repaint = true;
                    }
                    let dist = d_items.unsigned_abs() as f32;
                    let base_alpha = if manual { 0.55 } else { (0.40 - 0.05 * dist.min(5.0)).max(0.16) };
                    let bg_factor = if line.bg { 0.75 } else { 1.0 };
                    let dim_alpha = base_alpha * bg_factor * edge;
                    let bright_alpha = bg_factor * edge;
                    let scale = 1.0 + 0.045 * h_anim;

                    let hovered = pointer.is_some_and(|p| item_rect.expand2(vec2(pad * 0.6, 4.0)).contains(p));
                    if hovered {
                        painter.rect_filled(
                            item_rect.expand2(vec2(pad * 0.5, 6.0)),
                            CornerRadius::same(14),
                            Color32::from_white_alpha(10),
                        );
                        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if let Some(p) = click_pos
                        && item_rect.expand2(vec2(pad * 0.6, 4.0)).contains(p)
                    {
                        clicked_time = Some(line.time);
                    }

                    // Anchor for the scale: the aligned edge.
                    let anchor_x = if it.right {
                        item_rect.right()
                    } else if self.align == LyricsAlign::Center {
                        item_rect.center().x
                    } else {
                        item_rect.left()
                    };
                    let anchor = pos2(anchor_x, item_rect.center().y);
                    let xf = TSTransform::new(anchor.to_vec2() * (1.0 - scale), scale);

                    let size = font_size * it.scale;
                    let font = font_bold(size);
                    let rise_px = size * 0.07;
                    let row_offset = |row: u32| -> f32 {
                        let row_w = it.tokens.iter().filter(|t| t.row == row).map(|t| t.x + t.w).fold(0.0, f32::max);
                        if it.right {
                            it.width - row_w
                        } else if self.align == LyricsAlign::Center {
                            (it.width - row_w) / 2.0
                        } else {
                            0.0
                        }
                    };
                    let mut row_offsets: Vec<f32> = Vec::new();

                    for tok in &it.tokens {
                        while row_offsets.len() <= tok.row as usize {
                            row_offsets.push(row_offset(row_offsets.len() as u32));
                        }
                        let galley = painter.layout_no_wrap(tok.text.clone(), font.clone(), Color32::PLACEHOLDER);
                        let origin = pos2(
                            item_rect.left() + row_offsets[tok.row as usize] + tok.x,
                            item_rect.top() + tok.row as f32 * it.row_h + (it.row_h - galley.size().y) / 2.0,
                        );
                        // Karaoke progress of this token.
                        let p = match tok.timing {
                            Some((s, e)) => ((t - s) / (e - s).max(0.01)).clamp(0.0, 1.0) as f32,
                            None => 1.0,
                        };
                        let rise = if tok.timing.is_some() { rise_px * ease_out(p) * h_anim } else { 0.0 };
                        let origin = origin - vec2(0.0, rise);

                        let dim_col = Color32::from_white_alpha(alpha_u8(dim_alpha + (0.38 * bright_alpha - dim_alpha).max(0.0) * h_anim));
                        let lit_col = Color32::from_white_alpha(alpha_u8(bright_alpha));

                        // Soft ghost for far-away lines (cheap stand-in for blur).
                        if self.blur && !manual && dist >= 2.0 && !hl {
                            let r = (dist - 1.0).min(4.0) * 0.6;
                            let ghost = Color32::from_white_alpha(alpha_u8(dim_alpha * 0.22));
                            for off in [vec2(r, 0.0), vec2(-r, 0.0), vec2(0.0, r), vec2(0.0, -r)] {
                                let mut s = Shape::galley(origin + off, galley.clone(), ghost);
                                s.transform(xf);
                                painter.add(s);
                            }
                        }

                        if h_anim <= 0.001 || p <= 0.001 {
                            let mut s = Shape::galley(origin, galley, dim_col);
                            s.transform(xf);
                            painter.add(s);
                            continue;
                        }
                        if tok.timing.is_none() || p >= 0.999 {
                            // Whole token lit (line-synced line, or a sung word).
                            let col = dim_col.lerp_to_gamma(lit_col, h_anim);
                            let mut s = Shape::galley(origin, galley, col);
                            s.transform(xf);
                            painter.add(s);
                            continue;
                        }
                        // Partially sung word: dim base + lit part clipped left-to-right,
                        // with a short feathered edge.
                        let g_rect = Rect::from_min_size(origin, galley.size());
                        let split = g_rect.left() + g_rect.width() * p;
                        let mut base = Shape::galley(origin, galley.clone(), dim_col);
                        base.transform(xf);
                        painter.add(base);
                        let lit_rect = Rect::from_min_max(pos2(g_rect.left() - 2.0, g_rect.top() - 4.0), pos2(split, g_rect.bottom() + 4.0));
                        let mut lit = Shape::galley(origin, galley.clone(), lit_col);
                        lit.transform(xf);
                        painter.with_clip_rect(xf * lit_rect).add(lit);
                        for (k, a) in [(0.0f32, 0.6f32), (3.0, 0.3)] {
                            let feather = Rect::from_min_max(pos2(split + k, g_rect.top() - 4.0), pos2(split + k + 3.0, g_rect.bottom() + 4.0));
                            let mut f = Shape::galley(origin, galley.clone(), Color32::from_white_alpha(alpha_u8(bright_alpha * a)));
                            f.transform(xf);
                            painter.with_clip_rect((xf * feather).intersect(rect)).add(f);
                        }
                        // Held note: soft glow while it lasts.
                        if let Some((s, e)) = tok.timing
                            && e - s > 1.0
                        {
                            let glow = glow_tint.gamma_multiply(0.16 * bright_alpha * (1.0 - (p - 0.5).abs()));
                            for off in [vec2(1.5, 0.0), vec2(-1.5, 0.0), vec2(0.0, 1.5), vec2(0.0, -1.5)] {
                                let mut gs = Shape::galley(origin + off, galley.clone(), glow);
                                gs.transform(xf);
                                painter.add(gs);
                            }
                        }
                        needs_repaint = true;
                    }
                }
            }
        }

        // Provider caption after the last line.
        if !lyrics.provider.is_empty() {
            let y = rect.top() + layout.total_h - self.scroll + font_size * 0.6;
            if y < rect.bottom() && y > rect.top() {
                let (x, align) = match self.align {
                    LyricsAlign::Left => (rect.left() + pad, Align2::LEFT_TOP),
                    LyricsAlign::Center => (rect.center().x, Align2::CENTER_TOP),
                };
                painter.text(
                    pos2(x, y),
                    align,
                    format!("Lyrics from {}", lyrics.provider),
                    FontId::proportional(13.0),
                    theme::TEXT_SECONDARY,
                );
            }
        }

        if manual {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64((self.manual_until - now).max(0.0) + 0.01));
        }
        if needs_repaint || pos != self.last_pos {
            ctx.request_repaint();
        }
        self.last_pos = pos;
        clicked_time
    }

    fn ensure_layout(&mut self, painter: &egui::Painter, lyrics: &Lyrics, width: f32, font_size: f32) {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        lyrics.provider.hash(&mut h);
        (lyrics.sync as u8).hash(&mut h);
        lyrics.lines.len().hash(&mut h);
        for l in &lyrics.lines {
            l.time.to_bits().hash(&mut h);
            l.text.hash(&mut h);
            l.bg.hash(&mut h);
            l.agent.hash(&mut h);
            for w in &l.words {
                w.start.to_bits().hash(&mut h);
                w.end.to_bits().hash(&mut h);
                w.text.hash(&mut h);
            }
        }
        (width.round() as i32).hash(&mut h);
        font_size.to_bits().hash(&mut h);
        painter.pixels_per_point().to_bits().hash(&mut h);
        theme::family(painter.ctx(), theme::FONT_BOLD).hash(&mut h);
        let key = h.finish();
        if self.layout.as_ref().is_some_and(|l| l.key == key) {
            return;
        }
        let had_layout = self.layout.is_some();
        self.layout = Some(build_layout(painter, lyrics, width, font_size, key));
        if !had_layout {
            self.snap_next = true;
        }
    }
}

fn alpha_u8(a: f32) -> u8 {
    (a.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn ease_out(p: f32) -> f32 {
    1.0 - (1.0 - p).powi(3)
}

/// Agent ids that mean "everyone" rather than a second voice.
fn is_group_agent(a: &str) -> bool {
    let a = a.to_ascii_lowercase();
    a == "v1000" || a == "group" || a == "both" || a == "all"
}

fn build_layout(painter: &egui::Painter, lyrics: &Lyrics, width: f32, font_size: f32, key: u64) -> LayoutCache {
    let ctx = painter.ctx().clone();
    let lines = &lyrics.lines;
    let primary_agent = lines.iter().filter(|l| !l.bg).find_map(|l| l.agent.clone());
    let mut items: Vec<Item> = Vec::new();
    let mut line_item = vec![0usize; lines.len()];
    let gap_h = font_size * 1.3;
    let spacing = font_size * 0.75;
    let mut y = 0.0f32;
    let mut last_right = false;

    let push_gap = |items: &mut Vec<Item>, y: &mut f32, start: f64, end: f64, right: bool| {
        items.push(Item {
            kind: ItemKind::Gap { start, end },
            y: *y,
            h: gap_h,
            right,
            scale: 1.0,
            tokens: Vec::new(),
            row_h: gap_h,
            width,
        });
        *y += gap_h + spacing;
    };

    // Intro.
    if let Some(first) = lines.first()
        && first.time > MIN_INTERLUDE_SECS
    {
        push_gap(&mut items, &mut y, 0.0, first.time, false);
    }

    for (li, line) in lines.iter().enumerate() {
        let right = if line.bg {
            last_right
        } else {
            match (&line.agent, &primary_agent) {
                (Some(a), Some(p)) => a != p && !is_group_agent(a),
                _ => false,
            }
        };
        if !line.bg {
            last_right = right;
        }
        let next_time = lines.get(li + 1).map(|l| l.time);

        if line.text.trim().is_empty() && line.words.is_empty() {
            // Empty LRC line = instrumental break until the next line.
            match next_time {
                Some(nt) if nt - line.time > 1.0 => {
                    line_item[li] = items.len();
                    push_gap(&mut items, &mut y, line.time, nt, right);
                }
                _ => {
                    line_item[li] = items.len().saturating_sub(1);
                }
            }
            continue;
        }

        let scale = if line.bg { 0.68 } else { 1.0 };
        let size = font_size * scale;
        let font = theme::bold(&ctx, size);
        // Keep room for the active-line scale-up.
        let max_w = width / 1.05;
        let tokens = layout_tokens(painter, line, &font, max_w);
        let row_h = painter.fonts_mut(|f| f.row_height(&font)) * 1.12;
        let rows = tokens.iter().map(|t| t.row + 1).max().unwrap_or(1);
        let h = rows as f32 * row_h;
        line_item[li] = items.len();
        items.push(Item { kind: ItemKind::Line(li), y, h, right, scale, tokens, row_h, width: max_w });
        y += h + if line.bg { spacing * 0.5 } else { spacing };

        // Interlude after a word-synced line.
        if let (Some(last), Some(nt)) = (line.words.last(), next_time)
            && nt - last.end > MIN_INTERLUDE_SECS
        {
            let next_right = lines.get(li + 1).is_some_and(|n| match (&n.agent, &primary_agent) {
                (Some(a), Some(p)) => a != p && !is_group_agent(a),
                _ => false,
            });
            push_gap(&mut items, &mut y, last.end, nt, next_right);
        }
    }
    let total_h = (y - spacing).max(0.0);
    LayoutCache { key, items, line_item, total_h }
}

/// Optional (start, end) word timing in seconds.
type Timing = Option<(f64, f64)>;

/// Split a line into measured tokens and flow them into rows of `max_w`.
fn layout_tokens(painter: &egui::Painter, line: &LyricLine, font: &FontId, max_w: f32) -> Vec<Token> {
    let measure = |s: &str| painter.layout_no_wrap(s.to_owned(), font.clone(), Color32::PLACEHOLDER).size().x;
    let space_w = (measure("a a") - measure("aa")).max(font.size * 0.25);

    // (text, timing, space_after)
    let mut raw: Vec<(String, Timing, bool)> = Vec::new();
    if line.words.is_empty() {
        let words: Vec<&str> = line.text.split_whitespace().collect();
        for w in words {
            raw.push((w.to_owned(), None, true));
        }
    } else {
        let text = line.text.as_str();
        let mut cursor = 0usize;
        for (k, w) in line.words.iter().enumerate() {
            let trimmed = w.text.trim();
            if trimmed.is_empty() {
                if let Some(last) = raw.last_mut() {
                    last.2 = true;
                }
                continue;
            }
            let mut space = w.text.ends_with(char::is_whitespace)
                || line.words.get(k + 1).is_some_and(|n| n.text.starts_with(char::is_whitespace));
            if let Some(found) = text.get(cursor..).and_then(|s| s.find(trimmed)) {
                cursor += found + trimmed.len();
                space |= text[cursor..].starts_with(char::is_whitespace);
            }
            raw.push((trimmed.to_owned(), Some((w.start, w.end.max(w.start))), space));
        }
    }

    // Tokens wider than a whole row are split into characters (CJK lines
    // without spaces), sharing the word's time span proportionally.
    let mut toks: Vec<(String, f32, Timing, bool)> = Vec::new();
    for (text, timing, space) in raw {
        let w = measure(&text);
        if w <= max_w {
            toks.push((text, w, timing, space));
            continue;
        }
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len().max(1);
        for (i, c) in chars.iter().enumerate() {
            let sub = timing.map(|(s, e)| {
                let d = (e - s) / n as f64;
                (s + d * i as f64, s + d * (i + 1) as f64)
            });
            let cs = c.to_string();
            let cw = measure(&cs);
            toks.push((cs, cw, sub, space && i + 1 == n));
        }
    }

    // Flow: break only between "glued" groups (tokens without a space between
    // them stay together, e.g. syllables of one word) unless a group alone
    // is too wide.
    let mut out = Vec::with_capacity(toks.len());
    let mut x = 0.0f32;
    let mut row = 0u32;
    let mut i = 0;
    while i < toks.len() {
        let mut j = i;
        let mut group_w = 0.0;
        while j < toks.len() {
            group_w += toks[j].1;
            j += 1;
            if toks[j - 1].3 {
                break;
            }
        }
        if x > 0.0 && x + group_w > max_w {
            row += 1;
            x = 0.0;
        }
        for (text, w, timing, space) in toks[i..j].iter().cloned() {
            if x > 0.0 && x + w > max_w {
                row += 1;
                x = 0.0;
            }
            out.push(Token { text, x, row, w, timing });
            x += w;
            if space {
                x += space_w;
            }
        }
        i = j;
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn paint_dots(
    painter: &egui::Painter,
    rect: Rect,
    it: &Item,
    progress: f32,
    alpha: f32,
    time: f32,
    font_size: f32,
    align: LyricsAlign,
    glow: Color32,
) {
    let r = (font_size * 0.17).max(3.0);
    let gap = r * 1.2;
    let total_w = 3.0 * r * 2.0 + 2.0 * gap;
    let left = if it.right {
        rect.right() - total_w
    } else if align == LyricsAlign::Center {
        rect.center().x - total_w / 2.0
    } else {
        rect.left()
    };
    let cy = rect.center().y;
    // Finale: in the last 12 % all dots swell together as a cue.
    let finale = ((progress - 0.88) / 0.12).clamp(0.0, 1.0);
    let finale_scale = 1.0 + 0.25 * (finale * std::f32::consts::PI).sin();
    for i in 0..3 {
        let slice = (progress * 3.0 - i as f32) / 0.55;
        let lit = slice.clamp(0.0, 1.0);
        let bob = (time * std::f32::consts::TAU / 2.6 - i as f32 * 0.9).sin();
        let breathe = 1.0 + 0.08 * bob;
        let rr = r * breathe * finale_scale * (0.75 + 0.25 * alpha);
        let c = Pos2::new(left + r + i as f32 * (2.0 * r + gap), cy - bob * 1.8);
        let a = (0.3 + 0.7 * lit) * alpha;
        if lit > 0.0 {
            painter.circle_filled(c, rr * 1.9, glow.gamma_multiply(0.14 * lit * alpha));
        }
        painter.circle_filled(c, rr, Color32::from_white_alpha(alpha_u8(a)));
    }
}

fn centered_message(ui: &mut Ui, text: Option<(&str, &str)>, spinner: bool, accent: Color32) {
    let rect = ui.available_rect_before_wrap();
    ui.allocate_rect(rect, Sense::hover());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Center)));
    child.add_space((rect.height() * 0.38 - 30.0).max(0.0));
    if spinner {
        child.add(egui::Spinner::new().size(34.0).color(theme::readable_on_black(accent)));
        child.add_space(10.0);
        child.label(egui::RichText::new("Loading lyrics…").color(theme::TEXT_SECONDARY));
    }
    if let Some((title, sub)) = text {
        child.label(egui::RichText::new(title).font(theme::bold(ui.ctx(), 22.0)).color(Color32::WHITE));
        child.add_space(2.0);
        child.label(egui::RichText::new(sub).color(theme::TEXT_SECONDARY));
    }
}

/// Unsynced lyrics: static scrollable text.
fn plain(ui: &mut Ui, lyrics: &Lyrics, font_size: f32) {
    let text: String = match &lyrics.plain {
        Some(p) if !p.trim().is_empty() => p.clone(),
        _ => lyrics.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n"),
    };
    if text.trim().is_empty() {
        centered_message(ui, Some(("No lyrics for this song", "")), false, Color32::WHITE);
        return;
    }
    let font = theme::semibold(ui.ctx(), (font_size * 0.75).max(14.0));
    egui::ScrollArea::vertical().id_salt("mochy_plain_lyrics").auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width() - 20.0);
                ui.spacing_mut().item_spacing.y = font.size * 0.45;
                for line in text.lines() {
                    if line.trim().is_empty() {
                        ui.add_space(font.size * 0.6);
                    } else {
                        ui.label(egui::RichText::new(line).font(font.clone()).color(Color32::from_white_alpha(225)));
                    }
                }
                if !lyrics.provider.is_empty() {
                    ui.add_space(16.0);
                    ui.label(egui::RichText::new(format!("Lyrics from {}", lyrics.provider)).small().color(theme::TEXT_SECONDARY));
                }
                ui.add_space(24.0);
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use mochy_lyrics::LyricWord;

    fn line(t: f64, text: &str) -> LyricLine {
        LyricLine { time: t, text: text.into(), ..Default::default() }
    }

    #[test]
    fn active_line_lead_in_and_offset() {
        let lines = vec![line(1.0, "a"), line(5.0, "b"), line(9.0, "c")];
        assert_eq!(active_line_index(&lines, 0.0, 0), None);
        assert_eq!(active_line_index(&lines, 0.99, 0), None);
        assert_eq!(active_line_index(&lines, 1.01, 0), Some(0)); // activates exactly at its timestamp
        assert_eq!(active_line_index(&lines, 4.99, 0), Some(0));
        assert_eq!(active_line_index(&lines, 5.01, 0), Some(1));
        assert_eq!(active_line_index(&lines, 100.0, 0), Some(2));
        // Positive offset shows lines earlier.
        assert_eq!(active_line_index(&lines, 4.0, 1000), Some(1));
        // Negative offset delays them.
        assert_eq!(active_line_index(&lines, 5.0, -1000), Some(0));
        assert_eq!(active_line_index(&[], 3.0, 0), None);
    }

    fn word_lyrics() -> Lyrics {
        let mk = |t: f64, words: &[(&str, f64, f64)], bg: bool, agent: &str| LyricLine {
            time: t,
            text: words.iter().map(|w| w.0).collect::<Vec<_>>().join(" "),
            words: words.iter().map(|(w, s, e)| LyricWord { start: *s, end: *e, text: (*w).into() }).collect(),
            bg,
            agent: Some(agent.into()),
        };
        Lyrics {
            provider: "Test".into(),
            sync: SyncKind::Word,
            lines: vec![
                mk(6.0, &[("Hello", 6.0, 6.5), ("world", 6.5, 7.5)], false, "v1"),
                mk(7.6, &[("echo", 7.6, 8.0)], true, "v1"),
                mk(8.0, &[("second", 8.0, 9.0), ("voice", 9.0, 9.5)], false, "v2"),
                mk(20.0, &[("after", 20.0, 21.0), ("break", 21.0, 22.0)], false, "v1"),
            ],
            plain: None,
        }
    }

    #[test]
    fn layout_marks_gaps_duets_and_bg() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(Default::default(), |ui| {
            let painter = ui.painter().clone();
            let l = build_layout(&painter, &word_lyrics(), 600.0, 30.0, 0);
            // intro gap, 4 lines, interlude between 9.5 s and 20 s.
            let gaps: Vec<_> = l.items.iter().filter(|i| matches!(i.kind, ItemKind::Gap { .. })).collect();
            assert_eq!(gaps.len(), 2);
            assert!(matches!(l.items[0].kind, ItemKind::Gap { start, end } if start == 0.0 && end == 6.0));
            let line_items: Vec<&Item> = l.line_item.iter().map(|&i| &l.items[i]).collect();
            assert!(!line_items[0].right);
            assert!(line_items[1].scale < 1.0 && !line_items[1].right);
            assert!(line_items[2].right);
            assert!(!line_items[3].right);
            // Sorted by y, monotonically.
            assert!(l.items.windows(2).all(|w| w[0].y < w[1].y));
            assert!(l.total_h > 0.0);
        });
        out.textures_delta.clear();
    }

    #[test]
    fn long_lines_wrap() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(Default::default(), |ui| {
            let painter = ui.painter().clone();
            let font = FontId::proportional(30.0);
            let l = line(0.0, "this is a rather long lyric line that will certainly need to wrap");
            let toks = layout_tokens(&painter, &l, &font, 200.0);
            assert!(toks.iter().map(|t| t.row).max().unwrap() >= 2);
            assert!(toks.iter().all(|t| t.x + t.w <= 200.5 || t.x == 0.0));
            // A single huge token is split into characters.
            let l2 = line(0.0, "あいうえおかきくけこさしすせそたちつてと");
            let toks2 = layout_tokens(&painter, &l2, &font, 120.0);
            assert!(toks2.len() > 1);
        });
        out.textures_delta.clear();
    }

    #[test]
    fn show_states_do_not_panic() {
        let ctx = egui::Context::default();
        let mut view = LyricsView::default();
        let states = [
            LyricsState::Idle,
            LyricsState::Loading,
            LyricsState::NotFound,
            LyricsState::Error("boom".into()),
            LyricsState::Loaded(word_lyrics()),
            LyricsState::Loaded(Lyrics { sync: SyncKind::Plain, plain: Some("a\nb".into()), ..Default::default() }),
        ];
        for s in &states {
            for k in 0..3 {
                let mut out = ctx.run_ui(Default::default(), |ui| {
                    let _ = view.show(ui, s, 5.0 + k as f64 * 1.7, 0, Color32::RED);
                });
                out.textures_delta.clear();
            }
        }
    }
}
