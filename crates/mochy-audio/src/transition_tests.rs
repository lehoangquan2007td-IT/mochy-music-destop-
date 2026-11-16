//! Tests for transitions: fade curves, the transport fader, skip silence,
//! crossmix analysis / planning / rendering, and gapless + crossfaded
//! hand-overs through the real decode-thread loop (headless).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use crate::crossmix::analysis::{StreamingTrackAnalyzer, SAMPLE_RATE};
use crate::crossmix::fx::LaneFx;
use crate::crossmix::planner::{Plan, TrackInput, plan};
use crate::crossmix::styles::{FxParams, Role, Style, sweep};
use crate::decoder::{DecCmd, DecodeSession, DecoderStart, RateParams, TrackShared, run_decoder};
use crate::dsp::Fader;
use crate::mix::{MixOrder, NextLane};
use crate::output::Link;
use crate::silence::SilenceSkipper;
use crate::transition::*;
use crate::{CrossmixOptions, TrackSource};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mochy-transition-test-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn tone(freq: f64, sr: u32, secs: f64, amp: f32) -> Vec<f32> {
    let n = (sr as f64 * secs) as usize;
    let mut v = Vec::with_capacity(n * 2);
    for i in 0..n {
        let s = (amp as f64 * (2.0 * std::f64::consts::PI * freq * i as f64 / sr as f64).sin()) as f32;
        v.push(s);
        v.push(s);
    }
    v
}

/// Mono kick-like clicks (decaying 60 Hz bursts + noise) at `bpm`.
fn click_track(bpm: f64, secs: f64, sr: u32) -> Vec<f32> {
    let n = (sr as f64 * secs) as usize;
    let period = 60.0 / bpm * sr as f64;
    let mut v = vec![0f32; n];
    let mut seed = 12345u32;
    let mut beat = 0.0;
    while (beat as usize) < n {
        let start = beat as usize;
        for k in 0..(sr as usize / 10).min(n - start) {
            let t = k as f64 / sr as f64;
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = ((seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * 0.3;
            let env = (-t * 30.0).exp() as f32;
            v[start + k] += env * (0.8 * (2.0 * std::f64::consts::PI * 60.0 * t).sin() as f32 + noise);
        }
        beat += period;
    }
    v
}

// ---------------------------------------------------------------------------
// curves & fader
// ---------------------------------------------------------------------------

#[test]
fn equal_power_and_smoothstep_curves() {
    for i in 0..=20 {
        let p = i as f32 / 20.0;
        let (o, n) = equal_power(p);
        assert!((o * o + n * n - 1.0).abs() < 1e-5, "power not constant at {p}");
        let (go, gi) = equal_gain(p);
        assert!((go + gi - 1.0).abs() < 1e-5, "gain not constant at {p}");
    }
    assert_eq!(equal_power(0.0), (1.0, 0.0));
    assert!(equal_power(1.0).0.abs() < 1e-6);
    assert_eq!(smoothstep(0.0), 0.0);
    assert_eq!(smoothstep(1.0), 1.0);
    assert!((smoothstep(0.5) - 0.5).abs() < 1e-6);
    assert_eq!(smoothstep(-1.0), 0.0);
    // the tail fade holds until 80 %, then goes
    assert_eq!(tail_fade(0.5), 1.0);
    assert_eq!(tail_fade(0.8), 1.0);
    assert!(tail_fade(0.9) < 1.0 && tail_fade(0.9) > 0.0);
    assert_eq!(tail_fade(1.0), 0.0);
}

#[test]
fn transport_fade_lengths_follow_android() {
    let f = TransportFade { enabled: true, secs: 2.0 };
    assert_eq!(f.skip_out(), 1.0);
    assert_eq!(f.skip_in(), 1.0);
    let long = TransportFade { enabled: true, secs: 8.0 };
    assert_eq!(long.skip_out(), SKIP_OUT_MAX_SECS);
    assert_eq!(long.skip_in(), 4.0);
    let short = TransportFade { enabled: true, secs: 0.5 };
    assert_eq!(short.skip_in(), SKIP_IN_MIN_SECS);
    assert_eq!(f.resume_len(0.0), 2.0);
    assert!((f.resume_len(0.5) - 1.0).abs() < 1e-6);
    assert_eq!(f.resume_len(0.99), RESUME_MIN_SECS);
    assert!(!TransportFade { enabled: false, secs: 2.0 }.active());
}

#[test]
fn fader_ramps_smoothly_and_waits_for_audio() {
    let mut f = Fader::default();
    assert_eq!(f.level(), 1.0);
    f.ramp_to(0.0, 100, false);
    let mut last = 1.0;
    for _ in 0..100 {
        let g = f.next(true);
        assert!(g <= last + 1e-6, "not monotonic");
        last = g;
    }
    assert_eq!(f.level(), 0.0);
    assert!(f.is_settled());
    // a swell that waits for the new song: silence holds it at 0
    f.ramp_to(1.0, 50, true);
    for _ in 0..1000 {
        assert_eq!(f.next(false), 0.0);
    }
    for _ in 0..50 {
        f.next(true);
    }
    assert!((f.level() - 1.0).abs() < 1e-6);
    // the block helper scales samples
    let mut buf = vec![1.0f32; 20];
    f.set(0.5);
    f.process(&mut buf, 10);
    assert!(buf.iter().all(|&s| (s - 0.5).abs() < 1e-6));
}

// ---------------------------------------------------------------------------
// skip silence
// ---------------------------------------------------------------------------

#[test]
fn skip_silence_cuts_long_gaps_to_padding() {
    let sr = 44_100;
    let mut s = SilenceSkipper::new(sr);
    s.set_enabled(true);
    let mut input = tone(440.0, sr, 1.0, 0.5);
    input.extend(std::iter::repeat_n(0.0, sr as usize * 2 * 5)); // 5 s of silence
    input.extend(tone(440.0, sr, 1.0, 0.5));
    let mut out = Vec::new();
    for chunk in input.chunks(2 * 1024) {
        s.process(chunk, &mut out);
    }
    s.flush(&mut out);
    let frames = out.len() / 2;
    let pad = (sr as f64 * crate::silence::PADDING_SECS).round() as usize;
    // 2 s of tone + 20 ms either side of the cut silence (± a few zero-crossing frames of the tone)
    let expect = 2 * sr as usize + 2 * pad;
    assert!(frames.abs_diff(expect) < 10, "{frames} frames, expected ~{expect}");
    let skipped = s.take_skipped() as usize;
    assert_eq!(frames + skipped, input.len() / 2, "every frame is kept or counted as skipped");
}

#[test]
fn skip_silence_keeps_short_pauses_and_trims_the_end() {
    let sr = 22_050;
    let mut s = SilenceSkipper::new(sr);
    s.set_enabled(true);
    let mut input = tone(300.0, sr, 0.5, 0.5);
    input.extend(std::iter::repeat_n(0.0, sr as usize * 2)); // 1 s pause: kept
    input.extend(tone(300.0, sr, 0.5, 0.5));
    input.extend(std::iter::repeat_n(0.0, sr as usize * 2 * 4)); // 4 s trailing silence
    let mut out = Vec::new();
    s.process(&input, &mut out);
    s.flush(&mut out);
    let frames = out.len() / 2;
    let pad = (sr as f64 * crate::silence::PADDING_SECS).round() as usize;
    let expect = 2 * sr as usize + 2 * pad;
    assert!(frames.abs_diff(expect) < 10, "{frames} vs {expect}");
    // disabled = untouched
    let mut off = SilenceSkipper::new(sr);
    let mut out2 = Vec::new();
    off.process(&input, &mut out2);
    assert_eq!(out2.len(), input.len());
}

// ---------------------------------------------------------------------------
// crossmix analysis / planning / rendering
// ---------------------------------------------------------------------------

fn analyse(bpm: f64, secs: f64) -> crate::crossmix::TrackAnalysis {
    let mut a = StreamingTrackAnalyzer::new();
    let x = click_track(bpm, secs, SAMPLE_RATE);
    for c in x.chunks(4096) {
        a.feed(c);
    }
    a.finish(true).expect("analysis")
}

#[test]
fn analysis_finds_tempo_and_beats_of_a_click_track() {
    let a = analyse(120.0, 40.0);
    let bpm = a.bpm.expect("bpm");
    assert!((bpm - 120.0).abs() < 2.0, "bpm {bpm}");
    assert!(a.is_beat_reliable(), "confidence {}", a.bpm_confidence);
    assert!(a.beats_ms.len() > 60, "{} beats", a.beats_ms.len());
    let gaps: Vec<i64> = a.beats_ms.windows(2).map(|w| w[1] - w[0]).collect();
    let mut sorted = gaps.clone();
    sorted.sort_unstable();
    let median = sorted[sorted.len() / 2];
    assert!((median - 500).abs() <= 15, "median gap {median} ms");
    let grid = a.local_grid(20_000).expect("grid");
    assert!((grid.period_ms - 500.0).abs() < 8.0, "grid {}", grid.period_ms);
    // bars come every 4 beats
    assert!(!a.bar_beats.is_empty());
    for w in a.bar_beats.windows(2) {
        assert_eq!(w[1] - w[0], 4, "bars {:?}", &a.bar_beats[..a.bar_beats.len().min(12)]);
    }
    // too short to say anything
    let mut short = StreamingTrackAnalyzer::new();
    short.feed(&click_track(120.0, 3.0, SAMPLE_RATE));
    assert!(short.finish(true).is_none());
}

#[test]
fn camelot_distances() {
    use crate::crossmix::analysis::harmony::camelot_distance;
    assert_eq!(camelot_distance(Some("C major"), Some("C major")), Some(0));
    assert_eq!(camelot_distance(Some("A minor"), Some("C major")), Some(1));
    assert_eq!(camelot_distance(Some("C major"), Some("G major")), Some(1));
    assert_eq!(camelot_distance(Some("C major"), Some("F# major")), Some(6));
    assert_eq!(camelot_distance(None, Some("C major")), None);
}

#[test]
fn styles_hand_over_cleanly() {
    let mut p = FxParams::default();
    for style in Style::ALL {
        let len = style.lengths()[0];
        let swap = style.swap_at(len) as f32;
        let l = len as f32;
        style.evaluate(Role::Outgoing, 0.0, l, swap, 0, &mut p);
        let out0 = p.gain;
        style.evaluate(Role::Outgoing, l + style.tail_beats() as f32 + 1.0, l, swap, 0, &mut p);
        assert!(p.gain < 0.02, "{style:?} outgoing still at {} after the end", p.gain);
        style.evaluate(Role::Incoming, -1.0, l, swap, 0, &mut p);
        assert_eq!(p.gain, 0.0, "{style:?} incoming heard before it starts");
        style.evaluate(Role::Incoming, l + 2.0, l, swap, 0, &mut p);
        assert!(p.gain > 0.98, "{style:?} incoming only at {} after the end", p.gain);
        assert!(out0 > 0.5, "{style:?} outgoing starts at {out0}");
        assert!(p.gain.is_finite() && p.high_cut_hz.is_finite() && p.low_cut_hz.is_finite());
    }
    // the bass changes hands on the swap
    Style::BassSwap.evaluate(Role::Outgoing, 16.0, 16.0, 8.0, 0, &mut p);
    assert_eq!(p.bass_cut, 1.0);
    Style::BassSwap.evaluate(Role::Incoming, 16.0, 16.0, 8.0, 0, &mut p);
    assert_eq!(p.bass_cut, 0.0);
    assert_eq!(sweep(0.0, 20_000.0, 300.0), 20_000.0);
    assert_eq!(sweep(1.0, 20_000.0, 300.0), 300.0);
    assert!((sweep(0.5, 100.0, 10_000.0) - 1_000.0).abs() < 1.0);
}

#[test]
fn planner_beat_matches_close_tempos_and_falls_back_without_analysis() {
    let out = analyse(124.0, 90.0);
    let inc = analyse(121.0, 60.0);
    let opts = CrossmixOptions::default();
    let p = plan(TrackInput { duration_ms: 90_000, analysis: Some(&out) }, TrackInput { duration_ms: 200_000, analysis: Some(&inc) }, &opts);
    assert!(p.out_start_ms >= 90_000 - 30_000, "starts at {}", p.out_start_ms);
    assert!((p.out_start_ms as f64 + p.length_beats as f64 * p.out_beat_ms) <= 90_000.0, "ends after the song");
    assert!(p.style.blends_both(), "always mix picked {:?}", p.style);
    assert!(p.beat_matched, "close tempos should lock: {p:?}");
    assert!(p.in_speed >= 1.0 && p.in_speed <= 1.0 + opts.max_speed_up, "in speed {}", p.in_speed);
    // the incoming beat laid on the outgoing one
    let ratio = p.in_media_per_beat / p.in_speed as f64 / p.out_beat_ms * p.out_speed as f64;
    assert!((ratio - 1.0).abs() < 0.01, "beats don't line up: {ratio}");
    // deterministic
    let q = plan(TrackInput { duration_ms: 90_000, analysis: Some(&out) }, TrackInput { duration_ms: 200_000, analysis: Some(&inc) }, &opts);
    assert_eq!(p, q);

    // nothing known: always mix → a filter blend over the last ~10 s
    let f = plan(TrackInput { duration_ms: 180_000, analysis: None }, TrackInput { duration_ms: 0, analysis: None }, &opts);
    assert_eq!(f.style, Style::FilterSweep);
    assert_eq!(f.out_start_ms, 170_000);
    let off = CrossmixOptions { always_mix: false, ..opts };
    let d = plan(TrackInput { duration_ms: 180_000, analysis: None }, TrackInput { duration_ms: 0, analysis: None }, &off);
    assert_eq!(d.style, Style::Dampen);
    assert_eq!(d.out_start_ms, 171_000);

    let plain = Plan::plain(200_000, 5.0).unwrap();
    assert_eq!(plain.out_start_ms, 195_000);
    assert!((plain.out_media_at(plain.length_beats as f32) - 200_000.0).abs() < 1.0);
    assert!(Plan::plain(3_000, 5.0).is_none());
}

#[test]
fn lane_fx_is_finite_and_rings_out() {
    let sr = 48_000;
    let mut fx = LaneFx::new(sr);
    let mut p = FxParams { echo_send: 0.8, echo_feedback: 0.7, reverb_send: 0.9, reverb_freeze: 0.5, low_cut_hz: 300.0, high_cut_hz: 2_000.0, bass_cut: 0.5, pan: -0.6, width: 0.3, ..Default::default() };
    let mut x = tone(440.0, sr, 0.5, 0.9);
    for block in x.chunks_mut(256) {
        fx.process(block, &p, 1.0, 500.0);
    }
    assert!(x.iter().all(|s| s.is_finite() && s.abs() < 4.0));
    // the sends stop: the tails keep ringing for a while, then die
    p.echo_send = 0.0;
    p.reverb_send = 0.0;
    p.gain = 0.0;
    let mut silence = vec![0f32; 256];
    fx.process(&mut silence, &p, 1.0, 500.0);
    assert!(fx.tail_level() > 1e-4, "no echo tail");
    p.echo_feedback = 0.3;
    p.reverb_freeze = 0.0;
    for _ in 0..2000 {
        silence.fill(0.0);
        fx.process(&mut silence, &p, 1.0, 500.0);
    }
    assert!(fx.tail_level() < 1e-3, "tail never died: {}", fx.tail_level());
}

// ---------------------------------------------------------------------------
// hand-overs through the decode loop
// ---------------------------------------------------------------------------

struct Harness {
    link: Arc<Link>,
    consumer: rtrb::Consumer<f32>,
    ctl: mpsc::Sender<DecCmd>,
    shared: Arc<TrackShared>,
    cancel: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<(rtrb::Producer<f32>, Arc<Link>)>>,
    out: Vec<f32>,
}

impl Harness {
    fn start(path: &std::path::Path, sr: u32) -> Harness {
        let link = Arc::new(Link::new(sr));
        let (producer, consumer) = rtrb::RingBuffer::<f32>::new(sr as usize * 3);
        let (tx, rx) = mpsc::channel();
        let (etx, _erx) = mpsc::channel();
        let shared = Arc::new(TrackShared::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let start = DecoderStart {
            source: TrackSource::Local { path: path.to_path_buf() },
            start_secs: 0.0,
            rate: RateParams::default(),
            surround: false,
            producer,
            link: link.clone(),
            events: etx,
            shared: shared.clone(),
            cancel: cancel.clone(),
            ctl: rx,
            skip_silence: SkipSilence::default(),
        };
        let join = std::thread::spawn(move || run_decoder(start));
        Harness { link, consumer, ctl: tx, shared, cancel, join: Some(join), out: Vec::new() }
    }

    /// Act as the output callback: ack flushes, drain the ring.
    fn pump(&mut self) {
        let e = self.link.flush_epoch.load(Ordering::Acquire);
        if e != self.link.ack_epoch.load(Ordering::Relaxed) {
            let k = self.consumer.slots();
            if let Ok(c) = self.consumer.read_chunk(k) {
                c.commit_all();
            }
            self.link.consumed.fetch_add((k / 2) as u64, Ordering::AcqRel);
            self.link.ack_epoch.store(e, Ordering::Release);
            self.out.clear();
        }
        let k = self.consumer.slots() & !1;
        if k > 0
            && let Ok(c) = self.consumer.read_chunk(k)
        {
            let (a, b) = c.as_slices();
            self.out.extend_from_slice(a);
            self.out.extend_from_slice(b);
            c.commit_all();
            self.link.consumed.fetch_add((k / 2) as u64, Ordering::AcqRel);
        }
    }

    fn run_until(&mut self, done: impl Fn(&Harness) -> bool, limit: Duration) {
        let t0 = Instant::now();
        while !done(self) && t0.elapsed() < limit {
            self.pump();
            std::thread::sleep(Duration::from_millis(1));
        }
        self.pump();
    }

    fn finish(mut self) -> Vec<f32> {
        self.cancel.store(true, Ordering::Release);
        let _ = self.ctl.send(DecCmd::Stop);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        std::mem::take(&mut self.out)
    }
}

fn next_lane(path: &std::path::Path, token: u64) -> (Box<NextLane>, Arc<TrackShared>) {
    let session = DecodeSession::open(&TrackSource::Local { path: path.to_path_buf() }, Arc::new(AtomicBool::new(false))).unwrap();
    let shared = Arc::new(TrackShared::default());
    *shared.info.lock().unwrap() = Some(session.info.clone());
    shared.opened.store(true, Ordering::Release);
    (Box::new(NextLane { session, shared: shared.clone(), token, rate: RateParams::default(), rg_ratio: 1.0, allow_mix: true }), shared)
}

fn write(path: &std::path::Path, sr: u32, samples: &[f32]) {
    std::fs::write(path, crate::tests::wav_bytes(sr, 2, samples)).unwrap();
}

#[test]
fn gapless_hand_over_plays_both_tracks_back_to_back() {
    let dir = tmp("gapless");
    let sr = 48_000;
    let (a, b) = (dir.join("a.wav"), dir.join("b.wav"));
    write(&a, sr, &tone(440.0, sr, 1.0, 0.5));
    write(&b, sr, &tone(660.0, sr, 1.5, 0.5));
    let mut h = Harness::start(&a, sr);
    let (lane, next_shared) = next_lane(&b, 7);
    h.ctl.send(DecCmd::Next(lane)).unwrap();
    let ns = next_shared.clone();
    h.run_until(|_| ns.eof.load(Ordering::Acquire), Duration::from_secs(10));
    assert!(next_shared.switched.load(Ordering::Acquire), "never switched");
    assert!(!h.shared.eof.load(Ordering::Acquire), "the first track must not report its end");
    let at = next_shared.switch_at.load(Ordering::Acquire) as i64;
    assert!((at - sr as i64).abs() <= 2, "switch at frame {at}");
    let frames = next_shared.eof_index.load(Ordering::Acquire);
    assert!((frames as i64 - (sr as f64 * 2.5) as i64).abs() <= 4, "total {frames} frames");
    let out = h.finish();
    // no gap: every frame around the seam is loud
    let seam = sr as usize;
    let quiet = (seam - 200..seam + 200).filter(|&i| out[2 * i].abs() < 1e-6 && out[2 * i + 1].abs() < 1e-6).count();
    assert!(quiet < 6, "{quiet} silent frames at the seam");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn crossfade_overlaps_the_tracks_with_equal_power() {
    let dir = tmp("crossfade");
    let sr = 48_000;
    let (a, b) = (dir.join("a.wav"), dir.join("b.wav"));
    write(&a, sr, &tone(440.0, sr, 3.0, 0.5));
    write(&b, sr, &tone(440.0, sr, 3.0, 0.5));
    let mut h = Harness::start(&a, sr);
    let (lane, next_shared) = next_lane(&b, 9);
    h.ctl.send(DecCmd::Next(lane)).unwrap();
    let plan = Plan::plain(3_000, 1.0).unwrap();
    h.ctl.send(DecCmd::Order(MixOrder { token: 9, plan })).unwrap();
    let ns = next_shared.clone();
    h.run_until(|_| ns.eof.load(Ordering::Acquire), Duration::from_secs(10));
    assert!(next_shared.switched.load(Ordering::Acquire));
    let at = next_shared.switch_at.load(Ordering::Acquire) as i64;
    assert!((at - 2 * sr as i64).abs() < 300, "crossfade starts at {at}");
    let frames = next_shared.eof_index.load(Ordering::Acquire) as i64;
    // 3 s + 3 s − 1 s overlap
    assert!((frames - 5 * sr as i64).abs() < 600, "total {frames} frames");
    let out = h.finish();
    // two identical in-phase tones on equal-power gains: the level rises to √2 mid-fade, never dips to 0
    let peak_around = |c: usize| (c - 400..c + 400).map(|i| out[2 * i].abs()).fold(0f32, f32::max);
    let mid = (2.5 * sr as f64) as usize;
    let pm = peak_around(mid);
    assert!(pm > 0.6 && pm < 0.75, "mid-fade peak {pm}");
    assert!((peak_around((1.0 * sr as f64) as usize) - 0.5).abs() < 0.02);
    assert!((peak_around((4.0 * sr as f64) as usize) - 0.5).abs() < 0.02);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn seek_before_the_hand_over_calls_it_off_and_keeps_the_next_track() {
    let dir = tmp("seekmix");
    let sr = 48_000;
    let (a, b) = (dir.join("a.wav"), dir.join("b.wav"));
    write(&a, sr, &tone(440.0, sr, 3.0, 0.5));
    write(&b, sr, &tone(660.0, sr, 1.0, 0.5));
    let mut h = Harness::start(&a, sr);
    let (lane, next_shared) = next_lane(&b, 3);
    h.ctl.send(DecCmd::Next(lane)).unwrap();
    // a plan whose start the seek jumps past: the songs then meet gaplessly
    let plan = Plan::plain(3_000, 1.0).unwrap();
    h.ctl.send(DecCmd::Order(MixOrder { token: 3, plan })).unwrap();
    h.ctl.send(DecCmd::Seek { secs: 2.5, generation: 1 }).unwrap();
    let ns = next_shared.clone();
    h.run_until(|_| ns.eof.load(Ordering::Acquire), Duration::from_secs(10));
    assert!(next_shared.switched.load(Ordering::Acquire));
    assert!(h.shared.seek_done_gen.load(Ordering::Acquire) >= 1);
    let _ = h.finish();
    let _ = std::fs::remove_dir_all(&dir);
}
