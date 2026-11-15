//! Player controller (spec §4.1): owns the queue, resolves streams, drives
//! the audio engine, auto-skips on errors, prefetches the next URL, extends
//! the queue with radio results, records plays and persists the queue.
//!
//! All network / disk work is spawned; results come back as [`AppMsg`]s that
//! the app feeds into [`PlayerController::on_prepared`] etc.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use mochy_audio::{Command, EngineHandle, Event, TrackInfo, TrackSource};
use mochy_core::models::{QueueItem, Song};
use mochy_core::settings::{AppSettings, AudioLab, AudioQuality, RepeatMode};
use mochy_innertube::StreamResult;

use crate::queue::Queue;
use crate::services::{AppMsg, Services};

/// "Previous" restarts the track when the position is past this (spec §4.1).
pub const PREV_RESTART_SECS: f64 = 3.0;
const PREFETCH_AFTER_SECS: f64 = 5.0;
const PREFETCH_TTL: Duration = Duration::from_secs(30 * 60);
const QUEUE_SAVE_DEBOUNCE: Duration = Duration::from_millis(1500);
const MIN_RECORD_SECS: f64 = 5.0;
/// Consecutive failed tracks before auto-skip gives up. Kept small: when
/// YouTube is refusing streams every skip costs a full client walk (~8
/// requests) and makes the bot-check worse.
const MAX_FAIL_STREAK: usize = 3;

/// A track ready for `Command::Load`.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub source: TrackSource,
    pub info: TrackInfo,
    /// YTM `loudnessDb` (for normalization).
    pub loudness_db: Option<f32>,
    /// Human-readable format label (badge).
    pub format: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlayState {
    Idle,
    Loading,
    Playing,
    Paused,
    /// Reached the end of the queue.
    Ended,
    Error(String),
}

/// Settings subset the controller needs (refreshed by the app).
#[derive(Debug, Clone, Default)]
pub struct CtlCfg {
    pub quality: AudioQuality,
    pub normalization: bool,
    pub auto_skip: bool,
    pub replay_gain: bool,
    pub replay_gain_preamp: f32,
    pub persistent_queue: bool,
}

impl CtlCfg {
    pub fn from_settings(s: &AppSettings) -> Self {
        Self {
            quality: s.playback.audio_quality,
            normalization: s.playback.audio_normalization,
            auto_skip: s.playback.auto_skip_next_on_error,
            replay_gain: s.audio_lab.normalize_replay_gain,
            replay_gain_preamp: s.audio_lab.normalize_preamp_db,
            persistent_queue: s.misc.persistent_queue,
        }
    }
}

pub struct PlayerController {
    svc: Option<Services>,
    engine: Option<EngineHandle>,
    pub queue: Queue,
    pub state: PlayState,
    pub pos: f64,
    pub duration: Option<f64>,
    pub volume: f32,
    /// Codec badge for the current track.
    pub format: Option<String>,
    pub cfg: CtlCfg,
    generation: u64,
    /// uid of the entry whose Load was sent to the engine.
    loaded_uid: Option<u64>,
    /// True while a Load command is in flight (engine has not answered yet).
    awaiting_engine: bool,
    want_play: bool,
    prefetch: HashMap<String, (StreamResult, Instant)>,
    prefetch_requested: HashSet<String>,
    radio_inflight: bool,
    radio_seeds: HashSet<String>,
    listened: f64,
    last_pos: f64,
    fail_streak: usize,
    queue_dirty: Option<Instant>,
    /// Current loudness gain applied (for display / debugging).
    pub applied_gain_db: Option<f32>,
}

pub fn song_from_queue_item(q: &QueueItem) -> Song {
    let mut s = Song::remote(q.video_id.clone(), q.title.clone());
    s.artists = q.artists.clone();
    s.duration_secs = q.duration_secs;
    s.thumbnail_url = q.thumbnail_url.clone();
    s
}

pub fn queue_item_from_song(s: &Song) -> QueueItem {
    QueueItem {
        // Local songs are stored by their library id and re-hydrated from the DB.
        video_id: s.video_id.clone().unwrap_or_else(|| s.id.clone()),
        playlist_id: None,
        set_video_id: None,
        title: s.title.clone(),
        artists: s.artists.clone(),
        duration_secs: s.duration_secs,
        thumbnail_url: s.thumbnail_url.clone(),
    }
}

pub fn format_label(st: &StreamResult) -> String {
    let codec = match st.format {
        mochy_innertube::AudioFormat::AacM4a => "AAC",
        mochy_innertube::AudioFormat::OpusWebm => "OPUS",
        mochy_innertube::AudioFormat::Mp3 => "MP3",
        mochy_innertube::AudioFormat::Flac => "FLAC",
        mochy_innertube::AudioFormat::Unknown => "?",
    };
    match st.bitrate_kbps {
        Some(b) => format!("{codec} {b} kbps"),
        None => codec.to_string(),
    }
}

impl PlayerController {
    pub fn new(svc: Option<Services>, engine: Option<EngineHandle>, cfg: CtlCfg) -> Self {
        Self {
            svc,
            engine,
            queue: Queue::new(),
            state: PlayState::Idle,
            pos: 0.0,
            duration: None,
            volume: 1.0,
            format: None,
            cfg,
            generation: 0,
            loaded_uid: None,
            awaiting_engine: false,
            want_play: false,
            prefetch: HashMap::new(),
            prefetch_requested: HashSet::new(),
            radio_inflight: false,
            radio_seeds: HashSet::new(),
            listened: 0.0,
            last_pos: 0.0,
            fail_streak: 0,
            queue_dirty: None,
            applied_gain_db: None,
        }
    }

    pub fn engine(&self) -> Option<&EngineHandle> {
        self.engine.as_ref()
    }

    pub fn send(&self, cmd: Command) {
        if let Some(e) = &self.engine {
            e.send(cmd);
        }
    }

    pub fn current(&self) -> Option<&Song> {
        self.queue.current_song()
    }

    pub fn is_playing(&self) -> bool {
        self.state == PlayState::Playing || (self.state == PlayState::Loading && self.want_play)
    }

    fn mark_queue_dirty(&mut self) {
        self.queue_dirty = Some(Instant::now());
    }

    // ---------------------------------------------------------------------
    // Queue-level commands
    // ---------------------------------------------------------------------

    /// Replace the queue and start playing at `start`.
    pub fn play_songs(&mut self, songs: Vec<Song>, start: usize, shuffle: bool) {
        if songs.is_empty() {
            return;
        }
        self.radio_seeds.clear();
        self.queue.set_shuffle(false);
        self.queue.set(songs, start);
        if shuffle {
            self.queue.set_shuffle(true);
        }
        self.mark_queue_dirty();
        self.start_current(true);
    }

    /// Play a single song and fill the queue with its radio ("watch next").
    pub fn play_with_radio(&mut self, song: Song) {
        let seed = song.video_id.clone();
        self.play_songs(vec![song], 0, false);
        if let Some(seed) = seed {
            self.request_radio(seed, false);
        }
    }

    pub fn play_next(&mut self, song: Song) {
        let was_empty = self.queue.is_empty();
        self.queue.insert_next(song);
        self.mark_queue_dirty();
        if was_empty {
            self.start_current(true);
        }
    }

    pub fn add_to_queue(&mut self, songs: Vec<Song>) {
        let was_empty = self.queue.is_empty();
        self.queue.append(songs);
        self.mark_queue_dirty();
        if was_empty {
            self.start_current(true);
        }
    }

    pub fn play_index(&mut self, idx: usize) {
        if self.queue.jump_to(idx) {
            self.mark_queue_dirty();
            self.start_current(true);
        }
    }

    pub fn remove(&mut self, idx: usize) {
        let removed_current = self.queue.remove(idx);
        self.mark_queue_dirty();
        if removed_current {
            if self.queue.is_empty() {
                self.stop();
            } else {
                let play = self.is_playing();
                self.start_current(play);
            }
        }
    }

    pub fn move_entry(&mut self, from: usize, to: usize) {
        self.queue.move_entry(from, to);
        self.mark_queue_dirty();
    }

    pub fn clear_queue(&mut self) {
        self.queue.clear();
        self.mark_queue_dirty();
        self.stop();
    }

    pub fn set_shuffle(&mut self, on: bool) {
        self.queue.set_shuffle(on);
        self.prefetch_requested.clear();
        // Never persist an empty queue as a side effect (e.g. at startup,
        // before the saved queue has been restored).
        if !self.queue.is_empty() {
            self.mark_queue_dirty();
        }
    }

    pub fn set_repeat(&mut self, mode: RepeatMode) {
        self.queue.repeat = mode;
    }

    pub fn cycle_repeat(&mut self) -> RepeatMode {
        self.queue.cycle_repeat()
    }

    // ---------------------------------------------------------------------
    // Transport
    // ---------------------------------------------------------------------

    pub fn toggle(&mut self) {
        match self.state {
            PlayState::Playing => {
                self.want_play = false;
                self.send(Command::Pause);
            }
            PlayState::Paused if self.loaded_uid.is_some() => {
                self.want_play = true;
                self.send(Command::Play);
            }
            PlayState::Loading => {
                self.want_play = !self.want_play;
                self.send(if self.want_play { Command::Play } else { Command::Pause });
            }
            _ => {
                // Idle (restored queue), ended or failed: (re)load the current track.
                if self.queue.current().is_some() {
                    if self.state == PlayState::Ended {
                        self.queue.jump_to(0);
                    }
                    self.start_current(true);
                }
            }
        }
    }

    pub fn play(&mut self) {
        if !self.is_playing() {
            self.toggle();
        }
    }

    pub fn pause(&mut self) {
        if self.is_playing() {
            self.toggle();
        }
    }

    pub fn stop(&mut self) {
        self.flush_play_record();
        self.generation += 1;
        self.send(Command::Stop);
        self.loaded_uid = None;
        self.awaiting_engine = false;
        self.want_play = false;
        self.state = PlayState::Idle;
        self.pos = 0.0;
    }

    pub fn next(&mut self) {
        if self.queue.advance(false).is_some() {
            self.mark_queue_dirty();
            self.start_current(true);
        } else if let Some(seed) = self.queue.current_song().and_then(|s| s.video_id.clone()) {
            // End of queue: fetch radio and continue when it arrives.
            self.request_radio(seed, true);
        }
    }

    pub fn prev(&mut self) {
        if self.pos > PREV_RESTART_SECS {
            self.seek(0.0);
            return;
        }
        if self.queue.go_prev().is_some() {
            self.mark_queue_dirty();
            self.start_current(true);
        }
    }

    pub fn seek(&mut self, secs: f64) {
        let secs = match self.duration {
            Some(d) if d > 0.0 => secs.clamp(0.0, d),
            _ => secs.max(0.0),
        };
        self.pos = secs;
        self.last_pos = secs;
        if self.loaded_uid.is_some() {
            self.send(Command::SeekTo { secs });
        }
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
        self.send(Command::SetVolume { linear: self.volume });
    }

    // ---------------------------------------------------------------------
    // Loading
    // ---------------------------------------------------------------------

    /// Load the queue's current entry (resolving the stream if needed).
    fn start_current(&mut self, autoplay: bool) {
        self.flush_play_record();
        self.generation += 1;
        self.format = None;
        self.pos = 0.0;
        self.last_pos = 0.0;
        self.want_play = autoplay;
        let Some(entry) = self.queue.current().cloned() else {
            self.state = PlayState::Idle;
            return;
        };
        self.duration = entry.song.duration_secs;
        self.state = PlayState::Loading;
        self.loaded_uid = None;
        let generation = self.generation;
        let uid = entry.uid;
        let song = entry.song;
        let Some(svc) = self.svc.clone() else { return };

        // Make sure the song row exists for history / likes / playlists.
        {
            let s = song.clone();
            svc.db.run(move |db| crate::db::ensure_song(db, &s));
        }

        if let Some(path) = song.local_path.clone() {
            let duration = song.duration_secs;
            let rt = svc.rt.clone();
            rt.spawn(async move {
                let p = path.clone();
                let (gain, peak) = tokio::task::spawn_blocking(move || crate::local_scan::read_replay_gain(&p))
                    .await
                    .unwrap_or((None, None));
                let result = if path.exists() {
                    Ok(Prepared {
                        source: TrackSource::Local { path },
                        info: TrackInfo { duration_secs: duration, video_id: None, replay_gain_db: gain, replay_peak: peak },
                        loudness_db: None,
                        format: None,
                    })
                } else {
                    Err(format!("file not found: {}", path.display()))
                };
                svc.bus.send(AppMsg::Prepared { generation, uid, result });
            });
            return;
        }

        let Some(video_id) = song.video_id.clone() else {
            self.state = PlayState::Error("song has no source".into());
            return;
        };
        // Fresh prefetched URL?
        if let Some((st, at)) = self.prefetch.remove(&video_id)
            && at.elapsed() < PREFETCH_TTL
        {
            tracing::debug!(%video_id, "using prefetched stream");
            let p = prepared_from_stream(st, &song);
            self.on_prepared(generation, uid, Ok(p));
            return;
        }
        let quality = self.cfg.quality;
        let s2 = song.clone();
        svc.ytm_call(
            "resolve_stream",
            move |c| async move { c.resolve_stream(&video_id, quality).await },
            move |r| AppMsg::Prepared { generation, uid, result: r.map(|st| prepared_from_stream(st, &s2)) },
        );
    }

    /// Stream resolved (or failed) for generation `generation`.
    pub fn on_prepared(&mut self, generation: u64, uid: u64, result: Result<Prepared, String>) {
        if generation != self.generation || self.queue.current().map(|e| e.uid) != Some(uid) {
            return; // superseded
        }
        match result {
            Ok(p) => {
                // Normalization: YTM loudness or file ReplayGain (spec §4.6).
                let (gain_db, peak, preamp) = if let Some(l) = p.loudness_db.filter(|_| self.cfg.normalization) {
                    (Some(-l), None, 0.0)
                } else if self.cfg.replay_gain && p.info.replay_gain_db.is_some() {
                    (p.info.replay_gain_db, p.info.replay_peak, self.cfg.replay_gain_preamp)
                } else {
                    (None, None, 0.0)
                };
                self.applied_gain_db = gain_db;
                self.send(Command::SetReplayGain { gain_db, peak, preamp_db: preamp });
                let mut info = p.info;
                if !self.cfg.replay_gain {
                    info.replay_gain_db = None;
                    info.replay_peak = None;
                }
                if info.duration_secs.is_some() {
                    self.duration = info.duration_secs;
                }
                self.format = p.format;
                self.loaded_uid = Some(uid);
                self.awaiting_engine = true;
                tracing::info!(title = %self.current().map(|s| s.title.as_str()).unwrap_or(""), "loading track");
                self.send(Command::Load { source: p.source, info, autoplay: self.want_play });
            }
            Err(e) => {
                tracing::warn!("track failed to resolve: {e}");
                self.on_track_failed(e);
            }
        }
    }

    fn on_track_failed(&mut self, err: String) {
        self.loaded_uid = None;
        self.awaiting_engine = false;
        self.state = PlayState::Error(err.clone());
        if let Some(svc) = &self.svc {
            let title = self.current().map(|s| s.title.clone()).unwrap_or_default();
            svc.bus.toast(format!("Can't play \"{title}\": {err}"));
        }
        if self.cfg.auto_skip && self.fail_streak < MAX_FAIL_STREAK.min(self.queue.len().saturating_sub(1)) {
            self.fail_streak += 1;
            if self.queue.advance(false).is_some() {
                self.mark_queue_dirty();
                self.start_current(true);
            }
        }
    }

    // ---------------------------------------------------------------------
    // Engine events
    // ---------------------------------------------------------------------

    pub fn on_engine(&mut self, ev: Event) {
        match ev {
            Event::Loading => {
                if self.loaded_uid.is_some() {
                    self.state = PlayState::Loading;
                }
            }
            Event::Playing => {
                self.awaiting_engine = false;
                if self.loaded_uid.is_some() {
                    self.state = PlayState::Playing;
                }
            }
            Event::Paused => {
                self.awaiting_engine = false;
                if self.loaded_uid.is_some() {
                    self.state = PlayState::Paused;
                }
            }
            Event::Stopped => {}
            Event::Finished => {
                if self.loaded_uid.is_some() && !self.awaiting_engine {
                    self.on_track_end();
                }
            }
            Event::Error(e) => {
                tracing::warn!("engine error: {e}");
                if self.loaded_uid.is_some() {
                    self.on_track_failed(e);
                } else {
                    self.state = PlayState::Error(e);
                }
            }
            Event::Progress { pos_secs, duration_secs } => {
                if self.loaded_uid.is_none() || self.awaiting_engine {
                    return;
                }
                let delta = pos_secs - self.last_pos;
                if delta > 0.0 && delta < 1.5 {
                    self.listened += delta;
                }
                self.last_pos = pos_secs;
                self.pos = pos_secs;
                if let Some(d) = duration_secs.filter(|d| *d > 0.0) {
                    self.duration = Some(d);
                }
                if pos_secs > 1.0 {
                    self.fail_streak = 0;
                }
                if pos_secs > PREFETCH_AFTER_SECS {
                    self.maybe_prefetch();
                    self.maybe_extend_with_radio();
                }
            }
            Event::TrackInfo { codec, sample_rate, channels, bits_per_sample } => {
                let mut s = format!("{} {:.1} kHz", codec.to_uppercase(), sample_rate as f32 / 1000.0);
                if let Some(b) = bits_per_sample {
                    s.push_str(&format!(" {b}-bit"));
                }
                if channels == 1 {
                    s.push_str(" mono");
                }
                self.format = Some(match self.format.take() {
                    Some(f) if f.contains("kbps") => format!("{f} · {s}"),
                    _ => s,
                });
            }
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }

    fn on_track_end(&mut self) {
        let finished_uid = self.loaded_uid;
        self.flush_play_record();
        match self.queue.advance(true) {
            Some(_) => {
                if self.queue.current().map(|e| e.uid) == finished_uid && self.queue.repeat == RepeatMode::One {
                    tracing::debug!("repeat one");
                }
                self.mark_queue_dirty();
                self.start_current(true);
            }
            None => {
                self.state = PlayState::Ended;
                self.loaded_uid = None;
                self.pos = 0.0;
                if let Some(seed) = self.queue.current_song().and_then(|s| s.video_id.clone()) {
                    self.request_radio(seed, true);
                }
            }
        }
    }

    fn flush_play_record(&mut self) {
        let listened = std::mem::take(&mut self.listened);
        if listened < MIN_RECORD_SECS || self.loaded_uid.is_none() {
            return;
        }
        let Some(song) = self.current().cloned() else { return };
        let Some(svc) = &self.svc else { return };
        svc.db.run(move |db| {
            crate::db::ensure_song(db, &song);
            crate::db::log_err("record_play", db.record_play(&song.id, listened));
        });
    }

    // ---------------------------------------------------------------------
    // Prefetch & radio
    // ---------------------------------------------------------------------

    fn maybe_prefetch(&mut self) {
        let Some(idx) = self.queue.peek_next(true) else { return };
        let Some(next) = self.queue.entries().get(idx) else { return };
        let Some(vid) = next.song.video_id.clone() else { return };
        if next.song.is_local() || !self.prefetch_requested.insert(vid.clone()) {
            return;
        }
        let Some(svc) = self.svc.clone() else { return };
        let quality = self.cfg.quality;
        let rt = svc.rt.clone();
        rt.spawn(async move {
            let Ok(c) = svc.ytm().await else { return };
            match c.resolve_stream(&vid, quality).await {
                Ok(stream) => {
                    tracing::debug!(%vid, "prefetched next stream");
                    svc.bus.send(AppMsg::Prefetched { video_id: vid, stream });
                }
                Err(e) => tracing::debug!(%vid, "prefetch failed: {e}"),
            }
        });
    }

    pub fn on_prefetched(&mut self, video_id: String, stream: StreamResult) {
        self.prefetch.retain(|_, (_, at)| at.elapsed() < PREFETCH_TTL);
        self.prefetch.insert(video_id, (stream, Instant::now()));
    }

    /// Infinite queue: when the last track is playing, append radio results.
    fn maybe_extend_with_radio(&mut self) {
        if !self.queue.is_last() || self.queue.repeat == RepeatMode::One {
            return;
        }
        if let Some(seed) = self.queue.current_song().and_then(|s| s.video_id.clone()) {
            self.request_radio(seed, false);
        }
    }

    pub fn request_radio(&mut self, seed: String, play_after: bool) {
        if self.radio_inflight || !self.radio_seeds.insert(seed.clone()) {
            return;
        }
        let Some(svc) = self.svc.clone() else { return };
        self.radio_inflight = true;
        let s = seed.clone();
        svc.ytm_call(
            "get_queue",
            move |c| async move { c.get_queue(&s).await },
            move |r| AppMsg::Radio {
                seed,
                result: r.map(|items| items.iter().map(song_from_queue_item).collect()),
                play_after,
            },
        );
    }

    pub fn on_radio(&mut self, seed: String, result: Result<Vec<Song>, String>, play_after: bool) {
        self.radio_inflight = false;
        let songs = match result {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(%seed, "radio failed: {e}");
                if play_after && self.queue.repeat == RepeatMode::All && !self.queue.is_empty() {
                    self.queue.jump_to(0);
                    self.start_current(true);
                }
                return;
            }
        };
        let fresh: Vec<Song> = songs
            .into_iter()
            .filter(|s| s.video_id.as_deref() != Some(seed.as_str()) && !self.queue.contains_id(&s.id))
            .collect();
        tracing::info!(count = fresh.len(), "radio appended");
        if fresh.is_empty() {
            return;
        }
        self.queue.append(fresh);
        self.mark_queue_dirty();
        if play_after && matches!(self.state, PlayState::Ended | PlayState::Idle | PlayState::Error(_) | PlayState::Paused)
            && self.queue.advance(false).is_some()
        {
            self.start_current(true);
        }
    }

    // ---------------------------------------------------------------------
    // Persistence
    // ---------------------------------------------------------------------

    /// Ask the DB for the persisted queue (result: `AppMsg::QueueRestored`).
    pub fn request_restore(&self) {
        let Some(svc) = self.svc.clone() else { return };
        let bus = svc.bus.clone();
        svc.db.run(move |db| {
            let Some((items, index)) = crate::db::log_err("load_queue", db.load_queue()).flatten() else { return };
            if items.is_empty() {
                return;
            }
            let songs: Vec<Song> = items
                .iter()
                .map(|q| match db.get_song(&q.video_id) {
                    Ok(Some(s)) => s,
                    _ => song_from_queue_item(q),
                })
                .collect();
            bus.send(AppMsg::QueueRestored { songs, index });
        });
    }

    /// Restore a persisted queue, paused (nothing loaded until play).
    pub fn on_restored(&mut self, songs: Vec<Song>, index: usize) {
        if !self.queue.is_empty() {
            return; // the user already started something
        }
        let shuffle = self.queue.shuffle();
        self.queue.set_shuffle(false);
        self.queue.set(songs, index);
        if shuffle {
            // Keep the saved (already shuffled) order: mark shuffled without reshuffling.
            self.queue.set_shuffle_flag_only(true);
        }
        self.duration = self.queue.current_song().and_then(|s| s.duration_secs);
        self.state = PlayState::Idle;
        tracing::info!(len = self.queue.len(), index, "queue restored (paused)");
    }

    /// Periodic housekeeping (debounced queue save).
    pub fn tick(&mut self) {
        if let Some(t) = self.queue_dirty
            && t.elapsed() >= QUEUE_SAVE_DEBOUNCE
        {
            self.queue_dirty = None;
            self.save_queue_now();
        }
    }

    pub fn save_queue_now(&mut self) {
        if !self.cfg.persistent_queue {
            return;
        }
        let Some(svc) = &self.svc else { return };
        let items: Vec<QueueItem> = self.queue.songs().map(queue_item_from_song).collect();
        let index = self.queue.current_index().unwrap_or(0);
        svc.db.run(move |db| {
            crate::db::log_err("save_queue", db.save_queue(&items, index));
        });
    }

    /// Flush state at shutdown.
    pub fn shutdown(&mut self) {
        self.flush_play_record();
        self.save_queue_now();
        self.send(Command::Shutdown);
    }
}

fn prepared_from_stream(st: StreamResult, song: &Song) -> Prepared {
    let format = Some(format_label(&st));
    Prepared {
        info: TrackInfo {
            duration_secs: song.duration_secs,
            video_id: song.video_id.clone(),
            replay_gain_db: None,
            replay_peak: None,
        },
        loudness_db: st.loudness_db,
        format,
        source: TrackSource::Remote { url: st.url, headers: st.headers },
    }
}

/// Push every Audio Lab setting to the engine (startup + on change).
pub fn apply_audio_lab(engine: &EngineHandle, lab: &AudioLab) {
    engine.send(Command::SetEq { eq: lab.eq.clone() });
    engine.send(Command::SetLoudness {
        enabled: lab.loudness_enabled,
        boost_db: lab.loudness_boost_db,
        compression: lab.loudness_compression,
    });
    engine.send(Command::SetVolumeCurve {
        enabled: lab.volume_curve_enabled,
        exponent: lab.volume_curve_exponent,
        limit: lab.volume_curve_limit,
    });
    engine.send(Command::SetSpeed { factor: lab.speed });
    engine.send(Command::SetPitch { semitones: lab.pitch_semitones, preserve_pitch: lab.preserve_pitch });
    engine.send(Command::SetReverb { reverb: lab.reverb.clone() });
    engine.send(Command::SetSpatial { spatial: lab.spatial.clone() });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn songs(n: usize) -> Vec<Song> {
        (0..n).map(|i| Song::remote(format!("v{i}"), format!("Song {i}"))).collect()
    }

    fn ctl() -> PlayerController {
        PlayerController::new(None, None, CtlCfg { auto_skip: true, ..Default::default() })
    }

    #[test]
    fn prev_restarts_after_three_seconds() {
        let mut c = ctl();
        c.play_songs(songs(3), 1, false);
        c.pos = 10.0;
        c.prev();
        assert_eq!(c.queue.current_index(), Some(1));
        assert_eq!(c.pos, 0.0);
        c.pos = 1.0;
        c.prev();
        assert_eq!(c.queue.current_index(), Some(0));
    }

    #[test]
    fn next_and_stale_prepared_results_are_ignored() {
        let mut c = ctl();
        c.play_songs(songs(3), 0, false);
        let first_uid = c.queue.current().unwrap().uid;
        let first_gen = c.generation;
        c.next();
        assert_eq!(c.queue.current_index(), Some(1));
        // A late result for the first track must not load anything.
        c.on_prepared(first_gen, first_uid, Err("late".into()));
        assert_eq!(c.state, PlayState::Loading);
        assert_eq!(c.queue.current_index(), Some(1));
    }

    #[test]
    fn auto_skip_on_resolve_error() {
        let mut c = ctl();
        c.play_songs(songs(4), 0, false);
        let (generation, uid) = (c.generation, c.queue.current().unwrap().uid);
        c.on_prepared(generation, uid, Err("no stream".into()));
        assert_eq!(c.queue.current_index(), Some(1));
        assert_eq!(c.state, PlayState::Loading);
    }

    #[test]
    fn no_auto_skip_when_disabled() {
        let mut c = PlayerController::new(None, None, CtlCfg::default());
        c.play_songs(songs(4), 0, false);
        let (generation, uid) = (c.generation, c.queue.current().unwrap().uid);
        c.on_prepared(generation, uid, Err("no stream".into()));
        assert_eq!(c.queue.current_index(), Some(0));
        assert!(matches!(c.state, PlayState::Error(_)));
    }

    #[test]
    fn finished_advances_and_repeat_one_replays() {
        let mut c = ctl();
        c.play_songs(songs(3), 0, false);
        let (generation, uid) = (c.generation, c.queue.current().unwrap().uid);
        c.on_prepared(generation, uid, Ok(Prepared {
            source: TrackSource::Local { path: "x.mp3".into() },
            info: TrackInfo::default(),
            loudness_db: None,
            format: None,
        }));
        c.on_engine(Event::Playing);
        assert_eq!(c.state, PlayState::Playing);
        c.on_engine(Event::Finished);
        assert_eq!(c.queue.current_index(), Some(1));

        c.set_repeat(RepeatMode::One);
        let (generation, uid) = (c.generation, c.queue.current().unwrap().uid);
        c.on_prepared(generation, uid, Ok(Prepared {
            source: TrackSource::Local { path: "x.mp3".into() },
            info: TrackInfo::default(),
            loudness_db: None,
            format: None,
        }));
        c.on_engine(Event::Playing);
        c.on_engine(Event::Finished);
        assert_eq!(c.queue.current_index(), Some(1));
    }

    #[test]
    fn end_of_queue_with_repeat_off() {
        let mut c = ctl();
        c.set_repeat(RepeatMode::Off);
        c.play_songs(songs(1), 0, false);
        let (generation, uid) = (c.generation, c.queue.current().unwrap().uid);
        c.on_prepared(generation, uid, Ok(Prepared {
            source: TrackSource::Local { path: "x.mp3".into() },
            info: TrackInfo::default(),
            loudness_db: None,
            format: None,
        }));
        c.on_engine(Event::Playing);
        c.on_engine(Event::Finished);
        assert_eq!(c.state, PlayState::Ended);
        // radio arriving later continues playback
        c.on_radio("v0".into(), Ok(vec![Song::remote("r1", "R1"), Song::remote("v0", "seed")]), true);
        assert_eq!(c.queue.len(), 2);
        assert_eq!(c.current().unwrap().id, "r1");
    }

    #[test]
    fn remove_current_loads_successor() {
        let mut c = ctl();
        c.play_songs(songs(3), 1, false);
        c.remove(1);
        assert_eq!(c.current().unwrap().id, "v2");
        c.remove(0);
        c.remove(0);
        assert!(c.queue.is_empty());
        assert_eq!(c.state, PlayState::Idle);
    }

    #[test]
    fn queue_item_roundtrip() {
        let mut s = Song::remote("abc", "T");
        s.duration_secs = Some(12.0);
        let q = queue_item_from_song(&s);
        let back = song_from_queue_item(&q);
        assert_eq!(back.id, "abc");
        assert_eq!(back.duration_secs, Some(12.0));
        let mut local = Song { id: "LOC_1".into(), title: "L".into(), ..Default::default() };
        local.local_path = Some("a.mp3".into());
        assert_eq!(queue_item_from_song(&local).video_id, "LOC_1");
    }
}
