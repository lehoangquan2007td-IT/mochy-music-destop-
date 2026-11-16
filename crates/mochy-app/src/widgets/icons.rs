//! Vector icons painted with egui shapes (no icon font dependency).

use egui::{Color32, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2, pos2, vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    Prev,
    Next,
    Shuffle,
    Repeat,
    RepeatOne,
    Heart,
    HeartFilled,
    Volume,
    VolumeMute,
    Lyrics,
    Queue,
    Expand,
    Collapse,
    Back,
    Forward,
    Home,
    Search,
    Library,
    Settings,
    AudioLab,
    Plus,
    More,
    Close,
    Note,
    Folder,
    Trash,
    Radio,
    DragHandle,
    Playlist,
    // ---- library/explore/stats ----
    Compass,
    Chart,
    Grid,
    ListView,
    Bookmark,
    BookmarkFilled,
}

/// Map unit coordinates (0..1) into `r`.
fn p(r: Rect, x: f32, y: f32) -> Pos2 {
    pos2(r.left() + x * r.width(), r.top() + y * r.height())
}

pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    // Square, centered.
    let s = rect.width().min(rect.height());
    let r = Rect::from_center_size(rect.center(), Vec2::splat(s));
    let w = (s * 0.085).max(1.4);
    let st = Stroke::new(w, color);
    let line = |pts: Vec<Pos2>| painter.add(Shape::line(pts, st));
    let seg = |a: Pos2, b: Pos2| painter.line_segment([a, b], st);
    match icon {
        Icon::Play => {
            painter.add(Shape::convex_polygon(
                vec![p(r, 0.30, 0.18), p(r, 0.84, 0.5), p(r, 0.30, 0.82)],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Pause => {
            let cr = s * 0.04;
            painter.rect_filled(Rect::from_min_max(p(r, 0.24, 0.18), p(r, 0.42, 0.82)), cr, color);
            painter.rect_filled(Rect::from_min_max(p(r, 0.58, 0.18), p(r, 0.76, 0.82)), cr, color);
        }
        Icon::Prev | Icon::Next => {
            let flip = icon == Icon::Next;
            let x = |v: f32| if flip { 1.0 - v } else { v };
            painter.rect_filled(Rect::from_two_pos(p(r, x(0.18), 0.22), p(r, x(0.28), 0.78)), s * 0.03, color);
            painter.add(Shape::convex_polygon(
                if flip {
                    vec![p(r, x(0.80), 0.20), p(r, x(0.30), 0.5), p(r, x(0.80), 0.80)]
                } else {
                    vec![p(r, x(0.80), 0.20), p(r, x(0.80), 0.80), p(r, x(0.30), 0.5)]
                },
                color,
                Stroke::NONE,
            ));
        }
        Icon::Shuffle => {
            line(vec![p(r, 0.12, 0.30), p(r, 0.32, 0.30), p(r, 0.62, 0.70), p(r, 0.84, 0.70)]);
            line(vec![p(r, 0.12, 0.70), p(r, 0.32, 0.70), p(r, 0.62, 0.30), p(r, 0.84, 0.30)]);
            line(vec![p(r, 0.74, 0.20), p(r, 0.86, 0.30), p(r, 0.74, 0.40)]);
            line(vec![p(r, 0.74, 0.60), p(r, 0.86, 0.70), p(r, 0.74, 0.80)]);
        }
        Icon::Repeat | Icon::RepeatOne => {
            line(vec![p(r, 0.22, 0.56), p(r, 0.22, 0.32), p(r, 0.80, 0.32)]);
            line(vec![p(r, 0.78, 0.44), p(r, 0.78, 0.68), p(r, 0.20, 0.68)]);
            line(vec![p(r, 0.70, 0.22), p(r, 0.81, 0.32), p(r, 0.70, 0.42)]);
            line(vec![p(r, 0.30, 0.58), p(r, 0.19, 0.68), p(r, 0.30, 0.78)]);
            if icon == Icon::RepeatOne {
                seg(p(r, 0.50, 0.40), p(r, 0.50, 0.60));
                seg(p(r, 0.44, 0.45), p(r, 0.50, 0.40));
            }
        }
        Icon::Heart | Icon::HeartFilled => {
            let pts: Vec<Pos2> = (0..48)
                .map(|i| {
                    let t = i as f32 / 48.0 * std::f32::consts::TAU;
                    let x = 16.0 * t.sin().powi(3);
                    let y = 13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos();
                    p(r, 0.5 + x / 38.0, 0.47 - y / 38.0)
                })
                .collect();
            if icon == Icon::HeartFilled {
                // Non-convex: fill as a triangle fan from an interior point.
                let c = p(r, 0.5, 0.52);
                let mut mesh = egui::Mesh::default();
                mesh.colored_vertex(c, color);
                for q in &pts {
                    mesh.colored_vertex(*q, color);
                }
                let n = pts.len() as u32;
                for i in 0..n {
                    mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
                }
                painter.add(Shape::mesh(mesh));
            }
            painter.add(Shape::closed_line(pts, st));
        }
        Icon::Volume | Icon::VolumeMute => {
            painter.add(Shape::convex_polygon(
                vec![p(r, 0.14, 0.38), p(r, 0.30, 0.38), p(r, 0.50, 0.20), p(r, 0.50, 0.80), p(r, 0.30, 0.62), p(r, 0.14, 0.62)],
                color,
                Stroke::NONE,
            ));
            if icon == Icon::VolumeMute {
                seg(p(r, 0.62, 0.38), p(r, 0.86, 0.62));
                seg(p(r, 0.86, 0.38), p(r, 0.62, 0.62));
            } else {
                for (rad, n) in [(0.16f32, 10), (0.30, 14)] {
                    let c = p(r, 0.50, 0.5);
                    let pts: Vec<Pos2> = (0..=n)
                        .map(|i| {
                            let a = -0.85 + 1.7 * i as f32 / n as f32;
                            c + vec2(a.cos(), a.sin()) * rad * s
                        })
                        .collect();
                    line(pts);
                }
            }
        }
        Icon::Lyrics => {
            // speech bubble with two text lines
            let b = Rect::from_min_max(p(r, 0.14, 0.18), p(r, 0.86, 0.70));
            painter.rect_stroke(b, s * 0.12, st, egui::StrokeKind::Middle);
            line(vec![p(r, 0.30, 0.70), p(r, 0.26, 0.86), p(r, 0.46, 0.70)]);
            seg(p(r, 0.30, 0.38), p(r, 0.70, 0.38));
            seg(p(r, 0.30, 0.52), p(r, 0.58, 0.52));
        }
        Icon::Queue => {
            for (i, y) in [0.26f32, 0.46, 0.66].iter().enumerate() {
                let x1 = if i == 2 { 0.56 } else { 0.84 };
                seg(p(r, 0.16, *y), p(r, x1, *y));
            }
            painter.add(Shape::convex_polygon(
                vec![p(r, 0.66, 0.60), p(r, 0.88, 0.73), p(r, 0.66, 0.86)],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Expand => {
            line(vec![p(r, 0.22, 0.62), p(r, 0.5, 0.34), p(r, 0.78, 0.62)]);
        }
        Icon::Collapse => {
            line(vec![p(r, 0.22, 0.38), p(r, 0.5, 0.66), p(r, 0.78, 0.38)]);
        }
        Icon::Back => {
            line(vec![p(r, 0.62, 0.20), p(r, 0.34, 0.5), p(r, 0.62, 0.80)]);
        }
        Icon::Forward => {
            line(vec![p(r, 0.38, 0.20), p(r, 0.66, 0.5), p(r, 0.38, 0.80)]);
        }
        Icon::Home => {
            painter.add(Shape::closed_line(
                vec![
                    p(r, 0.18, 0.46),
                    p(r, 0.5, 0.18),
                    p(r, 0.82, 0.46),
                    p(r, 0.82, 0.84),
                    p(r, 0.18, 0.84),
                ],
                st,
            ));
            seg(p(r, 0.42, 0.84), p(r, 0.42, 0.62));
            seg(p(r, 0.42, 0.62), p(r, 0.58, 0.62));
            seg(p(r, 0.58, 0.62), p(r, 0.58, 0.84));
        }
        Icon::Search => {
            painter.circle_stroke(p(r, 0.43, 0.43), s * 0.24, st);
            seg(p(r, 0.61, 0.61), p(r, 0.84, 0.84));
        }
        Icon::Library => {
            seg(p(r, 0.22, 0.18), p(r, 0.22, 0.84));
            seg(p(r, 0.40, 0.18), p(r, 0.40, 0.84));
            seg(p(r, 0.56, 0.22), p(r, 0.78, 0.82));
        }
        Icon::Settings => {
            let c = p(r, 0.5, 0.5);
            painter.circle_stroke(c, s * 0.20, st);
            for i in 0..8 {
                let a = i as f32 / 8.0 * std::f32::consts::TAU;
                let d = vec2(a.cos(), a.sin());
                painter.line_segment([c + d * s * 0.27, c + d * s * 0.38], Stroke::new(w * 1.6, color));
            }
            painter.circle_filled(c, s * 0.07, color);
        }
        Icon::AudioLab => {
            for (x, k) in [(0.24f32, 0.36f32), (0.5, 0.64), (0.76, 0.44)] {
                seg(p(r, x, 0.16), p(r, x, 0.84));
                painter.circle_filled(p(r, x, k), s * 0.09, color);
            }
        }
        Icon::Plus => {
            seg(p(r, 0.5, 0.2), p(r, 0.5, 0.8));
            seg(p(r, 0.2, 0.5), p(r, 0.8, 0.5));
        }
        Icon::More => {
            for x in [0.25f32, 0.5, 0.75] {
                painter.circle_filled(p(r, x, 0.5), s * 0.075, color);
            }
        }
        Icon::Close => {
            seg(p(r, 0.25, 0.25), p(r, 0.75, 0.75));
            seg(p(r, 0.75, 0.25), p(r, 0.25, 0.75));
        }
        Icon::Note => {
            painter.circle_filled(p(r, 0.36, 0.72), s * 0.13, color);
            seg(p(r, 0.48, 0.72), p(r, 0.48, 0.18));
            line(vec![p(r, 0.48, 0.18), p(r, 0.74, 0.28), p(r, 0.74, 0.42), p(r, 0.48, 0.34)]);
        }
        Icon::Folder => {
            painter.add(Shape::closed_line(
                vec![
                    p(r, 0.14, 0.26),
                    p(r, 0.40, 0.26),
                    p(r, 0.48, 0.36),
                    p(r, 0.86, 0.36),
                    p(r, 0.86, 0.80),
                    p(r, 0.14, 0.80),
                ],
                st,
            ));
        }
        Icon::Trash => {
            seg(p(r, 0.18, 0.28), p(r, 0.82, 0.28));
            seg(p(r, 0.40, 0.18), p(r, 0.60, 0.18));
            line(vec![p(r, 0.26, 0.28), p(r, 0.32, 0.84), p(r, 0.68, 0.84), p(r, 0.74, 0.28)]);
        }
        Icon::Radio => {
            let c = p(r, 0.5, 0.5);
            painter.circle_filled(c, s * 0.08, color);
            for rad in [0.22f32, 0.36] {
                for side in [0.0f32, std::f32::consts::PI] {
                    let pts: Vec<Pos2> = (0..=10)
                        .map(|i| {
                            let a = side - 0.7 + 1.4 * i as f32 / 10.0;
                            c + vec2(a.cos(), a.sin()) * rad * s
                        })
                        .collect();
                    line(pts);
                }
            }
        }
        Icon::DragHandle => {
            for x in [0.38f32, 0.62] {
                for y in [0.28f32, 0.5, 0.72] {
                    painter.circle_filled(p(r, x, y), s * 0.06, color);
                }
            }
        }
        Icon::Playlist => {
            seg(p(r, 0.14, 0.26), p(r, 0.66, 0.26));
            seg(p(r, 0.14, 0.44), p(r, 0.66, 0.44));
            seg(p(r, 0.14, 0.62), p(r, 0.44, 0.62));
            painter.circle_filled(p(r, 0.64, 0.76), s * 0.1, color);
            seg(p(r, 0.73, 0.76), p(r, 0.73, 0.46));
            seg(p(r, 0.73, 0.46), p(r, 0.86, 0.50));
        }
        // ---- library/explore/stats ----
        Icon::Compass => {
            let c = p(r, 0.5, 0.5);
            painter.circle_stroke(c, s * 0.38, st);
            painter.add(Shape::convex_polygon(vec![p(r, 0.66, 0.34), p(r, 0.56, 0.56), p(r, 0.44, 0.44)], color, Stroke::NONE));
            painter.add(Shape::convex_polygon(vec![p(r, 0.34, 0.66), p(r, 0.44, 0.44), p(r, 0.56, 0.56)], color.gamma_multiply(0.5), Stroke::NONE));
        }
        Icon::Chart => {
            let cr = s * 0.03;
            painter.rect_filled(Rect::from_min_max(p(r, 0.16, 0.50), p(r, 0.32, 0.84)), cr, color);
            painter.rect_filled(Rect::from_min_max(p(r, 0.42, 0.22), p(r, 0.58, 0.84)), cr, color);
            painter.rect_filled(Rect::from_min_max(p(r, 0.68, 0.38), p(r, 0.84, 0.84)), cr, color);
        }
        Icon::Grid => {
            let cr = s * 0.04;
            for (x, y) in [(0.16f32, 0.16f32), (0.54, 0.16), (0.16, 0.54), (0.54, 0.54)] {
                painter.rect_stroke(Rect::from_min_max(p(r, x, y), p(r, x + 0.30, y + 0.30)), cr, st, egui::StrokeKind::Inside);
            }
        }
        Icon::ListView => {
            for y in [0.26f32, 0.5, 0.74] {
                painter.circle_filled(p(r, 0.18, y), s * 0.05, color);
                seg(p(r, 0.32, y), p(r, 0.84, y));
            }
        }
        Icon::Bookmark | Icon::BookmarkFilled => {
            let pts = vec![p(r, 0.26, 0.14), p(r, 0.74, 0.14), p(r, 0.74, 0.86), p(r, 0.5, 0.66), p(r, 0.26, 0.86)];
            if icon == Icon::BookmarkFilled {
                painter.add(Shape::convex_polygon(vec![p(r, 0.26, 0.14), p(r, 0.74, 0.14), p(r, 0.74, 0.86), p(r, 0.5, 0.66)], color, Stroke::NONE));
                painter.add(Shape::convex_polygon(vec![p(r, 0.26, 0.14), p(r, 0.5, 0.66), p(r, 0.26, 0.86)], color, Stroke::NONE));
            }
            painter.add(Shape::closed_line(pts, st));
        }
    }
}

/// Round icon button. `active` tints with `accent`.
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, active: bool, accent: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        if hovered {
            ui.painter().circle_filled(rect.center(), size * 0.5, Color32::from_white_alpha(18));
        }
        let color = if !ui.is_enabled() {
            Color32::from_gray(80)
        } else if active {
            accent
        } else if hovered {
            Color32::WHITE
        } else {
            Color32::from_gray(200)
        };
        paint(ui.painter(), rect.shrink(size * 0.2), icon, color);
    }
    resp
}

/// Filled circular button (main play/pause).
pub fn filled_icon_button(ui: &mut Ui, icon: Icon, size: f32, fill: Color32, fg: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if ui.is_rect_visible(rect) {
        let f = if resp.hovered() { fill.gamma_multiply(0.85) } else { fill };
        let scale = if resp.is_pointer_button_down_on() { 0.94 } else { 1.0 };
        ui.painter().circle_filled(rect.center(), size * 0.5 * scale, f);
        paint(ui.painter(), rect.shrink(size * 0.27), icon, fg);
    }
    resp
}
