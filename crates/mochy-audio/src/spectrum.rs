//! Spectrum analyser for `EngineHandle::spectrum()`: Hann-windowed real FFT
//! (4096 points) of the post-DSP output tap, folded into `SPECTRUM_BANDS`
//! log-spaced bands over 20 Hz – 20 kHz in dBFS (a full-scale sine reads
//! 0 dB), with fast attack / slower release smoothing. Runs on the engine
//! thread, never in the audio callback.

use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};

use crate::SPECTRUM_BANDS;

pub(crate) const FFT_SIZE: usize = 4096;
const FLOOR_DB: f32 = -90.0;
const ATTACK: f32 = 0.6;
const RELEASE: f32 = 0.18;

pub struct SpectrumAnalyzer {
    sample_rate: u32,
    fft: Arc<dyn RealToComplex<f32>>,
    history: Vec<f32>,
    write: usize,
    filled: usize,
    window: Vec<f32>,
    input: Vec<f32>,
    output: Vec<realfft::num_complex::Complex<f32>>,
    scratch: Vec<realfft::num_complex::Complex<f32>>,
    /// (first bin, last bin, centre bin, band contains whole bins) per band.
    bands: Vec<(usize, usize, f32, bool)>,
    smoothed: Vec<f32>,
    mags: Vec<f32>,
}

impl SpectrumAnalyzer {
    pub fn new(sample_rate: u32) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let window = (0..FFT_SIZE)
            .map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / FFT_SIZE as f64).cos()) as f32)
            .collect();
        let input = fft.make_input_vec();
        let output = fft.make_output_vec();
        let scratch = fft.make_scratch_vec();
        let mut a = SpectrumAnalyzer {
            sample_rate,
            fft,
            history: vec![0.0; FFT_SIZE],
            write: 0,
            filled: 0,
            window,
            input,
            output,
            scratch,
            bands: Vec::new(),
            smoothed: vec![FLOOR_DB; SPECTRUM_BANDS],
            mags: vec![0.0; FFT_SIZE / 2 + 1],
        };
        a.layout();
        a
    }

    fn layout(&mut self) {
        let bin_hz = self.sample_rate as f32 / FFT_SIZE as f32;
        let nyq_bin = FFT_SIZE / 2;
        let edge = |i: usize| 20.0 * 1000f32.powf(i as f32 / SPECTRUM_BANDS as f32);
        self.bands = (0..SPECTRUM_BANDS)
            .map(|b| {
                let lo = edge(b) / bin_hz;
                let hi = edge(b + 1) / bin_hz;
                let centre = (edge(b) * edge(b + 1)).sqrt() / bin_hz;
                let first = (lo.ceil() as usize).min(nyq_bin);
                let last = ((hi.floor() as usize).max(first)).min(nyq_bin);
                (first, last, centre.min(nyq_bin as f32), hi.floor() >= lo.ceil())
            })
            .collect();
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Feed mono output samples.
    pub fn push(&mut self, samples: &[f32]) {
        for &s in samples {
            self.history[self.write] = s;
            self.write = (self.write + 1) % FFT_SIZE;
        }
        self.filled = (self.filled + samples.len()).min(FFT_SIZE);
    }

    /// Forget the signal (track change / stop).
    pub fn clear(&mut self) {
        self.history.fill(0.0);
        self.filled = 0;
        self.smoothed.fill(FLOOR_DB);
    }

    /// Compute the current smoothed bands (dBFS).
    pub fn compute(&mut self) -> &[f32] {
        for i in 0..FFT_SIZE {
            let s = self.history[(self.write + i) % FFT_SIZE];
            self.input[i] = s * self.window[i];
        }
        if self.fft.process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch).is_err() {
            return &self.smoothed;
        }
        // Hann coherent gain 0.5 → amplitude = 4|X|/N
        let scale = 4.0 / FFT_SIZE as f32;
        for (m, c) in self.mags.iter_mut().zip(&self.output) {
            *m = c.norm() * scale;
        }
        for (b, &(first, last, centre, has_bins)) in self.bands.iter().enumerate() {
            let amp = if has_bins {
                self.mags[first..=last].iter().copied().fold(0.0f32, f32::max)
            } else {
                // band narrower than a bin: interpolate at its centre
                let i = centre.floor() as usize;
                let f = centre - i as f32;
                let a = self.mags[i.min(self.mags.len() - 1)];
                let c = self.mags[(i + 1).min(self.mags.len() - 1)];
                a + (c - a) * f
            };
            let db = (20.0 * amp.max(1e-9).log10()).clamp(FLOOR_DB, 6.0);
            let s = &mut self.smoothed[b];
            let k = if db > *s { ATTACK } else { RELEASE };
            *s += (db - *s) * k;
        }
        &self.smoothed
    }
}
