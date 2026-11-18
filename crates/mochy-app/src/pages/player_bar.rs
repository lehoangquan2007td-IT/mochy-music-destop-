//! Bottom now-playing bar.

use egui::{Align, Align2, Color32, FontId, Layout, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use mochy_core::settings::RepeatMode;

use super::{NowInfo, SideTab};
use crate::actions::{Action, Ctx};
use crate::controller::PlayState;
use crate::images::THUMB_PX;
use crate::nav::Route;
use crate::ui::i18n;
use crate::ui::i18n::tr;
use crate::widgets::{self, TEXT, TEXT_DIM, icons, icons::Icon};

pub const BAR_H: f32 = 88.0;

pub fn in_rect<R>(ui: &mut Ui, rect: Rect, layout: Layout, f: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope_builder(UiBuilder::new().max_rect(rect).layout(layout), f).inner
}

/// Transport row: shuffle, prev, play/pause, next, repeat.
pub fn transport(ui: &mut Ui, cx: &mut Ctx, now: &NowInfo, scale: f32) {
    let accent = widgets::accent_light(cx.accent);
    ui.spacing_mut().item_spacing.x = 10.0 * scale;
    if icons::icon_button(ui, Icon::Shuffle, 32.0 * scale, now.shuffle, accent).on_hover_text(tr("Shuffle")).clicked() {
        cx.act(Action::ToggleShuffle);
    }
    if icons::icon_button(ui, Icon::Prev, 36.0 * scale, false, accent).on_hover_text(tr("Previous (Ctrl+←)")).clicked() {
        cx.act(Action::Prev);
    }
    let loading = now.state == PlayState::Loading;
    let icon = if now.playing { Icon::Pause } else { Icon::Play };
    let r = icons::filled_icon_button(ui, icon, 44.0 * scale, Color32::WHITE, Color32::BLACK);
    if loading {
        let c = r.rect.center();
        let t = ui.input(|i| i.time) as f32;
        let rad = r.rect.width() * 0.5 + 3.0;
        let pts: Vec<egui::Pos2> = (0..=20)
            .map(|i| {
                let a = t * 5.0 + i as f32 / 20.0 * 2.0;
                c + vec2(a.cos(), a.sin()) * rad
            })
            .collect();
        ui.painter().add(egui::Shape::line(pts, egui::Stroke::new(2.0, accent)));
        crate::ui::pace::animate(ui.ctx());
    }
    if r.on_hover_text(tr("Play / pause (Space)")).clicked() {
        cx.act(Action::Toggle);
    }
    if icons::icon_button(ui, Icon::Next, 36.0 * scale, false, accent).on_hover_text(tr("Next (Ctrl+→)")).clicked() {
        cx.act(Action::Next);
    }
    let (ri, active, tip) = match now.repeat {
        RepeatMode::Off => (Icon::Repeat, false, tr("Repeat: off")),
        RepeatMode::All => (Icon::Repeat, true, tr("Repeat: all")),
        RepeatMode::One => (Icon::RepeatOne, true, tr("Repeat: one")),
    };
    if icons::icon_button(ui, ri, 32.0 * scale, active, accent).on_hover_text(tip).clicked() {
        cx.act(Action::CycleRepeat);
    }
}

/// Seek row: time, bar, time.
pub fn seek_row(ui: &mut Ui, cx: &mut Ctx, now: &NowInfo, width: f32) {
    let font = FontId::monospace(11.5);
    let tw = 46.0;
    let (lr, _) = ui.allocate_exact_size(vec2(tw, 18.0), Sense::hover());
    let out = widgets::seek_bar(ui, now.pos, now.duration, (width - 2.0 * tw - 16.0).max(60.0), cx.accent);
    let (rr, _) = ui.allocate_exact_size(vec2(tw, 18.0), Sense::hover());
    ui.painter().text(lr.right_center(), Align2::RIGHT_CENTER, widgets::fmt_time(out.display), font.clone(), TEXT_DIM);
    let dur = if now.duration > 0.0 { widgets::fmt_time(now.duration) } else { "--:--".into() };
    ui.painter().text(rr.left_center(), Align2::LEFT_CENTER, dur, font, TEXT_DIM);
    if let Some(t) = out.commit {
        cx.act(Action::Seek(t));
    }
}

pub fn volume(ui: &mut Ui, cx: &mut Ctx, now: &NowInfo, width: f32) {
    let icon = if now.muted || now.volume <= 0.001 { Icon::VolumeMute } else { Icon::Volume };
    if icons::icon_button(ui, icon, 30.0, false, cx.accent)
        .on_hover_text(i18n::volume_tip((now.volume * 100.0).round() as i64))
        .clicked()
    {
        cx.act(Action::ToggleMute);
    }
    let shown = if now.muted { 0.0 } else { now.volume };
    if let Some(v) = widgets::level_slider(ui, shown, width, cx.accent) {
        cx.act(Action::SetVolume(v));
    }
}

pub fn show(ui: &mut Ui, cx: &mut Ctx, now: &NowInfo, side: Option<SideTab>) {
    let rect = ui.max_rect();
    let cy = rect.center().y;
    let w = rect.width();

    // ---- left: artwork + title ---------------------------------------------
    let left_w = (w * 0.30).clamp(200.0, 380.0);
    let art = Rect::from_min_size(pos2(rect.left() + 16.0, cy - 28.0), vec2(56.0, 56.0));
    if let Some(song) = &now.song {
        let tex = crate::images::song_src(song).and_then(|s| cx.images.get(&s, THUMB_PX));
        widgets::paint_artwork(ui, art, tex.as_ref(), (cx.radius * 0.6).max(4.0));
        let ar = ui.interact(art, ui.scope_id().with("bar_art"), Sense::click()).on_hover_text(tr("Open now playing"));
        if ar.hovered() {
            ui.painter().rect_filled(art, 6.0, Color32::from_black_alpha(110));
            icons::paint(ui.painter(), art.shrink(16.0), Icon::Expand, Color32::WHITE);
        }
        if ar.clicked() {
            cx.act(Action::SetNowPlaying(true));
        }
        let tx = art.right() + 12.0;
        let tw = left_w - (tx - rect.left()) - 40.0;
        let title_r = widgets::paint_text(ui, pos2(tx, cy - 19.0), &song.title, FontId::proportional(14.5), TEXT, tw);
        let trr = ui.interact(title_r, ui.scope_id().with("bar_title"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        if trr.clicked() {
            match &song.album_id {
                Some(a) => cx.act(Action::Navigate(Route::Album(a.clone()))),
                None => cx.act(Action::SetNowPlaying(true)),
            }
        }
        let ar_text = widgets::artists_line(song);
        let arr = widgets::paint_text(ui, pos2(tx, cy + 2.0), &ar_text, FontId::proportional(12.5), TEXT_DIM, tw);
        if let Some(aid) = song.artists.iter().find_map(|a| a.id.clone()) {
            let r = ui.interact(arr, ui.scope_id().with("bar_artist"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
            if r.clicked() {
                cx.act(Action::Navigate(Route::Artist(aid)));
            }
        }
        let liked = cx.is_liked(song);
        let hx = (title_r.right().max(arr.right()) + 10.0).min(rect.left() + left_w - 28.0);
        let heart = Rect::from_center_size(pos2(hx + 14.0, cy), vec2(28.0, 28.0));
        let hr = ui.interact(heart, ui.scope_id().with("bar_heart"), Sense::click()).on_hover_text(if liked { tr("Unlike") } else { tr("Like") });
        if hr.hovered() {
            ui.painter().circle_filled(heart.center(), 14.0, Color32::from_white_alpha(18));
        }
        icons::paint(
            ui.painter(),
            heart.shrink(6.0),
            if liked { Icon::HeartFilled } else { Icon::Heart },
            if liked { Color32::from_rgb(0xFF, 0x5A, 0x7A) } else { TEXT_DIM },
        );
        if hr.clicked() {
            cx.act(Action::ToggleLike(song.clone()));
        }
        if let PlayState::Error(e) = &now.state {
            ui.painter().text(pos2(tx, cy + 22.0), Align2::LEFT_TOP, e.chars().take(60).collect::<String>(), FontId::proportional(10.5), Color32::from_rgb(0xFF, 0x8A, 0x8A));
        }
    } else {
        widgets::paint_artwork(ui, art, None, 6.0);
        ui.painter().text(pos2(art.right() + 12.0, cy), Align2::LEFT_CENTER, tr("Nothing playing"), FontId::proportional(14.0), TEXT_DIM);
    }

    // ---- right: format, lyrics, queue, volume, expand -----------------------
    let right_w = (w * 0.28).clamp(240.0, 360.0);
    let rrect = Rect::from_min_max(pos2(rect.right() - right_w, rect.top()), pos2(rect.right() - 12.0, rect.bottom()));
    in_rect(ui, rrect, Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if icons::icon_button(ui, Icon::Expand, 30.0, false, cx.accent).on_hover_text(tr("Now playing")).clicked() {
            cx.act(Action::SetNowPlaying(true));
        }
        ui.add_space(6.0);
        // right_to_left: slider first then icon
        let vw = 96.0;
        ui.allocate_ui_with_layout(vec2(vw + 34.0, 30.0), Layout::left_to_right(Align::Center), |ui| volume(ui, cx, now, vw));
        ui.add_space(6.0);
        if icons::icon_button(ui, Icon::Queue, 30.0, side == Some(SideTab::Queue), widgets::accent_light(cx.accent))
            .on_hover_text(tr("Queue"))
            .clicked()
        {
            cx.act(Action::ToggleSidePanel(SideTab::Queue));
        }
        if icons::icon_button(ui, Icon::Lyrics, 30.0, side == Some(SideTab::Lyrics), widgets::accent_light(cx.accent))
            .on_hover_text(tr("Lyrics"))
            .clicked()
        {
            cx.act(Action::ToggleSidePanel(SideTab::Lyrics));
        }
    });

    // ---- center: transport + seek -------------------------------------------
    let cw = (w - left_w - right_w - 32.0).clamp(260.0, 620.0);
    let crect = Rect::from_center_size(pos2(rect.center().x.max(rect.left() + left_w + cw / 2.0 + 8.0), cy), vec2(cw, BAR_H - 12.0));
    let top = Rect::from_min_max(crect.min, pos2(crect.right(), crect.top() + 46.0));
    let bottom = Rect::from_min_max(pos2(crect.left(), crect.top() + 46.0), crect.max);
    in_rect(ui, top, Layout::left_to_right(Align::Center), |ui| {
        let total = 32.0 * 2.0 + 36.0 * 2.0 + 44.0 + 40.0;
        ui.add_space(((cw - total) / 2.0).max(0.0));
        transport(ui, cx, now, 1.0);
    });
    in_rect(ui, bottom, Layout::left_to_right(Align::Center), |ui| {
        seek_row(ui, cx, now, cw);
    });
    if let Some(f) = &now.format {
        ui.painter().text(pos2(crect.right(), crect.bottom() + 2.0), Align2::RIGHT_BOTTOM, f, FontId::proportional(9.5), widgets::TEXT_FAINT);
    }
    crate::ui::transitions_ui::bar_indicator(ui, cx, crect);
}
