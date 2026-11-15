//! Engine thread: owns the output stream and the current decode thread,
//! turns `Command`s into decoder/callback messages, and emits `Event`s
//! (progress every 66 ms, finished once the device has played the last
//! frame), feeds the spectrum, and recovers from device loss.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mochy_core::eq::EqSettings;
use mochy_core::settings::{ReverbSettings, SpatialSettings};

use crate::decoder::{DecCmd, DecoderStart, RateParams, TrackShared, run_decoder};
use crate::dsp::chain::ParamMsg;
use crate::dsp::{ChainParams, EqDesign, LoudnessParams, ReverbParams, SpatialParams, replay_gain_factor, volume_curve_gain};
use crate::output::{Link, Output, open_output};
use crate::spectrum::SpectrumAnalyzer;
use crate::{Command, Event, TrackInfo, TrackSource};

const TICK: Duration = Duration::from_millis(8);
const PROGRESS_EVERY: Duration = Duration::from_millis(66);
const SPECTRUM_EVERY: Duration = Duration::from_millis(33);
const DEVICE_RETRY_EVERY: Duration = Duration::from_secs(3);
const WATCHDOG: Duration = Duration::from_millis(2500);

struct DecHandle {
    ctl: Sender<DecCmd>,
    cancel: Arc<AtomicBool>,
    shared: Arc<TrackShared>,
    join: JoinHandle<(rtrb::Producer<f32>, Arc<Link>)>,
}

struct TrackState {
    info_duration: Option<f64>,
    playing: bool,
    finished: bool,
    pending_seek: Option<(f64, u64)>,
    drained_at: Option<Instant>,
    rg_from_file_checked: bool,
    last_pos: f64,
}

struct Engine {
    events: Sender<Event>,
    spectrum: Arc<Mutex<Vec<f32>>>,
    spectrum_live: bool,
    out: Option<Output>,
    idle: Option<(rtrb::Producer<f32>, Arc<Link>)>,
    device_pref: Option<String>,
    output_error_reported: bool,
    analyzer: Option<SpectrumAnalyzer>,
    tap_buf: Vec<f32>,
    dec: Option<DecHandle>,
    track: Option<TrackState>,
    seek_gen: u64,
    // Audio Lab state
    eq: EqSettings,
    reverb: ReverbSettings,
    spatial: SpatialSettings,
    loudness: LoudnessParams,
    volume: f32,
    curve: (bool, f32, f32),
    rg_gain: Option<f32>,
    rg_peak: Option<f32>,
    rg_preamp: f32,
    rg_explicit: bool,
    speed: f32,
    semitones: f32,
    preserve_pitch: bool,
    // timers
    last_progress: Instant,
    last_spectrum: Instant,
    last_device_try: Instant,
    watchdog: (u64, Instant),
}

pub(crate) fn run(rx: Receiver<Command>, events: Sender<Event>, spectrum: Arc<Mutex<Vec<f32>>>) {
    let now = Instant::now();
    let mut e = Engine {
        events,
        spectrum,
        spectrum_live: false,
        out: None,
        idle: None,
        device_pref: None,
        output_error_reported: false,
        analyzer: None,
        tap_buf: vec![0.0; 16_384],
        dec: None,
        track: None,
        seek_gen: 0,
        eq: EqSettings::default(),
        reverb: ReverbSettings::default(),
        spatial: SpatialSettings::default(),
        loudness: LoudnessParams::default(),
        volume: 1.0,
        curve: (false, 2.0, 1.0),
        rg_gain: None,
        rg_peak: None,
        rg_preamp: 3.0,
        rg_explicit: false,
        speed: 1.0,
        semitones: 0.0,
        preserve_pitch: false,
        last_progress: now,
        last_spectrum: now,
        last_device_try: now,
        watchdog: (0, now),
    };
    e.rebuild_output();
    loop {
        match rx.recv_timeout(TICK) {
            Ok(cmd) => {
                if !e.handle(cmd) {
                    break;
                }
                // apply bursts of commands (slider drags) before ticking
                while let Ok(cmd) = rx.try_recv() {
                    if !e.handle(cmd) {
                        e.shutdown();
                        return;
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        e.tick();
    }
    e.shutdown();
}

impl Engine {
    fn emit(&self, ev: Event) {
        let _ = self.events.send(ev);
    }

    fn sample_rate(&self) -> u32 {
        self.out.as_ref().map_or(48_000, |o| o.link.sample_rate)
    }

    fn output_gain(&self) -> f32 {
        let (enabled, exp, limit) = self.curve;
        let v = self.volume.clamp(0.0, 1.0);
        let vol = if enabled { volume_curve_gain(v, exp, limit) } else { v };
        let rg = self.rg_gain.map_or(1.0, |g| replay_gain_factor(g, self.rg_peak, self.rg_preamp));
        vol * rg
    }

    fn chain_params(&self, sample_rate: u32) -> ChainParams {
        ChainParams {
            eq: EqDesign::from_settings(&self.eq, sample_rate as f32),
            reverb: ReverbParams::from_settings(&self.reverb),
            spatial: SpatialParams::from_settings(&self.spatial),
            loudness: self.loudness,
            gain: self.output_gain(),
        }
    }

    fn rate(&self) -> RateParams {
        RateParams::new(self.speed, self.semitones, self.preserve_pitch)
    }

    fn send_param(&mut self, msg: ParamMsg) {
        if let Some(o) = &mut self.out {
            o.send(msg);
        }
    }

    fn send_gain(&mut self) {
        let g = self.output_gain();
        self.send_param(ParamMsg::Gain(g));
    }

    fn send_dec(&self, cmd: DecCmd) {
        if let Some(d) = &self.dec {
            let _ = d.ctl.send(cmd);
        }
    }

    // -----------------------------------------------------------------------
    // output management
    // -----------------------------------------------------------------------

    /// (Re)open the preferred device (falling back to the default), keeping
    /// the playback position.
    fn rebuild_output(&mut self) {
        self.last_device_try = Instant::now();
        let pos = self.position();
        self.out = None;
        self.idle = None;
        let mut opened = None;
        let mut err = String::new();
        let mut tries: Vec<Option<String>> = vec![self.device_pref.clone()];
        if self.device_pref.is_some() {
            tries.push(None);
        }
        for name in tries {
            let params = |sr: u32| self.chain_params(sr);
            match open_output(name.as_deref(), &params) {
                Ok(v) => {
                    opened = Some(v);
                    break;
                }
                Err(e) => err = e,
            }
        }
        let Some((out, producer)) = opened else {
            if !self.output_error_reported {
                self.output_error_reported = true;
                self.emit(Event::Error(format!("audio output unavailable: {err}")));
            }
            return;
        };
        self.output_error_reported = false;
        let playing = self.track.as_ref().is_some_and(|t| t.playing);
        out.link.playing.store(playing, Ordering::Release);
        let link = out.link.clone();
        if self.analyzer.as_ref().is_none_or(|a| a.sample_rate() != link.sample_rate) {
            self.analyzer = Some(SpectrumAnalyzer::new(link.sample_rate));
        }
        self.watchdog = (0, Instant::now());
        self.out = Some(out);
        let mut producer = Some(producer);
        if let Some(d) = &self.dec {
            self.seek_gen += 1;
            let generation = self.seek_gen;
            let cmd = DecCmd::Rebind { producer: producer.take().expect("producer"), link: link.clone(), pos, generation };
            if let Err(std::sync::mpsc::SendError(DecCmd::Rebind { producer: p, .. })) = d.ctl.send(cmd) {
                producer = Some(p);
            }
            if let Some(t) = &mut self.track {
                t.pending_seek = Some((pos, generation));
                t.drained_at = None;
                t.last_pos = pos;
            }
        }
        if let Some(p) = producer {
            self.idle = Some((p, link));
        }
    }

    /// A producer for the current output's ring (rebuilding if lost).
    fn take_producer(&mut self) -> Option<(rtrb::Producer<f32>, Arc<Link>)> {
        let current = self.out.as_ref().map(|o| o.link.clone())?;
        match self.idle.take() {
            Some((p, l)) if Arc::ptr_eq(&l, &current) => Some((p, l)),
            _ => {
                self.rebuild_output();
                self.idle.take()
            }
        }
    }

    // -----------------------------------------------------------------------
    // decoder management
    // -----------------------------------------------------------------------

    fn stop_decoder(&mut self) {
        let Some(d) = self.dec.take() else { return };
        d.cancel.store(true, Ordering::Release);
        let _ = d.ctl.send(DecCmd::Stop);
        if let Ok((p, l)) = d.join.join()
            && self.out.as_ref().is_some_and(|o| Arc::ptr_eq(&o.link, &l))
        {
            self.idle = Some((p, l));
        }
    }

    fn flush_now(&self) {
        if let Some(o) = &self.out {
            o.link.flush_epoch.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn set_playing(&mut self, playing: bool) {
        if let Some(t) = &mut self.track {
            t.playing = playing;
        }
        if let Some(o) = &self.out {
            o.link.playing.store(playing, Ordering::Release);
        }
    }

    fn load(&mut self, source: TrackSource, info: TrackInfo, autoplay: bool) {
        self.stop_decoder();
        self.flush_now();
        self.track = None;
        self.emit(Event::Loading);
        self.rg_gain = info.replay_gain_db;
        self.rg_peak = info.replay_peak;
        self.rg_explicit = info.replay_gain_db.is_some();
        self.send_gain();
        if let Some(a) = &mut self.analyzer {
            a.clear();
        }
        if self.out.is_none() {
            self.rebuild_output();
        }
        let Some((producer, link)) = self.take_producer() else {
            self.emit(Event::Error("no audio output device available".into()));
            return;
        };
        link.playing.store(autoplay, Ordering::Release);
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(TrackShared::default());
        let start = DecoderStart {
            source,
            start_secs: 0.0,
            rate: self.rate(),
            surround: self.spatial.surround,
            producer,
            link,
            events: self.events.clone(),
            shared: shared.clone(),
            cancel: cancel.clone(),
            ctl: rx,
        };
        let join = match std::thread::Builder::new().name("mochy-audio-decode".into()).spawn(move || run_decoder(start)) {
            Ok(j) => j,
            Err(e) => {
                self.emit(Event::Error(format!("could not start decoder: {e}")));
                return;
            }
        };
        self.dec = Some(DecHandle { ctl: tx, cancel, shared, join });
        self.track = Some(TrackState {
            info_duration: info.duration_secs,
            playing: autoplay,
            finished: false,
            pending_seek: None,
            drained_at: None,
            rg_from_file_checked: false,
            last_pos: 0.0,
        });
        self.emit(if autoplay { Event::Playing } else { Event::Paused });
        self.emit(Event::Progress { pos_secs: 0.0, duration_secs: info.duration_secs });
        self.last_progress = Instant::now();
    }

    fn duration(&self) -> Option<f64> {
        let t = self.track.as_ref()?;
        let decoded = self
            .dec
            .as_ref()
            .and_then(|d| d.shared.info.lock().ok().and_then(|i| i.as_ref().and_then(|i| i.duration_secs)));
        decoded.or(t.info_duration)
    }

    /// Position heard right now: frames the device consumed (minus its
    /// latency), mapped through the speed markers.
    fn position(&self) -> f64 {
        let (Some(t), Some(d), Some(o)) = (&self.track, &self.dec, &self.out) else {
            return self.track.as_ref().map_or(0.0, |t| t.last_pos);
        };
        if let Some((secs, generation)) = t.pending_seek
            && d.shared.seek_done_gen.load(Ordering::Acquire) < generation
        {
            return secs;
        }
        let link = &o.link;
        let consumed = link.consumed.load(Ordering::Acquire);
        let heard = consumed.saturating_sub(link.latency_frames.load(Ordering::Relaxed));
        let mut q = d.shared.markers.lock().unwrap_or_else(|e| e.into_inner());
        if q.is_empty() {
            return t.last_pos;
        }
        let idx = q.iter().rposition(|m| m.index <= heard).unwrap_or(0);
        if idx > 0 {
            q.drain(..idx);
        }
        let m = q[0];
        let pos = m.media + heard.saturating_sub(m.index) as f64 / link.sample_rate as f64 * m.speed;
        let pos = pos.max(0.0);
        match self.duration() {
            Some(dur) => pos.min(dur),
            None => pos,
        }
    }

    // -----------------------------------------------------------------------
    // commands
    // -----------------------------------------------------------------------

    fn handle(&mut self, cmd: Command) -> bool {
        match cmd {
            Command::Load { source, info, autoplay } => self.load(source, info, autoplay),
            Command::Play => self.play(),
            Command::Pause => self.pause(),
            Command::Toggle => {
                if self.track.as_ref().is_some_and(|t| t.playing) {
                    self.pause();
                } else {
                    self.play();
                }
            }
            Command::Stop => {
                if let Some(t) = &mut self.track {
                    t.last_pos = 0.0;
                }
                self.stop_decoder();
                self.flush_now();
                self.set_playing(false);
                self.track = None;
                self.clear_spectrum();
                self.emit(Event::Stopped);
            }
            Command::SeekTo { secs } => self.seek(secs),
            Command::SetVolume { linear } => {
                self.volume = if linear.is_finite() { linear.clamp(0.0, 1.0) } else { 1.0 };
                self.send_gain();
            }
            Command::SetSpeed { factor } => {
                self.speed = if factor.is_finite() { factor.clamp(0.1, 8.0) } else { 1.0 };
                self.send_dec(DecCmd::Rate(self.rate()));
            }
            Command::SetPitch { semitones, preserve_pitch } => {
                self.semitones = if semitones.is_finite() { semitones.clamp(-24.0, 24.0) } else { 0.0 };
                self.preserve_pitch = preserve_pitch;
                self.send_dec(DecCmd::Rate(self.rate()));
            }
            Command::SetEq { eq } => {
                self.eq = eq;
                let d = EqDesign::from_settings(&self.eq, self.sample_rate() as f32);
                self.send_param(ParamMsg::Eq(d));
            }
            Command::SetLoudness { enabled, boost_db, compression } => {
                self.loudness = LoudnessParams {
                    enabled,
                    boost_db: boost_db.clamp(0.0, 12.0),
                    compression: compression.clamp(0.0, 1.0),
                };
                self.send_param(ParamMsg::Loudness(self.loudness));
            }
            Command::SetVolumeCurve { enabled, exponent, limit } => {
                self.curve = (enabled, exponent, limit);
                self.send_gain();
            }
            Command::SetReplayGain { gain_db, peak, preamp_db } => {
                self.rg_gain = gain_db;
                self.rg_peak = peak;
                self.rg_preamp = if preamp_db.is_finite() { preamp_db } else { 0.0 };
                self.rg_explicit = true;
                self.send_gain();
            }
            Command::SetReverb { reverb } => {
                self.reverb = reverb;
                self.send_param(ParamMsg::Reverb(ReverbParams::from_settings(&self.reverb)));
            }
            Command::SetSpatial { spatial } => {
                let surround_changed = spatial.surround != self.spatial.surround;
                self.spatial = spatial;
                self.send_param(ParamMsg::Spatial(SpatialParams::from_settings(&self.spatial)));
                if surround_changed {
                    self.send_dec(DecCmd::Surround(self.spatial.surround));
                }
            }
            Command::SetOutputDevice { name } => {
                self.device_pref = name.filter(|n| !n.is_empty());
                self.rebuild_output();
            }
            Command::Shutdown => return false,
        }
        true
    }

    fn play(&mut self) {
        let Some(t) = &mut self.track else { return };
        if t.finished {
            t.finished = false;
            t.drained_at = None;
            self.seek(0.0);
        }
        self.set_playing(true);
        self.emit(Event::Playing);
    }

    fn pause(&mut self) {
        if self.track.is_none() {
            return;
        }
        let pos = self.position();
        if let Some(t) = &mut self.track {
            t.last_pos = pos;
        }
        self.set_playing(false);
        self.emit(Event::Paused);
        self.clear_spectrum();
    }

    fn seek(&mut self, secs: f64) {
        if self.track.is_none() || !secs.is_finite() {
            return;
        }
        let dur = self.duration();
        let secs = match dur {
            Some(d) => secs.clamp(0.0, (d - 0.05).max(0.0)),
            None => secs.max(0.0),
        };
        self.seek_gen += 1;
        let generation = self.seek_gen;
        self.send_dec(DecCmd::Seek { secs, generation });
        if let Some(t) = &mut self.track {
            t.pending_seek = Some((secs, generation));
            t.finished = false;
            t.drained_at = None;
            t.last_pos = secs;
        }
        self.emit(Event::Progress { pos_secs: secs, duration_secs: dur });
        self.last_progress = Instant::now();
    }

    fn clear_spectrum(&mut self) {
        if self.spectrum_live {
            self.spectrum_live = false;
            if let Ok(mut s) = self.spectrum.lock() {
                s.clear();
            }
        }
    }

    // -----------------------------------------------------------------------
    // periodic work
    // -----------------------------------------------------------------------

    fn tick(&mut self) {
        let now = Instant::now();
        // device health
        if let Some(o) = &self.out {
            let cb = o.link.callbacks.load(Ordering::Relaxed);
            if cb != self.watchdog.0 {
                self.watchdog = (cb, now);
            }
            let broken = o.link.error.load(Ordering::Acquire) != 0;
            if broken || now.duration_since(self.watchdog.1) > WATCHDOG {
                self.rebuild_output();
            }
        } else if now.duration_since(self.last_device_try) > DEVICE_RETRY_EVERY {
            self.rebuild_output();
        }

        // decoder state
        let mut failed = false;
        let mut file_rg = false;
        if let (Some(d), Some(t)) = (&self.dec, &mut self.track) {
            if d.shared.failed.load(Ordering::Acquire) {
                failed = true;
            } else {
                if d.shared.seek_done_gen.load(Ordering::Acquire) >= t.pending_seek.map_or(0, |p| p.1) {
                    t.pending_seek = None;
                }
                if !t.rg_from_file_checked && d.shared.opened.load(Ordering::Acquire) {
                    t.rg_from_file_checked = true;
                    if !self.rg_explicit
                        && let Some(info) = d.shared.info.lock().ok().and_then(|i| i.clone())
                        && info.replay_gain.track_gain.is_some()
                    {
                        // file tags only when the app gave none for this track
                        self.rg_gain = info.replay_gain.track_gain;
                        self.rg_peak = info.replay_gain.track_peak;
                        file_rg = true;
                    }
                }
            }
        }
        if failed {
            self.stop_decoder();
            self.set_playing(false);
            self.track = None;
            self.clear_spectrum();
            return;
        }
        if file_rg {
            self.send_gain();
        }

        // end of track: decoder hit EOF and the device has drained the ring
        let mut finished = false;
        if let (Some(d), Some(t), Some(o)) = (&self.dec, &mut self.track, &self.out)
            && !t.finished
            && t.pending_seek.is_none()
            && d.shared.eof.load(Ordering::Acquire)
        {
            let end = d.shared.eof_index.load(Ordering::Acquire);
            if o.link.consumed.load(Ordering::Acquire) >= end {
                let lat = o.link.latency_frames.load(Ordering::Relaxed) as f64 / o.link.sample_rate as f64;
                let wait = Duration::from_secs_f64(lat + 0.03);
                match t.drained_at {
                    None => t.drained_at = Some(now),
                    Some(at) if now.duration_since(at) >= wait => finished = true,
                    _ => {}
                }
            }
        }
        if finished {
            let dur = self.duration();
            let pos = dur.unwrap_or_else(|| self.position());
            if let Some(t) = &mut self.track {
                t.finished = true;
                t.last_pos = pos;
            }
            self.set_playing(false);
            self.emit(Event::Progress { pos_secs: pos, duration_secs: dur });
            self.emit(Event::Finished);
            self.clear_spectrum();
        }

        // progress
        if now.duration_since(self.last_progress) >= PROGRESS_EVERY {
            // fixed cadence (not "66 ms after the last tick"), resync after a stall
            self.last_progress += PROGRESS_EVERY;
            if now.duration_since(self.last_progress) > PROGRESS_EVERY {
                self.last_progress = now;
            }
            if self.track.as_ref().is_some_and(|t| !t.finished) {
                let pos = self.position();
                let dur = self.duration();
                if let Some(t) = &mut self.track {
                    t.last_pos = pos;
                }
                self.emit(Event::Progress { pos_secs: pos, duration_secs: dur });
            }
        }

        // spectrum
        if now.duration_since(self.last_spectrum) >= SPECTRUM_EVERY {
            self.last_spectrum = now;
            self.update_spectrum();
        }
    }

    fn update_spectrum(&mut self) {
        let playing = self.track.as_ref().is_some_and(|t| t.playing && !t.finished);
        let (Some(o), Some(a)) = (&mut self.out, &mut self.analyzer) else { return };
        loop {
            let n = o.tap.slots().min(self.tap_buf.len());
            if n == 0 {
                break;
            }
            let (got, _) = o.tap.pop_partial_slice(&mut self.tap_buf[..n]);
            let k = got.len();
            a.push(&self.tap_buf[..k]);
        }
        if !playing {
            self.clear_spectrum();
            return;
        }
        let bands = a.compute().to_vec();
        if let Ok(mut s) = self.spectrum.lock() {
            *s = bands;
        }
        self.spectrum_live = true;
    }

    fn shutdown(&mut self) {
        self.stop_decoder();
        self.set_playing(false);
        self.track = None;
        self.out = None;
        self.idle = None;
        self.clear_spectrum();
    }
}
