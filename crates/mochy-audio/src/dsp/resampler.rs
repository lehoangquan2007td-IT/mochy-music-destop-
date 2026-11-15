//! Kaiser-windowed sinc resampler (spec §4.6 `SincResampler`): stopband
//! 120 dB, 192 taps at 1:1 (scaled up when going down in rate so the slope
//! stays as steep relative to the lower Nyquist), passband = lower Nyquist −
//! Kaiser transition.
//!
//! Unlike Android's fixed rational polyphase table, this one interpolates a
//! finely sampled kernel, so the ratio can change continuously — that is how
//! varispeed (speed without pitch preservation) and pitch shifting are done
//! without clicks. While the ratio is exactly 1:1 it passes samples through
//! untouched (bit-exact) and engages seamlessly when the ratio moves.

use std::sync::OnceLock;

const STOPBAND_DB: f64 = 120.0;
const BASE_TAPS: f64 = 192.0;
/// Kernel samples per zero crossing (linear interpolation error ≈ −130 dB).
const TABLE_RES: usize = 2048;
/// Never let a single output read more than this many input frames per side
/// (only reached above ~6× speed); keeps CPU bounded.
const MAX_HALF_LEN: f64 = 640.0;

struct Kernel {
    /// Cutoff relative to the lower Nyquist (6 dB point).
    cut: f64,
    /// Zero crossings per side.
    zeros: f64,
    /// `table[j] = sinc(j/RES)·kaiser(j/RES/zeros)`, j = 0 ..= zeros·RES (+1 guard).
    table: Vec<f32>,
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..64 {
        term *= (half / k as f64) * (half / k as f64);
        sum += term;
        if term < sum * 1e-14 {
            break;
        }
    }
    sum
}

fn kernel() -> &'static Kernel {
    static K: OnceLock<Kernel> = OnceLock::new();
    K.get_or_init(|| {
        // Android: transition (Hz at the input rate) = (A − 7.95)/(2.285·2π·taps)·rate,
        // stop = lower Nyquist, cutoff = stop − transition/2.
        let transition = (STOPBAND_DB - 7.95) / (2.285 * 2.0 * std::f64::consts::PI * BASE_TAPS) * 2.0;
        let cut = 1.0 - transition / 2.0;
        let zeros = (BASE_TAPS / 2.0 * cut).floor();
        let beta = 0.1102 * (STOPBAND_DB - 8.7);
        let i0b = bessel_i0(beta);
        let n = (zeros as usize) * TABLE_RES + 2;
        let mut table = Vec::with_capacity(n);
        for j in 0..n {
            let u = j as f64 / TABLE_RES as f64;
            let sinc = if u == 0.0 { 1.0 } else { (std::f64::consts::PI * u).sin() / (std::f64::consts::PI * u) };
            let r = u / zeros;
            let w = if r >= 1.0 { 0.0 } else { bessel_i0(beta * (1.0 - r * r).sqrt()) / i0b };
            table.push((sinc * w) as f32);
        }
        Kernel { cut, zeros, table }
    })
}

pub struct SincResampler {
    pub in_rate: u32,
    pub out_rate: u32,
    speed: f64,
    /// Input frames advanced per output frame.
    step: f64,
    channels: usize,
    /// Input history, interleaved; `hist[0]` is absolute input frame `base`.
    hist: Vec<f32>,
    base: u64,
    /// Absolute input position (fractional) of the next output frame.
    pos: f64,
    engaged: bool,
    flushed: bool,
    weights: Vec<f32>,
}

impl SincResampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        let _ = kernel();
        let mut r = SincResampler {
            in_rate: in_rate.max(1),
            out_rate: out_rate.max(1),
            speed: 1.0,
            step: 1.0,
            channels: 0,
            hist: Vec::new(),
            base: 0,
            pos: 0.0,
            engaged: false,
            flushed: false,
            weights: Vec::with_capacity(2 * MAX_HALF_LEN as usize + 4),
        };
        r.update_step();
        r
    }

    fn update_step(&mut self) {
        self.step = self.in_rate as f64 * self.speed / self.out_rate as f64;
    }

    /// Varispeed / pitch factor: the input is treated as `in_rate·speed`.
    pub fn set_speed(&mut self, speed: f64) {
        self.speed = speed.clamp(0.01, 16.0);
        self.update_step();
    }

    pub fn set_rates(&mut self, in_rate: u32, out_rate: u32) {
        self.in_rate = in_rate.max(1);
        self.out_rate = out_rate.max(1);
        self.update_step();
    }

    /// Output frames per input frame.
    pub fn ratio(&self) -> f64 {
        1.0 / self.step
    }

    pub fn is_passthrough(&self) -> bool {
        !self.engaged && self.step == 1.0
    }

    /// Forget all history (seek / flush).
    pub fn reset(&mut self) {
        self.hist.clear();
        self.base = 0;
        self.pos = 0.0;
        self.engaged = false;
        self.flushed = false;
    }

    /// Half filter length in input frames and the kernel scale λ.
    fn geometry(&self) -> (f64, f64) {
        let k = kernel();
        let rho = 1.0 / self.step; // out per in
        let lambda = k.cut * rho.min(1.0);
        let half = (k.zeros / lambda).min(MAX_HALF_LEN);
        (half, lambda)
    }

    /// Frames of delay the filter currently adds (in input frames).
    pub fn latency_frames(&self) -> usize {
        if self.engaged { self.geometry().0.ceil() as usize } else { 0 }
    }

    /// Resample `input` (interleaved, `channels`) appending to `output`.
    /// `input_done` flushes the filter tail. Returns output frames written.
    pub fn process(&mut self, input: &[f32], channels: usize, output: &mut Vec<f32>, input_done: bool) -> usize {
        if channels == 0 {
            return 0;
        }
        if channels != self.channels {
            self.channels = channels;
            self.reset();
        }
        if self.hist.is_empty() && self.base == 0 && self.pos == 0.0 {
            // keep a quiet past so the first engaged outputs have history
            let (half, _) = self.geometry_max();
            self.hist.resize((half.ceil() as usize + 2) * channels, 0.0);
            self.base = 0;
            self.pos = (half.ceil() + 2.0).floor();
        }
        self.hist.extend_from_slice(input);
        if input_done && !self.flushed {
            self.flushed = true;
            let (half, _) = self.geometry_max();
            let pad = (half.ceil() as usize + 2) * channels;
            self.hist.resize(self.hist.len() + pad, 0.0);
        }
        let start_len = output.len();
        let avail_end = self.base + (self.hist.len() / channels) as u64; // exclusive
        if !self.engaged && self.step == 1.0 && self.pos.fract() == 0.0 {
            // bit-exact pass-through
            let from = (self.pos as u64 - self.base) as usize;
            let to = (avail_end - self.base) as usize;
            if to > from {
                output.extend_from_slice(&self.hist[from * channels..to * channels]);
                self.pos = avail_end as f64;
            }
        } else {
            self.engaged = true;
            let k = kernel();
            let res = TABLE_RES as f64;
            loop {
                let (half, lambda) = self.geometry();
                let p = self.pos;
                let first = (p - half).ceil() as i64;
                let last = (p + half).floor() as i64;
                if last < 0 || (last as u64) >= avail_end {
                    break;
                }
                let first = first.max(self.base as i64) as u64;
                // weights shared by all channels
                self.weights.clear();
                let mut wsum = 0.0f64;
                for n in first..=(last as u64) {
                    let u = ((n as f64 - p).abs() * lambda * res).min((k.table.len() - 2) as f64);
                    let j = u as usize;
                    let f = (u - j as f64) as f32;
                    let w = k.table[j] + (k.table[j + 1] - k.table[j]) * f;
                    wsum += w as f64;
                    self.weights.push(w);
                }
                let norm = if wsum.abs() > 1e-9 { (1.0 / wsum) as f32 } else { 0.0 };
                let off = ((first - self.base) as usize) * channels;
                for c in 0..channels {
                    let mut acc = 0.0f32;
                    for (i, w) in self.weights.iter().enumerate() {
                        acc += w * self.hist[off + i * channels + c];
                    }
                    output.push(acc * norm);
                }
                self.pos += self.step;
            }
        }
        // drop history no output can reach any more (even after a ratio change)
        let keep_from = (self.pos - MAX_HALF_LEN - 2.0).floor().max(self.base as f64) as u64;
        let drop = (keep_from - self.base) as usize;
        if drop > 0 {
            let drop = drop.min(self.hist.len() / channels);
            self.hist.drain(..drop * channels);
            self.base += drop as u64;
        }
        (output.len() - start_len) / channels
    }

    /// Worst-case half length (used for history retention so a later ratio
    /// change never needs samples that were already dropped).
    fn geometry_max(&self) -> (f64, f64) {
        let (h, l) = self.geometry();
        (h.max(kernel().zeros / kernel().cut + 1.0), l)
    }
}
