//! "Your taste" page ("Gu của bạn"): what Mochy learned from the listening
//! history — top artists (recent / all time), top songs, artists by time of
//! day and the songs the user keeps skipping. Reads the published
//! [`crate::taste`] profile every frame (an `Arc` clone) and never computes
//! anything heavy itself.

use std::sync::Arc;

use egui::{Align, Align2, Color32, CornerRadius, FontId, Id, Layout, Rect, RichText, Sense, Stroke, Ui, Vec2, pos2, vec2};
use mochy_core::models::Song;

use crate::actions::{Action, Ctx};
use crate::browse::BrowseState;
use crate::images::{ImageSrc, THUMB_PX};
use crate::taste::{ArtistTaste, Daypart, SongTaste, TasteProfile};
use crate::ui::i18n::{ago_label, tr};
use crate::widgets::{self, SURFACE, SURFACE_2, SURFACE_3, TEXT, TEXT_DIM, TEXT_FAINT, icons, icons::Icon, menus, shelves};

/// Artists listed in "Top artists" (two columns).
const TOP_ARTISTS: usize = 10;
/// Songs listed in "Top songs" (two columns).
const TOP_SONGS: usize = 20;
/// Songs queued by "Play your taste".
const TASTE_QUEUE: usize = 50;
/// Artists per time-of-day card.
const DAYPART_ARTISTS: usize = 4;
const DAYPART_ROW_H: f32 = 40.0;
/// Songs listed in "Songs you often skip".
const SKIPPED: usize = 10;

// ---------------------------------------------------------------------------
// Pure logic (unit-tested)
// ---------------------------------------------------------------------------

/// Which window "Top artists" ranks by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// Last ~4 weeks (`short_term`).
    Recent,
    /// Whole profile (`score`).
    AllTime,
}

/// Artists ranked for `window`, each with its value relative to the first
/// (0..=1), best first, at most `n`. The second value is `true` when
/// `Recent` had no short-term data and fell back to the all-time score.
pub fn rank_artists(artists: &[ArtistTaste], window: Window, n: usize) -> (Vec<(&ArtistTaste, f32)>, bool) {
    let recent = window == Window::Recent && artists.iter().any(|a| a.short_term > 0.0);
    let value = |a: &ArtistTaste| if recent { a.short_term } else { a.score };
    let mut v: Vec<&ArtistTaste> = artists.iter().filter(|a| value(a) > 0.0 && !a.name.trim().is_empty()).collect();
    v.sort_by(|a, b| value(b).total_cmp(&value(a)));
    v.truncate(n);
    let top = v.first().map(|a| value(a)).unwrap_or(0.0);
    let ranked = v.into_iter().map(|a| (a, if top > 0.0 { (value(a) / top).clamp(0.0, 1.0) } else { 0.0 })).collect();
    (ranked, window == Window::Recent && !recent)
}

/// "A", "A and B", "A, B and C".
pub fn join_names(names: &[&str], and: &str) -> String {
    match names {
        [] => String::new(),
        [a] => a.to_string(),
        [rest @ .., last] => format!("{} {and} {last}", rest.join(", ")),
    }
}

/// The friendly one-liner under the title, `None` when there is nothing to
/// say yet.
pub fn summary(p: &TasteProfile) -> Option<String> {
    let (ranked, fell_back) = rank_artists(&p.artists, Window::Recent, 3);
    if !ranked.is_empty() {
        let names: Vec<&str> = ranked.iter().map(|(a, _)| a.name.trim()).collect();
        let names = join_names(&names, tr("and"));
        let template = if fell_back { tr("You listen to {names} the most") } else { tr("Lately you've been listening to {names} a lot") };
        return Some(template.replace("{names}", &names));
    }
    top_songs(p, 1).first().map(|s| tr("Lately you can't stop playing “{title}”").replace("{title}", &s.title))
}

/// Best songs by score, never the disliked ones.
pub fn top_songs(p: &TasteProfile, n: usize) -> Vec<Song> {
    p.songs.iter().filter(|s| s.score > 0.0 && !p.is_disliked(&s.song.id)).take(n).map(|s| s.song.clone()).collect()
}

/// Songs the user keeps skipping: the disliked ones first, then any with
/// skips, most skipped first.
pub fn skipped_songs(p: &TasteProfile, n: usize) -> Vec<&SongTaste> {
    let mut v: Vec<&SongTaste> = p.songs.iter().filter(|s| p.is_disliked(&s.song.id) || s.skips > 0).collect();
    v.sort_by(|a, b| p.is_disliked(&b.song.id).cmp(&p.is_disliked(&a.song.id)).then(b.skips.cmp(&a.skips)));
    v.truncate(n);
    v
}

/// Local hour (0..24) of an epoch-ms instant.
pub fn local_hour(now_ms: i64, utc_offset_secs: i64) -> u32 {
    ((now_ms.div_euclid(1000) + utc_offset_secs).rem_euclid(86_400) / 3600) as u32
}

/// "5:00 – 12:00" (the `Daypart::from_hour` ranges, end exclusive).
pub fn daypart_hours(d: Daypart) -> &'static str {
    match d {
        Daypart::Morning => "5:00 – 12:00",
        Daypart::Afternoon => "12:00 – 17:00",
        Daypart::Evening => "17:00 – 22:00",
        Daypart::Night => "22:00 – 5:00",
    }
}

fn daypart_name(d: Daypart) -> &'static str {
    tr(match d {
        Daypart::Morning => "Morning",
        Daypart::Afternoon => "Afternoon",
        Daypart::Evening => "Evening",
        Daypart::Night => "Night",
    })
}

fn plays_label(n: u32) -> String {
    tr("{n} plays").replace("{n}", &n.to_string())
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

fn card_frame() -> egui::Frame {
    egui::Frame::new().fill(SURFACE).corner_radius(14).inner_margin(16)
}

pub fn show(ui: &mut Ui, cx: &mut Ctx, st: &BrowseState) {
    let profile = crate::taste::current();
    widgets::page_title(ui, tr("Your taste"));
    let Some(p) = profile.filter(|p| p.total_events > 0 && !(p.artists.is_empty() && p.songs.is_empty())) else {
        empty_state(ui, crate::taste::current().is_none());
        return;
    };

    hero(ui, cx, &p);
    ui.add_space(14.0);
    top_artists(ui, cx, &p);
    top_songs_section(ui, cx, &p);
    let hour = local_hour(crate::browse::now_ms(), st.lib.utc_offset_secs);
    dayparts(ui, cx, &p, Daypart::from_hour(hour));
    skipped(ui, cx, &p);

    ui.add_space(8.0);
    ui.label(RichText::new(tr("Your taste is worked out on this computer from what you play in Mochy and never leaves it.")).size(12.0).color(TEXT_FAINT));
    ui.add_space(24.0);
}

fn empty_state(ui: &mut Ui, computing: bool) {
    card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            if computing {
                ui.add(egui::Spinner::new().size(18.0));
                ui.add_space(4.0);
            }
            ui.label(RichText::new(tr("Listen to a few more songs so Mochy can learn your taste")).size(18.0).strong());
        });
        ui.label(
            RichText::new(tr("Your favourite artists, songs and listening moments show up here as you play music."))
                .color(TEXT_DIM),
        );
    });
}

/// Circular artist picture, or the initial on a tinted disc while loading.
fn avatar(ui: &Ui, cx: &mut Ctx, rect: Rect, a: &ArtistTaste) {
    let tex = a.thumbnail_url.as_ref().and_then(|u| cx.images.get(&ImageSrc::Url(u.clone()), THUMB_PX));
    if tex.is_some() {
        widgets::paint_artwork(ui, rect, tex.as_ref(), rect.width() / 2.0);
        return;
    }
    let c = rect.center();
    ui.painter().circle_filled(c, rect.width() / 2.0, widgets::lerp_color(SURFACE_3, cx.accent, 0.35));
    let initial: String = a.name.trim().chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
    ui.painter().text(c, Align2::CENTER_CENTER, initial, FontId::proportional(rect.width() * 0.42), TEXT);
}

fn open_artist(cx: &mut Ctx, a: &ArtistTaste) {
    if let Some(id) = a.id.as_deref().filter(|id| !id.is_empty()) {
        cx.act(Action::Navigate(menus::artist_route(id)));
    }
}

/// Overlapping avatars of the top artists + the summary sentence.
fn hero(ui: &mut Ui, cx: &mut Ctx, p: &TasteProfile) {
    let (ranked, _) = rank_artists(&p.artists, Window::Recent, 3);
    card_frame().inner_margin(20).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            if !ranked.is_empty() {
                let size = 76.0;
                let step = size - 22.0;
                let w = size + step * (ranked.len() as f32 - 1.0);
                let (r, _) = ui.allocate_exact_size(vec2(w, size), Sense::hover());
                // Paint back to front so the top artist sits on top.
                for (i, (a, _)) in ranked.iter().enumerate().rev() {
                    let ar = Rect::from_min_size(pos2(r.left() + i as f32 * step, r.top()), Vec2::splat(size));
                    ui.painter().circle_filled(ar.center(), size / 2.0 + 3.0, SURFACE);
                    avatar(ui, cx, ar, a);
                }
                ui.add_space(14.0);
            }
            ui.vertical(|ui| {
                ui.add_space(6.0);
                ui.label(RichText::new(tr("Your taste right now")).size(12.5).color(widgets::accent_light(cx.accent)));
                if let Some(s) = summary(p) {
                    ui.add(egui::Label::new(RichText::new(s).size(21.0).strong().color(TEXT)).wrap());
                }
                let mut basis = tr("Based on {n} listens").replace("{n}", &p.total_events.to_string());
                if p.computed_at > 0 {
                    let minutes = ((crate::browse::now_ms() - p.computed_at).max(0) / 60_000) as u64;
                    basis.push_str(" · ");
                    basis.push_str(&tr("updated {ago}").replace("{ago}", &ago_label(minutes)));
                }
                ui.label(RichText::new(basis).size(12.5).color(TEXT_DIM));
            });
        });
    });
}

fn section_row(ui: &mut Ui, title: &str, right: impl FnOnce(&mut Ui)) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(20.0).strong().color(TEXT));
        ui.with_layout(Layout::right_to_left(Align::Center), right);
    });
    ui.add_space(6.0);
}

fn top_artists(ui: &mut Ui, cx: &mut Ctx, p: &TasteProfile) {
    if p.artists.is_empty() {
        return;
    }
    let mem = Id::unique("taste_artists_all_time");
    let mut all_time = ui.data(|d| d.get_temp::<bool>(mem)).unwrap_or(false);
    section_row(ui, tr("Top artists"), |ui| {
        // Right-to-left: last pill first.
        if widgets::pill(ui, tr("All time"), all_time).clicked() {
            all_time = true;
        }
        if widgets::pill(ui, tr("Recently"), !all_time).on_hover_text(tr("Last 4 weeks")).clicked() {
            all_time = false;
        }
    });
    ui.data_mut(|d| d.insert_temp(mem, all_time));
    let (ranked, fell_back) = rank_artists(&p.artists, if all_time { Window::AllTime } else { Window::Recent }, TOP_ARTISTS);
    if fell_back {
        ui.label(RichText::new(tr("Not much listening in the last 4 weeks yet — showing all time.")).size(12.5).color(TEXT_DIM));
        ui.add_space(4.0);
    }
    let gap = 24.0;
    let rows = ranked.len().div_ceil(2);
    card_frame().inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let col_w = ((ui.available_width() - gap) / 2.0).floor();
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = vec2(gap, 0.0);
            for c in 0..2 {
                ui.vertical(|ui| {
                    ui.set_width(col_w);
                    for r in 0..rows {
                        let i = c * rows + r;
                        if let Some((a, frac)) = ranked.get(i) {
                            artist_rank_row(ui, cx, i, a, *frac, col_w);
                        }
                    }
                });
            }
        });
    });
    ui.add_space(14.0);
}

/// "1  (avatar)  Name ▬▬▬▬▬▬▬▬▬   42 plays"
fn artist_rank_row(ui: &mut Ui, cx: &mut Ctx, rank: usize, a: &ArtistTaste, frac: f32, width: f32) {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 56.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let clickable = a.id.as_deref().is_some_and(|id| !id.is_empty());
    if resp.hovered() && clickable {
        ui.painter().rect_filled(rect, CornerRadius::same(8), Color32::from_white_alpha(10));
    }
    let cy = rect.center().y;
    let accent = widgets::accent_light(cx.accent);
    ui.painter().text(
        pos2(rect.left() + 18.0, cy),
        Align2::CENTER_CENTER,
        (rank + 1).to_string(),
        FontId::proportional(15.0),
        if rank == 0 { accent } else { TEXT_DIM },
    );
    let av = Rect::from_center_size(pos2(rect.left() + 58.0, cy), Vec2::splat(40.0));
    avatar(ui, cx, av, a);
    let x = av.right() + 12.0;
    let plays = format!("{} · {}h", plays_label(a.plays), super::stats::hours_decimal(a.ms));
    let plays_g = ui.painter().layout_no_wrap(plays, FontId::proportional(12.5), TEXT_DIM);
    let right = rect.right() - 10.0;
    let text_w = (right - plays_g.size().x - 14.0 - x).max(40.0);
    widgets::paint_text(ui, pos2(x, cy - 16.0), a.name.trim(), FontId::proportional(14.5), TEXT, text_w);
    // Relative bar under the name.
    let track = Rect::from_min_size(pos2(x, cy + 8.0), vec2(text_w, 5.0));
    ui.painter().rect_filled(track, CornerRadius::same(3), Color32::from_white_alpha(14));
    let mut fill = track;
    fill.set_right(track.left() + (track.width() * frac).max(5.0));
    ui.painter().rect_filled(fill, CornerRadius::same(3), if rank == 0 { accent } else { accent.gamma_multiply(0.62) });
    ui.painter().galley(pos2(right - plays_g.size().x, cy - plays_g.size().y / 2.0), plays_g, TEXT_DIM);
    if clickable {
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            open_artist(cx, a);
        }
    }
}

fn top_songs_section(ui: &mut Ui, cx: &mut Ctx, p: &TasteProfile) {
    let list = top_songs(p, TOP_SONGS);
    if list.is_empty() {
        return;
    }
    let queue = top_songs(p, TASTE_QUEUE);
    section_row(ui, tr("Top songs"), |ui| {
        if widgets::accent_button(ui, Icon::Shuffle, tr("Play your taste"), cx.accent, true)
            .on_hover_text(tr("Shuffle your top songs"))
            .clicked()
        {
            cx.act(Action::PlayList { songs: Arc::new(queue), start: 0, shuffle: true });
        }
    });
    let subs: Vec<String> = list
        .iter()
        .map(|s| {
            let plays = p.song_index.get(&s.id).map(|&i| p.songs[i].plays).unwrap_or(0);
            format!("{} · {}", widgets::artists_line(s), plays_label(plays))
        })
        .collect();
    let list = Arc::new(list);
    let gap = 24.0;
    let col_w = ((ui.available_width() - gap) / 2.0).floor();
    let rows = list.len().div_ceil(2);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing = vec2(gap, 2.0);
        for c in 0..2 {
            ui.vertical(|ui| {
                ui.set_width(col_w);
                for r in 0..rows {
                    let i = c * rows + r;
                    if i < list.len() {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.add_sized(vec2(28.0, 56.0), egui::Label::new(RichText::new(format!("{}", i + 1)).size(15.0).strong().color(TEXT_DIM)));
                            shelves::compact_song_row(ui, cx, &list, i, Some(&subs[i]), col_w - 32.0, true);
                        });
                    }
                }
            });
        }
    });
    ui.add_space(14.0);
}

fn daypart_icon(d: Daypart) -> Icon {
    match d {
        Daypart::Morning | Daypart::Afternoon => Icon::Sun,
        Daypart::Evening | Daypart::Night => Icon::Moon,
    }
}

fn dayparts(ui: &mut Ui, cx: &mut Ctx, p: &TasteProfile, now: Daypart) {
    if p.by_daypart.iter().all(Vec::is_empty) {
        return;
    }
    section_row(ui, tr("By time of day"), |_| {});
    let gap = 12.0;
    let card_w = ((ui.available_width() - 3.0 * gap) / 4.0).floor().max(150.0);
    let accent = widgets::accent_light(cx.accent);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing = vec2(gap, gap);
        for d in Daypart::ALL {
            let current = d == now;
            let frame = card_frame().inner_margin(14).stroke(if current { Stroke::new(1.5, accent.gamma_multiply(0.8)) } else { Stroke::NONE });
            frame.show(ui, |ui| {
                ui.set_width(card_w - 28.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.set_min_height(24.0);
                        let (ir, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                        icons::paint(ui.painter(), ir, daypart_icon(d), if current { accent } else { TEXT_DIM });
                        ui.label(RichText::new(daypart_name(d)).size(15.0).strong().color(TEXT));
                        if current {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                egui::Frame::new().fill(accent.gamma_multiply(0.22)).corner_radius(9).inner_margin(egui::Margin::symmetric(8, 2)).show(ui, |ui| {
                                    ui.label(RichText::new(tr("Now")).size(11.5).color(accent));
                                });
                            });
                        }
                    });
                    ui.label(RichText::new(daypart_hours(d)).size(12.0).color(TEXT_FAINT));
                    ui.add_space(6.0);
                    // Fixed-height list so all four cards line up.
                    let row_gap = 2.0;
                    let list_h = DAYPART_ARTISTS as f32 * (DAYPART_ROW_H + row_gap) - row_gap;
                    ui.allocate_ui_with_layout(vec2(card_w - 28.0, list_h), Layout::top_down(Align::Min), |ui| {
                        ui.set_min_height(list_h);
                        ui.spacing_mut().item_spacing.y = row_gap;
                        let artists = &p.by_daypart[d.index()];
                        if artists.is_empty() {
                            ui.label(RichText::new(tr("Nothing yet")).size(12.5).color(TEXT_FAINT));
                        }
                        for a in artists.iter().take(DAYPART_ARTISTS) {
                            daypart_artist_row(ui, cx, a, card_w - 28.0);
                        }
                    });
                });
            });
        }
    });
    ui.add_space(14.0);
}

fn daypart_artist_row(ui: &mut Ui, cx: &mut Ctx, a: &ArtistTaste, width: f32) {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, DAYPART_ROW_H), Sense::click());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let clickable = a.id.as_deref().is_some_and(|id| !id.is_empty());
    if resp.hovered() && clickable {
        ui.painter().rect_filled(rect.expand2(vec2(4.0, 0.0)), CornerRadius::same(8), Color32::from_white_alpha(10));
    }
    let av = Rect::from_center_size(pos2(rect.left() + 15.0, rect.center().y), Vec2::splat(30.0));
    avatar(ui, cx, av, a);
    let x = av.right() + 10.0;
    widgets::paint_text(ui, pos2(x, rect.center().y - 9.0), a.name.trim(), FontId::proportional(13.5), TEXT, rect.right() - x);
    if clickable && resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        open_artist(cx, a);
    }
}

fn skipped(ui: &mut Ui, cx: &mut Ctx, p: &TasteProfile) {
    let songs = skipped_songs(p, SKIPPED);
    if songs.is_empty() {
        return;
    }
    section_row(ui, tr("Songs you often skip"), |_| {});
    ui.label(
        RichText::new(tr("Mochy won't recommend these in mixes and radio. Play one again if you've changed your mind."))
            .size(12.5)
            .color(TEXT_DIM),
    );
    ui.add_space(6.0);
    let subs: Vec<String> = songs
        .iter()
        .map(|s| {
            let skips = if s.skips > 0 { tr("skipped {n} times").replace("{n}", &s.skips.to_string()) } else { tr("often skipped").to_string() };
            format!("{} · {}", widgets::artists_line(&s.song), skips)
        })
        .collect();
    let list: Arc<Vec<Song>> = Arc::new(songs.iter().map(|s| s.song.clone()).collect());
    card_frame().fill(widgets::lerp_color(SURFACE, SURFACE_2, 0.35)).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let gap = 24.0;
        let col_w = ((ui.available_width() - gap) / 2.0).floor();
        let rows = list.len().div_ceil(2);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = vec2(gap, 2.0);
            for c in 0..2 {
                ui.vertical(|ui| {
                    ui.set_width(col_w);
                    for r in 0..rows {
                        let i = c * rows + r;
                        if i < list.len() {
                            shelves::compact_song_row(ui, cx, &list, i, Some(&subs[i]), col_w, false);
                        }
                    }
                });
            }
        });
    });
    ui.add_space(14.0);
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn artist(name: &str, score: f32, short_term: f32) -> ArtistTaste {
        ArtistTaste { id: Some(format!("UC{name}")), name: name.into(), score, short_term, ..Default::default() }
    }

    fn song(id: &str, score: f32, skips: u32) -> SongTaste {
        SongTaste { song: Song::remote(id, id), score, plays: 1, completed: 0, skips, liked: false, last_played: 0 }
    }

    fn profile(artists: Vec<ArtistTaste>, songs: Vec<SongTaste>, disliked: &[&str]) -> TasteProfile {
        let mut p = TasteProfile { total_events: 10, artists, songs, disliked: disliked.iter().map(|s| s.to_string()).collect::<HashSet<_>>(), ..Default::default() };
        p.rebuild_index();
        p
    }

    #[test]
    fn ranks_recent_by_short_term() {
        let a = vec![artist("A", 10.0, 1.0), artist("B", 5.0, 4.0), artist("C", 3.0, 0.0)];
        let (r, fell_back) = rank_artists(&a, Window::Recent, 10);
        assert!(!fell_back);
        let names: Vec<&str> = r.iter().map(|(a, _)| a.name.as_str()).collect();
        assert_eq!(names, ["B", "A"]);
        assert_eq!(r[0].1, 1.0);
        assert!((r[1].1 - 0.25).abs() < 1e-6);
    }

    #[test]
    fn recent_falls_back_to_score() {
        let a = vec![artist("A", 2.0, 0.0), artist("B", 8.0, 0.0), artist("", 9.0, 0.0)];
        let (r, fell_back) = rank_artists(&a, Window::Recent, 1);
        assert!(fell_back);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0.name, "B");
        let (_, fell_back) = rank_artists(&a, Window::AllTime, 5);
        assert!(!fell_back);
        assert!(rank_artists(&[], Window::Recent, 5).0.is_empty());
    }

    #[test]
    fn joins_names() {
        assert_eq!(join_names(&[], "and"), "");
        assert_eq!(join_names(&["Vũ."], "và"), "Vũ.");
        assert_eq!(join_names(&["A", "B"], "and"), "A and B");
        assert_eq!(join_names(&["Sơn Tùng M-TP", "Vũ.", "Hoàng Dũng"], "và"), "Sơn Tùng M-TP, Vũ. và Hoàng Dũng");
    }

    #[test]
    fn summary_mentions_top_artists_or_song() {
        // `tr` may be Vietnamese while another test runs; check the names only.
        let p = profile(vec![artist("Sơn Tùng M-TP", 9.0, 3.0), artist("Vũ.", 8.0, 2.0), artist("Hoàng Dũng", 7.0, 1.0), artist("X", 6.0, 0.5)], vec![], &[]);
        let s = summary(&p).unwrap();
        assert!(s.contains("Sơn Tùng M-TP, Vũ.") && s.contains("Hoàng Dũng") && !s.contains('X'), "{s}");
        let p = profile(vec![], vec![song("dislike", 9.0, 3), song("Hay", 5.0, 0)], &["dislike"]);
        assert!(summary(&p).unwrap().contains("Hay"));
        assert_eq!(summary(&profile(vec![], vec![], &[])), None);
    }

    #[test]
    fn top_and_skipped_songs() {
        let p = profile(vec![], vec![song("a", 9.0, 0), song("b", 5.0, 1), song("c", 3.0, 4), song("d", 1.0, 0), song("z", 0.0, 0)], &["b"]);
        let top: Vec<String> = top_songs(&p, 10).into_iter().map(|s| s.id).collect();
        assert_eq!(top, ["a", "c", "d"]);
        let sk: Vec<&str> = skipped_songs(&p, 10).iter().map(|s| s.song.id.as_str()).collect();
        assert_eq!(sk, ["b", "c"]);
    }

    #[test]
    fn local_hours_and_dayparts() {
        // 2026-01-01 00:30 UTC = 07:30 in UTC+7.
        let ms = 1_767_227_400_000;
        assert_eq!(local_hour(ms, 0), 0);
        assert_eq!(local_hour(ms, 7 * 3600), 7);
        assert_eq!(local_hour(ms, -3600), 23);
        assert_eq!(Daypart::from_hour(local_hour(ms, 7 * 3600)), Daypart::Morning);
        assert_eq!(daypart_hours(Daypart::Night), "22:00 – 5:00");
    }
}
