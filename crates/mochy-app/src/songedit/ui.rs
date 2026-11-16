//! The "Edit song" window (Android `ui/menu/songedit/*`, desktop-sized):
//! title, artists, album, year, cover (link or picture file) and a lyrics
//! file; plus the stand-alone "Import lyrics…" picker.

use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use egui::{Color32, RichText, Sense, vec2};
use mochy_core::models::Song;
use mochy_download::lyrics_file::PickedLyrics;

use crate::images::{ImageCache, ImageSrc, LARGE_PX};
use crate::services::Services;
use crate::ui::i18n::tr;
use crate::widgets::{self, TEXT_DIM};

/// A file-picker result arriving from its thread.
enum Picked {
    Cover(Result<String, String>),
    Lyrics(Result<(String, PickedLyrics), String>),
}

struct EditDialog {
    song: Song,
    title: String,
    artists: String,
    album: String,
    year: String,
    cover_url: String,
    /// New cover chosen (URL typed or picture copied in).
    cover: Option<String>,
    lyrics: Option<(String, PickedLyrics)>,
    message: Option<String>,
    busy: bool,
    inbox: Arc<Mutex<Option<Picked>>>,
}

static DIALOG: LazyLock<Mutex<Option<EditDialog>>> = LazyLock::new(|| Mutex::new(None));
/// "Import lyrics…" without the dialog: (song, result).
type LyricsInbox = Vec<(Song, Result<(String, PickedLyrics), String>)>;
static LYRICS_INBOX: LazyLock<Mutex<LyricsInbox>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Open the edit window for `song`.
pub fn open(song: &Song) {
    let mut s = song.clone();
    super::apply(&mut s);
    let d = EditDialog {
        title: s.title.clone(),
        artists: s.artists.iter().map(|a| a.name.clone()).collect::<Vec<_>>().join(", "),
        album: s.album_name.clone().unwrap_or_default(),
        year: s.year.map(|y| y.to_string()).unwrap_or_default(),
        cover_url: String::new(),
        cover: None,
        lyrics: None,
        message: None,
        busy: false,
        inbox: Arc::new(Mutex::new(None)),
        song: s,
    };
    *DIALOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(d);
}

fn pick_lyrics_file(ctx: egui::Context, done: impl FnOnce(Result<(String, PickedLyrics), String>) + Send + 'static) {
    std::thread::spawn(move || {
        let Some(path) = rfd::FileDialog::new()
            .set_title(tr("Import lyrics"))
            .add_filter(tr("Lyrics"), &["lrc", "ttml", "xml", "txt", "srt", "vtt", "ass", "ssa", "yrc", "qrc", "krc"])
            .add_filter(tr("All files"), &["*"])
            .pick_file()
        else {
            return;
        };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let r = mochy_download::lyrics_file::read_file(&path).map(|p| (name, p)).ok_or_else(|| tr("No lyrics found in that file").to_string());
        done(r);
        ctx.request_repaint();
    });
}

/// Pick a lyrics file for `song` and store it right away.
pub fn import_lyrics(ctx: &egui::Context, song: &Song) {
    let s = song.clone();
    pick_lyrics_file(ctx.clone(), move |r| LYRICS_INBOX.lock().unwrap_or_else(|e| e.into_inner()).push((s, r)));
}

fn pick_cover(ctx: egui::Context, song_id: String, inbox: Arc<Mutex<Option<Picked>>>) {
    std::thread::spawn(move || {
        let Some(path): Option<PathBuf> = rfd::FileDialog::new()
            .set_title(tr("Choose a cover picture"))
            .add_filter(tr("Pictures"), &["jpg", "jpeg", "png", "webp", "bmp", "gif"])
            .pick_file()
        else {
            *inbox.lock().unwrap_or_else(|e| e.into_inner()) = Some(Picked::Cover(Err(String::new())));
            ctx.request_repaint();
            return;
        };
        let r = super::save_cover_file(&song_id, &path);
        *inbox.lock().unwrap_or_else(|e| e.into_inner()) = Some(Picked::Cover(r));
        ctx.request_repaint();
    });
}

/// Draw the edit window (if open) and finish stand-alone lyrics imports.
pub fn show(ctx: &egui::Context, svc: &Services, images: &mut ImageCache) {
    for (song, r) in std::mem::take(&mut *LYRICS_INBOX.lock().unwrap_or_else(|e| e.into_inner())) {
        match r {
            Ok((_, picked)) => super::import_lyrics(svc, &song, picked.text),
            Err(e) => svc.bus.toast(e),
        }
    }

    let mut guard = DIALOG.lock().unwrap_or_else(|e| e.into_inner());
    let Some(d) = guard.as_mut() else { return };
    if let Some(p) = d.inbox.lock().unwrap_or_else(|e| e.into_inner()).take() {
        d.busy = false;
        match p {
            Picked::Cover(Ok(url)) => {
                d.cover = Some(url);
                d.message = None;
            }
            Picked::Cover(Err(e)) => d.message = (!e.is_empty()).then_some(e),
            Picked::Lyrics(Ok(l)) => {
                d.lyrics = Some(l);
                d.message = None;
            }
            Picked::Lyrics(Err(e)) => d.message = Some(e),
        }
    }

    let mut open = true;
    let mut save = false;
    let mut cancel = false;
    egui::Window::new(tr("Edit song"))
        .id(egui::Id::unique("mochy_song_edit"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .default_width(520.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Cover preview.
                let (rect, _) = ui.allocate_exact_size(vec2(140.0, 140.0), Sense::hover());
                let src = d
                    .cover
                    .as_deref()
                    .or(d.song.thumbnail_url.as_deref())
                    .map(|u| match super::file_url_path(u) {
                        Some(p) => ImageSrc::Local(p),
                        None => ImageSrc::Url(u.to_string()),
                    })
                    .or_else(|| crate::images::song_src(&d.song));
                let tex = src.and_then(|s| images.get(&s, LARGE_PX));
                widgets::paint_artwork(ui, rect, tex.as_ref(), 10.0);
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    egui::Grid::new("song_edit_grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                        ui.label(tr("Title"));
                        ui.add(egui::TextEdit::singleline(&mut d.title).desired_width(300.0));
                        ui.end_row();
                        ui.label(tr("Artists"));
                        ui.add(egui::TextEdit::singleline(&mut d.artists).desired_width(300.0).hint_text(tr("Separate with commas")));
                        ui.end_row();
                        ui.label(tr("Album"));
                        ui.add(egui::TextEdit::singleline(&mut d.album).desired_width(300.0));
                        ui.end_row();
                        ui.label(tr("Year"));
                        ui.add(egui::TextEdit::singleline(&mut d.year).desired_width(80.0));
                        ui.end_row();
                    });
                });
            });
            ui.add_space(8.0);
            ui.label(RichText::new(tr("Cover")).strong());
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut d.cover_url).desired_width(300.0).hint_text("https://…"));
                if ui.button(tr("Use link")).clicked() {
                    let u = d.cover_url.trim();
                    if u.starts_with("http://") || u.starts_with("https://") {
                        d.cover = Some(u.to_string());
                        d.message = None;
                    } else {
                        d.message = Some(tr("Enter a link starting with https://").to_string());
                    }
                }
                if ui.add_enabled(!d.busy, egui::Button::new(tr("Choose picture…"))).clicked() {
                    d.busy = true;
                    pick_cover(ctx.clone(), d.song.id.clone(), d.inbox.clone());
                }
            });
            ui.add_space(4.0);
            ui.label(RichText::new(tr("Lyrics")).strong());
            ui.horizontal(|ui| {
                if ui.add_enabled(!d.busy, egui::Button::new(tr("Import lyrics file…"))).clicked() {
                    d.busy = true;
                    let inbox = d.inbox.clone();
                    pick_lyrics_file(ctx.clone(), move |r| {
                        *inbox.lock().unwrap_or_else(|e| e.into_inner()) = Some(Picked::Lyrics(r));
                    });
                }
                match &d.lyrics {
                    Some((name, p)) => {
                        let kind = if p.synced { tr("synced") } else { tr("plain") };
                        ui.label(RichText::new(format!("{name} · {} · {kind}", tr("{n} lines").replace("{n}", &p.lines.to_string()))).color(TEXT_DIM));
                    }
                    None => {
                        ui.label(RichText::new(tr(".lrc, .ttml, .srt, .txt …")).color(TEXT_DIM));
                    }
                }
            });
            if let Some(m) = &d.message {
                ui.label(RichText::new(m).color(Color32::from_rgb(0xFF, 0x8A, 0x8A)));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new(tr("Save")).strong())).clicked() {
                    save = true;
                }
                if ui.button(tr("Cancel")).clicked() {
                    cancel = true;
                }
            });
        });

    if save {
        let draft = super::Draft {
            title: d.title.clone(),
            artists: d.artists.clone(),
            album: d.album.clone(),
            year: d.year.clone(),
            cover: d.cover.clone(),
            lyrics: d.lyrics.as_ref().map(|l| l.1.text.clone()),
        };
        let song = d.song.clone();
        *guard = None;
        drop(guard);
        super::save(svc, &song, draft);
        return;
    }
    if cancel || !open {
        *guard = None;
    }
}
