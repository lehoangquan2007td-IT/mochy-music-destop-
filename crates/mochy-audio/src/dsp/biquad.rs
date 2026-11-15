//! RBJ biquads and the crossfading EQ cascade (spec §4.3,
//! `CustomEqualizerAudioProcessor.kt`). Coefficients come from
//! `mochy_core::eq::BiquadCoeffs::design` so the engine plays exactly the
//! curve the UI draws.

use mochy_core::eq::{BiquadCoeffs, EqSettings, FilterType, MAX_BANDS};

/// RBJ cookbook biquad, direct form I, f64 state.
#[derive(Debug, Clone, Copy, Default)]
pub struct Biquad {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    /// Design one section (spec §4.3: PK/LSC/HSC/LPQ/HPQ, RBJ cookbook).
    pub fn design(filter_type: FilterType, freq_hz: f32, gain_db: f32, q: f32, sample_rate: f32) -> Biquad {
        Self::from_coeffs(BiquadCoeffs::design(filter_type, freq_hz, gain_db, q, sample_rate))
    }

    pub fn from_coeffs(c: BiquadCoeffs) -> Biquad {
        Biquad { b0: c.b0, b1: c.b1, b2: c.b2, a1: c.a1, a2: c.a2, ..Default::default() }
    }

    pub fn coeffs(&self) -> BiquadCoeffs {
        BiquadCoeffs { b0: self.b0, b1: self.b1, b2: self.b2, a1: self.a1, a2: self.a2 }
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    /// Process one sample.
    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// A complete, allocation-free EQ design: up to `MAX_BANDS` sections plus
/// the preamp gain. `Copy`, so it can be handed to the audio callback.
#[derive(Debug, Clone, Copy)]
pub struct EqDesign {
    pub coeffs: [BiquadCoeffs; MAX_BANDS],
    pub count: usize,
    /// Linear preamp (effective preamp incl. auto headroom).
    pub gain: f64,
}

impl EqDesign {
    pub const fn identity() -> Self {
        EqDesign { coeffs: [BiquadCoeffs::IDENTITY; MAX_BANDS], count: 0, gain: 1.0 }
    }

    /// Design from the Audio Lab settings at `sample_rate` (spec §4.3).
    /// Returns `None` (straight wire) when the EQ is disabled, has no
    /// enabled nodes, or nothing audible remains and the preamp is 0 dB.
    pub fn from_settings(eq: &EqSettings, sample_rate: f32) -> Option<EqDesign> {
        if !eq.enabled {
            return None;
        }
        let mut active = mochy_core::eq::active_nodes(eq).peekable();
        active.peek()?;
        let mut d = EqDesign::identity();
        for n in active {
            // Android `Biquad.isAudible`: a 0 dB peak/shelf is a straight line.
            let audible = match n.filter_type {
                FilterType::LowPass | FilterType::HighPass => true,
                _ => n.gain_db.abs() >= 0.05,
            };
            if !audible || n.frequency_hz >= sample_rate / 2.0 || d.count >= MAX_BANDS {
                continue;
            }
            d.coeffs[d.count] = BiquadCoeffs::design(n.filter_type, n.frequency_hz, n.gain_db, n.q, sample_rate);
            d.count += 1;
        }
        let pre_db = mochy_core::eq::effective_preamp_db(eq) as f64;
        d.gain = 10f64.powf(pre_db / 20.0);
        if d.count == 0 && (d.gain - 1.0).abs() < 1e-9 {
            return None;
        }
        Some(d)
    }

    /// Combined magnitude (incl. preamp) in dB at `freq_hz`.
    pub fn magnitude_db(&self, freq_hz: f32, sample_rate: f32) -> f32 {
        let filters: f32 = self.coeffs[..self.count].iter().map(|c| c.magnitude_db(freq_hz, sample_rate)).sum();
        filters + (20.0 * self.gain.max(1e-12).log10()) as f32
    }
}

const STEREO: usize = 2;

/// Biquad cascade + preamp with per-band, per-channel state (stereo).
#[derive(Clone, Copy)]
struct FilterBank {
    design: EqDesign,
    /// [band * 2 + ch] -> (x1, x2, y1, y2)
    state: [[f64; 4]; MAX_BANDS * STEREO],
}

impl FilterBank {
    const fn new(design: EqDesign) -> Self {
        FilterBank { design, state: [[0.0; 4]; MAX_BANDS * STEREO] }
    }

    fn is_identity(&self) -> bool {
        self.design.count == 0 && self.design.gain == 1.0
    }

    /// Start from the filters this replaces, band for band, so the sound carries on.
    fn adopt_state(&mut self, prev: &FilterBank) {
        let shared = self.design.count.min(prev.design.count) * STEREO;
        self.state[..shared].copy_from_slice(&prev.state[..shared]);
    }

    fn reset(&mut self) {
        self.state = [[0.0; 4]; MAX_BANDS * STEREO];
    }

    #[inline]
    fn process(&mut self, buf: &mut [f32]) {
        if self.is_identity() {
            return;
        }
        let n = self.design.count;
        let g = self.design.gain;
        for frame in buf.as_chunks_mut::<STEREO>().0 {
            for (ch, s) in frame.iter_mut().enumerate() {
                let mut x = *s as f64;
                for b in 0..n {
                    let c = &self.design.coeffs[b];
                    let st = &mut self.state[b * STEREO + ch];
                    let y = c.b0 * x + c.b1 * st[0] + c.b2 * st[1] - c.a1 * st[2] - c.a2 * st[3];
                    st[1] = st[0];
                    st[0] = x;
                    st[3] = st[2];
                    // flush denormals so silent tails don't cost CPU
                    st[2] = if y.abs() < 1e-30 { 0.0 } else { y };
                    x = y;
                }
                *s = (x * g) as f32;
            }
        }
    }
}

/// Stereo EQ processor: a new design crossfades in over 30 ms
/// (`FadeSeconds = 0.03`) without resetting or re-seeking (spec §4.3).
/// Allocation-free after construction.
pub struct EqProcessor {
    sample_rate: u32,
    bank: FilterBank,
    from: FilterBank,
    fading: bool,
    fade_pos: usize,
    fade_len: usize,
    scratch: Vec<f32>,
}

/// Largest block processed in one go (frames); longer buffers are split.
pub(crate) const MAX_BLOCK_FRAMES: usize = 2048;

impl EqProcessor {
    pub fn new(sample_rate: u32, initial: Option<EqDesign>) -> Self {
        EqProcessor {
            sample_rate,
            bank: FilterBank::new(initial.unwrap_or(EqDesign::identity())),
            from: FilterBank::new(EqDesign::identity()),
            fading: false,
            fade_pos: 0,
            fade_len: 1,
            scratch: vec![0.0; MAX_BLOCK_FRAMES * STEREO],
        }
    }

    /// Replace the design; crossfades from what is playing.
    pub fn set_design(&mut self, design: Option<EqDesign>) {
        let mut next = FilterBank::new(design.unwrap_or(EqDesign::identity()));
        if next.is_identity() && self.bank.is_identity() && !self.fading {
            return;
        }
        next.adopt_state(&self.bank);
        // mid-fade, the sound being faded in becomes the one faded from
        self.from = self.bank;
        self.bank = next;
        self.fading = true;
        self.fade_pos = 0;
        self.fade_len = ((self.sample_rate as f64 * 0.03) as usize).max(1);
    }

    pub fn is_bypassed(&self) -> bool {
        !self.fading && self.bank.is_identity()
    }

    /// After a seek: filters start clean, no fade.
    pub fn reset(&mut self) {
        self.bank.reset();
        self.from.reset();
        self.fading = false;
    }

    /// Process stereo interleaved samples in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        for block in buf.chunks_mut(MAX_BLOCK_FRAMES * STEREO) {
            self.process_block(block);
        }
    }

    fn process_block(&mut self, buf: &mut [f32]) {
        if !self.fading {
            self.bank.process(buf);
            return;
        }
        let n = buf.len();
        let old = &mut self.scratch[..n];
        old.copy_from_slice(buf);
        self.from.process(old);
        self.bank.process(buf);
        let len = self.fade_len as f32;
        for (frame, old_frame) in buf.as_chunks_mut::<STEREO>().0.iter_mut().zip(old.as_chunks::<STEREO>().0) {
            let g = if self.fade_pos >= self.fade_len { 1.0 } else { self.fade_pos as f32 / len };
            for (s, o) in frame.iter_mut().zip(old_frame) {
                *s = *o + (*s - *o) * g;
            }
            if self.fade_pos < self.fade_len {
                self.fade_pos += 1;
            }
        }
        if self.fade_pos >= self.fade_len {
            self.fading = false;
        }
    }
}
