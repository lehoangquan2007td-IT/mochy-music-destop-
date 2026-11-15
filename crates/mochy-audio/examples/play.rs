//! Play a file or URL through the real engine and print events.
//!
//! ```text
//! cargo run -p mochy-audio --example play -- <path-or-url> [seconds] [--seek-at S --seek-to T]
//!     [--speed X] [--pitch SEMIS] [--eq PRESET] [--reverb PRESET] [--crossfeed] [--loudness]
//!     [--device NAME] [--devices]
//! ```
//!
//! Without `--seek-at`, it seeks to 2/3 of the duration halfway through the
//! run, to exercise seeking. Progress is printed about once a second.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use mochy_audio::{Command, Event, TrackInfo, TrackSource, output_devices, spawn_engine};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--devices") {
        for d in output_devices() {
            println!("device: {d}");
        }
        return;
    }
    let Some(target) = args.first().cloned() else {
        eprintln!("usage: play <path-or-url> [seconds] [--seek-at S --seek-to T] [--speed X] [--pitch SEMIS] [--eq PRESET] [--reverb PRESET] [--crossfeed] [--loudness] [--device NAME]");
        std::process::exit(2);
    };
    let secs: f64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10.0);
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let seek_at: Option<f64> = opt("--seek-at").and_then(|s| s.parse().ok());
    let seek_to: Option<f64> = opt("--seek-to").and_then(|s| s.parse().ok());
    // --pause-at S: pause for 1.5 s, then resume. --switch-at S --switch-to NAME: change device.
    let pause_at: Option<f64> = opt("--pause-at").and_then(|s| s.parse().ok());
    let switch_at: Option<f64> = opt("--switch-at").and_then(|s| s.parse().ok());
    let switch_to = opt("--switch-to");
    let mut paused_at: Option<f64> = None;
    let mut resumed = false;
    let mut switched = false;

    let source = if target.starts_with("http://") || target.starts_with("https://") {
        TrackSource::Remote { url: target.clone(), headers: vec![] }
    } else {
        TrackSource::Local { path: target.clone().into() }
    };

    let (tx, rx) = mpsc::channel();
    let engine = spawn_engine(tx).expect("spawn engine");
    if let Some(d) = opt("--device") {
        engine.send(Command::SetOutputDevice { name: Some(d) });
    }
    if let Some(s) = opt("--speed").and_then(|s| s.parse().ok()) {
        engine.send(Command::SetSpeed { factor: s });
    }
    if let Some(p) = opt("--pitch").and_then(|s| s.parse().ok()) {
        engine.send(Command::SetPitch { semitones: p, preserve_pitch: true });
    }
    if let Some(id) = opt("--eq")
        && let Some(p) = mochy_core::eq::preset_by_id(&id) {
            engine.send(Command::SetEq { eq: mochy_core::eq::settings_from_preset(p) });
        }
    if let Some(id) = opt("--reverb")
        && let Some(r) = mochy_core::settings::ReverbSettings::from_preset(&id, true) {
            engine.send(Command::SetReverb { reverb: r });
        }
    if flag("--crossfeed") {
        engine.send(Command::SetSpatial {
            spatial: mochy_core::settings::SpatialSettings { crossfeed_enabled: true, ..Default::default() },
        });
    }
    if flag("--loudness") {
        engine.send(Command::SetLoudness { enabled: true, boost_db: 4.0, compression: 0.3 });
    }
    engine.send(Command::Load { source, info: TrackInfo::default(), autoplay: true });

    let start = Instant::now();
    let mut seeked = false;
    let mut duration = None;
    let mut last_print = Instant::now() - Duration::from_secs(10);
    let mut last_pos = 0.0;
    let mut progress_events = 0u64;
    let mut max_band = f32::MIN;
    loop {
        let elapsed = start.elapsed().as_secs_f64();
        if elapsed > secs {
            println!("[{elapsed:6.2}s] time limit reached at pos {last_pos:.2}s");
            break;
        }
        let at = seek_at.unwrap_or(secs / 2.0);
        if !seeked && elapsed >= at {
            seeked = true;
            let to = seek_to.or(duration.map(|d: f64| d * 2.0 / 3.0)).unwrap_or(last_pos + 5.0);
            println!("[{elapsed:6.2}s] >>> SeekTo {to:.2}s (pos before {last_pos:.2}s)");
            engine.send(Command::SeekTo { secs: to });
        }
        if let (Some(p), None) = (pause_at, paused_at)
            && elapsed >= p {
                paused_at = Some(elapsed);
                println!("[{elapsed:6.2}s] >>> Pause (pos {last_pos:.2}s)");
                engine.send(Command::Pause);
            }
        if let (Some(p), false) = (paused_at, resumed)
            && elapsed >= p + 1.5 {
                resumed = true;
                println!("[{elapsed:6.2}s] >>> Play (pos {last_pos:.2}s)");
                engine.send(Command::Play);
            }
        if let (Some(t), false) = (switch_at, switched)
            && elapsed >= t {
                switched = true;
                println!("[{elapsed:6.2}s] >>> SetOutputDevice {switch_to:?} (pos {last_pos:.2}s)");
                engine.send(Command::SetOutputDevice { name: switch_to.clone() });
            }
        let spec = engine.spectrum();
        if let Some(m) = spec.iter().copied().reduce(f32::max) {
            max_band = max_band.max(m);
        }
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(Event::Progress { pos_secs, duration_secs }) => {
                progress_events += 1;
                last_pos = pos_secs;
                duration = duration_secs.or(duration);
                if last_print.elapsed() >= Duration::from_secs(1) {
                    last_print = Instant::now();
                    let spec = engine.spectrum();
                    let loudest = spec
                        .iter()
                        .enumerate()
                        .max_by(|a, b| a.1.total_cmp(b.1))
                        .map(|(i, v)| format!("band {i} {v:.1} dBFS"))
                        .unwrap_or_else(|| "spectrum empty".into());
                    println!(
                        "[{elapsed:6.2}s] pos {pos_secs:7.3}s / {} | {loudest}",
                        duration_secs.map_or("?".into(), |d| format!("{d:.2}s"))
                    );
                }
            }
            Ok(Event::Finished) => {
                println!("[{elapsed:6.2}s] Finished (pos {last_pos:.2}s)");
                break;
            }
            Ok(Event::Error(e)) => println!("[{elapsed:6.2}s] ERROR: {e}"),
            Ok(ev) => println!("[{elapsed:6.2}s] {ev:?}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    println!(
        "summary: {progress_events} progress events in {:.2}s ({:.1}/s), loudest spectrum band seen {max_band:.1} dBFS",
        start.elapsed().as_secs_f64(),
        progress_events as f64 / start.elapsed().as_secs_f64()
    );
    engine.send(Command::Shutdown);
    std::thread::sleep(Duration::from_millis(200));
}
