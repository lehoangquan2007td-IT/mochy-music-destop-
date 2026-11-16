//! Per-lane crossmix renderer (desktop counterpart of
//! `CrossmixTransitionAudioProcessor.kt`): applies a [`FxParams`] snapshot
//! to one side of a hand-over — filters, bands, the bass hand-off, echo and
//! reverb sends, width and pan. Runs on the decode thread in small blocks;
//! parameters are refreshed per block, gain is interpolated per sample.

use mochy_core::eq::FilterType;

use super::styles::{FxParams, OPEN_HZ};
use crate::dsp::{Biquad, Reverb, ReverbParams};

const MAX_ECHO_SECS: f32 = 4.0;

#[derive(Clone, Copy, Default)]
struct Stereo {
    l: Biquad,
    r: Biquad,
}

impl Stereo {
    fn set(&mut self, t: FilterType, hz: f32, gain_db: f32, q: f32, sr: f32) {
        let d = Biquad::design(t, hz, gain_db, q, sr);
        for b in [&mut self.l, &mut self.r] {
            b.b0 = d.b0;
            b.b1 = d.b1;
            b.b2 = d.b2;
            b.a1 = d.a1;
            b.a2 = d.a2;
        }
    }
    #[inline]
    fn run(&mut self, l: f32, r: f32) -> (f32, f32) {
        (self.l.process(l as f64) as f32, self.r.process(r as f64) as f32)
    }
    fn reset(&mut self) {
        self.l.reset();
        self.r.reset();
    }
}

pub struct LaneFx {
    sr: f32,
    low_cut: Stereo,
    low_cut_on: bool,
    high_cut: Stereo,
    high_cut_on: bool,
    low_band: Stereo,
    mid_band: Stereo,
    high_band: Stereo,
    bands_on: [bool; 3],
    bass: [Stereo; 2],
    echo: Vec<f32>,
    echo_pos: usize,
    echo_tone: [f32; 2],
    reverb: Reverb,
    reverb_buf: Vec<f32>,
    gain: f32,
    /// Peak of the effect returns in the last block (tails ringing out).
    tail_level: f32,
}

impl LaneFx {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8_000) as f32;
        let mut rp = ReverbParams::default();
        rp.enabled = true;
        rp.mix = 1.0;
        rp.dry = 0.0;
        rp.wet = 1.0;
        rp.width = 1.0;
        rp.pre_delay_ms = 10.0;
        rp.low_cut_hz = 200.0;
        rp.damping = 0.5;
        LaneFx {
            sr,
            low_cut: Stereo::default(),
            low_cut_on: false,
            high_cut: Stereo::default(),
            high_cut_on: false,
            low_band: Stereo::default(),
            mid_band: Stereo::default(),
            high_band: Stereo::default(),
            bands_on: [false; 3],
            bass: [Stereo::default(); 2],
            echo: vec![0.0; (sr * MAX_ECHO_SECS) as usize * 2],
            echo_pos: 0,
            echo_tone: [0.0; 2],
            reverb: Reverb::new(sample_rate, rp),
            reverb_buf: Vec::new(),
            gain: 1.0,
            tail_level: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.low_cut.reset();
        self.high_cut.reset();
        self.low_band.reset();
        self.mid_band.reset();
        self.high_band.reset();
        for b in &mut self.bass {
            b.reset();
        }
        self.echo.fill(0.0);
        self.echo_tone = [0.0; 2];
        self.reverb.reset();
        self.tail_level = 0.0;
    }

    /// Start a ramp at `gain` (no jump on the first block).
    pub fn set_gain(&mut self, gain: f32) {
        self.gain = gain;
    }

    pub fn tail_level(&self) -> f32 {
        self.tail_level
    }

    /// Process one interleaved stereo block in place with parameters `p`
    /// (gain ramps from the previous block's to `p.gain × extra_gain`);
    /// `beat_ms` sets the echo time.
    pub fn process(&mut self, buf: &mut [f32], p: &FxParams, extra_gain: f32, beat_ms: f64) {
        let frames = buf.len() / 2;
        if frames == 0 {
            return;
        }
        let sr = self.sr;
        let nyq = sr * 0.45;
        let q = p.resonance.clamp(0.3, 4.0);
        // filters (coefficients per block)
        self.low_cut_on = p.low_cut_hz > 20.0;
        if self.low_cut_on {
            self.low_cut.set(FilterType::HighPass, p.low_cut_hz.min(nyq), 0.0, q, sr);
        }
        self.high_cut_on = p.high_cut_hz < OPEN_HZ.min(nyq);
        if self.high_cut_on {
            self.high_cut.set(FilterType::LowPass, p.high_cut_hz.clamp(30.0, nyq), 0.0, q, sr);
        }
        let db = |g: f32| 20.0 * g.max(0.001).log10();
        self.bands_on = [(p.low_band - 1.0).abs() > 1e-3, (p.mid_band - 1.0).abs() > 1e-3, (p.high_band - 1.0).abs() > 1e-3];
        if self.bands_on[0] {
            self.low_band.set(FilterType::LowShelf, 250.0, db(p.low_band), 0.707, sr);
        }
        if self.bands_on[1] {
            self.mid_band.set(FilterType::Peaking, 1_000.0, db(p.mid_band), 0.7, sr);
        }
        if self.bands_on[2] {
            self.high_band.set(FilterType::HighShelf, 4_000f32.min(nyq), db(p.high_band), 0.707, sr);
        }
        let bass_cut = p.bass_cut.clamp(0.0, 1.0);
        if bass_cut > 0.0 {
            self.bass[0].set(FilterType::HighPass, 200.0, 0.0, 0.54, sr);
            self.bass[1].set(FilterType::HighPass, 200.0, 0.0, 1.31, sr);
        }
        // echo
        let echo_len = self.echo.len() / 2;
        let delay = ((p.echo_beats as f64 * beat_ms / 1000.0 * sr as f64) as usize).clamp(1, echo_len - 1);
        let feedback = p.echo_feedback.clamp(0.0, 0.95);
        let tone_hz = 800.0 + 7_200.0 * p.echo_tone.clamp(0.0, 1.0);
        let tone_a = (-2.0 * std::f32::consts::PI * tone_hz / sr).exp();
        let echo_active = p.echo_send > 0.0 || self.tail_level > 1e-5;
        // reverb
        let reverb_active = p.reverb_send > 0.0 || self.tail_level > 1e-5;
        if reverb_active {
            let mut rp = ReverbParams::default();
            rp.enabled = true;
            rp.mix = 1.0;
            rp.dry = 0.0;
            rp.wet = 1.0;
            rp.width = 1.0;
            rp.pre_delay_ms = 10.0;
            rp.low_cut_hz = 200.0;
            rp.damping = 0.5 * (1.0 - p.reverb_freeze);
            rp.room_size = (p.reverb_size + (1.0 - p.reverb_size) * p.reverb_freeze).clamp(0.0, 0.99);
            self.reverb.set_params(rp);
            self.reverb_buf.resize(frames * 2, 0.0);
        }
        // width / pan
        let width = p.width.clamp(0.0, 2.0);
        let pan = p.pan.clamp(-1.0, 1.0);
        // balance law: the far side dips, the near side stays whole
        let (pan_l, pan_r) = ((1.0 - pan).min(1.0), (1.0 + pan).min(1.0));

        let g0 = self.gain;
        let g1 = (p.gain * extra_gain).max(0.0);
        let mut tail = 0f32;
        for i in 0..frames {
            let (mut l, mut r) = (buf[2 * i], buf[2 * i + 1]);
            if self.low_cut_on {
                (l, r) = self.low_cut.run(l, r);
            }
            if self.high_cut_on {
                (l, r) = self.high_cut.run(l, r);
            }
            if self.bands_on[0] {
                (l, r) = self.low_band.run(l, r);
            }
            if self.bands_on[1] {
                (l, r) = self.mid_band.run(l, r);
            }
            if self.bands_on[2] {
                (l, r) = self.high_band.run(l, r);
            }
            if bass_cut > 0.0 {
                let (hl, hr) = self.bass[0].run(l, r);
                let (hl, hr) = self.bass[1].run(hl, hr);
                l += (hl - l) * bass_cut;
                r += (hr - r) * bass_cut;
            }
            if width != 1.0 {
                let m = 0.5 * (l + r);
                let s = 0.5 * (l - r) * width;
                l = m + s;
                r = m - s;
            }
            let g = g0 + (g1 - g0) * (i + 1) as f32 / frames as f32;
            let (dry_l, dry_r) = (l * g * pan_l, r * g * pan_r);
            let (mut wl, mut wr) = (0f32, 0f32);
            if echo_active {
                let read = (self.echo_pos + echo_len - delay) % echo_len;
                let (el, er) = (self.echo[2 * read], self.echo[2 * read + 1]);
                // tone filter in the feedback path
                self.echo_tone[0] = el + (self.echo_tone[0] - el) * tone_a;
                self.echo_tone[1] = er + (self.echo_tone[1] - er) * tone_a;
                let (fl, fr) = if p.echo_ping_pong { (self.echo_tone[1], self.echo_tone[0]) } else { (self.echo_tone[0], self.echo_tone[1]) };
                let send = p.echo_send;
                self.echo[2 * self.echo_pos] = l * send + fl * feedback;
                self.echo[2 * self.echo_pos + 1] = r * send + fr * feedback;
                self.echo_pos = (self.echo_pos + 1) % echo_len;
                wl += el;
                wr += er;
            }
            if reverb_active {
                self.reverb_buf[2 * i] = l * p.reverb_send;
                self.reverb_buf[2 * i + 1] = r * p.reverb_send;
            }
            buf[2 * i] = dry_l + wl;
            buf[2 * i + 1] = dry_r + wr;
            tail = tail.max(wl.abs()).max(wr.abs());
        }
        if reverb_active {
            self.reverb.process(&mut self.reverb_buf[..frames * 2]);
            for i in 0..frames * 2 {
                let w = self.reverb_buf[i];
                buf[i] += w;
                tail = tail.max(w.abs());
            }
        }
        self.gain = g1;
        self.tail_level = tail;
    }
}
