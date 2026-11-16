//! Controller side of transitions (Android `MusicService` crossfade wiring,
//! `SleepTimer.kt`, `SongTuning.kt`):
//! - preloads the queue's next entry into the engine (`PreloadNext`) once
//!   its stream is known, so it plays gaplessly / crossfaded / crossmixed,
//!   and keeps queue, history and play records right when the engine moves
//!   on by itself (`Event::NextStarted`);
//! - the sleep timer (minutes with a fade-out, or end of the current song);
//! - per-song speed / pitch, remembered in the DB and re-applied.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mochy_audio::{
    Command, CrossmixOptions, EngineHandle, NextGain, NextTrack, SkipSilence, SongTuning, TrackInfo, TrackSource,
    TransitionConfig, TransitionKind, TransportFade,
};
use mochy_core::models::Song;
use mochy_core::settings::{AppSettings, RepeatMode, TransitionMode};

use super::{PREFETCH_TTL, PlayState, PlayerController, prepared_from_stream};

/// What was handed to the engine as the next track.
#[derive(Debug, Clone)]
struct Preloaded {
    uid: u64,
    generation: u64,
    format: Option<String>,
    gain_db: Option<f32>,
}

/// Sleep timer (`SleepTimer.kt`): N minutes (ends with a fade-out) or the
/// end of the current song.
#[derive(Debug, Clone, Default)]
pub struct SleepTimer {
    trigger: Option<Instant>,
    total: Duration,
    /// Pause when the current song ends.
    pub end_of_song: bool,
}

impl SleepTimer {
    pub fn start_minutes(&mut self, minutes: u32, now: Instant) {
        self.start_secs(minutes as u64 * 60, now);
    }

    pub fn start_secs(&mut self, secs: u64, now: Instant) {
        self.clear();
        if secs == 0 {
            return;
        }
        self.total = Duration::from_secs(secs);
        self.trigger = Some(now + self.total);
    }

    pub fn start_end_of_song(&mut self) {
        self.clear();
        self.end_of_song = true;
    }

    /// −1 / +1 min; going below zero ends the timer now (returns `true`:
    /// pause now).
    pub fn adjust(&mut self, delta_secs: i64, now: Instant) -> bool {
        let Some(t) = self.trigger else { return false };
        let left = t.saturating_duration_since(now).as_secs_f64() + delta_secs as f64;
        if left <= 0.0 {
            self.clear();
            return true;
        }
        let left = Duration::from_secs_f64(left);
        self.total = self.total.max(left);
        self.trigger = Some(now + left);
        false
    }

    pub fn clear(&mut self) {
        *self = SleepTimer::default();
    }

    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        self.trigger.is_some() || self.end_of_song
    }

    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        self.trigger.map(|t| t.saturating_duration_since(now))
    }

    /// The countdown ran out (and is cleared).
    pub fn take_due(&mut self, now: Instant) -> bool {
        if self.trigger.is_some_and(|t| now >= t) {
            self.clear();
            return true;
        }
        false
    }

    /// The song ended with "end of song" armed (and it's cleared).
    pub fn take_end_of_song(&mut self) -> bool {
        std::mem::take(&mut self.end_of_song)
    }
}

/// Per-controller transition state (one field in `PlayerController`).
#[derive(Default)]
pub struct TransitionState {
    preloaded: Option<Preloaded>,
    pub sleep: SleepTimer,
    /// Song id → its own speed / pitch.
    tunings: Arc<Mutex<HashMap<String, SongTuning>>>,
}

/// What the player bar's sleep / speed controls show.
#[derive(Debug, Clone, Default)]
pub struct TransitionSnapshot {
    pub sleep_remaining_secs: Option<f64>,
    pub sleep_end_of_song: bool,
    pub has_song: bool,
    pub tuning: Option<SongTuning>,
    pub lab_speed: f32,
    pub lab_semitones: f32,
}

/// Engine transition config from the settings (`transitionModeOf`,
/// `CrossfadeConfig` durations).
pub fn transition_config(s: &AppSettings) -> TransitionConfig {
    let t = &s.transitions;
    let (kind, secs) = match s.playback.transition_mode {
        TransitionMode::Off => (TransitionKind::Gapless, 0.0),
        TransitionMode::AutoCrossfade => (TransitionKind::Crossfade, 5.0),
        TransitionMode::ManualCrossfade => (TransitionKind::Crossfade, s.playback.crossfade_duration_secs.clamp(1.0, 30.0)),
        TransitionMode::Crossmix => (TransitionKind::Crossmix, s.playback.crossfade_duration_secs),
    };
    TransitionConfig {
        kind,
        crossfade_secs: secs,
        crossmix: CrossmixOptions {
            length_scale: t.crossmix_length.scale(),
            max_speed_up: t.crossmix_tempo_match.max_speed_up(),
            style: t.crossmix_style.as_deref().and_then(mochy_audio::crossmix::Style::from_id),
            stunts: t.crossmix_stunts,
            always_mix: t.crossmix_always_mix,
        },
    }
}

/// Push transitions / fades / skip silence to the engine (startup + change).
pub fn apply_transition_settings(engine: &EngineHandle, s: &AppSettings) {
    let t = &s.transitions;
    engine.send(Command::SetTransition { config: transition_config(s) });
    engine.send(Command::SetTransportFade { fade: TransportFade { enabled: t.transport_fade, secs: t.transport_fade_secs } });
    engine.send(Command::SetSkipSilence { skip: SkipSilence { enabled: s.playback.skip_silence, instant: t.skip_silence_instant } });
}

/// Settings for `--smoke` with `MOCHY_SMOKE_TRANSITION` (gapless /
/// crossfade / crossmix).
pub fn smoke_transition_settings(mode: &str) -> AppSettings {
    let mut s = AppSettings::default();
    s.playback.transition_mode = match mode {
        "gapless" | "off" => TransitionMode::Off,
        "crossfade" => TransitionMode::ManualCrossfade,
        _ => TransitionMode::Crossmix,
    };
    s.playback.crossfade_duration_secs = 8.0;
    s
}

/// Two consecutive songs of one album (Android `isNextItemGapless`).
fn same_album(a: &Song, b: &Song) -> bool {
    match (&a.album_id, &b.album_id) {
        (Some(x), Some(y)) => x == y,
        _ => matches!((&a.album_name, &b.album_name), (Some(x), Some(y)) if !x.is_empty() && x == y),
    }
}

impl PlayerController {
    // ---------------------------------------------------------------------
    // preloading the next track
    // ---------------------------------------------------------------------

    /// Hand the queue's next entry to the engine once its stream is known
    /// (called on every progress tick; cheap when nothing changed).
    pub(super) fn maybe_preload_next(&mut self) {
        if self.engine.is_none() || self.loaded_uid.is_none() || self.awaiting_engine {
            return;
        }
        // "end of song" pauses at the end: nothing runs on into the next one
        if self.tx.sleep.end_of_song {
            if self.tx.preloaded.take().is_some() {
                self.send(Command::ClearNext);
            }
            return;
        }
        let want = self.queue.peek_next(true).and_then(|i| self.queue.entries().get(i)).map(|e| (e.uid, e.song.clone()));
        match (&self.tx.preloaded, &want) {
            (Some(p), Some((uid, _))) if p.uid == *uid && p.generation == self.generation => return,
            (Some(p), _) => {
                if p.generation == self.generation {
                    self.send(Command::ClearNext);
                }
                self.tx.preloaded = None;
            }
            _ => {}
        }
        let Some((uid, song)) = want else { return };
        let current = self.current().cloned();
        let (source, info, loudness_db, format) = if let Some(path) = song.local_path.clone().or_else(|| crate::downloads::offline_file(&song)) {
            if !path.exists() {
                return;
            }
            (TrackSource::Local { path }, TrackInfo { duration_secs: song.duration_secs, ..Default::default() }, None, None)
        } else {
            let Some(vid) = song.video_id.clone() else { return };
            // the stream comes from the prefetch (resolved after 5 s of the current song)
            let Some((st, at)) = self.prefetch.get(&vid) else { return };
            if at.elapsed() >= PREFETCH_TTL {
                return;
            }
            let p = prepared_from_stream(st.clone(), &song);
            (p.source, p.info, p.loudness_db, p.format)
        };
        // loudness as `on_prepared` sets it at load time
        let gain = if let Some(l) = loudness_db.filter(|_| self.cfg.normalization) {
            NextGain { gain_db: Some(-l), peak: None, preamp_db: 0.0, use_file_tags: false }
        } else if self.cfg.replay_gain && song.is_local() {
            NextGain { gain_db: None, peak: None, preamp_db: self.cfg.replay_gain_preamp, use_file_tags: true }
        } else {
            NextGain::default()
        };
        let repeat_one = self.queue.repeat == RepeatMode::One && Some(uid) == self.loaded_uid;
        let album_gapless = self.cfg.transitions.gapless_albums && current.as_ref().is_some_and(|c| same_album(c, &song));
        let tuning = self.tuning_for(&song);
        tracing::debug!(title = %song.title, repeat_one, album_gapless, "preloading next track");
        self.send(Command::PreloadNext {
            next: Box::new(NextTrack { source, info, token: uid, gain, tuning, allow_mix: !repeat_one && !album_gapless }),
        });
        self.tx.preloaded = Some(Preloaded { uid, generation: self.generation, format, gain_db: gain.gain_db });
    }

    /// A next track was handed to the engine.
    pub(super) fn tx_has_preload(&self) -> bool {
        self.tx.preloaded.is_some()
    }

    /// The engine ran into the preloaded track: book it like a natural end
    /// followed by a load, without loading anything.
    pub(super) fn on_next_started(&mut self, token: u64) {
        let pre = self.tx.preloaded.take();
        // another song is already on its way (skip / pick): the engine replaces this one anyway
        if self.loaded_uid.is_none() || self.awaiting_engine || pre.as_ref().is_none_or(|p| p.generation != self.generation) {
            tracing::debug!("ignoring a hand-over the queue has moved past");
            return;
        }
        self.flush_play_record();
        let next_matches = self.queue.peek_next(true).and_then(|i| self.queue.entries().get(i)).is_some_and(|e| e.uid == token);
        if next_matches {
            self.queue.advance(true);
        } else if let Some(i) = self.queue.entries().iter().position(|e| e.uid == token) {
            self.queue.jump_to(i);
        } else {
            // the queue moved on without it: play what the queue says now
            tracing::debug!("preloaded track no longer queued; reloading");
            self.start_current(true);
            return;
        }
        self.mark_queue_dirty();
        self.generation += 1;
        self.loaded_uid = Some(token);
        self.awaiting_engine = false;
        self.pos = 0.0;
        self.last_pos = 0.0;
        self.listened = 0.0;
        self.fail_streak = 0;
        self.duration = self.current().and_then(|s| s.duration_secs);
        self.format = pre.as_ref().and_then(|p| p.format.clone());
        self.applied_gain_db = pre.as_ref().and_then(|p| p.gain_db);
        if self.state != PlayState::Paused {
            self.state = PlayState::Playing;
        }
        if let (Some(svc), Some(song)) = (&self.svc, self.current().cloned()) {
            svc.db.run(move |db| crate::db::ensure_song(db, &song));
        }
        tracing::info!(title = %self.current().map(|s| s.title.as_str()).unwrap_or(""), "moved on to the preloaded track");
    }

    // ---------------------------------------------------------------------
    // sleep timer
    // ---------------------------------------------------------------------

    pub fn sleep_start_minutes(&mut self, minutes: u32) {
        self.tx.sleep.start_minutes(minutes, Instant::now());
    }

    pub fn sleep_end_of_song(&mut self) {
        self.tx.sleep.start_end_of_song();
        // no hand-over into the next song: it ends, then the player pauses
        self.maybe_preload_next();
    }

    pub fn sleep_adjust(&mut self, delta_secs: i64) {
        if self.tx.sleep.adjust(delta_secs, Instant::now()) {
            self.sleep_fire();
        }
    }

    pub fn sleep_cancel(&mut self) {
        self.tx.sleep.clear();
    }

    fn sleep_fire(&mut self) {
        tracing::info!("sleep timer: pausing");
        if self.is_playing() {
            self.want_play = false;
            let secs = self.cfg.transitions.sleep_fade_secs.clamp(0.0, 60.0);
            if secs > 0.0 && self.state == PlayState::Playing {
                self.send(Command::PauseFade { secs });
            } else {
                self.pause();
            }
        }
    }

    /// End-of-song sleep timer: `true` = the next song should load paused.
    pub(super) fn take_sleep_end_of_song(&mut self) -> bool {
        self.tx.sleep.take_end_of_song()
    }

    /// Per-frame housekeeping (the sleep countdown).
    pub(super) fn tick_transitions(&mut self) {
        if self.tx.sleep.take_due(Instant::now()) {
            self.sleep_fire();
        }
    }

    // ---------------------------------------------------------------------
    // per-song speed / pitch
    // ---------------------------------------------------------------------

    /// Read the saved tunings (startup).
    pub fn load_song_tunings(&self) {
        let Some(svc) = &self.svc else { return };
        let map = self.tx.tunings.clone();
        svc.db.run(move |db| match db.song_tunings() {
            Ok(all) => {
                let mut m = map.lock().unwrap_or_else(|e| e.into_inner());
                for (id, t) in all {
                    m.insert(id, SongTuning { speed: t.speed, semitones: t.semitones });
                }
            }
            Err(e) => tracing::warn!("song tunings: {e}"),
        });
    }

    fn tuning_for(&self, song: &Song) -> Option<SongTuning> {
        self.tx.tunings.lock().ok().and_then(|m| m.get(&song.id).copied())
    }

    /// Tell the engine the just-loaded song's own speed / pitch (sent right
    /// after `Load`).
    pub(super) fn send_song_tuning(&self) {
        let tuning = self.current().and_then(|s| self.tuning_for(s));
        self.send(Command::SetSongTuning { tuning });
    }

    /// Save (or with `None` forget) the current song's own speed / pitch and
    /// apply it now.
    pub fn set_song_tuning(&mut self, tuning: Option<SongTuning>) {
        let Some(song) = self.current().cloned() else { return };
        if let Ok(mut m) = self.tx.tunings.lock() {
            match tuning {
                Some(t) => m.insert(song.id.clone(), t),
                None => m.remove(&song.id),
            };
        }
        self.send(Command::SetSongTuning { tuning });
        if let Some(svc) = &self.svc {
            let id = song.id.clone();
            svc.db.run(move |db| {
                let r = match tuning {
                    Some(t) => db.set_song_tuning(&id, mochy_storage::SongTuningRow { speed: t.speed, semitones: t.semitones }),
                    None => db.clear_song_tuning(&id),
                };
                crate::db::log_err("song_tuning", r);
            });
        }
        // a preloaded copy of the same song plays with the new values too
        if self.tx.preloaded.as_ref().is_some_and(|p| self.queue.entries().iter().any(|e| e.uid == p.uid && e.song.id == song.id)) {
            self.tx.preloaded = None;
            self.send(Command::ClearNext);
        }
    }

    pub fn transition_snapshot(&self, s: &AppSettings) -> TransitionSnapshot {
        let now = Instant::now();
        TransitionSnapshot {
            sleep_remaining_secs: self.tx.sleep.remaining(now).map(|d| d.as_secs_f64()),
            sleep_end_of_song: self.tx.sleep.end_of_song,
            has_song: self.current().is_some(),
            tuning: self.current().and_then(|c| self.tuning_for(c)),
            lab_speed: s.audio_lab.speed,
            lab_semitones: s.audio_lab.pitch_semitones,
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::{CtlCfg, PlayState, Prepared};
    use mochy_audio::Event;

    fn songs(n: usize) -> Vec<Song> {
        (0..n).map(|i| Song::remote(format!("v{i}"), format!("Song {i}"))).collect()
    }

    fn loaded(c: &mut PlayerController) {
        let (generation, uid) = (c.generation, c.queue.current().unwrap().uid);
        c.on_prepared(generation, uid, Ok(Prepared {
            source: TrackSource::Local { path: "x.mp3".into() },
            info: TrackInfo::default(),
            loudness_db: None,
            format: None,
        }));
        c.on_engine(Event::Playing);
    }

    fn fake_preload(c: &mut PlayerController, uid: u64) {
        c.tx.preloaded = Some(Preloaded { uid, generation: c.generation, format: None, gain_db: None });
    }

    #[test]
    fn sleep_timer_counts_adjusts_and_fires() {
        let t0 = Instant::now();
        let mut s = SleepTimer::default();
        assert!(!s.is_active());
        s.start_minutes(10, t0);
        assert!(s.is_active());
        assert_eq!(s.remaining(t0), Some(Duration::from_secs(600)));
        assert!(!s.adjust(60, t0));
        assert_eq!(s.remaining(t0), Some(Duration::from_secs(660)));
        assert!(!s.take_due(t0 + Duration::from_secs(600)));
        assert!(s.take_due(t0 + Duration::from_secs(661)));
        assert!(!s.is_active());
        // going below zero ends it at once
        s.start_minutes(1, t0);
        assert!(s.adjust(-60, t0));
        assert!(!s.is_active());
        // end of song is one-shot
        s.start_end_of_song();
        assert!(s.is_active() && s.remaining(t0).is_none());
        assert!(s.take_end_of_song());
        assert!(!s.take_end_of_song());
        s.start_secs(0, t0);
        assert!(!s.is_active());
    }

    #[test]
    fn next_started_advances_queue_without_loading() {
        let mut c = PlayerController::new(None, None, CtlCfg::default());
        c.play_songs(songs(3), 0, false);
        loaded(&mut c);
        let next_uid = c.queue.entries()[1].uid;
        let generation = c.generation;
        c.pos = 120.0;
        fake_preload(&mut c, next_uid);
        c.on_engine(Event::NextStarted { token: next_uid });
        assert_eq!(c.queue.current_index(), Some(1));
        assert_eq!(c.loaded_uid, Some(next_uid));
        assert_eq!(c.state, PlayState::Playing);
        assert_eq!(c.pos, 0.0);
        assert!(c.generation > generation, "stale resolves must be ignored");
        // and the following natural end still advances normally
        c.on_engine(Event::Finished);
        assert_eq!(c.queue.current_index(), Some(2));
    }

    #[test]
    fn next_started_for_a_moved_entry_jumps_to_it() {
        let mut c = PlayerController::new(None, None, CtlCfg::default());
        c.play_songs(songs(4), 0, false);
        loaded(&mut c);
        let uid3 = c.queue.entries()[3].uid;
        fake_preload(&mut c, uid3);
        c.on_engine(Event::NextStarted { token: uid3 });
        assert_eq!(c.queue.current_index(), Some(3));
        assert_eq!(c.loaded_uid, Some(uid3));
    }

    #[test]
    fn stale_hand_over_is_ignored_while_another_song_loads() {
        let mut c = PlayerController::new(None, None, CtlCfg::default());
        c.play_songs(songs(3), 0, false);
        loaded(&mut c);
        let next_uid = c.queue.entries()[1].uid;
        fake_preload(&mut c, next_uid);
        c.play_index(2); // the user picked another song: resolving…
        c.on_engine(Event::NextStarted { token: next_uid });
        assert_eq!(c.queue.current_index(), Some(2));
        assert_eq!(c.state, PlayState::Loading);
    }

    #[test]
    fn end_of_song_sleep_loads_the_next_song_paused() {
        let mut c = PlayerController::new(None, None, CtlCfg::default());
        c.play_songs(songs(3), 0, false);
        loaded(&mut c);
        c.sleep_end_of_song();
        c.on_engine(Event::Finished);
        assert_eq!(c.queue.current_index(), Some(1));
        assert!(!c.is_playing(), "should wait paused on the next song");
        assert!(!c.tx.sleep.is_active());
    }

    #[test]
    fn same_album_detection() {
        let mut a = Song::remote("a", "A");
        let mut b = Song::remote("b", "B");
        assert!(!same_album(&a, &b));
        a.album_id = Some("MPRE1".into());
        b.album_id = Some("MPRE1".into());
        assert!(same_album(&a, &b));
        b.album_id = Some("MPRE2".into());
        assert!(!same_album(&a, &b));
        let mut c = Song::remote("c", "C");
        let mut d = Song::remote("d", "D");
        c.album_name = Some("Album".into());
        d.album_name = Some("Album".into());
        assert!(same_album(&c, &d));
    }

    #[test]
    fn transition_config_follows_the_mode() {
        let mut s = AppSettings::default();
        s.playback.transition_mode = TransitionMode::Off;
        assert_eq!(transition_config(&s).kind, TransitionKind::Gapless);
        s.playback.transition_mode = TransitionMode::AutoCrossfade;
        s.playback.crossfade_duration_secs = 12.0;
        assert_eq!(transition_config(&s).crossfade_secs, 5.0);
        s.playback.transition_mode = TransitionMode::ManualCrossfade;
        s.playback.crossfade_duration_secs = 45.0;
        let c = transition_config(&s);
        assert_eq!((c.kind, c.crossfade_secs), (TransitionKind::Crossfade, 30.0));
        s.playback.transition_mode = TransitionMode::Crossmix;
        s.transitions.crossmix_style = Some("BassSwap".into());
        let c = transition_config(&s);
        assert_eq!(c.kind, TransitionKind::Crossmix);
        assert_eq!(c.crossmix.style, Some(mochy_audio::crossmix::Style::BassSwap));
    }
}
