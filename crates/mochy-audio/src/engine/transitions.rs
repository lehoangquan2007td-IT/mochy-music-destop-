//! Engine side of transitions: transport fades (deferred pause / skip /
//! seek behind a fade-out), the preloaded next track (opener thread,
//! crossfade / crossmix orders, promotion when the device reaches the
//! switch), per-song tuning and crossmix analysis requests.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::Engine;
use super::TrackState;
use crate::crossmix::planner::{MAX_LEAD_MS, Plan, TrackInput, plan_from};
use crate::crossmix::worker::{LOCAL_HEAD_SECS, REMOTE_HEAD_SECS, REMOTE_TAIL_SECS, Span};
use crate::decoder::{DecCmd, DecodeSession, SourceInfo, TrackShared};
use crate::dsp::chain::ParamMsg;
use crate::dsp::{replay_gain_factor, volume_curve_gain};
use crate::mix::{MixOrder, NextLane};
use crate::transition::{NextGain, NextTrack, SongTuning, TransitionConfig, TransitionKind};
use crate::{Event, TrackInfo, TrackSource};

/// A little over the callback's own de-click, so the fade is fully heard.
const SETTLE: Duration = Duration::from_millis(30);
/// Crossmix: plan with whatever analysis exists this long before the
/// earliest possible hand-over.
const PLAN_DEADLINE_MS: i64 = 4_000;

/// An action waiting for its fade-out.
pub(crate) enum Deferred {
    /// Pause button; `rewind_to` = where pause was pressed (the fade let
    /// the song run on under it).
    Pause { rewind_to: Option<f64> },
    /// Sleep timer.
    PauseFade,
    Load { source: TrackSource, info: TrackInfo, autoplay: bool },
    Seek { secs: f64 },
}

/// The preloaded track.
pub(crate) struct NextSlot {
    token: u64,
    id: Option<String>,
    source: TrackSource,
    info: TrackInfo,
    gain: NextGain,
    tuning: Option<SongTuning>,
    allow_mix: bool,
    shared: Arc<TrackShared>,
    cancel: Arc<AtomicBool>,
    order_sent: bool,
    gain_at: Option<u64>,
}

/// Stable id for analysis caching: the video id, else the path / URL.
pub(crate) fn track_id(source: &TrackSource, info: &TrackInfo) -> Option<String> {
    if let Some(v) = &info.video_id {
        return Some(v.clone());
    }
    match source {
        TrackSource::Local { path } => Some(path.to_string_lossy().into_owned()),
        TrackSource::Remote { url, .. } => Some(url.split('?').next().unwrap_or(url).to_string()),
    }
}

/// Gain (dB), peak and preamp a preloaded track plays at.
fn next_rg(gain: &NextGain, info: Option<&SourceInfo>) -> (Option<f32>, Option<f32>, f32) {
    if gain.gain_db.is_some() {
        return (gain.gain_db, gain.peak, gain.preamp_db);
    }
    if gain.use_file_tags
        && let Some(i) = info
        && i.replay_gain.track_gain.is_some()
    {
        return (i.replay_gain.track_gain, i.replay_gain.track_peak, gain.preamp_db);
    }
    (None, None, gain.preamp_db)
}

fn rg_factor(gain: Option<f32>, peak: Option<f32>, preamp: f32) -> f32 {
    gain.map_or(1.0, |g| replay_gain_factor(g, peak, preamp))
}

impl Engine {
    fn fade_level(&self) -> f32 {
        self.out.as_ref().map_or(1.0, |o| f32::from_bits(o.link.fade_level.load(Ordering::Relaxed)))
    }

    fn frames(&self, secs: f32) -> u32 {
        (secs.max(0.0) * self.sample_rate() as f32) as u32
    }

    fn fade_to(&mut self, target: f32, secs: f32, wait: bool) {
        let frames = self.frames(secs);
        self.send_param(ParamMsg::Fade { target, frames, wait });
    }

    fn fade_set(&mut self, level: f32) {
        self.send_param(ParamMsg::FadeSet(level));
    }

    /// Volume × curve, without ReplayGain.
    fn volume_part(&self) -> f32 {
        let (enabled, exp, limit) = self.curve;
        let v = self.volume.clamp(0.0, 1.0);
        if enabled { volume_curve_gain(v, exp, limit) } else { v }
    }

    fn current_rg_factor(&self) -> f32 {
        rg_factor(self.rg_gain, self.rg_peak, self.rg_preamp)
    }

    /// What the play button should show: a pause fading out is a pause.
    pub(super) fn intends_playing(&self) -> bool {
        match &self.deferred {
            Some((_, Deferred::Pause { .. } | Deferred::PauseFade)) => false,
            Some((_, Deferred::Load { autoplay, .. })) => *autoplay,
            _ => self.track.as_ref().is_some_and(|t| t.playing),
        }
    }

    fn audible(&self) -> bool {
        self.track.as_ref().is_some_and(|t| t.playing && !t.finished)
    }

    // -----------------------------------------------------------------------
    // transport
    // -----------------------------------------------------------------------

    pub(super) fn cmd_load(&mut self, source: TrackSource, info: TrackInfo, autoplay: bool) {
        // another skip while one fades: the latest pick wins, on the same fade
        if let Some((_, Deferred::Load { source: s, info: i, autoplay: a })) = &mut self.deferred {
            *s = source;
            *i = info;
            *a = autoplay;
            self.tuning_pending = None;
            return;
        }
        let interrupted = self.deferred.is_some();
        if self.fade.active() && self.audible() && !interrupted {
            // a new song is coming: nothing may run on into the old next one meanwhile
            self.drop_next();
            let len = self.fade.skip_out();
            self.fade_to(0.0, len, false);
            self.deferred = Some((Instant::now() + Duration::from_secs_f32(len) + SETTLE, Deferred::Load { source, info, autoplay }));
            self.tuning_pending = None;
            self.emit(Event::Loading);
            return;
        }
        // a pause or seek fading out: the new song wins and swells in
        self.deferred = None;
        self.load_now(source, info, autoplay);
        if interrupted && autoplay && self.fade.active() {
            self.fade_set(0.0);
            self.fade_to(1.0, self.fade.skip_in(), true);
        } else {
            self.fade_set(1.0);
        }
    }

    pub(super) fn cmd_play(&mut self) {
        match self.deferred.take() {
            Some((_, Deferred::Pause { .. } | Deferred::PauseFade)) => {
                // a change of mind mid-pause comes back up from where the fade had got to
                let level = self.fade_level();
                let len = if self.fade.active() { self.fade.resume_len(level) } else { crate::transition::RESUME_MIN_SECS };
                self.fade_to(1.0, len, false);
                self.emit(Event::Playing);
                return;
            }
            Some((due, Deferred::Load { source, info, .. })) => {
                self.deferred = Some((due, Deferred::Load { source, info, autoplay: true }));
                return;
            }
            other => self.deferred = other,
        }
        let resuming = self.track.as_ref().is_some_and(|t| !t.playing);
        if resuming {
            if self.fade.active() {
                self.fade_set(0.0);
                self.fade_to(1.0, self.fade.len(), true);
            } else {
                self.fade_set(1.0);
            }
        }
        self.play();
    }

    /// `custom` = sleep-timer fade length (fades even with transport fades off).
    pub(super) fn cmd_pause(&mut self, custom: Option<f32>) {
        if self.track.is_none() {
            return;
        }
        match &mut self.deferred {
            Some((_, Deferred::Pause { .. } | Deferred::PauseFade)) => {
                // a second press means now
                if custom.is_none() {
                    self.finish_deferred_now();
                }
                return;
            }
            Some((_, Deferred::Load { autoplay, .. })) => {
                *autoplay = false;
                self.emit(Event::Paused);
                return;
            }
            Some((_, Deferred::Seek { .. })) => self.finish_deferred_now(),
            None => {}
        }
        let len = match custom {
            Some(s) if s > 0.0 => Some(s),
            Some(_) => None,
            None => self.fade.active().then(|| self.fade.len()),
        };
        if let (true, Some(len)) = (self.audible(), len) {
            let rewind_to = if custom.is_none() {
                let p = self.position();
                // near the end the song may roll on into the next one: don't jump back
                (self.next.is_none() && self.duration().is_none_or(|d| d - p > MAX_LEAD_MS as f64 / 1000.0)).then_some(p)
            } else {
                None
            };
            self.fade_to(0.0, len, false);
            let op = if custom.is_some() { Deferred::PauseFade } else { Deferred::Pause { rewind_to } };
            self.deferred = Some((Instant::now() + Duration::from_secs_f32(len) + SETTLE, op));
            self.emit(Event::Paused);
            return;
        }
        self.pause();
    }

    pub(super) fn cmd_seek(&mut self, secs: f64) {
        if self.track.is_none() || !secs.is_finite() {
            return;
        }
        self.promote_if_switched();
        let dur = self.duration();
        match &mut self.deferred {
            Some((_, Deferred::Seek { secs: s })) => {
                *s = secs;
                if let Some(t) = &mut self.track {
                    t.pending_seek = Some((secs, u64::MAX));
                    t.last_pos = secs;
                }
                self.emit(Event::Progress { pos_secs: secs, duration_secs: dur });
                return;
            }
            // seeking the song that's being skipped away from means nothing
            Some((_, Deferred::Load { .. })) => return,
            _ => {}
        }
        if self.fade.active() && self.audible() && self.deferred.is_none() {
            let len = self.fade.seek_len();
            self.fade_to(0.0, len, false);
            self.deferred = Some((Instant::now() + Duration::from_secs_f32(len) + SETTLE, Deferred::Seek { secs }));
            if let Some(t) = &mut self.track {
                t.pending_seek = Some((secs, u64::MAX));
                t.last_pos = secs;
                t.finished = false;
            }
            self.emit(Event::Progress { pos_secs: secs, duration_secs: dur });
            self.last_progress = Instant::now();
            return;
        }
        self.seek(secs);
    }

    pub(super) fn cancel_deferred(&mut self) {
        if self.deferred.take().is_some() {
            self.fade_set(1.0);
        }
        self.tuning_pending = None;
    }

    /// Run the waiting action now (its fade is over, or it was cut short).
    pub(super) fn finish_deferred_now(&mut self) {
        let Some((_, op)) = self.deferred.take() else { return };
        match op {
            Deferred::Pause { rewind_to } => {
                self.pause();
                if let Some(p) = rewind_to {
                    self.seek(p);
                }
            }
            Deferred::PauseFade => self.pause(),
            Deferred::Load { source, info, autoplay } => {
                if let Some(t) = self.tuning_pending.take() {
                    self.tuning = t;
                }
                self.load_now(source, info, autoplay);
                if autoplay && self.fade.active() {
                    self.fade_set(0.0);
                    self.fade_to(1.0, self.fade.skip_in(), true);
                } else {
                    self.fade_set(1.0);
                }
            }
            Deferred::Seek { secs } => {
                self.seek(secs);
                let len = self.fade.seek_len();
                self.fade_to(1.0, len, true);
            }
        }
    }

    pub(super) fn set_tuning(&mut self, tuning: Option<SongTuning>) {
        if matches!(self.deferred, Some((_, Deferred::Load { .. }))) {
            self.tuning_pending = Some(tuning);
            return;
        }
        if self.tuning != tuning {
            self.tuning = tuning;
            self.send_dec(DecCmd::Rate(self.rate()));
        }
    }

    pub(super) fn send_rates(&mut self) {
        self.send_dec(DecCmd::Rate(self.rate()));
        if let Some(n) = &self.next {
            let r = self.rate_for(n.tuning);
            self.send_dec(DecCmd::NextRate(r));
        }
    }

    // -----------------------------------------------------------------------
    // preloaded next track
    // -----------------------------------------------------------------------

    pub(super) fn set_transition(&mut self, config: TransitionConfig) {
        if config == self.transition {
            return;
        }
        let kind_changed = config.kind != self.transition.kind;
        self.transition = config;
        if kind_changed {
            self.analysis_requested = false;
        }
        // re-plan the hand-over coming up the new way
        let mut clear = false;
        if let Some(n) = &mut self.next
            && !n.shared.switched.load(Ordering::Acquire)
        {
            if n.order_sent {
                n.order_sent = false;
                clear = true;
            }
            if config.kind == TransitionKind::Crossmix
                && n.allow_mix
                && let Some(id) = n.id.clone()
            {
                let span = head_span(&n.source);
                let (src, dur, cancel) = (n.source.clone(), n.info.duration_secs, self.analysis_cancel.clone());
                self.analyses.request(id, src, span, dur, cancel);
            }
        }
        if clear {
            self.send_dec(DecCmd::ClearOrder);
        }
    }

    pub(super) fn preload_next(&mut self, next: NextTrack) {
        if matches!(self.deferred, Some((_, Deferred::Load { .. }))) {
            return;
        }
        if self.next.as_ref().is_some_and(|n| n.shared.switched.load(Ordering::Acquire)) {
            self.promote();
        }
        self.drop_next();
        let Some(d) = &self.dec else { return };
        if self.track.as_ref().is_none_or(|t| t.finished) || d.shared.eof.load(Ordering::Acquire) {
            return;
        }
        let shared = Arc::new(TrackShared::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let ctl = d.ctl.clone();
        let rate = self.rate_for(next.tuning);
        let cur_factor = self.current_rg_factor().max(1e-4);
        let id = track_id(&next.source, &next.info);
        {
            let (shared, cancel, source, gain) = (shared.clone(), cancel.clone(), next.source.clone(), next.gain);
            let (token, allow_mix) = (next.token, next.allow_mix);
            let spawned = std::thread::Builder::new().name("mochy-audio-preload".into()).spawn(move || {
                match DecodeSession::open(&source, cancel.clone()) {
                    Ok(session) => {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        let info = session.info.clone();
                        let (g, p, pre) = next_rg(&gain, Some(&info));
                        let rg_ratio = rg_factor(g, p, pre) / cur_factor;
                        *shared.info.lock().unwrap_or_else(|e| e.into_inner()) = Some(info);
                        shared.opened.store(true, Ordering::Release);
                        let lane = NextLane { session, shared: shared.clone(), token, rate, rg_ratio, allow_mix };
                        if ctl.send(DecCmd::Next(Box::new(lane))).is_err() {
                            shared.failed.store(true, Ordering::Release);
                        }
                    }
                    Err(_) => shared.failed.store(true, Ordering::Release),
                }
            });
            if spawned.is_err() {
                return;
            }
        }
        if self.transition.kind == TransitionKind::Crossmix
            && next.allow_mix
            && let Some(id) = id.clone()
        {
            let span = head_span(&next.source);
            self.analyses.request(id, next.source.clone(), span, next.info.duration_secs, self.analysis_cancel.clone());
        }
        self.next = Some(NextSlot {
            token: next.token,
            id,
            source: next.source,
            info: next.info,
            gain: next.gain,
            tuning: next.tuning,
            allow_mix: next.allow_mix,
            shared,
            cancel,
            order_sent: false,
            gain_at: None,
        });
    }

    /// Forget the preloaded track (a switch already made is kept).
    pub(super) fn drop_next(&mut self) {
        if self.next.as_ref().is_some_and(|n| n.shared.switched.load(Ordering::Acquire)) {
            self.promote();
            return;
        }
        let Some(n) = self.next.take() else { return };
        n.cancel.store(true, Ordering::Release);
        self.send_dec(DecCmd::ClearNext);
        if n.gain_at.is_some() {
            self.send_gain();
        }
    }

    /// A pending track-change gain, recomputed (volume moved).
    pub(super) fn resend_gain_at(&mut self) {
        let Some(n) = &self.next else { return };
        let Some(at) = n.gain_at else { return };
        let info = n.shared.info.lock().ok().and_then(|i| i.clone());
        let (g, p, pre) = next_rg(&n.gain, info.as_ref());
        let gain = self.volume_part() * rg_factor(g, p, pre);
        self.send_param(ParamMsg::GainAt { at, gain });
    }

    /// The decoder has made the preloaded track current: catch up now.
    pub(super) fn promote_if_switched(&mut self) {
        if self.next.as_ref().is_some_and(|n| n.shared.switched.load(Ordering::Acquire)) {
            self.promote();
        }
    }

    fn promote(&mut self) {
        let Some(slot) = self.next.take() else { return };
        let shared = slot.shared.clone();
        if let Some(d) = &mut self.dec {
            d.shared = shared.clone();
        }
        let playing = self.track.as_ref().is_none_or(|t| t.playing);
        self.track = Some(TrackState {
            info_duration: slot.info.duration_secs,
            playing,
            finished: false,
            pending_seek: None,
            drained_at: None,
            rg_from_file_checked: true,
            last_pos: 0.0,
        });
        let info = shared.info.lock().ok().and_then(|i| i.clone());
        let (g, p, pre) = next_rg(&slot.gain, info.as_ref());
        self.rg_gain = g;
        self.rg_peak = p;
        self.rg_preamp = pre;
        self.rg_explicit = true;
        if slot.gain_at.is_none() {
            self.send_gain();
        }
        self.tuning = slot.tuning;
        self.cur_id = slot.id;
        self.cur_source = Some(slot.source);
        self.analysis_requested = false;
        self.emit(Event::NextStarted { token: slot.token });
        if let Some(i) = info {
            self.emit(Event::TrackInfo { codec: i.codec, sample_rate: i.sample_rate, channels: i.channels, bits_per_sample: i.bits_per_sample });
        }
        let pos = self.position();
        let dur = self.duration();
        if let Some(t) = &mut self.track {
            t.last_pos = pos;
        }
        self.emit(Event::Progress { pos_secs: pos, duration_secs: dur });
        self.last_progress = Instant::now();
    }

    pub(super) fn tick_transitions(&mut self, now: Instant) {
        if self.deferred.as_ref().is_some_and(|(due, _)| now >= *due) {
            self.finish_deferred_now();
        }
        self.request_current_analysis();
        self.tick_next();
    }

    fn request_current_analysis(&mut self) {
        if self.analysis_requested || self.transition.kind != TransitionKind::Crossmix {
            return;
        }
        let (Some(id), Some(src)) = (self.cur_id.clone(), self.cur_source.clone()) else { return };
        let span = match &src {
            TrackSource::Local { .. } => Span::Whole,
            TrackSource::Remote { .. } => {
                if self.duration().is_none() {
                    return;
                }
                Span::Tail(REMOTE_TAIL_SECS)
            }
        };
        self.analysis_requested = true;
        let dur = self.duration();
        self.analyses.request(id, src, span, dur, self.analysis_cancel.clone());
    }

    fn tick_next(&mut self) {
        let Some(n) = &self.next else { return };
        if n.shared.failed.load(Ordering::Acquire) && !n.shared.switched.load(Ordering::Acquire) {
            self.next = None;
            return;
        }
        if n.shared.switched.load(Ordering::Acquire) {
            let at = n.shared.switch_at.load(Ordering::Acquire);
            if n.gain_at.is_none() {
                if let Some(slot) = &mut self.next {
                    slot.gain_at = Some(at);
                }
                self.resend_gain_at();
            }
            let heard = self.out.as_ref().map_or(u64::MAX, |o| {
                o.link.consumed.load(Ordering::Acquire).saturating_sub(o.link.latency_frames.load(Ordering::Relaxed))
            });
            if heard >= at {
                self.promote();
            }
            return;
        }
        if n.order_sent || !n.allow_mix || !n.shared.opened.load(Ordering::Acquire) {
            return;
        }
        let Some(dur) = self.duration() else { return };
        let dur_ms = (dur * 1000.0) as i64;
        let plan: Option<Plan> = match self.transition.kind {
            TransitionKind::Gapless => None,
            TransitionKind::Crossfade => Plan::plain(dur_ms, self.transition.crossfade_secs.clamp(1.0, 30.0)),
            TransitionKind::Crossmix => {
                let pos_ms = (self.position() * 1000.0) as i64;
                let cur_span = match &self.cur_source {
                    Some(TrackSource::Local { .. }) => Span::Whole,
                    _ => Span::Tail(REMOTE_TAIL_SECS),
                };
                let in_span = head_span(&n.source);
                let out_a = self.cur_id.as_deref().and_then(|id| self.analyses.get(id, cur_span));
                let in_a = n.id.as_deref().and_then(|id| self.analyses.get(id, in_span));
                let out_waiting = out_a.is_none() && self.cur_id.as_deref().is_some_and(|id| self.analyses.pending(id, cur_span));
                let in_waiting = in_a.is_none() && n.id.as_deref().is_some_and(|id| self.analyses.pending(id, in_span));
                let deadline = pos_ms >= dur_ms - MAX_LEAD_MS - PLAN_DEADLINE_MS;
                if (out_waiting || in_waiting) && !deadline {
                    return;
                }
                let in_dur = n
                    .info
                    .duration_secs
                    .or_else(|| n.shared.info.lock().ok().and_then(|i| i.as_ref().and_then(|i| i.duration_secs)))
                    .map_or(0, |d| (d * 1000.0) as i64);
                if dur_ms < 20_000 {
                    None
                } else {
                    Some(plan_from(
                        TrackInput { duration_ms: dur_ms, analysis: out_a.as_deref() },
                        TrackInput { duration_ms: in_dur, analysis: in_a.as_deref() },
                        &self.transition.crossmix,
                        pos_ms + 1_500,
                    ))
                }
            }
        };
        let token = n.token;
        if let Some(slot) = &mut self.next {
            slot.order_sent = true;
        }
        if let Some(plan) = plan {
            if cfg!(debug_assertions) {
                eprintln!(
                    "crossmix: plan {} {} beats from {} ms (beat {:.1} ms), swap {}, lands {} ms, in-speed {:.4}, out-speed {:.4}",
                    plan.style.label(),
                    plan.length_beats,
                    plan.out_start_ms,
                    plan.out_beat_ms,
                    plan.swap_beat,
                    plan.in_cue_ms,
                    plan.in_speed,
                    plan.out_speed
                );
            }
            self.send_dec(DecCmd::Order(MixOrder { token, plan }));
        }
    }
}

fn head_span(src: &TrackSource) -> Span {
    match src {
        TrackSource::Local { .. } => Span::Head(LOCAL_HEAD_SECS),
        TrackSource::Remote { .. } => Span::Head(REMOTE_HEAD_SECS),
    }
}
