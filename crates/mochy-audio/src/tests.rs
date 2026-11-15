//! Unit tests for the DSP blocks and headless decode → DSP pipeline tests
//! (no audio device needed).

use std::f64::consts::PI;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mochy_core::eq::{BiquadCoeffs, FilterType};

use crate::decoder::{DecodeSession, RateParams, RatePipeline};
use crate::dsp::*;
use crate::spectrum::SpectrumAnalyzer;
use crate::{SPECTRUM_BANDS, TrackSource};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn sine(freq: f64, sr: f64, secs: f64, amp: f64) -> Vec<f32> {
    let n = (sr * secs) as usize;
    (0..n).map(|i| (amp * (2.0 * PI * freq * i as f64 / sr).sin()) as f32).collect()
}

fn stereo(mono: &[f32]) -> Vec<f32> {
    mono.iter().flat_map(|&s| [s, s]).collect()
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

fn left(x: &[f32]) -> Vec<f32> {
    x.as_chunks::<2>().0.iter().map(|f| f[0]).collect()
}

/// Frequency from positive-going zero crossings (linear interpolation).
fn zc_freq(x: &[f32], sr: f64) -> f64 {
    let mut first = None;
    let mut last = 0.0;
    let mut count = 0usize;
    for i in 1..x.len() {
        if x[i - 1] < 0.0 && x[i] >= 0.0 {
            let t = (i - 1) as f64 + (-x[i - 1] as f64) / ((x[i] - x[i - 1]) as f64);
            if first.is_none() {
                first = Some(t);
            } else {
                count += 1;
            }
            last = t;
        }
    }
    count as f64 / ((last - first.unwrap_or(0.0)) / sr)
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mochy-audio-test-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 16-bit PCM WAV bytes.
pub(crate) fn wav_bytes(sr: u32, channels: u16, samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut v = Vec::with_capacity(44 + data_len as usize);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&sr.to_le_bytes());
    v.extend_from_slice(&(sr * channels as u32 * 2).to_le_bytes());
    v.extend_from_slice(&(channels * 2).to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        v.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    v
}

fn write_wav(path: &Path, sr: u32, channels: u16, samples: &[f32]) {
    std::fs::write(path, wav_bytes(sr, channels, samples)).unwrap();
}

fn decode_all(s: &mut DecodeSession) -> Vec<f32> {
    let mut out = Vec::new();
    while s.decode_next(&mut out).unwrap() {}
    out
}

// ---------------------------------------------------------------------------
// biquad / EQ
// ---------------------------------------------------------------------------

fn measured_gain_db(c: BiquadCoeffs, freq: f64, sr: f64) -> f64 {
    let mut bq = Biquad::from_coeffs(c);
    let x = sine(freq, sr, 1.0, 0.5);
    let y: Vec<f32> = x.iter().map(|&s| bq.process(s as f64) as f32).collect();
    let skip = (sr * 0.5) as usize; // settle
    20.0 * (rms(&y[skip..]) / rms(&x[skip..])).log10()
}

#[test]
fn biquad_sine_matches_magnitude_db() {
    let sr = 48_000.0;
    let cases = [
        (FilterType::Peaking, 1000.0, 6.0, 1.41),
        (FilterType::Peaking, 125.0, -9.0, 2.0),
        (FilterType::LowShelf, 200.0, 8.0, 0.71),
        (FilterType::HighShelf, 6000.0, -6.0, 0.71),
        (FilterType::LowPass, 3000.0, 0.0, 0.71),
        (FilterType::HighPass, 80.0, 0.0, 0.71),
    ];
    for (ft, f0, g, q) in cases {
        let c = BiquadCoeffs::design(ft, f0, g, q, sr as f32);
        for probe in [60.0, 250.0, 1000.0, 2500.0, 8000.0] {
            let want = c.magnitude_db(probe as f32, sr as f32) as f64;
            if want < -40.0 {
                continue; // deep stopband: measurement noise dominates
            }
            let got = measured_gain_db(c, probe, sr);
            assert!((got - want).abs() < 0.2, "{ft:?} {f0} Hz @ {probe} Hz: measured {got:.3} dB, curve {want:.3} dB");
        }
    }
}

#[test]
fn eq_cascade_matches_ui_curve() {
    let sr = 48_000u32;
    let eq = mochy_core::eq::settings_from_preset(mochy_core::eq::preset_by_id("rock").unwrap());
    let design = EqDesign::from_settings(&eq, sr as f32).expect("rock preset is audible");
    for probe in [40.0, 300.0, 1000.0, 5000.0, 12000.0] {
        let mut eqp = EqProcessor::new(sr, Some(design));
        let x = stereo(&sine(probe, sr as f64, 1.0, 0.1));
        let mut y = x.clone();
        eqp.process(&mut y);
        let skip = sr as usize; // half the frames (stereo)
        let got = 20.0 * (rms(&y[skip..]) / rms(&x[skip..])).log10();
        let want = mochy_core::eq::total_response_db(&eq, probe as f32, sr as f32) as f64;
        assert!((got - want).abs() < 0.2, "@{probe} Hz: engine {got:.3} dB, UI {want:.3} dB");
    }
    // disabled / empty → bypass
    let mut off = eq.clone();
    off.enabled = false;
    assert!(EqDesign::from_settings(&off, 48_000.0).is_none());
    let empty = mochy_core::eq::EqSettings { enabled: true, ..Default::default() };
    assert!(EqDesign::from_settings(&empty, 48_000.0).is_none());
}

#[test]
fn eq_design_change_crossfades_without_clicks() {
    let sr = 48_000u32;
    let a = mochy_core::eq::settings_from_gains([12.0, 12.0, 12.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let b = mochy_core::eq::settings_from_gains([-12.0, -12.0, -12.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let mut eqp = EqProcessor::new(sr, EqDesign::from_settings(&a, sr as f32));
    let x = stereo(&sine(60.0, sr as f64, 1.0, 0.1));
    let mut y = x.clone();
    let half = y.len() / 2;
    let (first, second) = y.split_at_mut(half);
    eqp.process(first);
    eqp.set_design(EqDesign::from_settings(&b, sr as f32));
    eqp.process(second);
    let l = left(&y);
    // a 60 Hz tone moves at most 2π·60/48000·A per sample; a click would be far bigger
    let max_step = l.windows(2).skip(1000).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
    let amp = peak(&l[1000..]);
    let bound = (2.0 * std::f32::consts::PI * 60.0 / sr as f32) * amp * 1.5;
    assert!(max_step < bound, "step {max_step} > {bound}");
}

// ---------------------------------------------------------------------------
// limiter
// ---------------------------------------------------------------------------

#[test]
fn limiter_never_exceeds_ceiling_on_plus_12_db_sine() {
    for (sr, freq) in [(48_000u32, 1000.0), (44_100, 55.0), (48_000, 7000.0)] {
        let amp = 10f64.powf(12.0 / 20.0);
        let mut x = stereo(&sine(freq, sr as f64, 1.0, amp));
        let mut lim = PeakLimiter::new(sr);
        for block in x.chunks_mut(512) {
            lim.process(block, 2);
        }
        let p = peak(&x);
        assert!(p <= 1.0 + 1e-5, "{freq} Hz @ {sr}: peak {p}");
        // and it is limiting, not muting: the steady state sits near full scale
        let tail = &x[x.len() / 2..];
        assert!(peak(tail) > 0.9, "{freq} Hz: limited too hard ({})", peak(tail));
    }
}

#[test]
fn limiter_resting_is_pure_delay() {
    let sr = 48_000;
    let x = stereo(&sine(440.0, sr as f64, 0.2, 0.5));
    let mut y = x.clone();
    let mut lim = PeakLimiter::new(sr);
    lim.process(&mut y, 2);
    let d = lim.latency_frames() * 2;
    assert_eq!(lim.latency_frames(), 240);
    assert!(y[..d].iter().all(|&s| s == 0.0));
    assert_eq!(&y[d..], &x[..x.len() - d]);
}

// ---------------------------------------------------------------------------
// resampler
// ---------------------------------------------------------------------------

#[test]
fn resampler_preserves_tone_frequency_and_level() {
    for (inr, outr, f) in [(44_100u32, 48_000u32, 1000.0), (48_000, 44_100, 3000.0), (96_000, 48_000, 440.0), (22_050, 48_000, 5000.0)] {
        let x = stereo(&sine(f, inr as f64, 1.0, 0.5));
        let mut r = SincResampler::new(inr, outr);
        let mut y = Vec::new();
        for block in x.chunks(2 * 997) {
            r.process(block, 2, &mut y, false);
        }
        r.process(&[], 2, &mut y, true);
        let frames = y.len() / 2;
        let expect = inr as f64 * 1.0 * outr as f64 / inr as f64;
        assert!((frames as f64 - expect).abs() < 8.0, "{inr}->{outr}: {frames} frames, expected {expect}");
        let l = left(&y);
        let mid = &l[outr as usize / 10..outr as usize * 9 / 10];
        let got = zc_freq(mid, outr as f64);
        assert!((got - f).abs() < 0.5, "{inr}->{outr}: tone {got:.3} Hz, expected {f}");
        let lvl = rms(mid) * 2f64.sqrt();
        assert!((lvl - 0.5).abs() < 0.005, "{inr}->{outr}: amplitude {lvl}");
    }
}

#[test]
fn resampler_passthrough_is_bit_exact_and_varispeed_shifts_pitch() {
    let x = stereo(&sine(1000.0, 48_000.0, 0.5, 0.5));
    let mut r = SincResampler::new(48_000, 48_000);
    let mut y = Vec::new();
    r.process(&x, 2, &mut y, false);
    assert_eq!(y, x);
    // varispeed ×2: same rates, double speed → half the frames, twice the pitch
    let x = stereo(&sine(1000.0, 48_000.0, 1.0, 0.5));
    let mut r = SincResampler::new(48_000, 48_000);
    r.set_speed(2.0);
    let mut y = Vec::new();
    r.process(&x, 2, &mut y, true);
    assert!(((y.len() / 2) as f64 - 24_000.0).abs() < 8.0);
    let l = left(&y);
    let got = zc_freq(&l[2400..21600], 48_000.0);
    assert!((got - 2000.0).abs() < 1.0, "varispeed tone {got}");
}

// ---------------------------------------------------------------------------
// gain stages
// ---------------------------------------------------------------------------

#[test]
fn replay_gain_and_volume_curve_formulas() {
    let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
    assert!(close(replay_gain_factor(-6.0, None, 3.0), 10f32.powf(-3.0 / 20.0)));
    assert!(close(replay_gain_factor(-6.0, Some(0.5), 0.0), 10f32.powf(-6.0 / 20.0)));
    // capped by 1/peak, never boosts past 1
    assert!(close(replay_gain_factor(-1.0, Some(0.99), 3.0), 1.0));
    assert!(close(replay_gain_factor(-8.0, Some(2.0), 3.0), 0.5));
    // floor 0.03
    assert!(close(replay_gain_factor(-60.0, None, 3.0), 0.03));
    assert!(close(volume_curve_gain(0.5, 2.0, 1.0), 0.25));
    assert!(close(volume_curve_gain(0.5, 3.0, 0.5), 0.0625));
    assert!(close(volume_curve_gain(1.0, 2.0, 0.0), 0.05)); // limit clamp
    assert!(close(volume_curve_gain(0.5, 9.0, 1.0), 0.5f32.powi(5))); // exponent clamp
    assert!(close(volume_curve_gain(0.5, 0.2, 1.0), 0.5)); // exponent ≥ 1
    assert_eq!(parse_replay_gain_number("-8.89 dB"), Some(-8.89));
    assert_eq!(parse_replay_gain_number("+2.1 db"), Some(2.1));
    assert_eq!(parse_replay_gain_number("1.000000"), Some(1.0));
    assert_eq!(parse_replay_gain_number("gain: 3"), Some(3.0));
    assert_eq!(parse_replay_gain_number("n/a"), None);
}

// ---------------------------------------------------------------------------
// reverb / spatial / loudness stay finite and bounded
// ---------------------------------------------------------------------------

fn noise(n: usize, amp: f32) -> Vec<f32> {
    let mut s: u32 = 0x1234_5678;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s as f32 / u32::MAX as f32 * 2.0 - 1.0) * amp
        })
        .collect()
}

#[test]
fn reverb_is_finite_and_bounded() {
    let sr = 48_000;
    for (id, ..) in mochy_core::settings::REVERB_PRESETS {
        let settings = mochy_core::settings::ReverbSettings::from_preset(id, true).unwrap();
        let mut rv = Reverb::new(sr, ReverbParams::from_settings(&settings));
        let mut x = noise(sr as usize * 2 * 2, 1.0);
        for (i, block) in x.chunks_mut(1024).enumerate() {
            if i == 40 {
                let mut p = ReverbParams::from_settings(&settings);
                p.room_size = 1.0;
                p.damping = 0.0;
                rv.set_params(p);
            }
            rv.process(block);
        }
        assert!(x.iter().all(|s| s.is_finite()), "{id}: non-finite output");
        assert!(peak(&x) <= 1.0, "{id}: peak {}", peak(&x));
        // the room actually rings after the input stops
        let mut tail = vec![0.0f32; 4096];
        rv.process(&mut tail);
        assert!(peak(&tail) > 1e-4, "{id}: no tail");
    }
}

#[test]
fn spatial_is_finite_and_bounded() {
    let sr = 48_000;
    let mut settings = mochy_core::settings::SpatialSettings::default();
    let mut sp = Spatial::new(sr, SpatialParams::from_settings(&settings));
    let mut x = noise(sr as usize * 4 * 2, 1.0);
    let steps: [&dyn Fn(&mut mochy_core::settings::SpatialSettings); 5] = [
        &|s| s.crossfeed_enabled = true,
        &|s| s.virtual_speakers = true,
        &|s| s.mono = true,
        &|s| {
            s.mono = false;
            s.width = 2.0
        },
        &|s| s.speaker_angle_deg = 90.0,
    ];
    let blocks = x.len() / 2048;
    for (i, block) in x.chunks_mut(2048).enumerate() {
        let k = i * steps.len() / blocks.max(1);
        if i > 0 && k < steps.len() && (i - 1) * steps.len() / blocks.max(1) != k {
            steps[k](&mut settings);
            sp.set_params(SpatialParams::from_settings(&settings));
        }
        sp.process(block);
    }
    assert!(x.iter().all(|s| s.is_finite()));
    assert!(peak(&x) <= 1.0, "peak {}", peak(&x));
    // mono really folds: L == R once settled
    let mut m = mochy_core::settings::SpatialSettings { mono: true, ..Default::default() };
    let mut sp = Spatial::new(sr, SpatialParams::from_settings(&m));
    let mut y = noise(4096, 0.5);
    sp.process(&mut y);
    assert!(y.as_chunks::<2>().0.iter().all(|f| (f[0] - f[1]).abs() < 1e-6));
    m.mono = false;
    sp.set_params(SpatialParams::from_settings(&m));
    sp.process(&mut noise(48_000, 0.5));
    assert!(sp.is_bypassed());
}

#[test]
fn loudness_is_bounded_and_louder() {
    let sr = 48_000;
    let mut lp = LoudnessProcessor::new(sr, 2, 6.0, 0.5);
    let quiet = stereo(&sine(300.0, sr as f64, 1.0, 0.05));
    let mut y = quiet.clone();
    lp.process(&mut y);
    assert!(rms(&y[48_000..]) > rms(&quiet[48_000..]) * 1.5);
    let mut loud = stereo(&sine(300.0, sr as f64, 1.0, 1.0));
    lp.process(&mut loud);
    assert!(loud.iter().all(|s| s.is_finite()));
    assert!(peak(&loud) <= 1.0);
    assert!(lp.reduction_db() < 0.0);
}

// ---------------------------------------------------------------------------
// time-stretch
// ---------------------------------------------------------------------------

#[test]
fn time_stretch_keeps_duration_ratio_and_pitch() {
    let sr = 48_000u32;
    let secs = 3.0;
    let x = stereo(&sine(440.0, sr as f64, secs, 0.5));
    for tempo in [0.5, 0.8, 1.25, 1.5, 2.0, 3.0] {
        let mut ts = TimeStretch::new(sr, 2);
        ts.set_tempo(tempo);
        let mut y = Vec::new();
        for block in x.chunks(2 * 1500) {
            ts.process(block, &mut y);
        }
        ts.flush(&mut y);
        let got = (y.len() / 2) as f64 / sr as f64;
        let want = secs / tempo;
        assert!((got / want - 1.0).abs() < 0.02, "tempo {tempo}: {got:.3} s, expected {want:.3} s");
        let l = left(&y);
        let mid = &l[l.len() / 5..l.len() * 4 / 5];
        let f = zc_freq(mid, sr as f64);
        assert!((f / 440.0 - 1.0).abs() < 0.02, "tempo {tempo}: pitch moved to {f:.1} Hz");
        let lvl = rms(mid) * 2f64.sqrt();
        assert!((0.4..0.6).contains(&lvl), "tempo {tempo}: level {lvl}");
    }
}

#[test]
fn rate_pipeline_pitch_shift_keeps_duration() {
    let sr = 48_000u32;
    let x = stereo(&sine(440.0, sr as f64, 2.0, 0.5));
    // +12 semitones at speed 1 with pitch preserved → same length, 880 Hz
    let mut p = RatePipeline::new(sr, sr, RateParams::new(1.0, 12.0, true));
    let mut y = Vec::new();
    for block in x.chunks(2 * 1024) {
        p.process(block, &mut y);
    }
    p.flush(&mut y);
    let got = (y.len() / 2) as f64 / sr as f64;
    assert!((got / 2.0 - 1.0).abs() < 0.02, "duration {got}");
    let l = left(&y);
    let f = zc_freq(&l[l.len() / 5..l.len() * 4 / 5], sr as f64);
    assert!((f / 880.0 - 1.0).abs() < 0.02, "pitch {f}");
    // varispeed 1.5 (pitch follows)
    let mut p = RatePipeline::new(sr, sr, RateParams::new(1.5, 0.0, false));
    let mut y = Vec::new();
    p.process(&x, &mut y);
    p.flush(&mut y);
    let got = (y.len() / 2) as f64 / sr as f64;
    assert!((got - 2.0 / 1.5).abs() < 0.01);
    let f = zc_freq(&left(&y)[4800..40000], sr as f64);
    assert!((f - 660.0).abs() < 1.0, "varispeed pitch {f}");
}

// ---------------------------------------------------------------------------
// spectrum
// ---------------------------------------------------------------------------

#[test]
fn spectrum_peaks_at_tone_in_dbfs() {
    let sr = 48_000u32;
    let mut a = SpectrumAnalyzer::new(sr);
    let x = sine(1000.0, sr as f64, 0.5, 1.0);
    a.push(&x);
    let mut bands = Vec::new();
    for _ in 0..12 {
        bands = a.compute().to_vec();
    }
    assert_eq!(bands.len(), SPECTRUM_BANDS);
    let edge = |i: usize| 20.0 * 1000f32.powf(i as f32 / SPECTRUM_BANDS as f32);
    let b = (0..SPECTRUM_BANDS).find(|&i| edge(i) <= 1000.0 && 1000.0 < edge(i + 1)).unwrap();
    assert!(bands[b] > -1.5 && bands[b] < 1.0, "1 kHz band reads {} dBFS", bands[b]);
    assert!(bands[2] < -60.0, "20 Hz-ish band reads {}", bands[2]);
    assert!(bands[SPECTRUM_BANDS - 2] < -60.0);
}

// ---------------------------------------------------------------------------
// headless pipeline: WAV file → decode → rate stages → FX chain
// ---------------------------------------------------------------------------

fn run_chain(sr: u32, params: &ChainParams, x: &mut [f32]) {
    let mut chain = OutputChain::new(sr, params);
    for block in x.chunks_mut(2 * 512) {
        chain.process(block);
    }
}

#[test]
fn headless_wav_pipeline() {
    let dir = temp_dir("pipeline");
    let path = dir.join("tone.wav");
    let src_sr = 44_100u32;
    let tone = sine(440.0, src_sr as f64, 2.0, 0.5);
    write_wav(&path, src_sr, 2, &stereo(&tone));

    let mut s = DecodeSession::open(&TrackSource::Local { path: path.clone() }, Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!(s.info.sample_rate, 44_100);
    assert_eq!(s.info.channels, 2);
    assert_eq!(s.info.bits_per_sample, Some(16));
    assert!(s.info.codec.starts_with("pcm"), "codec {}", s.info.codec);
    assert!((s.info.duration_secs.unwrap() - 2.0).abs() < 1e-3);
    let decoded = decode_all(&mut s);
    assert_eq!(decoded.len(), tone.len() * 2);

    let out_sr = 48_000u32;
    let mut pipe = RatePipeline::new(src_sr, out_sr, RateParams::default());
    let mut y = Vec::new();
    for block in decoded.chunks(2 * 1152) {
        pipe.process(block, &mut y);
    }
    pipe.flush(&mut y);
    let flat = mochy_core::eq::settings_from_preset(mochy_core::eq::preset_by_id("flat").unwrap());
    let params = ChainParams { eq: EqDesign::from_settings(&flat, out_sr as f32), gain: 0.5, ..Default::default() };
    run_chain(out_sr, &params, &mut y);
    let frames = y.len() / 2;
    assert!((frames as f64 / out_sr as f64 - 2.0).abs() < 0.01, "{frames} frames");
    let l = left(&y);
    let mid = &l[out_sr as usize / 2..out_sr as usize * 3 / 2];
    // 0.5 amplitude × 0.5 gain → 0.25 peak → RMS 0.1768 (−15.05 dBFS)
    let db = 20.0 * rms(mid).log10();
    assert!((db - (-15.05)).abs() < 0.2, "level {db:.2} dBFS");
    assert!((zc_freq(mid, out_sr as f64) - 440.0).abs() < 0.5);

    // seeking: land at exactly 1.25 s
    let at = s.seek(1.25).unwrap();
    assert!((at - 1.25).abs() < 1e-6);
    let rest = decode_all(&mut s);
    assert!(((rest.len() / 2) as f64 - 0.75 * src_sr as f64).abs() <= 2.0, "{} frames after seek", rest.len() / 2);
    let first = rest[0] as f64;
    let expect = 0.5 * (2.0 * PI * 440.0 * 1.25).sin();
    assert!((first - expect).abs() < 2e-3, "first sample after seek {first} vs {expect}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn headless_mono_wav_is_spread_to_stereo_and_stretched() {
    let dir = temp_dir("mono");
    let path = dir.join("mono.wav");
    let tone = sine(330.0, 48_000.0, 1.5, 0.4);
    write_wav(&path, 48_000, 1, &tone);
    let mut s = DecodeSession::open(&TrackSource::Local { path }, Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!(s.info.channels, 1);
    let decoded = decode_all(&mut s);
    assert_eq!(decoded.len(), tone.len() * 2);
    assert!(decoded.as_chunks::<2>().0.iter().all(|f| f[0] == f[1]));
    // 1.5× with pitch preserved
    let mut pipe = RatePipeline::new(48_000, 48_000, RateParams::new(1.5, 0.0, true));
    let mut y = Vec::new();
    for block in decoded.chunks(4096) {
        pipe.process(block, &mut y);
    }
    pipe.flush(&mut y);
    let secs = (y.len() / 2) as f64 / 48_000.0;
    assert!((secs / 1.0 - 1.0).abs() < 0.02, "{secs} s");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap().join(dir.file_name().unwrap()));
}

// ---------------------------------------------------------------------------
// remote source against a local HTTP server (ranged + non-ranged)
// ---------------------------------------------------------------------------

/// Minimal HTTP/1.1 server with keep-alive and optional Range support.
fn serve(data: Arc<Vec<u8>>, honor_range: bool) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let req_count = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let data = data.clone();
            let req_count = req_count.clone();
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut tmp) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&tmp[..n]),
                        }
                    }
                    let end = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                    let head = String::from_utf8_lossy(&buf[..end]).to_string();
                    buf.drain(..end);
                    req_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let range = head.lines().find_map(|l| {
                        let l = l.to_ascii_lowercase();
                        l.strip_prefix("range: bytes=").map(|r| {
                            let (a, b) = r.split_once('-').unwrap();
                            let a: usize = a.trim().parse().unwrap();
                            let b: usize = b.trim().parse().unwrap_or(data.len() - 1);
                            (a, b.min(data.len() - 1))
                        })
                    });
                    let resp = match range {
                        Some((a, b)) if honor_range && a < data.len() => {
                            let mut r = format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Type: audio/wav\r\nContent-Range: bytes {a}-{b}/{}\r\nContent-Length: {}\r\n\r\n",
                                data.len(),
                                b - a + 1
                            )
                            .into_bytes();
                            r.extend_from_slice(&data[a..=b]);
                            r
                        }
                        _ => {
                            let mut r = format!("HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\n\r\n", data.len())
                                .into_bytes();
                            r.extend_from_slice(&data);
                            r
                        }
                    };
                    if stream.write_all(&resp).is_err() {
                        return;
                    }
                }
            });
        }
    });
    (format!("http://127.0.0.1:{port}/track.wav"), requests)
}

#[test]
fn remote_ranged_source_decodes_and_seeks() {
    let tone = stereo(&sine(440.0, 44_100.0, 8.0, 0.5)); // ~1.4 MB → 3 chunks
    let bytes = Arc::new(wav_bytes(44_100, 2, &tone));
    let (url, requests) = serve(bytes.clone(), true);
    let src = TrackSource::Remote { url, headers: vec![("X-Test".into(), "1".into())] };
    let mut s = DecodeSession::open(&src, Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!(s.info.sample_rate, 44_100);
    assert!((s.info.duration_secs.unwrap() - 8.0).abs() < 1e-3);
    let at = s.seek(6.0).unwrap();
    assert!((at - 6.0).abs() < 1e-6);
    let rest = decode_all(&mut s);
    assert!(((rest.len() / 2) as f64 - 2.0 * 44_100.0).abs() <= 2.0, "{} frames", rest.len() / 2);
    let back = s.seek(0.5).unwrap();
    assert!((back - 0.5).abs() < 1e-6);
    let all = decode_all(&mut s);
    assert!(((all.len() / 2) as f64 - 7.5 * 44_100.0).abs() <= 2.0);
    assert!(requests.load(std::sync::atomic::Ordering::SeqCst) >= 3, "expected ranged chunk requests");

    // the raw byte source honours Range/seek exactly
    use std::io::{Seek, SeekFrom};
    let mut hs = crate::http_source::HttpByteSource::new(src_url(&src), vec![]).unwrap();
    assert_eq!(hs.len(), Some(bytes.len() as u64));
    hs.seek(SeekFrom::Start(1_000_000)).unwrap();
    let mut b = [0u8; 100_000];
    hs.read_exact(&mut b).unwrap();
    assert_eq!(&b[..], &bytes[1_000_000..1_100_000]);
    hs.seek(SeekFrom::End(-10)).unwrap();
    let mut t = Vec::new();
    hs.read_to_end(&mut t).unwrap();
    assert_eq!(&t[..], &bytes[bytes.len() - 10..]);
}

fn src_url(s: &TrackSource) -> String {
    match s {
        TrackSource::Remote { url, .. } => url.clone(),
        _ => unreachable!(),
    }
}

#[test]
fn remote_source_without_range_support_still_plays() {
    let tone = stereo(&sine(220.0, 22_050.0, 3.0, 0.5));
    let bytes = Arc::new(wav_bytes(22_050, 2, &tone));
    let (url, _) = serve(bytes, false);
    let mut s = DecodeSession::open(&TrackSource::Remote { url, headers: vec![] }, Arc::new(AtomicBool::new(false))).unwrap();
    let all = decode_all(&mut s);
    assert_eq!(all.len(), tone.len());
    s.seek(1.0).unwrap();
    let rest = decode_all(&mut s);
    assert!(((rest.len() / 2) as f64 - 2.0 * 22_050.0).abs() <= 2.0);
}

#[test]
fn opus_head_parsing() {
    let mut head = b"OpusHead".to_vec();
    head.extend_from_slice(&[1, 2]);
    head.extend_from_slice(&312u16.to_le_bytes());
    head.extend_from_slice(&48_000u32.to_le_bytes());
    head.extend_from_slice(&(-256i16).to_le_bytes());
    head.push(0);
    let (ch, skip, gain, fam) = crate::decoder::parse_opus_head(&head).unwrap();
    assert_eq!((ch, skip, fam), (2, 312, 0));
    assert!((gain + 1.0).abs() < 1e-6);
}
