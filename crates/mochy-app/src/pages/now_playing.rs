//! Now Playing: expanded "Flagship" view (gradient backdrop, big artwork,
//! controls) + the Lyrics / Queue / Related tabs (also used by the side panel).

use std::sync::Arc;

use egui::{Align, Align2, Color32, CornerRadius, FontId, Layout, Mesh, Rect, RichText, Sense, Shape, Ui, pos2, vec2};
use mochy_core::models::Song;

use super::player_bar::{in_rect, seek_row, transport, volume};
use super::{NowInfo, SideTab};
use crate::actions::{Action, Ctx, LoadState};
use crate::images::{LARGE_PX, THUMB_PX};
use crate::nav::Route;
use crate::queue::Queue;
use crate::ui::i18n::{self, tr};
use crate::ui::lyrics_view::{LyricsState, LyricsView};
use crate::widgets::{self, TEXT, TEXT_DIM, icons, icons::Icon};

#[derive(Default)]
pub struct NpUi {
    /// Queue row being dragged (index) — reorder by drag.
    pub queue_drag: Option<usize>,
    pub scroll_to_current: bool,
    /// Cached Arc of related songs (for row playback context).
    pub related_arc: Option<(String, Arc<Vec<Song>>)>,
}

/// Borrowed inputs for the tab contents.
pub struct SideData<'a> {
    pub queue: &'a Queue,
    pub lyrics: &'a LyricsState,
    pub related: Option<&'a LoadState<Vec<Song>>>,
    pub lyrics_scale: f32,
    pub lyrics_enabled: bool,
    /// Global sync compensation (settings), added to the per-song offset.
    pub lyrics_global_offset_ms: i32,
}

pub fn tabs(ui: &mut Ui, tab: &mut SideTab) {
    ui.horizontal(|ui| {
        for t in SideTab::ALL {
            if widgets::pill(ui, t.label(), *tab == t).clicked() {
                *tab = t;
            }
        }
    });
}

pub fn tab_content(ui: &mut Ui, cx: &mut Ctx, tab: SideTab, now: &NowInfo, d: &SideData, np: &mut NpUi, lv: &mut LyricsView) {
    match tab {
        SideTab::Lyrics => lyrics(ui, cx, now, d, lv),
        SideTab::Queue => queue_view(ui, cx, d.queue, np),
        SideTab::Related => related(ui, cx, now, d, np),
    }
}

fn lyrics(ui: &mut Ui, cx: &mut Ctx, now: &NowInfo, d: &SideData, lv: &mut LyricsView) {
    let Some(song) = &now.song else {
        ui.label(RichText::new(tr("Nothing playing")).color(TEXT_DIM));
        return;
    };
    if !d.lyrics_enabled {
        ui.label(RichText::new(tr("Lyrics are turned off in Settings → Lyrics.")).color(TEXT_DIM));
        return;
    }
    let offset = song.lyrics_offset_ms + d.lyrics_global_offset_ms;
    // Offset controls pinned to the bottom, lyrics fill the rest.
    ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("Offset")).size(12.0).color(TEXT_DIM));
            if ui.small_button("−0.5 s").clicked() {
                cx.act(Action::LyricsOffset(-500));
            }
            ui.label(RichText::new(format!("{:+.1} s", offset as f32 / 1000.0)).size(12.0).color(TEXT));
            if ui.small_button("+0.5 s").clicked() {
                cx.act(Action::LyricsOffset(500));
            }
            if song.lyrics_offset_ms != 0 && ui.small_button(tr("Reset")).clicked() {
                // Reset only the per-song part — the global offset is a setting, not this song's.
                cx.act(Action::LyricsOffset(-song.lyrics_offset_ms));
            }
            if d.lyrics_global_offset_ms != 0 {
                ui.label(
                    RichText::new(format!("· {} {:+} ms", tr("global"), d.lyrics_global_offset_ms))
                        .size(11.0)
                        .color(widgets::TEXT_FAINT),
                );
            }
            if let LyricsState::Loaded(l) = d.lyrics {
                ui.label(RichText::new(format!("· {}", l.provider)).size(11.0).color(widgets::TEXT_FAINT));
            }
            crate::lyrics_extras::toolbar(ui, lv);
        });
        ui.with_layout(Layout::top_down(Align::Min), |ui| {
            let scale = d.lyrics_scale.clamp(0.5, 2.5);
            lv.font_size = crate::ui::lyrics_view::LyricsView::default().font_size * scale;
            if let Some(t) = lv.show(ui, d.lyrics, now.pos, offset, cx.accent) {
                // Seek to where the clicked line is active *and* its vocals are
                // heard now: the same unreported device latency that makes
                // lyrics run ahead also delays what is heard after a seek.
                cx.act(Action::Seek(t - offset as f64 / 1000.0));
            }
        });
    });
}

const QROW_H: f32 = 52.0;

fn queue_view(ui: &mut Ui, cx: &mut Ctx, q: &Queue, np: &mut NpUi) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} · {}", tr("Queue"), i18n::songs_count(q.len()))).size(14.0).color(TEXT_DIM));
        if q.shuffle() {
            ui.label(RichText::new(tr("· shuffled")).size(12.0).color(widgets::TEXT_FAINT));
        }
        if !q.is_empty() {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button(tr("Clear")).on_hover_text(tr("Clear the queue")).clicked() {
                    cx.act(Action::QueueClear);
                }
            });
        }
    });
    ui.add_space(6.0);
    if q.is_empty() {
        ui.label(RichText::new(tr("The queue is empty.")).color(TEXT_DIM));
        return;
    }
    let cur = q.current_index();
    let scroll_to = std::mem::take(&mut np.scroll_to_current);
    egui::ScrollArea::vertical().id_salt("queue_scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let top = ui.cursor().top();
        let n = q.len();
        for (i, e) in q.entries().iter().enumerate() {
            let song = &e.song;
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), QROW_H), Sense::click());
            if scroll_to && Some(i) == cur {
                ui.scroll_to_rect(rect, Some(Align::Center));
            }
            if !ui.is_rect_visible(rect) {
                continue;
            }
            let is_cur = Some(i) == cur;
            let dragging_this = np.queue_drag == Some(i);
            if is_cur {
                ui.painter().rect_filled(rect, CornerRadius::same(8), cx.accent.gamma_multiply(0.22));
            } else if resp.hovered() || dragging_this {
                ui.painter().rect_filled(rect, CornerRadius::same(8), Color32::from_white_alpha(10));
            }
            let cy = rect.center().y;
            let handle = Rect::from_center_size(pos2(rect.left() + 14.0, cy), vec2(22.0, 30.0));
            let hresp = ui.interact(handle, ui.scope_id().with(("qdrag", e.uid)), Sense::drag()).on_hover_cursor(egui::CursorIcon::Grab);
            icons::paint(ui.painter(), handle.shrink(4.0), Icon::DragHandle, if hresp.hovered() { TEXT } else { widgets::TEXT_FAINT });
            if hresp.drag_started() {
                np.queue_drag = Some(i);
            }
            let art = Rect::from_min_size(pos2(rect.left() + 30.0, cy - 18.0), vec2(36.0, 36.0));
            let tex = crate::images::song_src(song).and_then(|s| cx.images.get(&s, THUMB_PX));
            widgets::paint_artwork(ui, art, tex.as_ref(), 5.0);
            let tx = art.right() + 10.0;
            let tw = rect.right() - tx - 70.0;
            let tc = if is_cur { widgets::accent_light(cx.accent) } else { TEXT };
            widgets::paint_text(ui, pos2(tx, cy - 17.0), &song.title, FontId::proportional(13.5), tc, tw);
            widgets::paint_text(ui, pos2(tx, cy + 1.0), &widgets::artists_line(song), FontId::proportional(12.0), TEXT_DIM, tw);
            let dur = song.duration_secs.map(widgets::fmt_time).unwrap_or_default();
            ui.painter().text(pos2(rect.right() - 36.0, cy), Align2::RIGHT_CENTER, dur, FontId::proportional(12.0), TEXT_DIM);
            let xr = Rect::from_center_size(pos2(rect.right() - 18.0, cy), vec2(24.0, 24.0));
            let xresp = ui.interact(xr, ui.scope_id().with(("qx", e.uid)), Sense::click()).on_hover_text(tr("Remove"));
            if resp.hovered() || xresp.hovered() {
                icons::paint(ui.painter(), xr.shrink(6.0), Icon::Close, if xresp.hovered() { TEXT } else { TEXT_DIM });
            }
            if xresp.clicked() {
                cx.act(Action::QueueRemove(i));
            }
            if resp.double_clicked() {
                cx.act(Action::QueuePlay(i));
            }
            resp.context_menu(|ui| {
                if ui.button(tr("Play now")).clicked() {
                    cx.act(Action::QueuePlay(i));
                    ui.close();
                }
                if i > 0 && ui.button(tr("Move up")).clicked() {
                    cx.act(Action::QueueMove { from: i, to: i - 1 });
                    ui.close();
                }
                if i + 1 < n && ui.button(tr("Move down")).clicked() {
                    cx.act(Action::QueueMove { from: i, to: i + 1 });
                    ui.close();
                }
                if let Some(c) = cur
                    && i != c
                    && i != c + 1
                    && ui.button(tr("Play next")).clicked()
                {
                    let to = if i < c { c } else { c + 1 };
                    cx.act(Action::QueueMove { from: i, to });
                    ui.close();
                }
                if ui.button(tr("Remove from queue")).clicked() {
                    cx.act(Action::QueueRemove(i));
                    ui.close();
                }
                ui.separator();
                widgets::song_menu(ui, cx, song, None);
            });
        }
        // Drag & drop reorder.
        if let Some(from) = np.queue_drag {
            if let Some(p) = ui.ctx().pointer_interact_pos() {
                let target = (((p.y - top) / QROW_H).floor().max(0.0) as usize).min(n - 1);
                let y = top + target as f32 * QROW_H + if target > from { QROW_H } else { 0.0 };
                let x0 = ui.min_rect().left();
                let x1 = ui.min_rect().right();
                ui.painter().line_segment([pos2(x0, y), pos2(x1, y)], egui::Stroke::new(2.0, widgets::accent_light(cx.accent)));
                if ui.input(|i| i.pointer.any_released()) {
                    if target != from {
                        cx.act(Action::QueueMove { from, to: target });
                    }
                    np.queue_drag = None;
                }
            }
            if !ui.input(|i| i.pointer.any_down()) {
                np.queue_drag = None;
            }
        }
    });
}

fn related(ui: &mut Ui, cx: &mut Ctx, now: &NowInfo, d: &SideData, np: &mut NpUi) {
    let Some(song) = &now.song else {
        ui.label(RichText::new(tr("Nothing playing")).color(TEXT_DIM));
        return;
    };
    if song.video_id.is_none() {
        ui.label(RichText::new(tr("Related songs are available for YouTube Music tracks.")).color(TEXT_DIM));
        return;
    }
    match d.related {
        None | Some(LoadState::Loading) => widgets::loading(ui, tr("Loading related…")),
        Some(LoadState::Failed(e)) => {
            ui.label(RichText::new(e).color(TEXT_DIM));
        }
        Some(LoadState::Ready(list)) => {
            if list.is_empty() {
                ui.label(RichText::new(tr("No related songs.")).color(TEXT_DIM));
                return;
            }
            let key = song.id.clone();
            let arc = match &np.related_arc {
                Some((k, a)) if *k == key => a.clone(),
                _ => {
                    let a = Arc::new(list.as_ref().clone());
                    np.related_arc = Some((key, a.clone()));
                    a
                }
            };
            egui::ScrollArea::vertical().id_salt("related_scroll").auto_shrink([false, false]).show(ui, |ui| {
                for i in 0..arc.len() {
                    widgets::song_row(ui, cx, &arc, i, widgets::RowOpts::default());
                }
            });
        }
    }
}

/// Paint a 4-corner gradient into `rect`.
pub fn gradient(ui: &Ui, rect: Rect, tl: Color32, tr: Color32, bl: Color32, br: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(rect.left_top(), tl);
    m.colored_vertex(rect.right_top(), tr);
    m.colored_vertex(rect.left_bottom(), bl);
    m.colored_vertex(rect.right_bottom(), br);
    m.add_triangle(0, 1, 2);
    m.add_triangle(1, 3, 2);
    ui.painter().add(Shape::mesh(m));
}

/// Full-window expanded player.
#[allow(clippy::too_many_arguments)]
pub fn expanded(ui: &mut Ui, cx: &mut Ctx, now: &NowInfo, d: &SideData, tab: &mut SideTab, np: &mut NpUi, lv: &mut LyricsView, grad: (Color32, Color32)) {
    let rect = ui.max_rect();
    // ---- backdrop ------------------------------------------------------------
    let dark = |c: Color32, t: f32| widgets::lerp_color(c, Color32::BLACK, t);
    let (g1, g2) = grad;
    gradient(ui, rect, dark(g1, 0.25), dark(widgets::lerp_color(g1, g2, 0.5), 0.4), dark(widgets::lerp_color(g1, g2, 0.6), 0.55), dark(g2, 0.6));
    if let Some(song) = &now.song
        && let Some(tex) = crate::images::song_src(song).and_then(|s| cx.images.get(&s, LARGE_PX))
    {
        // Oversized, faint cover as a frosted wash (center-cropped, no stretch).
        let side = rect.width().max(rect.height()) * 1.2;
        let r = Rect::from_center_size(rect.center(), vec2(side, side));
        ui.painter().image(tex.id(), r, widgets::cover_uv(tex.size_vec2(), r.size()), Color32::from_white_alpha(34));
    }
    gradient(ui, rect, Color32::from_black_alpha(40), Color32::from_black_alpha(70), Color32::from_black_alpha(150), Color32::from_black_alpha(170));

    // ---- top bar ---------------------------------------------------------------
    let topbar = Rect::from_min_size(rect.min + vec2(16.0, 12.0), vec2(rect.width() - 32.0, 40.0));
    in_rect(ui, topbar, Layout::left_to_right(Align::Center), |ui| {
        if icons::icon_button(ui, Icon::Collapse, 36.0, false, cx.accent).on_hover_text(tr("Close (Esc)")).clicked() {
            cx.act(Action::SetNowPlaying(false));
        }
        ui.label(RichText::new(tr("Now playing")).size(14.0).color(TEXT_DIM));
        if let Some(f) = &now.format {
            ui.add_space(8.0);
            egui::Frame::new().fill(Color32::from_white_alpha(16)).corner_radius(8).inner_margin(egui::Margin::symmetric(8, 2)).show(ui, |ui| {
                ui.label(RichText::new(f).size(11.0).color(TEXT));
            });
        }
    });

    let body = Rect::from_min_max(pos2(rect.left() + 32.0, rect.top() + 64.0), pos2(rect.right() - 32.0, rect.bottom() - 24.0));
    let left_w = (body.width() * 0.46).clamp(340.0, 620.0);
    let left = Rect::from_min_max(body.min, pos2(body.left() + left_w, body.bottom()));
    let right = Rect::from_min_max(pos2(left.right() + 40.0, body.top()), body.max);

    // ---- left column: cover + info + controls ------------------------------------
    let controls_h = 210.0;
    let art_side = (left.width() - 40.0).min(left.height() - controls_h).max(160.0);
    let art = Rect::from_min_size(pos2(left.center().x - art_side / 2.0, left.top() + ((left.height() - controls_h - art_side) / 2.0).max(0.0)), vec2(art_side, art_side));
    let tex = now.song.as_ref().and_then(crate::images::song_src).and_then(|s| cx.images.get(&s, LARGE_PX));
    // soft shadow
    ui.painter().rect_filled(art.expand(6.0).translate(vec2(0.0, 8.0)), CornerRadius::same(28), Color32::from_black_alpha(70));
    widgets::paint_artwork(ui, art, tex.as_ref(), 22.0);

    let info = Rect::from_min_max(pos2(art.left(), art.bottom() + 18.0), pos2(art.right(), left.bottom()));
    in_rect(ui, info, Layout::top_down(Align::Min), |ui| {
        if let Some(song) = &now.song {
            ui.horizontal(|ui| {
                let w = ui.available_width() - 44.0;
                ui.vertical(|ui| {
                    ui.set_max_width(w);
                    let g = widgets::galley_trunc(ui, &song.title, FontId::proportional(24.0), TEXT, w);
                    let (r, resp) = ui.allocate_exact_size(g.size(), Sense::click());
                    ui.painter().galley(r.min, g, TEXT);
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
                        && let Some(a) = &song.album_id
                    {
                        cx.act(Action::Navigate(Route::Album(a.clone())));
                        cx.act(Action::SetNowPlaying(false));
                    }
                    let artists = widgets::artists_line(song);
                    let g = widgets::galley_trunc(ui, &artists, FontId::proportional(16.0), TEXT_DIM, w);
                    let (r, resp) = ui.allocate_exact_size(g.size(), Sense::click());
                    ui.painter().galley(r.min, g, TEXT_DIM);
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
                        && let Some(aid) = song.artists.iter().find_map(|a| a.id.clone())
                    {
                        cx.act(Action::Navigate(Route::Artist(aid)));
                        cx.act(Action::SetNowPlaying(false));
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let liked = cx.is_liked(song);
                    let heart_col = if liked { Color32::from_rgb(0xFF, 0x5A, 0x7A) } else { TEXT };
                    if icons::icon_button(ui, if liked { Icon::HeartFilled } else { Icon::Heart }, 38.0, liked, heart_col).clicked() {
                        cx.act(Action::ToggleLike(song.clone()));
                    }
                });
            });
        } else {
            ui.label(RichText::new(tr("Nothing playing")).size(22.0).color(TEXT_DIM));
        }
        ui.add_space(10.0);
        let w = ui.available_width();
        ui.horizontal(|ui| seek_row(ui, cx, now, w));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let total = (32.0 * 2.0 + 36.0 * 2.0 + 44.0) * 1.2 + 48.0;
            ui.add_space(((w - total) / 2.0).max(0.0));
            transport(ui, cx, now, 1.2);
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(((w - 200.0) / 2.0).max(0.0));
            volume(ui, cx, now, 160.0);
        });
    });

    // ---- right column: tabs ---------------------------------------------------------
    if right.width() > 220.0 {
        in_rect(ui, right, Layout::top_down(Align::Min), |ui| {
            tabs(ui, tab);
            ui.add_space(12.0);
            let r = ui.available_rect_before_wrap();
            ui.painter().rect_filled(r, CornerRadius::same(18), Color32::from_black_alpha(60));
            in_rect(ui, r.shrink(14.0), Layout::top_down(Align::Min), |ui| {
                tab_content(ui, cx, *tab, now, d, np, lv);
            });
        });
    }
}
