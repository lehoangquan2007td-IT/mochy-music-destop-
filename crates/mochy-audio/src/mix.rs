//! Decode-thread lanes: one [`Lane`] per open track (session → skip
//! silence → rate pipeline → device-rate buffer), and the transition runner
//! that takes the current lane into a preloaded next one — gapless, or
//! through a crossfade / crossmix [`Plan`] with both lanes rendered and
//! summed before the output ring.
//!
//! Position bookkeeping: every lane pushes `Marker`s (ring index → media
//! time) into its own `TrackShared`; when the next lane becomes the current
//! one its `switch_at` ring index is published, and the engine promotes it
//! once the device has played up to there (`Event::NextStarted`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::crossmix::fx::LaneFx;
use crate::crossmix::styles::{FxParams, Role, Style, headroom, ramp};
use crate::crossmix::Plan;
use crate::decoder::{
    Ctl, DecCmd, DecodeSession, DecoderStart, Feeder, Marker, RateParams, RatePipeline, TrackShared, adopt_rebind, flush_ring,
    push_marker, wait_cmd,
};
use crate::silence::{INSTANT_MAX_HOPS, INSTANT_STEP_SECS, SilenceSkipper};
use crate::transition::SkipSilence;
use crate::{Event, SourceInfo};

/// Device-rate audio kept queued ahead of the device.
pub(crate) const TARGET_FILL_SECS: f64 = 0.25;
/// …and while a hand-over is coming up or running: room for the incoming
/// song's network reads and seek without an underrun.
const MIX_FILL_SECS: f64 = 1.0;
/// The hand-over is "imminent" this long before it starts.
const IMMINENT_SECS: f64 = 10.0;
/// Frames rendered per mix block (parameters refresh per block).
const MIX_BLOCK: usize = 128;
/// Speed ramps: steps per second of media (markers on change).
const RAMP_EPS: f64 = 0.0008;

/// A preloaded track, opened by the engine's opener thread.
pub(crate) struct NextLane {
    pub session: DecodeSession,
    pub shared: Arc<TrackShared>,
    pub token: u64,
    pub rate: RateParams,
    /// Linear ReplayGain of the next track ÷ that of the current one.
    pub rg_ratio: f32,
    pub allow_mix: bool,
}

/// How to hand over to next lane `token`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MixOrder {
    pub token: u64,
    pub plan: Plan,
}

pub(crate) struct Lane {
    pub session: DecodeSession,
    pub shared: Arc<TrackShared>,
    pub info: SourceInfo,
    pipe: RatePipeline,
    base: RateParams,
    tempo_mul: f64,
    marked_mul: f64,
    /// Ramp `tempo_mul` back to 1 at this rate per media second.
    relax_per_sec: f64,
    out_rate: u32,
    buf: Vec<f32>,
    off: usize,
    pub eof: bool,
    /// Media seconds of the next frame to be emitted.
    pub emit_media: f64,
    marker_dirty: bool,
    marker_clear: bool,
    skipper: SilenceSkipper,
    instant: bool,
    hops: u32,
    skip_pending: f64,
    decoded: Vec<f32>,
    filtered: Vec<f32>,
}

impl Lane {
    pub fn new(session: DecodeSession, shared: Arc<TrackShared>, out_rate: u32, rate: RateParams, start_media: f64) -> Lane {
        let info = session.info.clone();
        let pipe = RatePipeline::new(info.sample_rate, out_rate, rate);
        Lane {
            skipper: SilenceSkipper::new(info.sample_rate),
            session,
            shared,
            info,
            pipe,
            base: rate,
            tempo_mul: 1.0,
            marked_mul: 1.0,
            relax_per_sec: 0.0,
            out_rate,
            buf: Vec::new(),
            off: 0,
            eof: false,
            emit_media: start_media,
            marker_dirty: true,
            marker_clear: true,
            instant: false,
            hops: 0,
            skip_pending: 0.0,
            decoded: Vec::new(),
            filtered: Vec::new(),
        }
    }

    pub fn effective(&self) -> RateParams {
        RateParams { tempo: self.base.tempo * self.tempo_mul, pitch: self.base.pitch }
    }

    pub fn duration(&self) -> Option<f64> {
        self.info.duration_secs
    }

    fn apply_rate(&mut self) {
        let eff = self.effective();
        self.pipe.set_params(eff);
        if (self.tempo_mul - self.marked_mul).abs() > RAMP_EPS || self.tempo_mul == 1.0 && self.marked_mul != 1.0 {
            self.marker_dirty = true;
        }
    }

    pub fn set_base(&mut self, p: RateParams) {
        if p != self.base {
            self.base = p;
            self.pipe.set_params(self.effective());
            self.marker_dirty = true;
        }
    }

    pub fn set_tempo_mul(&mut self, m: f64) {
        let m = m.clamp(0.5, 2.0);
        if (m - self.tempo_mul).abs() > 1e-6 {
            self.tempo_mul = m;
            self.apply_rate();
        }
    }

    /// After a mix: back to the listener's speed over `secs` of media.
    pub fn relax_over(&mut self, secs: f64) {
        self.relax_per_sec = if secs > 0.0 { (self.tempo_mul - 1.0).abs() / secs } else { f64::INFINITY };
    }

    pub fn set_skip_silence(&mut self, s: SkipSilence) {
        self.skipper.set_enabled(s.enabled);
        self.instant = s.enabled && s.instant;
    }

    /// Pause silence skipping (a mix needs the planned media timeline).
    pub fn suspend_skip(&mut self, s: SkipSilence, suspended: bool) {
        let mut s = s;
        if suspended {
            s.enabled = false;
        }
        self.set_skip_silence(s);
    }

    pub fn available(&self) -> usize {
        (self.buf.len() - self.off) / 2
    }

    /// Decode until `frames` are buffered or the source ends.
    pub fn fill(&mut self, frames: usize, cancel: &AtomicBool) -> Result<(), String> {
        while self.available() < frames && !self.eof {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            if self.off > 0 && self.off * 2 > self.buf.len() {
                self.buf.drain(..self.off);
                self.off = 0;
            }
            self.decoded.clear();
            match self.session.decode_next(&mut self.decoded) {
                Ok(true) => {
                    self.filtered.clear();
                    self.skipper.process(&self.decoded, &mut self.filtered);
                    let skipped = self.skipper.take_skipped();
                    if skipped > 0 {
                        self.skip_pending += skipped as f64 / self.info.sample_rate.max(1) as f64;
                    }
                    self.pipe.process(&self.filtered, &mut self.buf);
                    if self.skipper.in_long_silence() && self.instant {
                        self.instant_hop();
                    } else if !self.skipper.in_long_silence() {
                        self.hops = 0;
                    }
                }
                Ok(false) => {
                    self.filtered.clear();
                    self.skipper.flush(&mut self.filtered);
                    self.pipe.process(&self.filtered, &mut self.buf);
                    self.pipe.flush(&mut self.buf);
                    self.eof = true;
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }

    /// Instant skip: jump 15 s ahead while it stays silent.
    fn instant_hop(&mut self) {
        let Some(dur) = self.duration() else { return };
        if dur <= INSTANT_STEP_SECS || self.hops >= INSTANT_MAX_HOPS {
            return;
        }
        let here = self.session.position_secs();
        let target = (here + INSTANT_STEP_SECS).min(dur - 0.5);
        if target <= here {
            return;
        }
        if let Ok(actual) = self.session.seek(target) {
            self.hops += 1;
            self.skip_pending += (actual - here).max(0.0);
        }
    }

    /// Emit `n` frames (≤ available) into `out`; `index` is the ring index
    /// of the first one.
    pub fn take(&mut self, n: usize, out: &mut Vec<f32>, index: u64) {
        let n = n.min(self.available());
        if self.skip_pending > 0.0 {
            self.emit_media += std::mem::take(&mut self.skip_pending);
            self.marker_dirty = true;
        }
        self.mark(index);
        out.extend_from_slice(&self.buf[self.off..self.off + n * 2]);
        self.off += n * 2;
        self.advance(n);
    }

    /// Emit `n` frames into a scratch block (mixing); returns how many.
    pub fn take_into(&mut self, n: usize, block: &mut [f32], index: u64) -> usize {
        let n = n.min(self.available()).min(block.len() / 2);
        self.mark(index);
        block[..n * 2].copy_from_slice(&self.buf[self.off..self.off + n * 2]);
        self.off += n * 2;
        self.advance(n);
        n
    }

    fn advance(&mut self, n: usize) {
        let dt = n as f64 / self.out_rate as f64 * self.effective().tempo;
        self.emit_media += dt;
        if self.relax_per_sec > 0.0 && self.tempo_mul != 1.0 {
            let step = self.relax_per_sec * dt;
            let m = if (self.tempo_mul - 1.0).abs() <= step { 1.0 } else { self.tempo_mul - step * (self.tempo_mul - 1.0).signum() };
            if m == 1.0 {
                self.relax_per_sec = 0.0;
            }
            self.set_tempo_mul(m);
        }
    }

    /// Push a marker at ring `index` if the media/speed mapping changed.
    pub fn mark(&mut self, index: u64) {
        if !self.marker_dirty {
            return;
        }
        self.marker_dirty = false;
        let clear = std::mem::take(&mut self.marker_clear);
        self.marked_mul = self.tempo_mul;
        push_marker(&self.shared, Marker { index, media: self.emit_media, speed: self.effective().tempo }, clear);
    }

    pub fn seek(&mut self, secs: f64) -> Result<f64, String> {
        let actual = self.session.seek(secs).map_err(|e| e.to_string())?;
        self.reset_buffers();
        self.emit_media = actual;
        Ok(actual)
    }

    fn reset_buffers(&mut self) {
        self.pipe.reset();
        self.buf.clear();
        self.off = 0;
        self.eof = false;
        self.skipper.reset();
        self.skip_pending = 0.0;
        self.hops = 0;
        self.marker_dirty = true;
        self.marker_clear = true;
    }

    /// New output device rate.
    pub fn rebind(&mut self, out_rate: u32) {
        self.out_rate = out_rate;
        self.pipe = RatePipeline::new(self.info.sample_rate, out_rate, self.effective());
        self.reset_buffers();
    }
}

/// A running hand-over.
struct Mix {
    out: Lane,
    inc: Lane,
    plan: Plan,
    fx_out: LaneFx,
    fx_in: LaneFx,
    in_started: bool,
    switched: bool,
    rg_ratio: f32,
    token: u64,
    po: FxParams,
    pi: FxParams,
    block_out: Vec<f32>,
    block_in: Vec<f32>,
}

enum Stage {
    Solo(Lane),
    Mixing(Box<Mix>),
    /// Only while `take_current` holds the lane.
    Empty,
}

/// The preloaded lane, waiting.
struct Pending {
    lane: Lane,
    token: u64,
    rg_ratio: f32,
    allow_mix: bool,
    /// Seeked to the plan's entry point (done once, when imminent).
    cued: bool,
}

/// Decode thread body (replaces the single-track loop): returns the ring
/// producer (and its link) so the engine can reuse it.
pub(crate) fn run_lanes(s: DecoderStart) -> (rtrb::Producer<f32>, Arc<crate::output::Link>) {
    let DecoderStart { source, start_secs, rate, surround, producer, link, events, shared, cancel, ctl, skip_silence } = s;
    let mut ctl = Ctl::new(ctl);
    let mut feeder = Feeder::new(producer, link);

    let mut session = match DecodeSession::open(&source, cancel.clone()) {
        Ok(s) => s,
        Err(e) => {
            if !cancel.load(Ordering::Relaxed) {
                let _ = events.send(Event::Error(format!("could not open track: {e}")));
            }
            shared.failed.store(true, Ordering::Release);
            return adopt_rebind(feeder, &mut ctl);
        }
    };
    session.set_surround(surround);
    let info = session.info.clone();
    let _ = events.send(Event::TrackInfo {
        codec: info.codec.clone(),
        sample_rate: info.sample_rate,
        channels: info.channels,
        bits_per_sample: info.bits_per_sample,
    });
    *shared.info.lock().unwrap_or_else(|e| e.into_inner()) = Some(info.clone());

    let mut out_rate = feeder.link.sample_rate;
    let mut media_start = 0.0;
    if start_secs > 0.0 {
        media_start = session.seek(start_secs).unwrap_or(0.0);
    }
    flush_ring(&feeder.link, &cancel);
    let mut lane = Lane::new(session, shared.clone(), out_rate, rate, media_start);
    lane.set_skip_silence(skip_silence);
    lane.mark(feeder.index());
    shared.opened.store(true, Ordering::Release);

    let mut silence = skip_silence;
    let mut surround = surround;
    let mut stage = Stage::Solo(lane);
    let mut next: Option<Pending> = None;
    let mut order: Option<MixOrder> = None;
    // a seek past the planned start (or a plan that came too late) gives up the mix for this track
    let mut mix_spent = false;
    let mut at_eof = false;

    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        // --- commands ---
        let mut seek: Option<(f64, u64)> = None;
        let mut stop = false;
        let mut rebind = None;
        while let Some(cmd) = ctl.try_recv() {
            match cmd {
                DecCmd::Seek { secs, generation } => seek = Some((secs, generation)),
                DecCmd::Rate(p) => match &mut stage {
                    Stage::Empty => {}
                    Stage::Solo(l) => l.set_base(p),
                    Stage::Mixing(m) => {
                        if m.switched {
                            m.inc.set_base(p)
                        } else {
                            m.out.set_base(p)
                        }
                    }
                },
                DecCmd::NextRate(p) => {
                    if let Some(n) = &mut next {
                        n.lane.set_base(p);
                    } else if let Stage::Mixing(m) = &mut stage
                        && !m.switched
                    {
                        m.inc.set_base(p);
                    }
                }
                DecCmd::Surround(on) => {
                    surround = on;
                    match &mut stage {
                        Stage::Empty => {}
                        Stage::Solo(l) => l.session.set_surround(on),
                        Stage::Mixing(m) => {
                            m.out.session.set_surround(on);
                            m.inc.session.set_surround(on);
                        }
                    }
                    if let Some(n) = &mut next {
                        n.lane.session.set_surround(on);
                    }
                }
                DecCmd::SkipSilence(sk) => {
                    silence = sk;
                    if let Stage::Solo(l) = &mut stage {
                        l.set_skip_silence(sk);
                    }
                }
                DecCmd::Next(n) => {
                    if at_eof {
                        // too late for a seamless hand-over: the engine finishes this track normally
                        n.shared.failed.store(true, Ordering::Release);
                        continue;
                    }
                    let NextLane { mut session, shared, token, rate, rg_ratio, allow_mix } = *n;
                    session.set_surround(surround);
                    let lane = Lane::new(session, shared, out_rate, rate, 0.0);
                    next = Some(Pending { lane, token, rg_ratio, allow_mix, cued: false });
                }
                DecCmd::ClearNext => {
                    next = None;
                    order = None;
                }
                DecCmd::Order(o) => {
                    order = Some(o);
                }
                DecCmd::ClearOrder => order = None,
                DecCmd::Rebind { producer, link, pos, generation } => rebind = Some((producer, link, pos, generation)),
                DecCmd::Stop => stop = true,
            }
        }
        if stop {
            if let Some((p, l, _, _)) = rebind {
                feeder.producer = p;
                feeder.link = l;
            }
            break;
        }
        if let Some((producer, link, pos, generation)) = rebind {
            feeder.producer = producer;
            feeder.link = link;
            feeder.clear_pending();
            out_rate = feeder.link.sample_rate;
            let mut l = take_current(&mut stage, &mut next);
            l.rebind(out_rate);
            let _ = l.seek(pos);
            l.set_skip_silence(silence);
            if let Some(n) = &mut next {
                n.lane.rebind(out_rate);
                let _ = n.lane.seek(0.0);
                n.cued = false;
            }
            at_eof = false;
            l.shared.eof.store(false, Ordering::Release);
            l.mark(feeder.index());
            l.shared.seek_done_gen.fetch_max(generation, Ordering::AcqRel);
            stage = Stage::Solo(l);
        }
        if let Some((secs, generation)) = seek {
            // a seek ends any hand-over: before the switch it stays on the outgoing song, after on the new one
            let mut l = take_current(&mut stage, &mut next);
            match l.seek(secs) {
                Ok(_) => {
                    at_eof = false;
                    l.shared.eof.store(false, Ordering::Release);
                    l.set_skip_silence(silence);
                    l.set_tempo_mul(1.0);
                    flush_ring(&feeder.link, &cancel);
                    feeder.clear_pending();
                    l.mark(feeder.index());
                    // re-arm only if the planned start is still ahead
                    mix_spent = order.is_some_and(|o| secs * 1000.0 >= o.plan.out_media_at(o.plan.in_start_beat.min(0.0)) - 500.0);
                }
                Err(_) => {}
            }
            l.shared.seek_done_gen.fetch_max(generation, Ordering::AcqRel);
            stage = Stage::Solo(l);
        }

        // --- feed ---
        feeder.pump();
        if feeder.pending_frames() > 0 {
            wait_cmd(&mut ctl, &mut feeder, Duration::from_millis(4), fill_target(&stage, &next, &order, mix_spent));
            continue;
        }
        if at_eof {
            wait_cmd(&mut ctl, &mut feeder, Duration::from_millis(20), TARGET_FILL_SECS);
            continue;
        }
        let target_secs = fill_target(&stage, &next, &order, mix_spent);
        let target = (out_rate as f64 * target_secs) as u64;
        if feeder.queued_frames() >= target {
            // idle time: cue the incoming song where the plan enters it
            if let (Stage::Solo(_), Some(n), Some(o)) = (&stage, &mut next, &order)
                && !n.cued
                && o.token == n.token
                && target_secs > TARGET_FILL_SECS
            {
                n.cued = true;
                let at = o.plan.in_start_ms() as f64 / 1000.0;
                if at > 0.05 {
                    let _ = n.lane.seek(at);
                }
            }
            wait_cmd(&mut ctl, &mut feeder, Duration::from_millis(5), target_secs);
            continue;
        }

        stage = match stage {
            Stage::Empty => break,
            Stage::Solo(mut l) => {
                // a hand-over armed and due?
                let armed = match (&next, &order) {
                    (Some(n), Some(o)) if o.token == n.token && n.allow_mix && !mix_spent => Some(o.plan),
                    _ => None,
                };
                let mut chunk = 4096usize;
                if let Some(plan) = armed {
                    let begin = plan.out_media_at(plan.in_start_beat.min(0.0)) / 1000.0;
                    // the outgoing song is brought up to a faster next song's tempo just before
                    if plan.out_speed > 1.0005 {
                        let steps = ((plan.out_speed - 1.0) / 0.005).ceil() as f64;
                        let ramp_secs = steps * plan.out_beat_ms / 1000.0;
                        let k = ramp((l.emit_media) as f32, (begin - ramp_secs) as f32, begin as f32) as f64;
                        l.set_tempo_mul(1.0 + (plan.out_speed as f64 - 1.0) * k);
                    }
                    if l.emit_media >= begin - 1e-4 {
                        if l.emit_media > begin + plan.out_beat_ms / 1000.0 {
                            // arrived too late: no mix this time
                            mix_spent = true;
                        } else if let Some(n) = next.take() {
                            stage = Stage::Mixing(Box::new(start_mix(l, n, plan, out_rate, silence)));
                            continue;
                        }
                    } else {
                        let eff = l.effective().tempo;
                        let frames = ((begin - l.emit_media) / eff * out_rate as f64).ceil().max(1.0) as usize;
                        chunk = chunk.min(frames);
                    }
                }
                if let Err(e) = l.fill(chunk.min(2048), &cancel) {
                    if !cancel.load(Ordering::Relaxed) {
                        let _ = events.send(Event::Error(format!("playback failed: {e}")));
                        l.shared.failed.store(true, Ordering::Release);
                    }
                    break;
                }
                let n = l.available().min(chunk);
                if n > 0 {
                    let idx = feeder.index();
                    l.take(n, &mut feeder.pending, idx);
                    Stage::Solo(l)
                } else if l.eof {
                    // end of this song: run straight into the preloaded one, or publish the end
                    let idx = feeder.index();
                    match next.take() {
                        Some(mut n) => {
                            if n.cued {
                                let _ = n.lane.seek(0.0);
                            }
                            let mut lane = n.lane;
                            lane.set_skip_silence(silence);
                            lane.mark(idx);
                            publish_switch(&lane.shared, idx);
                            order = None;
                            mix_spent = false;
                            Stage::Solo(lane)
                        }
                        None => {
                            at_eof = true;
                            l.shared.eof_index.store(idx, Ordering::Release);
                            l.shared.eof.store(true, Ordering::Release);
                            Stage::Solo(l)
                        }
                    }
                } else {
                    Stage::Solo(l)
                }
            }
            Stage::Mixing(mut m) => {
                let idx = feeder.index();
                match mix_block(&mut m, &mut feeder.pending, idx, &cancel) {
                    Ok(true) => Stage::Mixing(m),
                    Ok(false) => {
                        // over: the incoming song plays on alone
                        let mut inc = m.inc;
                        if !m.switched {
                            publish_switch(&inc.shared, idx);
                        }
                        let beats_secs = 16.0 * m.plan.in_media_per_beat / 1000.0;
                        inc.relax_over(beats_secs);
                        inc.set_skip_silence(silence);
                        order = None;
                        mix_spent = false;
                        Stage::Solo(inc)
                    }
                    Err(e) => {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        // the incoming song failed before taking over: the outgoing one carries on
                        if !m.switched {
                            m.inc.shared.failed.store(true, Ordering::Release);
                            mix_spent = true;
                            let mut out = m.out;
                            out.set_tempo_mul(1.0);
                            Stage::Solo(out)
                        } else {
                            let _ = events.send(Event::Error(format!("playback failed: {e}")));
                            m.inc.shared.failed.store(true, Ordering::Release);
                            break;
                        }
                    }
                }
            }
        };
    }

    adopt_rebind(feeder, &mut ctl)
}

fn publish_switch(shared: &TrackShared, idx: u64) {
    shared.switch_at.store(idx, Ordering::Release);
    shared.switched.store(true, Ordering::Release);
}

/// How far ahead to decode: more while a hand-over is near or running.
fn fill_target(stage: &Stage, next: &Option<Pending>, order: &Option<MixOrder>, spent: bool) -> f64 {
    match stage {
        Stage::Empty => TARGET_FILL_SECS,
        Stage::Mixing(_) => MIX_FILL_SECS,
        Stage::Solo(l) => match (next, order) {
            (Some(n), Some(o)) if o.token == n.token && n.allow_mix && !spent => {
                let begin = o.plan.out_media_at(o.plan.in_start_beat.min(0.0)) / 1000.0;
                if l.emit_media >= begin - IMMINENT_SECS { MIX_FILL_SECS } else { TARGET_FILL_SECS }
            }
            _ => TARGET_FILL_SECS,
        },
    }
}

/// The lane that stays current when a hand-over is called off.
fn take_current(stage: &mut Stage, next: &mut Option<Pending>) -> Lane {
    match std::mem::replace(stage, Stage::Empty) {
        Stage::Empty => unreachable!("stage is always restored"),
        Stage::Solo(l) => l,
        Stage::Mixing(m) => {
            let m = *m;
            if m.switched {
                let mut inc = m.inc;
                inc.set_tempo_mul(1.0);
                inc
            } else {
                // back to waiting: the incoming lane goes back to preloaded, from the top
                let mut inc = m.inc;
                inc.set_tempo_mul(1.0);
                let _ = inc.seek(0.0);
                *next = Some(Pending { lane: inc, token: m.token, rg_ratio: m.rg_ratio, allow_mix: true, cued: false });
                let mut out = m.out;
                out.set_tempo_mul(1.0);
                out
            }
        }
    }
}

fn start_mix(out: Lane, n: Pending, plan: Plan, out_rate: u32, silence: SkipSilence) -> Mix {
    let mut out = out;
    let mut inc = n.lane;
    out.suspend_skip(silence, true);
    inc.suspend_skip(silence, true);
    if !n.cued {
        let at = plan.in_start_ms() as f64 / 1000.0;
        if at > 0.05 {
            let _ = inc.seek(at);
        }
    }
    inc.set_tempo_mul(plan.in_speed as f64);
    let mut fx_out = LaneFx::new(out_rate);
    let mut fx_in = LaneFx::new(out_rate);
    fx_out.set_gain(1.0);
    fx_in.set_gain(0.0);
    Mix {
        out,
        inc,
        plan,
        fx_out,
        fx_in,
        in_started: false,
        switched: false,
        rg_ratio: n.rg_ratio,
        token: n.token,
        po: FxParams::default(),
        pi: FxParams::default(),
        block_out: vec![0.0; MIX_BLOCK * 2],
        block_in: vec![0.0; MIX_BLOCK * 2],
    }
}

/// The beat (switch point) the app's "current track" moves to the incoming
/// song: the start for a plain crossfade, the swap for a mix.
fn switch_beat(plan: &Plan) -> f32 {
    if plan.style == Style::PlainFade { 0.0 } else { plan.swap_beat as f32 }
}

/// Render one block of the hand-over into `out`. `Ok(false)` = finished.
fn mix_block(m: &mut Mix, out: &mut Vec<f32>, idx: u64, cancel: &AtomicBool) -> Result<bool, String> {
    let plan = m.plan;
    let beat_ms = plan.out_beat_ms.max(1.0);
    let beat_at = |media_s: f64| ((media_s * 1000.0 - plan.out_start_ms as f64) / beat_ms) as f32;
    let beat = beat_at(m.out.emit_media);
    let length = plan.length_beats as f32;
    let swap = plan.swap_beat as f32;

    if beat >= plan.end_beat() || (m.out.eof && m.out.available() == 0 && beat >= length && m.fx_out.tail_level() < 1e-4) {
        return Ok(false);
    }
    if !m.switched && beat >= switch_beat(&plan) {
        m.switched = true;
        publish_switch(&m.inc.shared, idx);
    }

    // outgoing block
    m.out.fill(MIX_BLOCK, cancel).map_err(|e| format!("outgoing: {e}"))?;
    m.block_out.fill(0.0);
    let got_out = m.out.take_into(MIX_BLOCK, &mut m.block_out, idx);
    let frames = MIX_BLOCK;
    let beat_end = beat + (frames as f64 / m.out.out_rate as f64 * 1000.0 * m.out.effective().tempo / beat_ms) as f32;
    let _ = got_out;

    // incoming block, from the frame its start beat falls on
    m.block_in.fill(0.0);
    let mut in_from = frames;
    if m.in_started {
        in_from = 0;
    } else if beat_end >= plan.in_start_beat {
        let span = (beat_end - beat).max(1e-6);
        let k = (((plan.in_start_beat - beat) / span).clamp(0.0, 1.0) * frames as f32) as usize;
        in_from = k.min(frames - 1);
        m.in_started = true;
    }
    if in_from < frames {
        m.inc.fill(frames - in_from, cancel)?;
        let mut tmp = [0f32; MIX_BLOCK * 2];
        let n = m.inc.take_into(frames - in_from, &mut tmp[..(frames - in_from) * 2], idx + in_from as u64);
        m.block_in[in_from * 2..(in_from + n) * 2].copy_from_slice(&tmp[..n * 2]);
    }

    // parameters at the middle of the block
    let mid = 0.5 * (beat + beat_end);
    plan.style.evaluate(Role::Outgoing, mid, length, swap, plan.variation, &mut m.po);
    plan.style.evaluate(Role::Incoming, mid, length, swap, plan.variation, &mut m.pi);
    if plan.yield_amount > 0.0 {
        plan.style.yield_vocals(Role::Outgoing, mid, length, swap, plan.yield_amount, &mut m.po);
        plan.style.yield_vocals(Role::Incoming, mid, length, swap, plan.yield_amount, &mut m.pi);
    }
    let room = if plan.style == Style::PlainFade { 1.0 } else { headroom(m.po.gain, m.pi.gain) };
    // levels relative to whichever song the output gain is set for
    let (rel_out, rel_in) = if m.switched { (1.0 / m.rg_ratio.max(1e-3), 1.0) } else { (1.0, m.rg_ratio) };
    m.fx_out.process(&mut m.block_out, &m.po, room * rel_out, beat_ms);
    m.fx_in.process(&mut m.block_in, &m.pi, room * rel_in, plan.in_media_per_beat.max(1.0));
    for i in 0..frames * 2 {
        out.push(m.block_out[i] + m.block_in[i]);
    }
    Ok(true)
}
