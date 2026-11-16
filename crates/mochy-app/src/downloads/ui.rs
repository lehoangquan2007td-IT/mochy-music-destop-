//! Download UI: song-menu entries, the row badge, the "Download" button of
//! album / playlist pages, the Library → Downloaded tab (Android
//! `LibraryCachedTab` + the transfer sheet) and Settings → Downloads &
//! storage (Android `settings/storage/*`).

use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use egui::{Color32, CornerRadius, Painter, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use mochy_core::models::Song;
use mochy_core::settings::{AppSettings, AudioQuality};

use super::{Item, Stage, Status};
use crate::actions::{Action, Ctx};
use crate::ui::i18n::tr;
use crate::widgets::{self, RowOpts, TEXT, TEXT_DIM};

/// A downward arrow into a tray.
pub fn paint_download_icon(painter: &Painter, rect: Rect, color: Color32) {
    let s = rect.width().min(rect.height());
    let r = Rect::from_center_size(rect.center(), vec2(s, s));
    let w = (s * 0.1).max(1.3);
    let st = Stroke::new(w, color);
    let p = |x: f32, y: f32| pos2(r.left() + x * s, r.top() + y * s);
    painter.line_segment([p(0.5, 0.12), p(0.5, 0.66)], st);
    painter.add(egui::Shape::line(vec![p(0.28, 0.45), p(0.5, 0.67), p(0.72, 0.45)], st));
    painter.add(egui::Shape::line(vec![p(0.16, 0.7), p(0.16, 0.86), p(0.84, 0.86), p(0.84, 0.7)], st));
}

/// Small state mark after a song row's title: done ✓-arrow in accent,
/// a progress ring while downloading.
pub fn row_badge(ui: &Ui, rect: Rect, song_id: &str, accent: Color32) {
    let Some((status, progress)) = super::status(song_id) else { return };
    let painter = ui.painter();
    match status {
        Status::Done => paint_download_icon(painter, rect, widgets::accent_light(accent)),
        Status::Queued | Status::Running => {
            let c = rect.center();
            let r = rect.width().min(rect.height()) * 0.42;
            painter.circle_stroke(c, r, Stroke::new(1.5, Color32::from_white_alpha(40)));
            let n = 24;
            let end = (n as f32 * progress.max(0.04)).ceil() as usize;
            let pts: Vec<_> = (0..=end)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2 + i as f32 / n as f32 * std::f32::consts::TAU;
                    pos2(c.x + r * a.cos(), c.y + r * a.sin())
                })
                .collect();
            painter.add(egui::Shape::line(pts, Stroke::new(1.8, accent)));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
        }
        Status::Failed => {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, "!", egui::FontId::proportional(rect.height()), Color32::from_rgb(0xFF, 0x8A, 0x8A));
        }
        Status::Cancelled => {}
    }
}

/// Download / remove / edit entries for the shared song menu.
pub fn song_menu_items(ui: &mut Ui, cx: &mut Ctx, song: &Song) {
    let _ = cx;
    ui.separator();
    if song.video_id.is_some() && !song.is_local() {
        let id = song.video_id.clone().unwrap_or_default();
        match super::status(&id).map(|s| s.0) {
            Some(Status::Done) => {
                if ui.button(tr("Remove download")).clicked() {
                    super::remove(&id);
                    ui.close();
                }
            }
            Some(Status::Queued | Status::Running) => {
                if ui.button(tr("Cancel download")).clicked() {
                    super::cancel(&id);
                    ui.close();
                }
            }
            _ => {
                if ui.button(tr("Download")).clicked() {
                    super::enqueue(vec![song.clone()]);
                    ui.close();
                }
            }
        }
    }
    if ui.button(tr("Edit song…")).clicked() {
        crate::songedit::ui::open(song);
        ui.close();
    }
    if ui.button(tr("Import lyrics…")).clicked() {
        crate::songedit::ui::import_lyrics(ui.ctx(), song);
        ui.close();
    }
}

/// "Download" for a whole album / playlist (detail page header).
pub fn download_all_button(ui: &mut Ui, cx: &mut Ctx, songs: &Arc<Vec<Song>>) {
    let ids: Vec<&str> = songs.iter().filter(|s| !s.is_local()).filter_map(|s| s.video_id.as_deref()).collect();
    if ids.is_empty() {
        return;
    }
    let mut done = 0;
    let mut active = 0;
    for id in &ids {
        match super::status(id).map(|s| s.0) {
            Some(Status::Done) => done += 1,
            Some(Status::Queued | Status::Running) => active += 1,
            _ => {}
        }
    }
    let label = if done == ids.len() {
        tr("Downloaded").to_string()
    } else if active > 0 {
        tr("Downloading {done}/{total}").replace("{done}", &done.to_string()).replace("{total}", &ids.len().to_string())
    } else {
        tr("Download").to_string()
    };
    let resp = icon_text_button(ui, &label, cx.accent, done == ids.len());
    if resp.clicked() {
        if done == ids.len() {
            cx.act(Action::Navigate(crate::nav::Route::Library(crate::nav::LibTab::Downloaded)));
        } else if active > 0 {
            for id in &ids {
                if matches!(super::status(id).map(|s| s.0), Some(Status::Queued | Status::Running)) {
                    super::cancel(id);
                }
            }
        } else {
            super::enqueue(songs.as_ref().clone());
        }
    }
    if active > 0 {
        resp.on_hover_text(tr("Click to cancel"));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(400));
    }
}

/// Pill button with the download icon (same look as `widgets::accent_button` outlined).
fn icon_text_button(ui: &mut Ui, text: &str, accent: Color32, highlighted: bool) -> egui::Response {
    let font = egui::FontId::proportional(14.0);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, TEXT);
    let size = vec2(galley.size().x + 52.0, 36.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let fill = if resp.hovered() { Color32::from_white_alpha(28) } else { Color32::from_white_alpha(14) };
    ui.painter().rect_filled(rect, CornerRadius::same(18), fill);
    let icon_rect = Rect::from_center_size(pos2(rect.left() + 22.0, rect.center().y), vec2(18.0, 18.0));
    paint_download_icon(ui.painter(), icon_rect, if highlighted { widgets::accent_light(accent) } else { TEXT });
    ui.painter().galley(pos2(rect.left() + 38.0, rect.center().y - galley.size().y / 2.0), galley, TEXT);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn fmt_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} GB", b / (1024.0 * 1024.0 * 1024.0))
    } else if b >= 1024.0 * 1024.0 {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    } else {
        format!("{:.0} KB", b / 1024.0)
    }
}

fn open_in_explorer(p: &std::path::Path) {
    let _ = std::fs::create_dir_all(p);
    #[cfg(windows)]
    let r = std::process::Command::new("explorer").arg(p).spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(p).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(p).spawn();
    if let Err(e) = r {
        tracing::warn!("open folder failed: {e}");
    }
}

/// Finished songs as a list for `song_list`, rebuilt only when downloads change.
static DONE_LIST: LazyLock<Mutex<(u64, Arc<Vec<Song>>)>> = LazyLock::new(|| Mutex::new((u64::MAX, Arc::new(Vec::new()))));

/// Library → Downloaded.
pub fn downloaded_tab(ui: &mut Ui, cx: &mut Ctx) {
    let (version, items) = super::snapshot();
    let done_list = {
        let mut cache = DONE_LIST.lock().unwrap_or_else(|e| e.into_inner());
        if cache.0 != version {
            let mut songs: Vec<Song> = items
                .iter()
                .filter(|i| i.status == Status::Done)
                .map(|i| {
                    let mut s = i.song.clone();
                    crate::songedit::apply(&mut s);
                    s
                })
                .collect();
            songs.reverse(); // newest first
            *cache = (version, Arc::new(songs));
        }
        cache.1.clone()
    };
    let (dl_bytes, _) = super::usage();
    let pending: Vec<&Item> = items.iter().filter(|i| i.status != Status::Done).collect();

    ui.horizontal(|ui| {
        if !done_list.is_empty() {
            if widgets::accent_button(ui, widgets::icons::Icon::Play, tr("Play"), cx.accent, true).clicked() {
                cx.act(Action::PlayList { songs: done_list.clone(), start: 0, shuffle: false });
            }
            if widgets::accent_button(ui, widgets::icons::Icon::Shuffle, tr("Shuffle"), cx.accent, false).clicked() {
                cx.act(Action::PlayList { songs: done_list.clone(), start: 0, shuffle: true });
            }
        }
        if widgets::accent_button(ui, widgets::icons::Icon::Folder, tr("Open folder"), cx.accent, false).clicked() {
            open_in_explorer(&super::current_folder());
        }
        ui.label(RichText::new(format!("{} · {}", crate::ui::i18n::songs_count(done_list.len()), fmt_size(dl_bytes))).color(TEXT_DIM));
    });
    ui.add_space(12.0);

    if !pending.is_empty() {
        ui.horizontal(|ui| {
            widgets::section_title(ui, tr("In progress"));
            ui.add_space(10.0);
            if pending.iter().any(|i| i.is_active()) && ui.button(tr("Cancel all")).clicked() {
                super::cancel_all();
            }
            if pending.iter().any(|i| matches!(i.status, Status::Failed | Status::Cancelled)) {
                if ui.button(tr("Retry failed")).clicked() {
                    super::retry_failed();
                }
                if ui.button(tr("Clear")).clicked() {
                    super::clear_failed();
                }
            }
        });
        ui.add_space(6.0);
        for item in &pending {
            pending_row(ui, cx, item);
        }
        ui.add_space(16.0);
        if pending.iter().any(|i| i.is_active()) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    if done_list.is_empty() {
        if pending.is_empty() {
            ui.label(RichText::new(tr("No downloads yet — right-click a song and choose Download.")).color(TEXT_DIM));
        }
        return;
    }
    widgets::song_list(ui, cx, &done_list, RowOpts { show_album: true, ..Default::default() });
}

fn pending_row(ui: &mut Ui, cx: &mut Ctx, item: &Item) {
    let id = item.song.video_id.clone().unwrap_or_default();
    egui::Frame::new().fill(widgets::SURFACE).corner_radius(10).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 330.0).max(160.0);
            ui.vertical(|ui| {
                ui.set_width(w);
                let mut title = item.song.title.clone();
                if title.chars().count() > 70 {
                    title = title.chars().take(70).collect::<String>() + "…";
                }
                ui.label(RichText::new(title).color(TEXT));
                ui.label(RichText::new(widgets::artists_line(&item.song)).size(12.0).color(TEXT_DIM));
            });
            let status = match (item.status, item.stage) {
                (Status::Running, Stage::Resolving) => tr("Finding stream…").to_string(),
                (Status::Running, Stage::Tagging) => tr("Saving…").to_string(),
                (Status::Running, Stage::Fetching) => match item.total_bytes {
                    Some(t) => format!("{} / {}", fmt_size(item.done_bytes), fmt_size(t)),
                    None => fmt_size(item.done_bytes),
                },
                (s, _) => s.label().to_string(),
            };
            ui.add(egui::ProgressBar::new(item.progress()).desired_width(140.0).fill(cx.accent));
            ui.label(RichText::new(status).size(12.0).color(if item.status == Status::Failed { Color32::from_rgb(0xFF, 0x8A, 0x8A) } else { TEXT_DIM }))
                .on_hover_text(item.error.clone().unwrap_or_default());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if item.is_active() {
                    if ui.small_button(tr("Cancel")).clicked() {
                        super::cancel(&id);
                    }
                } else {
                    if ui.small_button(tr("Remove")).clicked() {
                        super::remove(&id);
                    }
                    if ui.small_button(tr("Retry")).clicked() {
                        super::retry(&id);
                    }
                }
            });
        });
    });
    ui.add_space(4.0);
}

// ---------------------------------------------------------------------------
// Settings → Downloads & storage
// ---------------------------------------------------------------------------

static FOLDER_PICK: LazyLock<Mutex<Option<PathBuf>>> = LazyLock::new(|| Mutex::new(None));

fn quality_name(q: AudioQuality) -> &'static str {
    tr(match q {
        AudioQuality::Low => "Low",
        AudioQuality::Medium => "Medium",
        AudioQuality::High => "High",
        _ => "Auto",
    })
}

fn cache_label(mb: i32) -> String {
    match mb {
        m if m < 0 => tr("Unlimited").to_string(),
        0 => tr("Off").to_string(),
        m if m < 1024 => format!("{m} MB"),
        m => format!("{} GB", m / 1024),
    }
}

/// The "Downloads & storage" settings group (called from the Settings page).
pub fn settings_section(ui: &mut Ui, s: &mut AppSettings) {
    if let Some(p) = FOLDER_PICK.lock().unwrap_or_else(|e| e.into_inner()).take() {
        s.downloads.folder = p.display().to_string();
    }
    ui.add_space(10.0);
    ui.label(RichText::new(tr("Downloads & storage")).size(18.0).strong().color(TEXT));
    ui.add_space(6.0);
    egui::Frame::new().fill(widgets::SURFACE).corner_radius(14).inner_margin(16).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 10.0;
        let d = &mut s.downloads;
        let hint = |ui: &mut Ui, t: &str| {
            ui.label(RichText::new(t).size(12.0).color(TEXT_DIM));
        };

        ui.horizontal(|ui| {
            ui.label(tr("Download folder"));
            let default = super::default_folder().display().to_string();
            ui.add(egui::TextEdit::singleline(&mut d.folder).desired_width(320.0).hint_text(default));
            if ui.button(tr("Browse…")).clicked() {
                let ctx = ui.ctx().clone();
                std::thread::spawn(move || {
                    if let Some(dir) = rfd::FileDialog::new().set_title(tr("Download folder")).pick_folder() {
                        *FOLDER_PICK.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir);
                        ctx.request_repaint();
                    }
                });
            }
            if ui.button(tr("Open")).clicked() {
                open_in_explorer(&super::folder_of(d));
            }
        });
        ui.horizontal(|ui| {
            ui.label(tr("File name"));
            ui.add(egui::TextEdit::singleline(&mut d.filename_template).desired_width(260.0).hint_text(mochy_download::naming::DEFAULT_TEMPLATE));
            if ui.small_button(tr("Reset")).clicked() {
                d.filename_template = mochy_download::naming::DEFAULT_TEMPLATE.into();
            }
        });
        let example = mochy_download::render_template(
            &d.filename_template,
            &mochy_download::NameFields {
                title: "Lạc Trôi".into(),
                artist: "Sơn Tùng M-TP".into(),
                album: "m-tp M-TP".into(),
                album_artist: "Sơn Tùng M-TP".into(),
                year: Some(2017),
                id: "Llw9Q6akRo4".into(),
            },
        );
        hint(ui, &format!("{} {{title}} {{artist}} {{album}} {{album_artist}} {{year}} {{id}} — {} \"/\" · {} {}.opus", tr("Tokens:"), tr("sub-folders with"), tr("e.g."), example.display()));

        ui.horizontal(|ui| {
            ui.label(tr("Download quality"));
            egui::ComboBox::from_id_salt("dl_quality").selected_text(quality_name(d.quality)).show_ui(ui, |ui| {
                for q in [AudioQuality::Low, AudioQuality::Medium, AudioQuality::High] {
                    ui.selectable_value(&mut d.quality, q, quality_name(q));
                }
            });
            ui.checkbox(&mut d.prefer_opus, tr("Prefer Opus (.opus) over AAC (.m4a)"));
        });
        ui.horizontal(|ui| {
            ui.label(tr("Downloads at once"));
            ui.add(egui::Slider::new(&mut d.max_parallel, 1..=6));
        });
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut d.embed_cover, tr("Embed cover art"));
            ui.checkbox(&mut d.embed_lyrics, tr("Embed lyrics"));
            ui.checkbox(&mut d.lyrics_file, tr("Save lyrics file (.lrc)"));
        });
        ui.checkbox(&mut d.play_offline, tr("Play downloaded songs from disk (offline)"));
        let was = d.auto_download_liked;
        ui.checkbox(&mut d.auto_download_liked, tr("Download songs as I like them"));
        if d.auto_download_liked && !was {
            // Songs liked before switching it on aren't fetched (Android AutoDownloads mark).
            d.auto_download_liked_since = Some(
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|t| t.as_millis() as i64).unwrap_or(0),
            );
        }

        ui.separator();
        let (dl, cache) = super::usage();
        ui.horizontal(|ui| {
            ui.label(tr("Song cache"));
            egui::ComboBox::from_id_salt("song_cache").selected_text(cache_label(d.cache_max_mb)).show_ui(ui, |ui| {
                for mb in [0, 256, 512, 1024, 2048, 4096, 8192, -1] {
                    ui.selectable_value(&mut d.cache_max_mb, mb, cache_label(mb));
                }
            });
            if ui.button(tr("Clear cache")).clicked() {
                super::clear_cache();
            }
        });
        hint(ui, tr("Keeps played songs so they play again without a connection. Each song is fetched a second time in the background."));
        hint(
            ui,
            &format!(
                "{}: {} · {}: {} · {}: {}",
                tr("Downloads"),
                fmt_size(dl),
                tr("Song cache"),
                fmt_size(cache),
                tr("Cache folder"),
                super::cache_folder().display()
            ),
        );
    });
}
