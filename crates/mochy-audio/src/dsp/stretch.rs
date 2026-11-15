//! WSOLA time-stretch (waveform-similarity overlap-add, Verhelst & Roelands)
//! for "preserve pitch" and pitch shifting (spec §4.7). Changes tempo
//! without changing pitch: windows of ~42 ms (periodic Hann, 50 % overlap)
//! are taken from the input every `hop·tempo` frames, each one nudged by up
//! to ±12 ms to the offset whose waveform best continues the previous
//! window (normalised cross-correlation on a mono mix, coarse-to-fine), and
//! overlap-added at a fixed output hop. All channels share one offset so the
//! stereo image stays put.
//!
//! At tempo 1 it passes audio through bit-exactly; it engages seamlessly
//! (the first window is aligned with what was already played) and the tempo
//! may change at any time.

pub struct TimeStretch {
    channels: usize,
    tempo: f64,
    /// Output hop (frames) = half window.
    hs: usize,
    /// Search radius (frames).
    delta: usize,
    /// Coarse search decimation.
    dec: usize,
    window: Vec<f32>,
    inbuf: Vec<f32>,
    in_base: i64,
    engaged: bool,
    /// Bypass: next input frame (absolute) to pass through.
    bypass_next: i64,
    /// Engaged: nominal analysis position of the next window.
    ana_pos: f64,
    prev_start: i64,
    tail: Vec<f32>,
    tail_pending: bool,
    /// End of real input (absolute frame) once `flush` was called.
    real_end: Option<i64>,
    tail_emitted: bool,
    tmpl: Vec<f32>,
    region: Vec<f32>,
    tmono: Vec<f32>,
    rmono: Vec<f32>,
}

impl TimeStretch {
    pub fn new(sample_rate: u32, channels: usize) -> Self {
        let sr = sample_rate.max(8000) as f64;
        let hs = (0.021 * sr).round() as usize;
        let n = hs * 2;
        let delta = (0.012 * sr).round() as usize;
        let dec = if sr >= 32_000.0 { 2 } else { 1 };
        // periodic Hann: w[i] + w[i + hs] == 1 exactly (up to rounding)
        let window = (0..n)
            .map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()) as f32)
            .collect();
        let channels = channels.max(1);
        TimeStretch {
            channels,
            tempo: 1.0,
            hs,
            delta,
            dec,
            window,
            inbuf: Vec::new(),
            in_base: 0,
            engaged: false,
            bypass_next: 0,
            ana_pos: 0.0,
            prev_start: 0,
            tail: vec![0.0; hs * channels],
            tail_pending: true,
            real_end: None,
            tail_emitted: false,
            tmpl: Vec::new(),
            region: Vec::new(),
            tmono: Vec::new(),
            rmono: Vec::new(),
        }
    }

    /// Tempo factor (>1 faster/shorter). Clamped to 0.05–16.
    pub fn set_tempo(&mut self, tempo: f64) {
        self.tempo = tempo.clamp(0.05, 16.0);
    }

    pub fn tempo(&self) -> f64 {
        self.tempo
    }

    pub fn is_passthrough(&self) -> bool {
        !self.engaged
    }

    pub fn reset(&mut self) {
        self.inbuf.clear();
        self.in_base = 0;
        self.engaged = false;
        self.bypass_next = 0;
        self.ana_pos = 0.0;
        self.prev_start = 0;
        self.tail.fill(0.0);
        self.tail_pending = true;
        self.real_end = None;
        self.tail_emitted = false;
    }

    fn in_end(&self) -> i64 {
        self.in_base + (self.inbuf.len() / self.channels) as i64
    }

    /// Stretch interleaved `input`, appending to `output`. Returns frames written.
    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) -> usize {
        let start = output.len();
        self.inbuf.extend_from_slice(input);
        if !self.engaged {
            if self.tempo == 1.0 {
                let ch = self.channels;
                let from = ((self.bypass_next - self.in_base) as usize) * ch;
                output.extend_from_slice(&self.inbuf[from..]);
                self.bypass_next = self.in_end();
                self.trim();
                return (output.len() - start) / ch;
            }
            self.engage();
        }
        while self.step(output) {}
        self.trim();
        (output.len() - start) / self.channels
    }

    /// Input finished: emit everything still inside the stretcher.
    pub fn flush(&mut self, output: &mut Vec<f32>) -> usize {
        let start = output.len();
        if !self.engaged {
            return 0;
        }
        if self.real_end.is_none() {
            self.real_end = Some(self.in_end());
            let pad = (self.hs * 2 + self.delta * 2 + 4) * self.channels;
            self.inbuf.resize(self.inbuf.len() + pad, 0.0);
        }
        while self.step(output) {}
        (output.len() - start) / self.channels
    }

    fn engage(&mut self) {
        self.engaged = true;
        let p = self.bypass_next;
        self.prev_start = p - self.hs as i64;
        self.ana_pos = p as f64;
        self.tail_pending = true;
    }

    /// Keep only input a future window can reach.
    fn trim(&mut self) {
        let keep_from = if self.engaged {
            (self.prev_start).min(self.ana_pos.floor() as i64 - self.delta as i64) - 1
        } else {
            self.bypass_next - (self.hs * 2 + self.delta) as i64
        };
        let drop = (keep_from - self.in_base).max(0) as usize;
        let drop = drop.min(self.inbuf.len() / self.channels);
        if drop > 0 {
            self.inbuf.drain(..drop * self.channels);
            self.in_base += drop as i64;
        }
    }

    fn fetch(&self, abs_start: i64, frames: usize, out: &mut Vec<f32>) {
        let ch = self.channels;
        out.clear();
        out.resize(frames * ch, 0.0);
        let end = self.in_end();
        let a = abs_start.max(self.in_base);
        let b = (abs_start + frames as i64).min(end);
        if b > a {
            let src = ((a - self.in_base) as usize) * ch;
            let dst = ((a - abs_start) as usize) * ch;
            let len = ((b - a) as usize) * ch;
            out[dst..dst + len].copy_from_slice(&self.inbuf[src..src + len]);
        }
    }

    /// One output hop. Returns false when more input is needed / finished.
    fn step(&mut self, output: &mut Vec<f32>) -> bool {
        let (hs, d, n, ch) = (self.hs, self.delta, self.hs * 2, self.channels);
        let tau = self.ana_pos.round() as i64;
        // output frame j of this hop plays input ≈ tau + j·tempo: stop exactly at the input's end
        let mut emit = hs;
        if let Some(end) = self.real_end {
            if tau >= end || self.tail_emitted {
                return false;
            }
            let left = ((end - tau) as f64 / self.tempo).ceil() as usize;
            if left < hs {
                emit = left;
                self.tail_emitted = true;
            }
        }
        let need = (self.prev_start + n as i64).max(tau + (d + n) as i64);
        if self.in_end() < need {
            return false;
        }
        let mut tmpl = std::mem::take(&mut self.tmpl);
        let mut region = std::mem::take(&mut self.region);
        self.fetch(self.prev_start + hs as i64, hs, &mut tmpl);
        self.fetch(tau - d as i64, 2 * d + n, &mut region);

        // mono views for the similarity search
        let mono = |src: &[f32], dst: &mut Vec<f32>| {
            dst.clear();
            dst.extend(src.chunks_exact(ch).map(|f| f.iter().sum::<f32>() / ch as f32));
        };
        let mut tm = std::mem::take(&mut self.tmono);
        let mut rm = std::mem::take(&mut self.rmono);
        mono(&tmpl, &mut tm);
        mono(&region, &mut rm);

        let tmpl_energy: f32 = tm.iter().map(|x| x * x).sum();
        let best = if tmpl_energy < 1e-9 {
            d // silence: no preference, take the nominal position
        } else {
            let score = |off: usize, stride: usize| -> f32 {
                let mut xy = 0.0f32;
                let mut yy = 0.0f32;
                let mut i = 0;
                while i < hs {
                    let y = rm[off + i];
                    xy += tm[i] * y;
                    yy += y * y;
                    i += stride;
                }
                xy / (yy + 1e-9).sqrt()
            };
            let mut best = d;
            let mut best_score = f32::MIN;
            let mut off = 0;
            while off <= 2 * d {
                let s = score(off, self.dec);
                if s > best_score {
                    best_score = s;
                    best = off;
                }
                off += self.dec;
            }
            if self.dec > 1 {
                let lo = best.saturating_sub(self.dec - 1);
                let hi = (best + self.dec - 1).min(2 * d);
                let mut best_fine = f32::MIN;
                for off in lo..=hi {
                    let s = score(off, 1);
                    if s > best_fine {
                        best_fine = s;
                        best = off;
                    }
                }
            }
            best
        };

        if self.tail_pending {
            // the window before the first one: what was already played continues
            for i in 0..hs {
                let w = self.window[hs + i];
                for c in 0..ch {
                    self.tail[i * ch + c] = w * tmpl[i * ch + c];
                }
            }
            self.tail_pending = false;
        }
        let seg = &region[best * ch..(best + n) * ch];
        output.reserve(hs * ch);
        for i in 0..emit {
            let w = self.window[i];
            for c in 0..ch {
                output.push(self.tail[i * ch + c] + w * seg[i * ch + c]);
            }
        }
        for i in 0..hs {
            let w = self.window[hs + i];
            for c in 0..ch {
                self.tail[i * ch + c] = w * seg[(hs + i) * ch + c];
            }
        }
        self.prev_start = tau - d as i64 + best as i64;
        self.ana_pos += hs as f64 * self.tempo;

        self.tmpl = tmpl;
        self.region = region;
        self.tmono = tm;
        self.rmono = rm;
        true
    }
}
