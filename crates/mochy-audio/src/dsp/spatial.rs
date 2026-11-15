//! Headphone spatial audio — port of Android `SpatialAudioProcessor.kt`
//! (spec §4.6): bs2b crossfeed (cutoff 700 Hz, feed 4.5 dB), virtual
//! speakers via Brown & Duda's spherical head (radius 0.0875 m, c = 343,
//! αmin 0.1, θmin 150°, speakers ±30°, gain 0.6), mono fold-down, plus a
//! mid/side stereo width control (desktop addition). Every element glides
//! over 0.15 s so toggles and mode switches never click; a soft knee above
//! 0.9 guards the output while anything is active. Multichannel sources are
//! rendered to two ears by the decoder with the same head model
//! ([`HeadRenderer`], gain 0.45) when `surround` is on.

use super::loudness::soft_knee;

const TOGGLE_SECONDS: f32 = 0.15;
const SPEAKER_GAIN: f32 = 0.6;
pub(crate) const SURROUND_GAIN: f32 = 0.45;
const KNEE: f32 = 0.9;

/// Copyable spatial parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpatialParams {
    pub crossfeed: bool,
    pub cutoff_hz: f32,
    pub feed_db: f32,
    pub speakers: bool,
    pub speaker_angle_deg: f32,
    pub surround: bool,
    pub mono: bool,
    pub width: f32,
}

impl Default for SpatialParams {
    fn default() -> Self {
        Self::from_settings(&mochy_core::settings::SpatialSettings::default())
    }
}

impl SpatialParams {
    pub fn from_settings(s: &mochy_core::settings::SpatialSettings) -> Self {
        SpatialParams {
            crossfeed: s.crossfeed_enabled,
            cutoff_hz: s.crossfeed_cutoff_hz,
            feed_db: s.crossfeed_feed_db,
            speakers: s.virtual_speakers,
            speaker_angle_deg: s.speaker_angle_deg,
            surround: s.surround,
            mono: s.mono,
            width: s.width,
        }
    }
}

/// bs2b crossfeed; state carries over when the settings move.
#[derive(Clone, Copy)]
struct Crossfeed {
    cutoff: f32,
    feed: f32,
    a0_lo: f32,
    b1_lo: f32,
    a0_hi: f32,
    a1_hi: f32,
    b1_hi: f32,
    gain: f32,
    lo_l: f32,
    lo_r: f32,
    hi_l: f32,
    hi_r: f32,
    asis_l: f32,
    asis_r: f32,
}

impl Crossfeed {
    fn new(sample_rate: u32) -> Self {
        let mut c = Crossfeed {
            cutoff: f32::NAN,
            feed: f32::NAN,
            a0_lo: 0.0,
            b1_lo: 0.0,
            a0_hi: 1.0,
            a1_hi: 0.0,
            b1_hi: 0.0,
            gain: 1.0,
            lo_l: 0.0,
            lo_r: 0.0,
            hi_l: 0.0,
            hi_r: 0.0,
            asis_l: 0.0,
            asis_r: 0.0,
        };
        c.configure(sample_rate, 700.0, 4.5);
        c
    }

    fn configure(&mut self, sample_rate: u32, cutoff: f32, feed: f32) {
        if self.cutoff == cutoff && self.feed == feed {
            return;
        }
        self.cutoff = cutoff;
        self.feed = feed;
        let sr = sample_rate.max(1) as f64;
        let fc = (cutoff as f64).clamp(300.0, 2000.0);
        let fd = (feed as f64).clamp(1.0, 15.0);
        let gb_lo = fd * -5.0 / 6.0 - 3.0;
        let gb_hi = fd / 6.0 - 3.0;
        let g_lo = 10f64.powf(gb_lo / 20.0);
        let g_hi = 1.0 - 10f64.powf(gb_hi / 20.0);
        let fc_hi = fc * 2f64.powf((gb_lo - 20.0 * g_hi.log10()) / 12.0);
        let x_lo = (-2.0 * std::f64::consts::PI * fc / sr).exp();
        self.b1_lo = x_lo as f32;
        self.a0_lo = (g_lo * (1.0 - x_lo)) as f32;
        let x_hi = (-2.0 * std::f64::consts::PI * fc_hi / sr).exp();
        self.b1_hi = x_hi as f32;
        self.a0_hi = (1.0 - g_hi * (1.0 - x_hi)) as f32;
        self.a1_hi = (-x_hi) as f32;
        self.gain = (1.0 / (1.0 - g_hi + g_lo)) as f32;
    }

    #[inline]
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        self.lo_l = self.a0_lo * l + self.b1_lo * self.lo_l;
        self.lo_r = self.a0_lo * r + self.b1_lo * self.lo_r;
        self.hi_l = self.a0_hi * l + self.a1_hi * self.asis_l + self.b1_hi * self.hi_l;
        self.hi_r = self.a0_hi * r + self.a1_hi * self.asis_r + self.b1_hi * self.hi_r;
        self.asis_l = l;
        self.asis_r = r;
        ((self.hi_l + self.lo_r) * self.gain, (self.hi_r + self.lo_l) * self.gain)
    }

    fn clear(&mut self) {
        self.lo_l = 0.0;
        self.lo_r = 0.0;
        self.hi_l = 0.0;
        self.hi_r = 0.0;
        self.asis_l = 0.0;
        self.asis_r = 0.0;
    }
}

const DELAY_SIZE: usize = 64;
pub(crate) const MAX_SOURCES: usize = 8;
const HEAD_RADIUS: f64 = 0.0875;
const SPEED_OF_SOUND: f64 = 343.0;
const ALPHA_MIN: f64 = 0.1;
const THETA_MIN: f64 = 150.0 * std::f64::consts::PI / 180.0;

/// Brown & Duda spherical head: per source and ear a Woodworth time delay
/// and a first-order head-shadow shelf. Azimuths in degrees (0 ahead,
/// positive right, NaN = LFE).
pub(crate) struct HeadRenderer {
    sample_rate: u32,
    n: usize,
    line: [[f32; DELAY_SIZE]; MAX_SOURCES],
    write: usize,
    delay: [f32; MAX_SOURCES * 2],
    b0: [f32; MAX_SOURCES * 2],
    b1: [f32; MAX_SOURCES * 2],
    a1: [f32; MAX_SOURCES * 2],
    x1: [f32; MAX_SOURCES * 2],
    y1: [f32; MAX_SOURCES * 2],
    weight: [f32; MAX_SOURCES],
    azimuths: [f32; MAX_SOURCES],
}

impl HeadRenderer {
    pub(crate) fn new(sample_rate: u32, azimuths: &[f32]) -> Self {
        let mut h = HeadRenderer {
            sample_rate,
            n: 0,
            line: [[0.0; DELAY_SIZE]; MAX_SOURCES],
            write: 0,
            delay: [0.0; MAX_SOURCES * 2],
            b0: [1.0; MAX_SOURCES * 2],
            b1: [0.0; MAX_SOURCES * 2],
            a1: [0.0; MAX_SOURCES * 2],
            x1: [0.0; MAX_SOURCES * 2],
            y1: [0.0; MAX_SOURCES * 2],
            weight: [0.0; MAX_SOURCES],
            azimuths: [f32::INFINITY; MAX_SOURCES],
        };
        h.configure(azimuths);
        h
    }

    /// New positions; delay lines and filter memories carry over.
    #[allow(clippy::needless_range_loop)]
    pub(crate) fn configure(&mut self, azimuths: &[f32]) {
        let n = azimuths.len().min(MAX_SOURCES);
        let same = n == self.n
            && azimuths.iter().zip(self.azimuths.iter()).all(|(a, b)| a == b || (a.is_nan() && b.is_nan()));
        if same {
            return;
        }
        self.n = n;
        let w0 = SPEED_OF_SOUND / HEAD_RADIUS;
        let k = 2.0 * self.sample_rate.max(1) as f64;
        let mut raw = [0.0f64; MAX_SOURCES * 2];
        for s in 0..n {
            let az = azimuths[s];
            self.azimuths[s] = az;
            self.weight[s] = if az.is_nan() {
                0.5
            } else if az.abs() < 1.0 {
                0.71
            } else {
                1.0
            };
            for ear in 0..2 {
                let i = s * 2 + ear;
                if az.is_nan() {
                    self.b0[i] = 1.0;
                    self.b1[i] = 0.0;
                    self.a1[i] = 0.0;
                    continue;
                }
                let ear_az = if ear == 0 { -90.0 } else { 90.0 };
                let theta = wrap(az as f64 - ear_az).abs().to_radians();
                let alpha = (1.0 + ALPHA_MIN / 2.0) + (1.0 - ALPHA_MIN / 2.0) * (theta / THETA_MIN * std::f64::consts::PI).cos();
                let norm = 2.0 * w0 + k;
                self.b0[i] = ((2.0 * w0 + alpha * k) / norm) as f32;
                self.b1[i] = ((2.0 * w0 - alpha * k) / norm) as f32;
                self.a1[i] = ((2.0 * w0 - k) / norm) as f32;
                raw[i] = if theta < std::f64::consts::FRAC_PI_2 {
                    -HEAD_RADIUS / SPEED_OF_SOUND * theta.cos()
                } else {
                    HEAD_RADIUS / SPEED_OF_SOUND * (theta - std::f64::consts::FRAC_PI_2)
                };
            }
        }
        let earliest = (0..n * 2)
            .filter(|i| !azimuths[i / 2].is_nan())
            .map(|i| raw[i])
            .fold(f64::INFINITY, f64::min);
        let earliest = if earliest.is_finite() { earliest } else { 0.0 };
        for i in 0..n * 2 {
            self.delay[i] = if azimuths[i / 2].is_nan() {
                0.0
            } else {
                (((raw[i] - earliest) * self.sample_rate as f64) as f32).clamp(0.0, (DELAY_SIZE - 2) as f32)
            };
        }
    }

    pub(crate) fn clear(&mut self) {
        self.line = [[0.0; DELAY_SIZE]; MAX_SOURCES];
        self.x1 = [0.0; MAX_SOURCES * 2];
        self.y1 = [0.0; MAX_SOURCES * 2];
    }

    /// Render one frame of `n` source samples to (left, right).
    #[inline]
    pub(crate) fn render(&mut self, frame: &[f32]) -> (f32, f32) {
        let mut l = 0.0;
        let mut r = 0.0;
        for s in 0..self.n {
            self.line[s][self.write] = frame.get(s).copied().unwrap_or(0.0);
            for ear in 0..2 {
                let i = s * 2 + ear;
                let d = self.delay[i];
                let whole = d as usize;
                let frac = d - whole as f32;
                let p0 = (self.write + DELAY_SIZE - whole) % DELAY_SIZE;
                let p1 = if p0 == 0 { DELAY_SIZE - 1 } else { p0 - 1 };
                let x = self.line[s][p0] + (self.line[s][p1] - self.line[s][p0]) * frac;
                let y = self.b0[i] * x + self.b1[i] * self.x1[i] - self.a1[i] * self.y1[i];
                self.x1[i] = x;
                self.y1[i] = y;
                if ear == 0 {
                    l += y * self.weight[s];
                } else {
                    r += y * self.weight[s];
                }
            }
        }
        self.write = if self.write + 1 == DELAY_SIZE { 0 } else { self.write + 1 };
        (l, r)
    }
}

fn wrap(deg: f64) -> f64 {
    let mut d = deg % 360.0;
    if d > 180.0 {
        d -= 360.0;
    }
    if d < -180.0 {
        d += 360.0;
    }
    d
}

#[inline]
fn glide(value: f32, target: f32, step: f32) -> f32 {
    if target > value { (value + step).min(target) } else { (value - step).max(target) }
}

/// Stereo spatial processor. Allocation-free.
pub struct Spatial {
    sample_rate: u32,
    params: SpatialParams,
    crossfeed: Crossfeed,
    head: HeadRenderer,
    cf_amount: f32,
    sp_amount: f32,
    mono_amount: f32,
    width: f32,
}

impl Spatial {
    pub fn new(sample_rate: u32, params: SpatialParams) -> Self {
        let angle = params.speaker_angle_deg.clamp(5.0, 90.0);
        let mut s = Spatial {
            sample_rate,
            params,
            crossfeed: Crossfeed::new(sample_rate),
            head: HeadRenderer::new(sample_rate, &[-angle, angle]),
            cf_amount: 0.0,
            sp_amount: 0.0,
            mono_amount: 0.0,
            width: 1.0,
        };
        s.reset();
        s
    }

    pub fn set_params(&mut self, params: SpatialParams) {
        self.params = params;
    }

    fn targets(&self) -> (f32, f32, f32, f32) {
        let p = &self.params;
        let sp = if p.speakers { 1.0 } else { 0.0 };
        let cf = if p.crossfeed && !p.speakers { 1.0 } else { 0.0 };
        let mono = if p.mono { 1.0 } else { 0.0 };
        (cf, sp, mono, p.width.clamp(0.0, 2.0))
    }

    pub fn reset(&mut self) {
        self.crossfeed.clear();
        self.head.clear();
        let (cf, sp, mono, width) = self.targets();
        self.cf_amount = cf;
        self.sp_amount = sp;
        self.mono_amount = mono;
        self.width = width;
    }

    pub fn is_bypassed(&self) -> bool {
        let (cf, sp, mono, width) = self.targets();
        cf == 0.0
            && sp == 0.0
            && mono == 0.0
            && width == 1.0
            && self.cf_amount == 0.0
            && self.sp_amount == 0.0
            && self.mono_amount == 0.0
            && self.width == 1.0
    }

    /// Process stereo interleaved samples in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        if self.is_bypassed() {
            return;
        }
        let p = self.params;
        let (cf_t, sp_t, mono_t, width_t) = self.targets();
        let step = 1.0 / (self.sample_rate.max(1) as f32 * TOGGLE_SECONDS);
        self.crossfeed.configure(self.sample_rate, p.cutoff_hz, p.feed_db);
        let angle = p.speaker_angle_deg.clamp(5.0, 90.0);
        self.head.configure(&[-angle, angle]);
        for frame in buf.as_chunks_mut::<2>().0 {
            let mut l = frame[0];
            let mut r = frame[1];
            self.mono_amount = glide(self.mono_amount, mono_t, step);
            if self.mono_amount > 0.0 {
                let m = (l + r) * 0.5;
                l += (m - l) * self.mono_amount;
                r += (m - r) * self.mono_amount;
            }
            self.width = glide(self.width, width_t, step * 2.0);
            if self.width != 1.0 {
                let m = (l + r) * 0.5;
                let s = (l - r) * 0.5 * self.width;
                l = m + s;
                r = m - s;
            }
            self.cf_amount = glide(self.cf_amount, cf_t, step);
            if self.cf_amount > 0.0 {
                let (cl, cr) = self.crossfeed.process(l, r);
                l += (cl - l) * self.cf_amount;
                r += (cr - r) * self.cf_amount;
            }
            self.sp_amount = glide(self.sp_amount, sp_t, step);
            if self.sp_amount > 0.0 {
                let (hl, hr) = self.head.render(&[l, r]);
                l += (hl * SPEAKER_GAIN - l) * self.sp_amount;
                r += (hr * SPEAKER_GAIN - r) * self.sp_amount;
            }
            frame[0] = soft_knee(l, KNEE);
            frame[1] = soft_knee(r, KNEE);
        }
        if self.cf_amount == 0.0 {
            self.crossfeed.clear();
        }
        if self.sp_amount == 0.0 {
            self.head.clear();
        }
    }
}
