//! Listening stats & recap (port of `ui/screens/recap/StatsScreen.kt`,
//! `StatsCharts.kt`, `RecapData.kt`, `RecapStory*.kt`): totals for a
//! week / month / year / all time, top songs / artists / albums, listening
//! by day, hour and weekday drawn with the egui painter, and a paged recap
//! "story". Everything comes from the local `event` table.

use std::time::{Duration, Instant};

use egui::{Align, Align2, Color32, CornerRadius, FontId, Id, Layout, Rect, RichText, Sense, Stroke, Ui, UiBuilder, Vec2, pos2, vec2};
use mochy_core::models::ShelfItem;
use mochy_storage::StatsPeriod;

use super::library_views::month_name;
use crate::actions::{Action, Ctx};
use crate::browse::{BrowseAction, BrowseState, RecapUi, Slot, StatsData};
use crate::images::{ImageSrc, LARGE_PX};
use crate::ui::i18n::tr;
use crate::widgets::shelves;
use crate::widgets::{self, SURFACE, SURFACE_2, TEXT, TEXT_DIM, TEXT_FAINT, icons::Icon};

const RECAP_PAGES: usize = 7;
const RECAP_PAGE_SECS: f32 = 9.0;

/// "12h 05m" (Android `formatHoursMinutes`).
pub fn hours_minutes(ms: i64) -> String {
    let m = (ms / 60_000).max(0);
    format!("{}h {:02}m", m / 60, m % 60)
}

/// "3.4" hours with one decimal (Android `formatHoursDecimal`).
pub fn hours_decimal(ms: i64) -> String {
    let t = (ms / 360_000).max(0);
    format!("{}.{}", t / 10, t % 10)
}

fn trf(key: &'static str, args: &[(&str, String)]) -> String {
    let mut s = tr(key).to_string();
    for (k, v) in args {
        s = s.replace(k, v);
    }
    s
}

fn weekday_name(i: usize) -> &'static str {
    tr(["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"][i % 7])
}

fn weekday_short(i: usize) -> &'static str {
    tr(["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][i % 7])
}

fn time_of_day(hour: usize) -> &'static str {
    match hour {
        5..=11 => tr("in the mornings"),
        12..=16 => tr("in the afternoons"),
        17..=21 => tr("in the evenings"),
        _ => tr("at night"),
    }
}

fn argmax(v: &[i64]) -> Option<usize> {
    let (i, m) = v.iter().enumerate().max_by_key(|(_, m)| **m)?;
    (*m > 0).then_some(i)
}

/// "17/11"
fn short_day(key: &str) -> String {
    match (key.get(8..10), key.get(5..7)) {
        (Some(d), Some(m)) => format!("{}/{}", d.trim_start_matches('0'), m),
        _ => key.to_string(),
    }
}

/// "17 November 2026"
fn long_day(key: &str) -> String {
    let (y, m) = month_of(key);
    let d = key.get(8..10).unwrap_or("").trim_start_matches('0');
    format!("{d} {} {y}", month_name(m))
}

fn month_of(key: &str) -> (i64, u32) {
    (key.get(..4).and_then(|y| y.parse().ok()).unwrap_or(1970), key.get(5..7).and_then(|m| m.parse().ok()).unwrap_or(1))
}

/// Human label of the selected period.
pub fn period_label(d: &StatsData) -> String {
    match d.period {
        Some(StatsPeriod::Week) => match d.offset {
            0 => tr("This week").into(),
            1 => tr("Last week").into(),
            _ => format!("{} – {}", short_day(&d.first_day), short_day(&d.last_day)),
        },
        Some(StatsPeriod::Month) => {
            let (y, m) = month_of(&d.first_day);
            format!("{} {y}", month_name(m))
        }
        Some(StatsPeriod::Year) => d.first_day.get(..4).unwrap_or("").to_string(),
        Some(StatsPeriod::All) | None => tr("All time").into(),
    }
}

// ---------------------------------------------------------------------------
// Charts
// ---------------------------------------------------------------------------

/// Single-series bar chart: one accent hue, rounded data ends on a shared
/// baseline, 2 px gaps, recessive axis labels, hover tooltip per bar and a
/// direct label on the peak only.
fn bar_chart(ui: &mut Ui, id: &str, values: &[i64], labels: &[String], label_every: usize, height: f32, accent: Color32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    if !ui.is_rect_visible(rect) || values.is_empty() {
        return;
    }
    let p = ui.painter();
    let axis_h = 18.0;
    let plot = Rect::from_min_max(pos2(rect.left(), rect.top() + 16.0), pos2(rect.right(), rect.bottom() - axis_h));
    let max = values.iter().copied().max().unwrap_or(0).max(1) as f32;
    let n = values.len();
    let slot = plot.width() / n as f32;
    let gap = if slot > 8.0 { 2.0f32.max(slot * 0.18) } else { 1.0 };
    let bar_w = (slot - gap).max(1.0);
    // Recessive baseline + half grid line.
    p.line_segment([plot.left_bottom(), plot.right_bottom()], Stroke::new(1.0, Color32::from_white_alpha(28)));
    let mid = plot.bottom() - plot.height() * 0.5;
    p.line_segment([pos2(plot.left(), mid), pos2(plot.right(), mid)], Stroke::new(1.0, Color32::from_white_alpha(10)));
    let peak = argmax(values);
    let fill = widgets::accent_light(accent);
    let hover = ui.input(|i| i.pointer.hover_pos()).filter(|h| rect.contains(*h));
    let mut hovered: Option<usize> = None;
    for (i, &v) in values.iter().enumerate() {
        let x0 = plot.left() + i as f32 * slot + gap / 2.0;
        let col = Rect::from_min_max(pos2(x0, plot.top()), pos2(x0 + bar_w, plot.bottom()));
        if hover.is_some_and(|h| h.x >= x0 - gap / 2.0 && h.x < x0 + bar_w + gap / 2.0) {
            hovered = Some(i);
            p.rect_filled(col, CornerRadius::same(4), Color32::from_white_alpha(8));
        }
        if v > 0 {
            let h = (v as f32 / max * plot.height()).max(3.0);
            let r = Rect::from_min_max(pos2(x0, plot.bottom() - h), pos2(x0 + bar_w, plot.bottom()));
            let cr = (bar_w / 2.0).min(4.0) as u8;
            let c = if hovered == Some(i) || peak == Some(i) { fill } else { fill.gamma_multiply(0.62) };
            p.rect_filled(r, CornerRadius { nw: cr, ne: cr, sw: 0, se: 0 }, c);
            if peak == Some(i) && hovered.is_none() {
                p.text(pos2(r.center().x, r.top() - 3.0), Align2::CENTER_BOTTOM, hours_decimal(v) + "h", FontId::proportional(11.0), TEXT_DIM);
            }
        }
        if label_every > 0 && (i % label_every == 0 || i + 1 == n) && labels.len() == n {
            p.text(pos2(x0 + bar_w / 2.0, plot.bottom() + 4.0), Align2::CENTER_TOP, &labels[i], FontId::proportional(10.5), TEXT_FAINT);
        }
    }
    if let Some(i) = hovered {
        let tip = format!("{}: {}", labels.get(i).cloned().unwrap_or_default(), hours_minutes(values[i]));
        egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), Id::unique((id, "tip")), egui::PopupAnchor::Pointer).show(|ui| {
            ui.label(tip);
        });
    }
}

fn card_frame() -> egui::Frame {
    egui::Frame::new().fill(SURFACE).corner_radius(14).inner_margin(16)
}

fn stat_tile(ui: &mut Ui, value: &str, label: &str, w: f32) {
    card_frame().show(ui, |ui| {
        ui.set_width(w - 32.0);
        ui.label(RichText::new(value).size(26.0).strong().color(TEXT));
        ui.label(RichText::new(label).size(12.5).color(TEXT_DIM));
    });
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

pub fn show(ui: &mut Ui, cx: &mut Ctx, st: &mut BrowseState) {
    let mut period = st.stats.period;
    let mut offset = st.stats.offset;
    let data = match &st.stats.data {
        Some(Slot::Ready(d)) => Some(d.clone()),
        _ => None,
    };
    ui.horizontal(|ui| {
        widgets::page_title(ui, tr("Stats"));
        ui.add_space(16.0);
        for (p, label) in [(StatsPeriod::Week, tr("Week")), (StatsPeriod::Month, tr("Month")), (StatsPeriod::Year, tr("Year")), (StatsPeriod::All, tr("All time"))] {
            if widgets::pill(ui, label, period == p).clicked() && period != p {
                period = p;
                offset = 0;
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let ready = data.as_ref().is_some_and(|d| d.summary.plays > 0);
            ui.add_enabled_ui(ready, |ui| {
                if widgets::accent_button(ui, Icon::Play, tr("Recap"), cx.accent, true).on_hover_text(tr("Your listening story")).clicked() {
                    st.stats.recap = Some(RecapUi { page: 0, page_started: Instant::now() });
                }
            });
        });
    });
    if period != StatsPeriod::All {
        ui.horizontal(|ui| {
            if widgets::icons::icon_button(ui, Icon::Back, 28.0, false, cx.accent).on_hover_text(tr("Previous")).clicked() {
                offset += 1;
            }
            let label = data.as_ref().filter(|d| d.period == Some(period) && d.offset == offset).map(|d| period_label(d)).unwrap_or_else(|| "…".into());
            ui.label(RichText::new(label).size(16.0).strong().color(TEXT));
            ui.add_enabled_ui(offset > 0, |ui| {
                if widgets::icons::icon_button(ui, Icon::Forward, 28.0, false, cx.accent).on_hover_text(tr("Next")).clicked() {
                    offset = offset.saturating_sub(1);
                }
            });
        });
    }
    ui.add_space(10.0);
    if (period, offset) != (st.stats.period, st.stats.offset) {
        st.set_stats_period(period, offset);
    }

    let d = match &st.stats.data {
        None | Some(Slot::Loading) => {
            widgets::loading(ui, tr("Loading…"));
            return;
        }
        Some(Slot::Failed(e)) => {
            if widgets::error_box(ui, e) {
                cx.act(Action::Browse(BrowseAction::Retry("stats".into())));
            }
            return;
        }
        Some(Slot::Ready(d)) => d.clone(),
    };
    if d.summary.plays == 0 {
        card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(trf("Nothing played in {period} yet.", &[("{period}", period_label(&d))])).size(18.0).strong());
            ui.label(RichText::new(tr("Play a few songs and this fills itself in.")).color(TEXT_DIM));
        });
        privacy_note(ui, d.since_day.as_deref());
        return;
    }

    // Totals.
    let avail = ui.available_width();
    let tile_w = ((avail - 4.0 * 12.0) / 5.0).max(120.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
        card_frame().show(ui, |ui| {
            ui.set_width(tile_w * 2.0 + 12.0 - 32.0);
            ui.label(RichText::new(tr("Total listening time")).size(12.5).color(TEXT_DIM));
            ui.label(RichText::new(hours_minutes(d.summary.total_ms)).size(40.0).strong().color(widgets::accent_light(cx.accent)));
        });
        stat_tile(ui, &d.summary.plays.to_string(), tr("plays"), tile_w);
        stat_tile(ui, &d.summary.songs.to_string(), tr("songs"), tile_w);
        stat_tile(ui, &d.summary.artists.to_string(), tr("artists"), tile_w);
        stat_tile(ui, &d.summary.albums.to_string(), tr("albums"), tile_w);
    });
    ui.add_space(14.0);

    // Timeline.
    let (values, labels): (Vec<i64>, Vec<String>) = d
        .timeline
        .iter()
        .map(|(k, ms)| {
            let label = match d.period {
                Some(StatsPeriod::Week) => {
                    let i = d.timeline.iter().position(|(x, _)| x == k).unwrap_or(0);
                    weekday_short(i).to_string()
                }
                Some(StatsPeriod::Month) => k.get(8..10).unwrap_or("").trim_start_matches('0').to_string(),
                Some(StatsPeriod::Year) => month_name(month_of(&format!("{k}-01")).1).chars().take(3).collect(),
                _ => {
                    let (y, m) = month_of(&format!("{k}-01"));
                    format!("{m}/{:02}", y % 100)
                }
            };
            (*ms, label)
        })
        .unzip();
    let every = match values.len() {
        0..=12 => 1,
        13..=31 => 5,
        _ => 6,
    };
    card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        let title = match d.period {
            Some(StatsPeriod::Year | StatsPeriod::All) => tr("Listening by month"),
            _ => tr("Listening by day"),
        };
        ui.label(RichText::new(title).size(15.0).strong());
        bar_chart(ui, "timeline", &values, &labels, every, 170.0, cx.accent);
    });
    ui.add_space(12.0);

    // Hour / weekday.
    let half = ((ui.available_width() - 12.0) / 2.0).max(260.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
        card_frame().show(ui, |ui| {
            ui.set_width(half - 32.0);
            ui.label(RichText::new(tr("By hour of day")).size(15.0).strong());
            if let Some(h) = argmax(&d.buckets.by_hour) {
                ui.label(
                    RichText::new(trf("You love to listen {when}, around {hour}", &[("{when}", time_of_day(h).into()), ("{hour}", format!("{h}:00"))]))
                        .size(12.5)
                        .color(TEXT_DIM),
                );
            }
            let labels: Vec<String> = (0..24).map(|h| format!("{h}")).collect();
            bar_chart(ui, "hours", &d.buckets.by_hour, &labels, 3, 140.0, cx.accent);
        });
        card_frame().show(ui, |ui| {
            ui.set_width(half - 32.0);
            ui.label(RichText::new(tr("By weekday")).size(15.0).strong());
            if let Some(w) = argmax(&d.buckets.by_weekday) {
                ui.label(
                    RichText::new(trf("Your favourite day is {day}, {hours} hours", &[("{day}", weekday_name(w).into()), ("{hours}", hours_decimal(d.buckets.by_weekday[w]))]))
                        .size(12.5)
                        .color(TEXT_DIM),
                );
            }
            let labels: Vec<String> = (0..7).map(|i| weekday_short(i).to_string()).collect();
            bar_chart(ui, "weekdays", &d.buckets.by_weekday, &labels, 1, 140.0, cx.accent);
        });
    });
    ui.add_space(14.0);

    // Top songs.
    if !d.top_songs.is_empty() {
        shelves::section_header(ui, cx, tr("Top songs"), None, None, false);
        let list = d.top_song_list.clone();
        let w = ui.available_width();
        for (i, s) in d.top_songs.iter().take(10).enumerate() {
            ui.horizontal(|ui| {
                ui.add_sized(vec2(28.0, 56.0), egui::Label::new(RichText::new(format!("{}", i + 1)).size(15.0).strong().color(TEXT_DIM)));
                let sub = format!("{} · {} · {}h", widgets::artists_line(&s.song), trf("{n} plays", &[("{n}", s.plays.to_string())]), hours_decimal(s.ms));
                shelves::compact_song_row(ui, cx, &list, i, Some(&sub), w - 40.0, true);
            });
        }
        ui.add_space(12.0);
    }
    if !d.top_artists.is_empty() {
        shelves::section_header(ui, cx, tr("Top artists"), None, None, false);
        let items: Vec<ShelfItem> = d
            .top_artists
            .iter()
            .map(|a| {
                let mut ar = a.artist.clone();
                if ar.thumbnail_url.is_none() {
                    ar.thumbnail_url = a.fallback_thumbnail.clone();
                }
                ShelfItem::Artist(ar)
            })
            .collect();
        let subs: Vec<String> = d.top_artists.iter().map(|a| format!("{} · {}h", trf("{n} plays", &[("{n}", a.plays.to_string())]), hours_decimal(a.ms))).collect();
        shelves::cards_row(ui, cx, &items, &subs, "stats_artists");
        ui.add_space(12.0);
    }
    if !d.top_albums.is_empty() {
        shelves::section_header(ui, cx, tr("Top albums"), None, None, false);
        let items: Vec<ShelfItem> = d.top_albums.iter().map(|a| ShelfItem::Album(a.album.clone())).collect();
        let subs: Vec<String> = d.top_albums.iter().map(|a| format!("{} · {}h", a.artist, hours_decimal(a.ms))).collect();
        shelves::cards_row(ui, cx, &items, &subs, "stats_albums");
        ui.add_space(12.0);
    }
    privacy_note(ui, d.since_day.as_deref());

    if st.stats.recap.is_some() {
        recap(ui, cx, st, &d);
    }
}

fn privacy_note(ui: &mut Ui, since: Option<&str>) {
    ui.add_space(8.0);
    if let Some(day) = since {
        ui.label(RichText::new(trf("Listening with Mochy since {date}", &[("{date}", long_day(day))])).size(12.5).color(TEXT_DIM));
    }
    ui.label(RichText::new(tr("Everything here comes from what you played in this app and never leaves your computer.")).size(12.0).color(TEXT_FAINT));
    ui.add_space(24.0);
}

// ---------------------------------------------------------------------------
// Recap story
// ---------------------------------------------------------------------------

fn story_bg(accent: Color32, page: usize) -> (Color32, Color32) {
    let a = widgets::accent_light(accent);
    let tints = [
        Color32::from_rgb(0x2B, 0x16, 0x4A),
        Color32::from_rgb(0x12, 0x2E, 0x3A),
        Color32::from_rgb(0x3A, 0x18, 0x22),
        Color32::from_rgb(0x16, 0x30, 0x1E),
        Color32::from_rgb(0x34, 0x2A, 0x10),
        Color32::from_rgb(0x1A, 0x1E, 0x40),
        Color32::from_rgb(0x24, 0x24, 0x26),
    ];
    let t = tints[page % tints.len()];
    (widgets::lerp_color(t, a, 0.35), widgets::lerp_color(t, Color32::BLACK, 0.55))
}

fn big(ui: &mut Ui, text: &str, size: f32) {
    ui.label(RichText::new(text).size(size).strong().color(Color32::WHITE));
}

fn dim(ui: &mut Ui, text: &str, size: f32) {
    ui.label(RichText::new(text).size(size).color(Color32::from_white_alpha(200)));
}

fn art(ui: &mut Ui, cx: &mut Ctx, url: Option<&str>, size: f32, round: bool) {
    let (r, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let tex = url.and_then(|u| cx.images.get(&ImageSrc::Url(u.to_string()), LARGE_PX));
    widgets::paint_artwork(ui, r, tex.as_ref(), if round { size / 2.0 } else { 14.0 });
}

fn recap_page(ui: &mut Ui, cx: &mut Ctx, d: &StatsData, page: usize) {
    let label = period_label(d);
    match page {
        0 => {
            ui.add_space(60.0);
            dim(ui, &trf("{period} recap", &[("{period}", label.clone())]), 18.0);
            ui.add_space(24.0);
            dim(ui, tr("you have been listening to music for"), 17.0);
            big(ui, &hours_minutes(d.summary.total_ms), 54.0);
            ui.add_space(18.0);
            dim(ui, &trf("{plays} plays · {songs} songs · {artists} artists", &[
                ("{plays}", d.summary.plays.to_string()),
                ("{songs}", d.summary.songs.to_string()),
                ("{artists}", d.summary.artists.to_string()),
            ]), 15.0);
        }
        1 => {
            let Some(s) = d.top_songs.first() else { return };
            ui.add_space(30.0);
            dim(ui, tr("Your favourite song"), 18.0);
            ui.add_space(16.0);
            art(ui, cx, s.song.thumbnail_url.as_deref(), 220.0, false);
            ui.add_space(14.0);
            big(ui, &s.song.title, 26.0);
            dim(ui, &widgets::artists_line(&s.song), 16.0);
            ui.add_space(10.0);
            big(ui, &trf("{n} plays!", &[("{n}", s.plays.to_string())]), 30.0);
            if let Some(second) = d.top_songs.get(1) {
                let more = s.plays.saturating_sub(second.plays);
                if more > 0 {
                    dim(ui, &trf("{n} more than your #2", &[("{n}", more.to_string())]), 14.0);
                } else {
                    dim(ui, tr("neck and neck with your #2"), 14.0);
                }
            }
        }
        2 => {
            let Some(a) = d.top_artists.first() else { return };
            ui.add_space(30.0);
            dim(ui, tr("You really love listening to this artist"), 18.0);
            ui.add_space(16.0);
            art(ui, cx, a.artist.thumbnail_url.as_deref().or(a.fallback_thumbnail.as_deref()), 200.0, true);
            ui.add_space(14.0);
            big(ui, &a.artist.name, 30.0);
            dim(ui, &trf("{n} plays · {h} hours", &[("{n}", a.plays.to_string()), ("{h}", hours_decimal(a.ms))]), 16.0);
        }
        3 => {
            ui.add_space(30.0);
            big(ui, tr("Your top 5"), 30.0);
            ui.add_space(14.0);
            for (i, s) in d.top_songs.iter().take(5).enumerate() {
                ui.horizontal(|ui| {
                    ui.add_space(24.0);
                    big(ui, &format!("{}", i + 1), 24.0);
                    ui.add_space(8.0);
                    art(ui, cx, s.song.thumbnail_url.as_deref(), 52.0, false);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&s.song.title).size(15.5).strong().color(Color32::WHITE));
                        ui.label(RichText::new(trf("{n} plays", &[("{n}", s.plays.to_string())])).size(12.5).color(Color32::from_white_alpha(190)));
                    });
                });
                ui.add_space(6.0);
            }
        }
        4 => {
            let Some(h) = argmax(&d.buckets.by_hour) else { return };
            ui.add_space(50.0);
            dim(ui, &trf("And you love to listen {when}, exactly around", &[("{when}", time_of_day(h).into())]), 18.0);
            ui.add_space(10.0);
            big(ui, &format!("{h}:00"), 64.0);
            dim(ui, &trf("totaling {h} hours at this time!", &[("{h}", hours_decimal(d.buckets.by_hour[h]))]), 15.0);
            ui.add_space(20.0);
            let labels: Vec<String> = (0..24).map(|x| x.to_string()).collect();
            ui.scope(|ui| {
                ui.set_max_width(ui.available_width() - 40.0);
                bar_chart(ui, "recap_hours", &d.buckets.by_hour, &labels, 6, 120.0, Color32::WHITE);
            });
        }
        5 => {
            let Some(w) = argmax(&d.buckets.by_weekday) else { return };
            ui.add_space(70.0);
            big(ui, weekday_name(w), 56.0);
            dim(ui, tr("is your favourite day to listen to music"), 18.0);
            ui.add_space(10.0);
            dim(ui, &trf("totaling {h} hours", &[("{h}", hours_decimal(d.buckets.by_weekday[w]))]), 15.0);
        }
        _ => {
            // The receipt.
            ui.add_space(20.0);
            big(ui, tr("Let's see your bill"), 24.0);
            ui.add_space(10.0);
            egui::Frame::new().fill(Color32::from_rgb(0xF4, 0xF1, 0xEA)).corner_radius(6).inner_margin(18).show(ui, |ui| {
                ui.set_width(ui.available_width().min(380.0) - 36.0);
                let ink = Color32::from_rgb(0x22, 0x22, 0x22);
                let mono = |ui: &mut Ui, t: &str, size: f32| {
                    ui.label(RichText::new(t).monospace().size(size).color(ink));
                };
                mono(ui, &trf("{period} recap", &[("{period}", label.clone())]).to_uppercase(), 15.0);
                mono(ui, "--------------------------------", 11.0);
                mono(ui, tr("TOP ARTIST"), 12.0);
                for a in d.top_artists.iter().take(3) {
                    mono(ui, &format!("{:<22} {:>6}h", truncate(&a.artist.name, 22), hours_decimal(a.ms)), 12.0);
                }
                mono(ui, "--------------------------------", 11.0);
                mono(ui, tr("TOP MUSIC"), 12.0);
                for s in d.top_songs.iter().take(5) {
                    mono(ui, &format!("{:<24} x{:>3}", truncate(&s.song.title, 24), s.plays), 12.0);
                }
                mono(ui, "--------------------------------", 11.0);
                mono(ui, &trf("TOTAL: {t} playtime", &[("{t}", hours_minutes(d.summary.total_ms))]), 13.0);
                mono(ui, &trf("{plays} plays · {songs} songs · {artists} artists", &[
                    ("{plays}", d.summary.plays.to_string()),
                    ("{songs}", d.summary.songs.to_string()),
                    ("{artists}", d.summary.artists.to_string()),
                ]), 11.5);
                mono(ui, "--------------------------------", 11.0);
                mono(ui, tr("END OF LINE"), 12.0);
            });
            ui.add_space(12.0);
            dim(ui, tr("There you go"), 16.0);
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n - 1).collect::<String>()) }
}

/// Which story pages have content for this period.
fn recap_pages(d: &StatsData) -> Vec<usize> {
    (0..RECAP_PAGES)
        .filter(|&p| match p {
            1 | 3 => !d.top_songs.is_empty(),
            2 => !d.top_artists.is_empty(),
            4 => argmax(&d.buckets.by_hour).is_some(),
            5 => argmax(&d.buckets.by_weekday).is_some(),
            _ => true,
        })
        .collect()
}

fn recap(ui: &mut Ui, cx: &mut Ctx, st: &mut BrowseState, d: &StatsData) {
    let pages = recap_pages(d);
    let Some(r) = st.stats.recap.as_mut() else { return };
    r.page = r.page.min(pages.len().saturating_sub(1));
    let ctx = ui.ctx().clone();
    let screen = ctx.content_rect();
    let mut close = false;
    let mut step: i32 = 0;
    let elapsed = r.page_started.elapsed().as_secs_f32();
    if elapsed >= RECAP_PAGE_SECS && r.page + 1 < pages.len() {
        step = 1;
    }
    ctx.request_repaint_after(Duration::from_millis(50));
    egui::Area::new(Id::unique("mochy_recap"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(&ctx, |ui| {
            let (bg_rect, bg) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(bg_rect, 0.0, Color32::from_black_alpha(215));
            let h = (screen.height() - 60.0).clamp(420.0, 860.0);
            let w = (h * 0.6).min(screen.width() - 40.0);
            let card = Rect::from_center_size(screen.center(), vec2(w, h));
            let (top, bottom) = story_bg(cx.accent, pages[r.page]);
            // Two-tone card: accent-tinted top band fading into the darker base.
            ui.painter().rect_filled(card, CornerRadius::same(20), bottom);
            let bands = 12;
            for i in 0..bands {
                let t0 = i as f32 / bands as f32;
                let band = Rect::from_min_max(pos2(card.left(), card.top() + card.height() * t0 * 0.6), pos2(card.right(), card.top() + card.height() * (t0 + 1.0 / bands as f32) * 0.6));
                let cr = if i == 0 { CornerRadius { nw: 20, ne: 20, sw: 0, se: 0 } } else { CornerRadius::ZERO };
                ui.painter().rect_filled(band, cr, widgets::lerp_color(top, bottom, t0));
            }
            ui.painter().rect_stroke(card, CornerRadius::same(20), Stroke::new(2.0, Color32::from_black_alpha(90)), egui::StrokeKind::Outside);
            let card_resp = ui.interact(card, Id::unique("mochy_recap_card"), Sense::click());
            if card_resp.clicked()
                && let Some(p) = card_resp.interact_pointer_pos()
            {
                step = if p.x < card.left() + card.width() * 0.33 { -1 } else { 1 };
            }
            if bg.clicked() {
                close = true;
            }
            // Progress segments.
            let n = pages.len().max(1);
            let seg_w = (card.width() - 32.0 - (n as f32 - 1.0) * 4.0) / n as f32;
            for i in 0..n {
                let x = card.left() + 16.0 + i as f32 * (seg_w + 4.0);
                let bar = Rect::from_min_size(pos2(x, card.top() + 14.0), vec2(seg_w, 3.0));
                ui.painter().rect_filled(bar, CornerRadius::same(2), Color32::from_white_alpha(60));
                let frac = if i < r.page { 1.0 } else if i == r.page { (elapsed / RECAP_PAGE_SECS).min(1.0) } else { 0.0 };
                let mut f = bar;
                f.set_right(bar.left() + bar.width() * frac);
                ui.painter().rect_filled(f, CornerRadius::same(2), Color32::WHITE);
            }
            let inner = card.shrink2(vec2(22.0, 34.0));
            ui.scope_builder(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Center)), |ui| {
                recap_page(ui, cx, d, pages[r.page]);
            });
            // Controls.
            let close_r = Rect::from_center_size(pos2(card.right() - 24.0, card.top() + 38.0), Vec2::splat(30.0));
            if ui.put(close_r, egui::Button::new(RichText::new("✕").size(16.0).color(Color32::WHITE)).frame(false)).on_hover_text(tr("Close")).clicked() {
                close = true;
            }
            let prev_r = Rect::from_center_size(pos2(card.left() - 34.0, card.center().y), Vec2::splat(44.0));
            let next_r = Rect::from_center_size(pos2(card.right() + 34.0, card.center().y), Vec2::splat(44.0));
            if r.page > 0 && ui.put(prev_r, egui::Button::new(RichText::new("‹").size(28.0)).corner_radius(22).fill(SURFACE_2)).clicked() {
                step = -1;
            }
            if r.page + 1 < pages.len() && ui.put(next_r, egui::Button::new(RichText::new("›").size(28.0)).corner_radius(22).fill(SURFACE_2)).clicked() {
                step = 1;
            }
            ui.painter().text(
                pos2(card.center().x, card.bottom() - 14.0),
                Align2::CENTER_BOTTOM,
                tr("Click to continue"),
                FontId::proportional(11.0),
                Color32::from_white_alpha(140),
            );
        });
    if step != 0 {
        let next = r.page as i32 + step;
        if next >= pages.len() as i32 {
            close = true;
        } else {
            r.page = next.max(0) as usize;
            r.page_started = Instant::now();
        }
    }
    if close {
        st.stats.recap = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_formats() {
        assert_eq!(hours_minutes(0), "0h 00m");
        assert_eq!(hours_minutes(3_660_000 + 5 * 60_000), "1h 06m");
        assert_eq!(hours_decimal(5_400_000), "1.5");
        assert_eq!(short_day("2026-11-07"), "7/11");
        assert_eq!(argmax(&[0, 3, 2]), Some(1));
        assert_eq!(argmax(&[0, 0]), None);
    }

    #[test]
    fn recap_skips_empty_pages() {
        let d = StatsData::default();
        assert_eq!(recap_pages(&d), vec![0, 6]);
    }
}
