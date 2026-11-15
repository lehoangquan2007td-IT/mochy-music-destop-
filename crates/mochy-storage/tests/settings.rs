//! `SettingsStore` tests: round-trip, partial/old files, invalid values,
//! corruption backup, atomic save.

use std::path::PathBuf;

use mochy_core::eq::{EqNode, UserEqPreset};
use mochy_core::settings::{AppSettings, AudioQuality, RepeatMode};
use mochy_storage::SettingsStore;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("mochy-settings-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn store(&self) -> SettingsStore {
        SettingsStore::open(self.0.join("settings.json"))
    }
    fn files(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn customized() -> AppSettings {
    let mut s = AppSettings::default();
    s.appearance.pure_black = true;
    s.appearance.selected_theme_color = Some(0x00FF_8800);
    s.playback.audio_quality = AudioQuality::High;
    s.playback.repeat_mode = RepeatMode::One;
    s.playback.player_volume = 0.42;
    s.playback.output_device = Some("Speakers (Realtek)".into());
    s.audio_lab.pitch_semitones = -3.5;
    s.audio_lab.speed = 1.25;
    s.audio_lab.reverb = mochy_core::settings::ReverbSettings::from_preset("hall", true).unwrap();
    s.audio_lab.spatial.crossfeed_enabled = true;
    s.audio_lab.spatial.width = 1.4;
    s.audio_lab.user_presets.push(UserEqPreset {
        id: "user_1794796661000".into(),
        name: "Bass boost (mine)".into(),
        nodes: vec![EqNode::peaking(60.0, 6.0), EqNode::peaking(8000.0, -2.5)],
        preamp_db: -4.0,
    });
    s.lyrics.provider_order = vec!["lrclib".into(), "betterlyrics".into()];
    s.misc.content_country = "VN".into();
    s.misc.window_width = 1280.0;
    s
}

#[test]
fn missing_file_gives_defaults() {
    let dir = TempDir::new("missing");
    let (s, report) = dir.store().load_report().unwrap();
    assert_eq!(s, AppSettings::default());
    assert!(report.missing);
}

#[test]
fn roundtrip_full_settings() {
    let dir = TempDir::new("roundtrip");
    let store = dir.store();
    let s = customized();
    store.save(&s).unwrap();
    assert_eq!(store.load().unwrap(), s);
    // Overwrite works (rename replaces an existing file on Windows too) and
    // leaves no temp files behind.
    let mut s2 = s.clone();
    s2.playback.output_device = None;
    store.save(&s2).unwrap();
    assert_eq!(store.load().unwrap(), s2);
    assert_eq!(dir.files(), ["settings.json"]);
    let text = std::fs::read_to_string(store.path()).unwrap();
    assert!(text.contains("\n  \"appearance\""), "pretty JSON");
}

#[test]
fn old_partial_file_fills_defaults() {
    let dir = TempDir::new("partial");
    let store = dir.store();
    // An older build: no reverb/spatial/pitch/user_presets/output_device, no misc section.
    std::fs::write(
        store.path(),
        r#"{
            "appearance": {"pure_black": true},
            "playback": {"audio_quality": "Low", "player_volume": 0.5},
            "audio_lab": {"speed": 1.5, "loudness_enabled": true},
            "some_removed_section": {"x": 1}
        }"#,
    )
    .unwrap();
    let (s, report) = store.load_report().unwrap();
    assert!(report.rejected.is_empty() && !report.corrupted && report.backup.is_none());
    assert!(s.appearance.pure_black);
    assert_eq!(s.appearance.thumbnail_corner_radius, 8.0);
    assert_eq!(s.playback.audio_quality, AudioQuality::Low);
    assert_eq!(s.playback.player_volume, 0.5);
    assert_eq!(s.playback.output_device, None);
    assert_eq!(s.audio_lab.speed, 1.5);
    assert!(s.audio_lab.loudness_enabled);
    assert_eq!(s.audio_lab.pitch_semitones, 0.0);
    assert_eq!(s.audio_lab.reverb, Default::default());
    assert_eq!(s.audio_lab.spatial, Default::default());
    assert!(s.audio_lab.user_presets.is_empty());
    assert_eq!(s.misc, Default::default());
}

#[test]
fn invalid_values_fall_back_per_field() {
    let dir = TempDir::new("invalid");
    let store = dir.store();
    std::fs::write(
        store.path(),
        r#"{
            "playback": {"audio_quality": "UltraHD", "player_volume": 0.3, "repeat_mode": "One"},
            "audio_lab": {"speed": "fast", "pitch_semitones": 2.0, "reverb": {"enabled": true, "mix": "lots"}},
            "misc": {"content_country": "VN"}
        }"#,
    )
    .unwrap();
    let (s, report) = store.load_report().unwrap();
    let mut rejected = report.rejected.clone();
    rejected.sort();
    assert_eq!(rejected, ["audio_lab.reverb.mix", "audio_lab.speed", "playback.audio_quality"]);
    assert!(report.backup.as_ref().is_some_and(|b| b.exists()), "original kept before dropping values");
    assert_eq!(s.playback.audio_quality, AudioQuality::Auto);
    assert_eq!(s.playback.player_volume, 0.3);
    assert_eq!(s.playback.repeat_mode, RepeatMode::One);
    assert_eq!(s.audio_lab.speed, 1.0);
    assert_eq!(s.audio_lab.pitch_semitones, 2.0);
    assert!(s.audio_lab.reverb.enabled);
    assert_eq!(s.audio_lab.reverb.mix, 0.5);
    assert_eq!(s.misc.content_country, "VN");
}

#[test]
fn corrupted_file_is_backed_up() {
    let dir = TempDir::new("corrupt");
    let store = dir.store();
    std::fs::write(store.path(), "{\"appearance\": {\"pure_black\": tru").unwrap();
    let (s, report) = store.load_report().unwrap();
    assert_eq!(s, AppSettings::default());
    assert!(report.corrupted);
    let backup = report.backup.expect("backup path");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), "{\"appearance\": {\"pure_black\": tru");
    assert!(backup.file_name().unwrap().to_string_lossy().starts_with("settings.json.corrupt-"));
    // load() (no report) behaves the same and saving afterwards works.
    assert_eq!(store.load().unwrap(), AppSettings::default());
    store.save(&customized()).unwrap();
    assert_eq!(store.load().unwrap(), customized());

    // Non-object JSON and an empty file are handled too.
    std::fs::write(store.path(), "[1,2,3]").unwrap();
    assert!(store.load_report().unwrap().1.corrupted);
    std::fs::write(store.path(), "").unwrap();
    assert_eq!(store.load().unwrap(), AppSettings::default());
    // UTF-8 BOM (Notepad) is tolerated.
    std::fs::write(store.path(), "\u{feff}{\"misc\":{\"content_country\":\"JP\"}}").unwrap();
    assert_eq!(store.load().unwrap().misc.content_country, "JP");
}

#[test]
fn save_creates_parent_dirs() {
    let dir = TempDir::new("mkdir");
    let store = SettingsStore::open(dir.0.join("nested").join("deeper").join("settings.json"));
    store.save(&customized()).unwrap();
    assert_eq!(store.load().unwrap(), customized());
}
