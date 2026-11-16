//! Lyrics extras on the app side: per-line romanization and translation for the loaded
//! lyrics (Android `LyricsReadings` + `LyricsTranslationHelper`), the lyrics toolbar toggles
//! and the settings section.
//!
//! [`ExtrasEngine`] lives in the egui context's data (so the stand-alone widgets demo can
//! keep including `lyrics_view.rs`) and is driven once per frame by
//! [`LyricsView::update_extras`]: when the lyrics change it romanizes them off the UI thread;
//! while translation is on it fetches a translation (memory cache → disk cache → Google
//! Translate). Results come back over a channel and are handed to the view as
//! [`LineExtras`], which draws them as sub-lines.
//!
//! Translation is a session toggle like Android's translate button: it starts off for each
//! new song unless "translate every song" is set.

pub mod settings_ui;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{RichText, Ui};
use mochy_core::models::Song;
use mochy_core::settings::{LyricsExtras, LyricsSettings};
use mochy_lyrics::romanize::{self, RomanizeLang};
use mochy_lyrics::{Lyrics, translate};

use crate::services::Services;
use crate::ui::i18n::tr;
use crate::ui::lyrics_view::{LineExtras, LyricsAlign, LyricsState, LyricsView, SubLines};
use crate::widgets::{TEXT_DIM, TEXT_FAINT};

/// Translation progress, for the toolbar.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum TranslateStatus {
    #[default]
    Idle,
    Translating,
    Done,
    Failed,
}

enum Msg {
    Romanized { key: u64, sig: String, lang: Option<RomanizeLang>, lines: SubLines },
    Translated { key: u64, target: String, source: String, lines: Option<SubLines> },
}

/// Background romanization / translation state for the lyrics on screen.
pub struct ExtrasEngine {
    /// Display options (copied from settings each frame).
    pub opts: LyricsExtras,
    /// Session toggle: translations shown.
    pub translation_on: bool,
    /// Session toggle: readings shown (starts from the setting).
    pub romanize_on: bool,
    pub status: TranslateStatus,
    /// The language the current song is romanized as (None = nothing romanizable).
    pub roman_lang: Option<RomanizeLang>,
    romanized: Option<SubLines>,
    translated: Option<SubLines>,
    lyrics_key: u64,
    song_id: Option<String>,
    roman_req: Option<(u64, String)>,
    trans_req: Option<(u64, String, String)>,
    trans_have: Option<(u64, String, String)>,
    last_auto: bool,
    last_romanize_setting: bool,
    mem_cache: HashMap<(u64, String, String), SubLines>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    ctx: Option<egui::Context>,
}

impl Default for ExtrasEngine {
    fn default() -> Self {
        let (tx, rx) = channel();
        let opts = LyricsExtras::default();
        Self {
            translation_on: opts.translate_auto,
            romanize_on: opts.romanize,
            last_auto: opts.translate_auto,
            last_romanize_setting: opts.romanize,
            opts,
            status: TranslateStatus::Idle,
            roman_lang: None,
            romanized: None,
            translated: None,
            lyrics_key: 0,
            song_id: None,
            roman_req: None,
            trans_req: None,
            trans_have: None,
            mem_cache: HashMap::new(),
            tx,
            rx,
            ctx: None,
        }
    }
}

/// FNV-1a: stable across Rust releases (used for the on-disk cache names).
pub fn fnv1a(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.as_bytes().iter().chain(std::iter::once(&0xFFu8)) {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }
    h
}

/// Identity of a set of lyrics (the line texts, in order).
pub fn lyrics_key(l: &Lyrics) -> u64 {
    let texts: Vec<&str> = l.lines.iter().map(|x| x.text.as_str()).collect();
    fnv1a(&texts)
}

fn cache_dir() -> PathBuf {
    mochy_storage::cache_dir().join("lyrics_translations")
}

fn cache_file(key: u64, target: &str, source: &str) -> PathBuf {
    let name = format!("{:016x}.json", fnv1a(&[&key.to_string(), target, source]));
    cache_dir().join(name)
}

/// Read a translation saved by an earlier play (so it shows offline too).
pub fn read_disk_cache(key: u64, target: &str, source: &str) -> Option<Vec<Option<String>>> {
    let bytes = std::fs::read(cache_file(key, target, source)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_disk_cache(key: u64, target: &str, source: &str, lines: &[Option<String>]) {
    let dir = cache_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(json) = serde_json::to_vec(lines) {
        let path = cache_file(key, target, source);
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

impl ExtrasEngine {
    fn wake(&self) -> impl Fn() + Send + 'static {
        let ctx = self.ctx.clone();
        move || {
            if let Some(c) = &ctx {
                c.request_repaint();
            }
        }
    }

    /// Romanized sub-lines for the current lyrics, when shown.
    pub fn romanized(&self) -> Option<&SubLines> {
        if !self.romanize_on || !self.opts.romanize {
            return None;
        }
        self.romanized.as_ref()
    }

    /// Translated sub-lines for the current lyrics, when shown.
    pub fn translated(&self) -> Option<&SubLines> {
        if !self.translation_on {
            return None;
        }
        self.translated.as_ref()
    }

    /// Flip the session translation toggle (the toolbar's translate button).
    pub fn toggle_translation(&mut self) {
        self.translation_on = !self.translation_on;
        if !self.translation_on {
            self.status = TranslateStatus::Idle;
        }
    }

    fn reset_for_lyrics(&mut self, key: u64) {
        self.lyrics_key = key;
        self.romanized = None;
        self.translated = None;
        self.roman_lang = None;
        self.roman_req = None;
        self.trans_req = None;
        self.trans_have = None;
        self.status = TranslateStatus::Idle;
    }

    fn poll(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Romanized { key, sig, lang, lines } => {
                    if key == self.lyrics_key && self.roman_req.as_ref() == Some(&(key, sig)) {
                        self.roman_lang = lang;
                        self.romanized = Some(lines);
                    }
                }
                Msg::Translated { key, target, source, lines } => {
                    let req = (key, target.clone(), source.clone());
                    if let Some(l) = &lines {
                        self.mem_cache.insert(req.clone(), l.clone());
                        if self.mem_cache.len() > 32 {
                            // small and simple: drop everything but this one
                            self.mem_cache.retain(|k, _| *k == req);
                        }
                    }
                    if self.trans_req.as_ref() == Some(&req) {
                        self.trans_req = None;
                        match lines {
                            Some(l) => {
                                self.translated = Some(l);
                                self.trans_have = Some(req);
                                self.status = TranslateStatus::Done;
                            }
                            None => {
                                self.trans_have = Some(req);
                                self.status = TranslateStatus::Failed;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Drive the engine for this frame.
    pub fn update(&mut self, svc: &Services, settings: &LyricsSettings, state: &LyricsState, song: Option<&Song>) {
        self.opts = settings.extras.clone();
        // the setting toggles act on the session toggles too
        if self.opts.translate_auto != self.last_auto {
            self.last_auto = self.opts.translate_auto;
            self.translation_on = self.opts.translate_auto;
        }
        if self.opts.romanize != self.last_romanize_setting {
            self.last_romanize_setting = self.opts.romanize;
            self.romanize_on = self.opts.romanize;
        }
        let sid = song.map(|s| s.id.clone());
        if sid != self.song_id {
            self.song_id = sid;
            // Android `onSongChanged`: translation carries over only when "every song" is on
            self.translation_on = self.opts.translate_auto;
            self.status = TranslateStatus::Idle;
        }
        self.poll();

        let LyricsState::Loaded(lyrics) = state else {
            if self.lyrics_key != 0 {
                self.reset_for_lyrics(0);
            }
            return;
        };
        let key = lyrics_key(lyrics);
        if key != self.lyrics_key {
            self.reset_for_lyrics(key);
        }

        // ---- romanization (cheap; computed whenever readings are on) ----
        if self.opts.romanize && self.romanize_on {
            let sig = self.opts.romanize_disabled.join(",");
            if self.roman_req.as_ref() != Some(&(key, sig.clone())) {
                self.roman_req = Some((key, sig.clone()));
                self.romanized = None;
                let texts: Vec<String> = lyrics.lines.iter().map(|l| l.text.clone()).collect();
                let disabled: Vec<String> = self.opts.romanize_disabled.clone();
                let tx = self.tx.clone();
                let wake = self.wake();
                svc.spawn(async move {
                    let res = tokio::task::spawn_blocking(move || {
                        let body = texts.join(" ");
                        let lang = romanize::song_language(&body);
                        let allowed = |l: RomanizeLang| !disabled.iter().any(|d| d.eq_ignore_ascii_case(l.id()));
                        let lines = romanize::romanize_song(&texts, allowed);
                        (lang, lines)
                    })
                    .await;
                    if let Ok((lang, lines)) = res {
                        let _ = tx.send(Msg::Romanized { key, sig, lang, lines: Arc::new(lines) });
                        wake();
                    }
                });
            }
        }

        // ---- translation (only while the toggle is on) ----
        if self.translation_on {
            let target = if self.opts.translate_language.trim().is_empty() { "en".to_string() } else { self.opts.translate_language.trim().to_string() };
            let source = if self.opts.translate_source.trim().is_empty() { "auto".to_string() } else { self.opts.translate_source.trim().to_string() };
            let req = (key, target.clone(), source.clone());
            if self.trans_have.as_ref() != Some(&req) && self.trans_req.as_ref() != Some(&req) {
                if let Some(hit) = self.mem_cache.get(&req) {
                    self.translated = Some(hit.clone());
                    self.trans_have = Some(req);
                    self.status = TranslateStatus::Done;
                } else {
                    self.translated = None;
                    self.trans_req = Some(req);
                    self.status = TranslateStatus::Translating;
                    let texts: Vec<String> = lyrics.lines.iter().map(|l| l.text.clone()).collect();
                    let http = svc.http.clone();
                    let tx = self.tx.clone();
                    let wake = self.wake();
                    svc.spawn(async move {
                        let (t2, s2) = (target.clone(), source.clone());
                        let cached = tokio::task::spawn_blocking(move || read_disk_cache(key, &t2, &s2)).await.ok().flatten();
                        let lines = match cached.filter(|c| c.len() == texts.len()) {
                            Some(c) => Some(c),
                            None => {
                                let res = translate::translate_song(&http, &texts, &target, &source).await;
                                if let Some(r) = &res {
                                    let (t3, s3, r3) = (target.clone(), source.clone(), r.clone());
                                    let _ = tokio::task::spawn_blocking(move || write_disk_cache(key, &t3, &s3, &r3)).await;
                                } else {
                                    tracing::warn!(target = %target, "lyrics translation failed");
                                }
                                res
                            }
                        };
                        let _ = tx.send(Msg::Translated { key, target, source, lines: lines.map(Arc::new) });
                        wake();
                    });
                }
            }
        }
    }
}

/// The app's one engine, kept in the egui context's data.
fn engine(ctx: &egui::Context) -> Arc<Mutex<ExtrasEngine>> {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_insert_with(egui::Id::unique("mochy_lyrics_extras_engine"), || Arc::new(Mutex::new(ExtrasEngine::default())))
            .clone()
    })
}

impl LyricsView {
    /// Once per frame, before drawing: apply the display settings and drive translation /
    /// romanization for the lyrics in `state`.
    pub fn update_extras(&mut self, svc: &Services, settings: &LyricsSettings, state: &LyricsState, song: Option<&Song>, ctx: &egui::Context) {
        let engine = engine(ctx);
        let Ok(mut e) = engine.lock() else { return };
        if e.ctx.is_none() {
            e.ctx = Some(ctx.clone());
        }
        e.update(svc, settings, state, song);
        self.extras = LineExtras { opts: e.opts.clone(), romanized: e.romanized().cloned(), translated: e.translated().cloned() };
        self.line_spacing = settings.line_spacing;
        let x = &settings.extras;
        self.blur = x.blur_inactive;
        self.align = match x.align {
            mochy_core::settings::LyricsTextAlign::Left => LyricsAlign::Left,
            mochy_core::settings::LyricsTextAlign::Center => LyricsAlign::Center,
            mochy_core::settings::LyricsTextAlign::Right => LyricsAlign::Right,
        };
    }
}

/// The lyrics toolbar toggles (Now Playing → Lyrics, next to the offset buttons).
pub fn toolbar(ui: &mut Ui, _lv: &mut LyricsView) {
    let engine = engine(ui.ctx());
    let Ok(mut guard) = engine.lock() else { return };
    let e = &mut *guard;
    let label = match e.status {
        TranslateStatus::Translating if e.translation_on => format!("文A {}", tr("Translating…")),
        TranslateStatus::Failed if e.translation_on => format!("文A {}", tr("Translation failed")),
        _ => "文A".to_string(),
    };
    let tip = if e.translation_on { tr("Hide translation") } else { tr("Translate lyrics") };
    let btn = egui::Button::new(RichText::new(label).size(12.0)).selected(e.translation_on).small();
    if ui.add(btn).on_hover_text(tip).clicked() {
        e.toggle_translation();
    }
    if e.opts.romanize && e.roman_lang.is_some() {
        let tip = if e.romanize_on { tr("Hide romanization") } else { tr("Show romanization") };
        let btn = egui::Button::new(RichText::new("Aa").size(12.0)).selected(e.romanize_on).small();
        if ui.add(btn).on_hover_text(tip).clicked() {
            e.romanize_on = !e.romanize_on;
        }
    }
    if e.translation_on && e.status == TranslateStatus::Done {
        ui.label(RichText::new(format!("→ {}", e.opts.translate_language)).size(11.0).color(TEXT_FAINT));
    } else if e.translation_on && e.status == TranslateStatus::Failed {
        ui.label(RichText::new(tr("(check the connection)")).size(11.0).color(TEXT_DIM));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_is_stable() {
        assert_eq!(fnv1a(&[]), 0xcbf2_9ce4_8422_2325);
        assert_ne!(fnv1a(&["ab", "c"]), fnv1a(&["a", "bc"]));
        assert_eq!(fnv1a(&["x"]), fnv1a(&["x"]));
    }

    #[test]
    fn disk_cache_roundtrip() {
        let key = 0xDEAD_BEEF_0000_0001;
        let lines = vec![Some("Hello".to_string()), None];
        write_disk_cache(key, "en", "test", &lines);
        assert_eq!(read_disk_cache(key, "en", "test"), Some(lines));
        let _ = std::fs::remove_file(cache_file(key, "en", "test"));
        assert_eq!(read_disk_cache(key, "en", "test"), None);
    }
}
