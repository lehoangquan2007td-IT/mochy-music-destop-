//! Headless decode of a file or URL (no audio device): prints format info,
//! decoded length, levels, decode speed, and checks a mid-file seek.
//!
//! `cargo run -p mochy-audio --example decode_check -- <path-or-url> [more…]`

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use mochy_audio::{DecodeSession, TrackSource};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: decode_check <path-or-url>...");
        std::process::exit(2);
    }
    for target in args {
        let source = if target.starts_with("http://") || target.starts_with("https://") {
            TrackSource::Remote { url: target.clone(), headers: vec![] }
        } else {
            TrackSource::Local { path: target.clone().into() }
        };
        println!("== {target}");
        let t0 = Instant::now();
        let mut s = match DecodeSession::open(&source, Arc::new(AtomicBool::new(false))) {
            Ok(s) => s,
            Err(e) => {
                println!("   open failed: {e}");
                continue;
            }
        };
        println!("   opened in {:.0} ms: {:?}", t0.elapsed().as_secs_f64() * 1000.0, s.info);
        let t1 = Instant::now();
        let mut out = Vec::new();
        let mut total = 0usize;
        let (mut sum2, mut peak) = (0f64, 0f32);
        loop {
            out.clear();
            match s.decode_next(&mut out) {
                Ok(true) => {
                    total += out.len() / 2;
                    for &x in &out {
                        sum2 += (x as f64) * (x as f64);
                        peak = peak.max(x.abs());
                    }
                }
                Ok(false) => break,
                Err(e) => {
                    println!("   decode error after {total} frames: {e}");
                    break;
                }
            }
        }
        let secs = total as f64 / s.info.sample_rate as f64;
        let el = t1.elapsed().as_secs_f64();
        let rms_db = 10.0 * (sum2 / (total.max(1) * 2) as f64).max(1e-20).log10();
        println!(
            "   decoded {total} frames = {secs:.3} s in {el:.2} s ({:.0}x realtime), RMS {rms_db:.1} dBFS, peak {peak:.3}",
            secs / el.max(1e-9)
        );
        let target_t = secs / 2.0;
        match s.seek(target_t) {
            Ok(at) => {
                let mut rest = 0usize;
                out.clear();
                while let Ok(true) = s.decode_next(&mut out) {
                    rest += out.len() / 2;
                    out.clear();
                }
                let rest_s = rest as f64 / s.info.sample_rate as f64;
                println!(
                    "   seek {target_t:.3} s -> landed {at:.3} s, {rest_s:.3} s remain (expected {:.3})",
                    secs - at
                );
            }
            Err(e) => println!("   seek failed: {e}"),
        }
    }
}
