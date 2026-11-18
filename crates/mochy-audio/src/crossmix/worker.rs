//! Background track analysis (desktop counterpart of
//! `CrossmixAnalysisRepository.kt`): decodes a song (or the part the
//! planner needs) on its own low-priority thread, feeds the analyser and
//! keeps results in memory. Failures are never cached (an expired stream
//! URL may work next time).
//!
//! Remote songs only fetch what the planner looks at — the last 45 s of the
//! playing song, the first 90 s of the next — instead of downloading every
//! song twice; local files are analysed whole.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::analysis::{SAMPLE_RATE, StreamingTrackAnalyzer, TrackAnalysis};
use crate::TrackSource;
use crate::decoder::DecodeSession;
use crate::dsp::SincResampler;

/// Which part of a song to analyse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Span {
    Whole,
    /// The first N seconds (the song coming in).
    Head(f64),
    /// The last N seconds (the song going out); needs the duration.
    Tail(f64),
}

const MAX_TRACK_SECS: f64 = 15.0 * 60.0;
const TIME_LIMIT: Duration = Duration::from_secs(60);
pub const REMOTE_TAIL_SECS: f64 = 45.0;
pub const REMOTE_HEAD_SECS: f64 = 90.0;
pub const LOCAL_HEAD_SECS: f64 = 150.0;

#[derive(Default)]
struct Inner {
    done: HashMap<String, Arc<TrackAnalysis>>,
    inflight: HashSet<String>,
}

/// Shared cache + job launcher.
#[derive(Clone, Default)]
pub struct Analyses {
    inner: Arc<Mutex<Inner>>,
}

fn key_of(id: &str, span: Span) -> String {
    match span {
        Span::Whole => format!("{id}|whole"),
        Span::Head(_) => format!("{id}|head"),
        Span::Tail(_) => format!("{id}|tail"),
    }
}

impl Analyses {
    /// A finished analysis covering `span` (a whole-track result answers
    /// head and tail questions too).
    pub fn get(&self, id: &str, span: Span) -> Option<Arc<TrackAnalysis>> {
        let g = self.inner.lock().ok()?;
        g.done.get(&key_of(id, Span::Whole)).or_else(|| g.done.get(&key_of(id, span))).cloned()
    }

    /// Start analysing unless cached or already running.
    pub fn request(&self, id: String, source: TrackSource, span: Span, duration_secs: Option<f64>, cancel: Arc<AtomicBool>) {
        if self.get(&id, span).is_some() {
            return;
        }
        let key = key_of(&id, span);
        {
            let Ok(mut g) = self.inner.lock() else { return };
            if !g.inflight.insert(key.clone()) {
                return;
            }
        }
        let inner = self.inner.clone();
        let spawned = std::thread::Builder::new().name("mochy-crossmix-analysis".into()).spawn(move || {
            crate::priority::lower_current_thread();
            let started = Instant::now();
            let result = analyze_source(&source, span, duration_secs, &cancel);
            let Ok(mut g) = inner.lock() else { return };
            g.inflight.remove(&key);
            match result {
                Some(a) => {
                    tracing_log(&format!(
                        "crossmix: analysed {key} in {:.1}s: bpm {:?} (conf {:.2}), {} beats, {} bars, key {:?}",
                        started.elapsed().as_secs_f32(),
                        a.bpm,
                        a.bpm_confidence,
                        a.beats_ms.len(),
                        a.bar_beats.len(),
                        a.key
                    ));
                    g.done.insert(key, Arc::new(a));
                    // keep memory bounded
                    if g.done.len() > 64 {
                        let drop: Vec<String> = g.done.keys().take(16).cloned().collect();
                        for k in drop {
                            g.done.remove(&k);
                        }
                    }
                }
                None => tracing_log(&format!("crossmix: analysis of {key} failed")),
            }
        });
        if spawned.is_err()
            && let Ok(mut g) = self.inner.lock()
        {
            g.inflight.remove(&key_of(&id, span));
        }
    }

    /// Still running.
    pub fn pending(&self, id: &str, span: Span) -> bool {
        self.inner.lock().map(|g| g.inflight.contains(&key_of(id, span))).unwrap_or(false)
    }

    /// Insert a result directly (tests).
    pub fn insert(&self, id: &str, span: Span, a: TrackAnalysis) {
        if let Ok(mut g) = self.inner.lock() {
            g.done.insert(key_of(id, span), Arc::new(a));
        }
    }
}

fn tracing_log(msg: &str) {
    // the audio crate has no logging dependency: debug builds print to stderr
    if cfg!(debug_assertions) {
        eprintln!("{msg}");
    }
}

/// Decode `span` of `source` into the analyser.
pub fn analyze_source(source: &TrackSource, span: Span, duration_secs: Option<f64>, cancel: &AtomicBool) -> Option<TrackAnalysis> {
    let cancel_arc = Arc::new(AtomicBool::new(false));
    let mut session = DecodeSession::open(source, cancel_arc.clone()).ok()?;
    let sr = session.sample_rate().max(1);
    let duration = duration_secs.or(session.info.duration_secs);
    let mut offset = 0.0;
    let limit = match span {
        Span::Whole => MAX_TRACK_SECS,
        Span::Head(s) => s,
        Span::Tail(s) => {
            let d = duration?;
            let start = (d - s).max(0.0);
            if start > 0.5 {
                offset = session.seek(start).ok()?;
            }
            s + 5.0
        }
    };
    let mut analyzer = StreamingTrackAnalyzer::new();
    let mut resampler = SincResampler::new(sr, SAMPLE_RATE);
    let mut decoded = Vec::new();
    let mut mono = Vec::new();
    let mut out = Vec::new();
    let mut fed_secs = 0.0;
    let started = Instant::now();
    let mut reached_end = false;
    loop {
        if cancel.load(Ordering::Relaxed) || started.elapsed() > TIME_LIMIT {
            cancel_arc.store(true, Ordering::Relaxed);
            return None;
        }
        decoded.clear();
        match session.decode_next(&mut decoded) {
            Ok(true) => {}
            Ok(false) => {
                reached_end = true;
                break;
            }
            Err(_) => break,
        }
        mono.clear();
        mono.extend(decoded.chunks_exact(2).map(|f| 0.5 * (f[0] + f[1])));
        fed_secs += mono.len() as f64 / sr as f64;
        out.clear();
        resampler.process(&mono, 1, &mut out, false);
        analyzer.feed(&out);
        if fed_secs >= limit {
            break;
        }
    }
    out.clear();
    resampler.process(&[], 1, &mut out, true);
    analyzer.feed(&out);
    let whole_stream = reached_end && duration.is_none_or(|d| offset + fed_secs >= d - 3.0);
    let complete = match span {
        Span::Head(_) => false,
        _ => whole_stream || fed_secs >= MAX_TRACK_SECS,
    };
    let a = analyzer.finish(complete)?;
    Some(a.shifted((offset * 1000.0) as i64))
}
