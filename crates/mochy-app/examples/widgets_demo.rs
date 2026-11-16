//! Standalone demo of the Mochy widget modules (Audio Lab, synced lyrics,
//! theme/palette). Run with:
//!
//! ```text
//! cargo run -p mochy-app --example widgets_demo
//! ```
//!
//! Env vars for automated checks:
//! * `MOCHY_DEMO_SHOTS=<dir>` — screenshot every tab into `<dir>` and exit.
//! * `MOCHY_DEMO_EXIT_AFTER=<secs>` — close the window after that long.

#![allow(dead_code)]

#[path = "../src/ui/theme.rs"]
mod theme;

#[path = "../src/ui/audio_lab.rs"]
mod audio_lab;

// What the Audio Lab v2 pages reach for outside the widget files.
mod ui {
    #[path = "../../src/ui/i18n.rs"]
    pub mod i18n;
}

#[path = "../src/stream_facts.rs"]
mod stream_facts;

#[path = "../src/ui/lyrics_view.rs"]
mod lyrics_view;

use std::time::Instant;

use eframe::egui::{self, Color32, Mesh, RichText, Shape};
use lyrics_view::{LyricsState, LyricsView};
use mochy_core::settings::AudioLab;
use mochy_lyrics::{LyricLine, LyricWord, Lyrics, SyncKind};

const SONG_LEN: f64 = 150.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    AudioLab,
    Lyrics,
    Theme,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LyricsDemo {
    Word,
    Line,
    Plain,
    Loading,
    NotFound,
    Error,
    Idle,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ImageKind {
    Sunset,
    Ocean,
    Forest,
    TwoTone,
    Gray,
}

struct Demo {
    tab: Tab,
    lab: AudioLab,
    lab_view: audio_lab::AudioLabView,
    lab_changes: u32,
    spectrum_on: bool,
    lyrics_view: LyricsView,
    lyrics_demo: LyricsDemo,
    word_lyrics: Lyrics,
    line_lyrics: Lyrics,
    plain_lyrics: Lyrics,
    // fake clock
    playing: bool,
    clock_base: f64,
    clock_start: Instant,
    offset_ms: i32,
    // theme
    image_kind: ImageKind,
    image_tex: Option<egui::TextureHandle>,
    image_px: Vec<u8>,
    accent: Color32,
    gradient: (Color32, Color32),
    use_image_accent: bool,
    pure_black: bool,
    // automation
    shots_dir: Option<std::path::PathBuf>,
    exit_after: Option<f64>,
    frame: u64,
    started: Instant,
}

impl Demo {
    fn new() -> Self {
        let shots_dir = std::env::var_os("MOCHY_DEMO_SHOTS").map(std::path::PathBuf::from);
        let exit_after = std::env::var("MOCHY_DEMO_EXIT_AFTER").ok().and_then(|s| s.parse().ok());
        let lab = AudioLab {
            eq: mochy_core::eq::settings_from_preset(mochy_core::eq::preset_by_id("rock").unwrap()),
            ..Default::default()
        };
        let mut d = Self {
            tab: Tab::AudioLab,
            lab,
            lab_view: Default::default(),
            lab_changes: 0,
            spectrum_on: true,
            lyrics_view: LyricsView::default(),
            lyrics_demo: LyricsDemo::Word,
            word_lyrics: word_lyrics(),
            line_lyrics: line_lyrics(),
            plain_lyrics: Lyrics {
                provider: "LrcLib".into(),
                sync: SyncKind::Plain,
                lines: Vec::new(),
                plain: Some("Unsynced lyrics are shown as static text\nThey scroll like a normal page\n\nVerse two\nCòn đây là tiếng Việt\n日本語の歌詞も表示できます\n한국어 가사도 됩니다".into()),
            },
            playing: true,
            clock_base: 0.0,
            clock_start: Instant::now(),
            offset_ms: 0,
            image_kind: ImageKind::Sunset,
            image_tex: None,
            image_px: Vec::new(),
            accent: theme::DEFAULT_ACCENT,
            gradient: (Color32::from_rgb(0x59, 0x59, 0x59), Color32::from_rgb(0x0D, 0x0D, 0x0D)),
            use_image_accent: true,
            pure_black: false,
            shots_dir,
            exit_after,
            frame: 0,
            started: Instant::now(),
        };
        d.regen_image();
        d
    }

    fn pos(&self) -> f64 {
        let p = if self.playing {
            self.clock_base + self.clock_start.elapsed().as_secs_f64()
        } else {
            self.clock_base
        };
        p % SONG_LEN
    }

    fn seek(&mut self, t: f64) {
        self.clock_base = t.clamp(0.0, SONG_LEN);
        self.clock_start = Instant::now();
    }

    fn regen_image(&mut self) {
        let (w, h) = (160u32, 160u32);
        self.image_px = gen_image(self.image_kind, w, h);
        let accent = theme::extract_accent(&self.image_px, w, h);
        self.gradient = theme::extract_gradient(&self.image_px, w, h);
        if self.use_image_accent {
            self.accent = accent;
        }
        self.image_tex = None;
    }

    fn automation(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        if let Some(secs) = self.exit_after
            && self.started.elapsed().as_secs_f64() > secs
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let Some(dir) = self.shots_dir.clone() else { return };
        ctx.request_repaint();
        // Each shot: set up at frame 1 of its 60-frame slot, capture at frame 50.
        #[allow(clippy::type_complexity)]
        let shots: [(&str, Tab, Option<f64>, LyricsDemo, [f32; 2]); 8] = [
            ("audio_lab", Tab::AudioLab, None, LyricsDemo::Word, [1280.0, 880.0]),
            ("audio_lab_tall", Tab::AudioLab, None, LyricsDemo::Word, [1280.0, 2400.0]),
            ("audio_lab_narrow", Tab::AudioLab, None, LyricsDemo::Word, [700.0, 3200.0]),
            ("lyrics_words", Tab::Lyrics, Some(11.2), LyricsDemo::Word, [1280.0, 880.0]),
            ("lyrics_gap", Tab::Lyrics, Some(35.0), LyricsDemo::Word, [1280.0, 880.0]),
            ("lyrics_cjk", Tab::Lyrics, Some(52.8), LyricsDemo::Word, [1280.0, 880.0]),
            ("lyrics_line", Tab::Lyrics, Some(17.0), LyricsDemo::Line, [1280.0, 880.0]),
            ("theme", Tab::Theme, None, LyricsDemo::Word, [1280.0, 880.0]),
        ];
        let idx = (self.frame / 60) as usize;
        let phase = self.frame % 60;
        if idx >= shots.len() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let (name, tab, seek, demo, size) = shots[idx];
        if phase == 1 {
            self.tab = tab;
            self.lyrics_demo = demo;
            if let Some(t) = seek {
                self.seek(t);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(size[0], size[1])));
        }
        if phase == 50 {
            let path = dir.join(format!("{name}.png"));
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                let [w, h] = img.size;
                let mut buf = Vec::with_capacity(w * h * 4);
                for p in &img.pixels {
                    buf.extend_from_slice(&p.to_array());
                }
                if let Some(rgba) = image::RgbaImage::from_raw(w as u32, h as u32, buf) {
                    let _ = rgba.save(&path);
                }
            })));
        }
    }
}

impl eframe::App for Demo {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        theme::apply(&ctx, self.accent, self.pure_black);
        self.automation(&ctx);

        egui::Panel::top("tabs").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Mochy widgets").font(theme::bold(&ctx, 18.0)).color(Color32::WHITE));
                ui.add_space(16.0);
                for (t, name) in [(Tab::AudioLab, "Audio Lab"), (Tab::Lyrics, "Lyrics"), (Tab::Theme, "Theme")] {
                    if ui.selectable_label(self.tab == t, name).clicked() {
                        self.tab = t;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.checkbox(&mut self.pure_black, "Pure black");
                    let (r, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 8.0, self.accent);
                    ui.label(RichText::new("accent").small());
                });
            });
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::AudioLab => self.audio_lab_tab(ui),
            Tab::Lyrics => self.lyrics_tab(ui),
            Tab::Theme => self.theme_tab(ui),
        });
    }
}

impl Demo {
    fn audio_lab_tab(&mut self, ui: &mut egui::Ui) {
        if self.spectrum_on {
            let t = ui.input(|i| i.time) as f32;
            let bands: Vec<f32> = (0..64)
                .map(|i| {
                    let x = i as f32 / 63.0;
                    let base = -18.0 - 40.0 * x * x; // pink-ish tilt
                    let wob = 8.0 * (t * 2.3 + i as f32 * 0.37).sin() + 5.0 * (t * 5.1 + i as f32 * 0.11).sin();
                    let kick = if (t * 2.0).fract() < 0.12 && i < 10 { 10.0 } else { 0.0 };
                    (base + wob + kick).clamp(-90.0, 0.0)
                })
                .collect();
            self.lab_view.set_spectrum(bands);
            ui.ctx().request_repaint();
        } else {
            self.lab_view.set_spectrum(Vec::new());
        }
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.spectrum_on, "Fake live spectrum");
            ui.label(RichText::new(format!("changes reported: {}", self.lab_changes)).small());
        });
        if self.lab_view.show(ui, &mut self.lab, self.accent) {
            self.lab_changes += 1;
        }
    }

    fn lyrics_tab(&mut self, ui: &mut egui::Ui) {
        let pos = self.pos();
        ui.horizontal_wrapped(|ui| {
            if ui.button(if self.playing { "Pause" } else { "Play" }).clicked() {
                let p = self.pos();
                self.playing = !self.playing;
                self.seek(p);
            }
            let mut p = pos;
            if ui.add(egui::Slider::new(&mut p, 0.0..=SONG_LEN).text("pos").fixed_decimals(1)).changed() {
                self.seek(p);
            }
            ui.add(egui::DragValue::new(&mut self.offset_ms).speed(10).suffix(" ms offset"));
            ui.add(egui::Slider::new(&mut self.lyrics_view.font_size, 16.0..=56.0).text("size"));
            ui.checkbox(&mut self.lyrics_view.blur, "blur");
            let mut center = self.lyrics_view.align == lyrics_view::LyricsAlign::Center;
            if ui.checkbox(&mut center, "center").changed() {
                self.lyrics_view.align =
                    if center { lyrics_view::LyricsAlign::Center } else { lyrics_view::LyricsAlign::Left };
            }
            egui::ComboBox::from_id_salt("lyrics_demo")
                .selected_text(match self.lyrics_demo {
                    LyricsDemo::Word => "Word-synced",
                    LyricsDemo::Line => "Line-synced",
                    LyricsDemo::Plain => "Plain",
                    LyricsDemo::Loading => "Loading",
                    LyricsDemo::NotFound => "Not found",
                    LyricsDemo::Error => "Error",
                    LyricsDemo::Idle => "Idle",
                })
                .show_ui(ui, |ui| {
                    for (v, n) in [
                        (LyricsDemo::Word, "Word-synced"),
                        (LyricsDemo::Line, "Line-synced"),
                        (LyricsDemo::Plain, "Plain"),
                        (LyricsDemo::Loading, "Loading"),
                        (LyricsDemo::NotFound, "Not found"),
                        (LyricsDemo::Error, "Error"),
                        (LyricsDemo::Idle, "Idle"),
                    ] {
                        ui.selectable_value(&mut self.lyrics_demo, v, n);
                    }
                });
        });

        // Player-style backdrop from the artwork gradient.
        let rect = ui.available_rect_before_wrap();
        let mut mesh = Mesh::default();
        let (a, b) = self.gradient;
        mesh.colored_vertex(rect.left_top(), a);
        mesh.colored_vertex(rect.right_top(), a);
        mesh.colored_vertex(rect.right_bottom(), b);
        mesh.colored_vertex(rect.left_bottom(), b);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        ui.painter().add(Shape::mesh(mesh));
        ui.painter().rect_filled(rect, 0.0, Color32::from_black_alpha(60));

        let state = match self.lyrics_demo {
            LyricsDemo::Word => LyricsState::Loaded(self.word_lyrics.clone()),
            LyricsDemo::Line => LyricsState::Loaded(self.line_lyrics.clone()),
            LyricsDemo::Plain => LyricsState::Loaded(self.plain_lyrics.clone()),
            LyricsDemo::Loading => LyricsState::Loading,
            LyricsDemo::NotFound => LyricsState::NotFound,
            LyricsDemo::Error => LyricsState::Error("HTTP 503 from every provider".into()),
            LyricsDemo::Idle => LyricsState::Idle,
        };
        if let Some(t) = self.lyrics_view.show(ui, &state, pos, self.offset_ms, self.accent) {
            self.seek(t);
        }
        if self.playing {
            ui.ctx().request_repaint();
        }
    }

    fn theme_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (k, n) in [
                (ImageKind::Sunset, "Sunset"),
                (ImageKind::Ocean, "Ocean"),
                (ImageKind::Forest, "Forest"),
                (ImageKind::TwoTone, "Two-tone"),
                (ImageKind::Gray, "Gray"),
            ] {
                if ui.selectable_label(self.image_kind == k, n).clicked() {
                    self.image_kind = k;
                    self.regen_image();
                }
            }
            if ui.checkbox(&mut self.use_image_accent, "Use as app accent").changed() {
                self.accent = if self.use_image_accent {
                    theme::extract_accent(&self.image_px, 160, 160)
                } else {
                    theme::DEFAULT_ACCENT
                };
            }
        });
        let tex = self
            .image_tex
            .get_or_insert_with(|| {
                ui.ctx().load_texture(
                    "demo_art",
                    egui::ColorImage::from_rgba_unmultiplied([160, 160], &self.image_px),
                    egui::TextureOptions::LINEAR,
                )
            })
            .clone();
        let accent = theme::extract_accent(&self.image_px, 160, 160);
        let swatches = theme::palette(&self.image_px, 160, 160, 8);
        theme::card_frame().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::Image::new(&tex).fit_to_exact_size(egui::vec2(220.0, 220.0)).corner_radius(12));
                ui.vertical(|ui| {
                    ui.label(RichText::new("Extracted accent").font(theme::semibold(ui.ctx(), 16.0)));
                    swatch(ui, accent, &format!("#{:02X}{:02X}{:02X}", accent.r(), accent.g(), accent.b()));
                    ui.label(RichText::new("Gradient").font(theme::semibold(ui.ctx(), 16.0)));
                    ui.horizontal(|ui| {
                        swatch(ui, self.gradient.0, "top");
                        swatch(ui, self.gradient.1, "bottom");
                    });
                    ui.label(RichText::new("Palette (median cut, 8)").font(theme::semibold(ui.ctx(), 16.0)));
                    ui.horizontal_wrapped(|ui| {
                        for s in &swatches {
                            swatch(ui, Color32::from_rgb(s.rgb[0], s.rgb[1], s.rgb[2]), &format!("{}", s.population));
                        }
                    });
                });
            });
        });
        ui.add_space(8.0);
        theme::card_frame().show(ui, |ui| {
            ui.label(RichText::new("Typography").font(theme::semibold(ui.ctx(), 16.0)));
            ui.label(RichText::new("Display — Inter Bold").font(theme::bold(ui.ctx(), 32.0)));
            ui.heading("Heading — Inter SemiBold");
            ui.label("Body — Inter Medium. Tiếng Việt: Hôm nay trời đẹp quá!");
            ui.label("CJK / Korean / Thai fallbacks: 夜に駆ける · 晴天 · 봄날 · เพลงไทย · emoji 🎧");
            ui.label(RichText::new("Small secondary text").small().color(theme::TEXT_SECONDARY));
            ui.hyperlink_to("A hyperlink in the accent", "https://github.com/rsms/inter");
            let mut v = 0.6f32;
            ui.add(egui::Slider::new(&mut v, 0.0..=1.0).text("Slider"));
        });
    }
}

fn swatch(ui: &mut egui::Ui, c: Color32, label: &str) {
    ui.vertical(|ui| {
        let (r, _) = ui.allocate_exact_size(egui::vec2(56.0, 36.0), egui::Sense::hover());
        ui.painter().rect_filled(r, 8.0, c);
        ui.label(RichText::new(label).small());
    });
}

fn gen_image(kind: ImageKind, w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let n = (((x * 7919 + y * 104729) % 97) as f32 / 97.0 - 0.5) * 18.0;
            let c: [f32; 3] = match kind {
                ImageKind::Sunset => {
                    let sun = (-(((fx - 0.65).powi(2) + (fy - 0.55).powi(2)) * 30.0)).exp();
                    [230.0 - 60.0 * fy + 25.0 * sun, 90.0 + 60.0 * (1.0 - fy) + 120.0 * sun, 80.0 + 90.0 * fy]
                }
                ImageKind::Ocean => [20.0 + 30.0 * fy, 90.0 + 80.0 * fx, 170.0 + 60.0 * (1.0 - fy)],
                ImageKind::Forest => {
                    let stripe = ((fx * 18.0).sin() * 0.5 + 0.5) * 40.0;
                    [30.0 + stripe * 0.3, 110.0 + stripe, 50.0 + 20.0 * fy]
                }
                ImageKind::TwoTone => {
                    if fx + fy * 0.3 < 0.62 { [205.0, 40.0, 60.0] } else { [30.0, 70.0, 200.0] }
                }
                ImageKind::Gray => {
                    let g = 60.0 + 120.0 * fy;
                    [g, g, g]
                }
            };
            for ch in c {
                v.push((ch + n).clamp(0.0, 255.0) as u8);
            }
            v.push(255);
        }
    }
    v
}

/// Build a word-synced line: words spread evenly over `dur` seconds.
fn wl(t: f64, text: &str, dur: f64, agent: &str, bg: bool) -> LyricLine {
    let words: Vec<&str> = text.split(' ').collect();
    let step = dur / words.len() as f64;
    LyricLine {
        time: t,
        text: text.into(),
        words: words
            .iter()
            .enumerate()
            .map(|(i, w)| LyricWord {
                start: t + step * i as f64,
                end: t + step * (i + 1) as f64,
                text: if i + 1 < words.len() { format!("{w} ") } else { (*w).to_string() },
            })
            .collect(),
        bg,
        agent: Some(agent.into()),
    }
}

fn word_lyrics() -> Lyrics {
    let mut lines = vec![
        wl(7.0, "Neon rivers running through the city night", 3.2, "v1", false),
        wl(10.4, "I can hear the echo of your heartbeat", 3.0, "v1", false),
        wl(12.6, "(heartbeat, heartbeat)", 1.6, "v1", true),
        wl(14.2, "Every window glowing like a satellite", 3.4, "v1", false),
        wl(18.0, "Hold on, hold on", 3.5, "v1", false),
        wl(22.0, "You were singing from the other side", 3.0, "v2", false),
        wl(25.2, "Calling out across the great divide", 3.0, "v2", false),
        wl(28.0, "(ooh, across the great divide)", 2.0, "v2", true),
        wl(30.5, "Together we can light the sky", 3.0, "v1000", false),
        // ~9 s instrumental gap here
        wl(42.0, "Second verse and the stars are falling slowly down around us both tonight while the music keeps on playing", 6.0, "v1", false),
        wl(48.5, "Hôm nay trời đẹp, mình cùng nhau hát", 3.2, "v1", false),
        wl(52.0, "夜に駆ける 君と一緒に", 3.0, "v2", false),
        wl(55.5, "우리 함께 노래해", 2.6, "v1", false),
        wl(58.5, "Hoooooold this note", 4.0, "v1", false),
    ];
    let mut t = 64.0;
    let fillers = [
        "Lines keep coming for the scrolling test",
        "Tap any line to jump right there",
        "Scroll the wheel to look around",
        "Auto-follow returns in three seconds",
        "Neon rivers running through the city night",
        "Every window glowing like a satellite",
    ];
    let mut k = 0;
    while t < SONG_LEN - 5.0 {
        let agent = if k % 5 == 3 { "v2" } else { "v1" };
        lines.push(wl(t, fillers[k % fillers.len()], 2.8, agent, false));
        t += 3.4;
        k += 1;
    }
    Lyrics { provider: "Apple Music".into(), sync: SyncKind::Word, lines, plain: None }
}

fn line_lyrics() -> Lyrics {
    let mut lines = Vec::new();
    let texts = [
        "This is a line-synced song",
        "Each line lights up all at once",
        "",
        "An empty line marks an instrumental",
        "And the dots count it down",
        "Clicking a line seeks to it",
    ];
    let mut t = 3.0;
    for i in 0..60 {
        let text = texts[i % texts.len()];
        lines.push(LyricLine { time: t, text: text.into(), ..Default::default() });
        t += if text.is_empty() { 7.0 } else { 2.4 };
    }
    Lyrics { provider: "LrcLib".into(), sync: SyncKind::Line, lines, plain: None }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 880.0])
            .with_min_inner_size([480.0, 360.0])
            .with_title("Mochy widgets demo"),
        ..Default::default()
    };
    eframe::run_native("Mochy widgets demo", options, Box::new(|_cc| Ok(Box::new(Demo::new()))))
}
