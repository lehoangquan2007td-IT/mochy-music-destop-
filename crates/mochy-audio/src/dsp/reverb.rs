//! Freeverb-style room — port of Android `ReverbAudioProcessor.kt`
//! (spec §4.2/§4.6): eight damped combs into four allpasses per side (the
//! right detuned by 23 samples for width), a pre-delay (≤ 150 ms) and a
//! one-pole low cut on the send so the bass stays dry. Gains glide across
//! each buffer so dragging never zips; enable/disable ramps over 0.15 s;
//! soft knee above 0.89 on the output while active.

use super::loudness::soft_knee;

const COMB_TUNING: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNING: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;
const FIXED_GAIN: f32 = 0.015;
const SCALE_WET: f32 = 3.0;
const SCALE_DAMP: f32 = 0.4;
const SCALE_ROOM: f32 = 0.28;
const OFFSET_ROOM: f32 = 0.7;
const MAX_PRE_DELAY_MS: f32 = 150.0;
const TOGGLE_SECONDS: f32 = 0.15;
const ANTI_DENORMAL: f32 = 1e-18;
const KNEE: f32 = 0.89;

/// Copyable reverb parameters (values 0..1 except the two in real units).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReverbParams {
    pub enabled: bool,
    pub mix: f32,
    pub room_size: f32,
    pub damping: f32,
    pub wet: f32,
    pub dry: f32,
    pub width: f32,
    pub pre_delay_ms: f32,
    pub low_cut_hz: f32,
}

impl Default for ReverbParams {
    fn default() -> Self {
        Self::from_settings(&mochy_core::settings::ReverbSettings::default())
    }
}

impl ReverbParams {
    pub fn from_settings(s: &mochy_core::settings::ReverbSettings) -> Self {
        ReverbParams {
            enabled: s.enabled,
            mix: s.mix,
            room_size: s.room_size,
            damping: s.damping,
            wet: s.wet,
            dry: s.dry,
            width: s.width,
            pre_delay_ms: s.pre_delay_ms,
            low_cut_hz: s.low_cut_hz,
        }
    }
}

struct Comb {
    line: Vec<f32>,
    index: usize,
    store: f32,
    feedback: f32,
    damp1: f32,
    damp2: f32,
}

impl Comb {
    fn new(size: usize) -> Self {
        Comb { line: vec![0.0; size.max(1)], index: 0, store: 0.0, feedback: 0.84, damp1: 0.2, damp2: 0.8 }
    }
    fn set(&mut self, feedback: f32, damp: f32) {
        self.feedback = feedback;
        self.damp1 = damp;
        self.damp2 = 1.0 - damp;
    }
    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let out = self.line[self.index];
        self.store = out * self.damp2 + self.store * self.damp1;
        self.line[self.index] = input + self.store * self.feedback;
        self.index += 1;
        if self.index == self.line.len() {
            self.index = 0;
        }
        out
    }
    fn clear(&mut self) {
        self.line.fill(0.0);
        self.store = 0.0;
    }
}

struct Allpass {
    line: Vec<f32>,
    index: usize,
}

impl Allpass {
    fn new(size: usize) -> Self {
        Allpass { line: vec![0.0; size.max(1)], index: 0 }
    }
    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let buffered = self.line[self.index];
        self.line[self.index] = input + buffered * 0.5;
        self.index += 1;
        if self.index == self.line.len() {
            self.index = 0;
        }
        buffered - input
    }
    fn clear(&mut self) {
        self.line.fill(0.0);
    }
}

/// Stereo Freeverb. Allocation-free after construction.
pub struct Reverb {
    sample_rate: u32,
    params: ReverbParams,
    combs_l: Vec<Comb>,
    combs_r: Vec<Comb>,
    allpass_l: Vec<Allpass>,
    allpass_r: Vec<Allpass>,
    pre_delay: Vec<f32>,
    pre_index: usize,
    low_cut_state: f32,
    low_cut_input: f32,
    amount: f32,
    wet1: f32,
    wet2: f32,
    dry_gain: f32,
    mix: f32,
    room_empty: bool,
}

impl Reverb {
    pub fn new(sample_rate: u32, params: ReverbParams) -> Self {
        let k = sample_rate.max(1) as f32 / 44_100.0;
        let sz = |n: usize| (n as f32 * k).round() as usize;
        let mut r = Reverb {
            sample_rate,
            params,
            combs_l: COMB_TUNING.iter().map(|&n| Comb::new(sz(n))).collect(),
            combs_r: COMB_TUNING.iter().map(|&n| Comb::new(sz(n + STEREO_SPREAD))).collect(),
            allpass_l: ALLPASS_TUNING.iter().map(|&n| Allpass::new(sz(n))).collect(),
            allpass_r: ALLPASS_TUNING.iter().map(|&n| Allpass::new(sz(n + STEREO_SPREAD))).collect(),
            pre_delay: vec![0.0; (sample_rate as f32 * MAX_PRE_DELAY_MS / 1000.0).round() as usize + 2],
            pre_index: 0,
            low_cut_state: 0.0,
            low_cut_input: 0.0,
            amount: 0.0,
            wet1: 0.0,
            wet2: 0.0,
            dry_gain: 1.0,
            mix: 0.0,
            room_empty: false,
        };
        r.reset();
        r
    }

    pub fn set_params(&mut self, params: ReverbParams) {
        self.params = params;
    }

    fn targets(&self) -> (f32, f32, f32, f32) {
        let p = &self.params;
        let width = p.width.clamp(0.0, 1.0);
        let wet = p.wet.clamp(0.0, 1.0) * SCALE_WET;
        (wet * (width / 2.0 + 0.5), wet * ((1.0 - width) / 2.0), p.dry.clamp(0.0, 1.0), p.mix.clamp(0.0, 1.0))
    }

    /// After a seek or a new track: the old tail belongs to music that's gone.
    pub fn reset(&mut self) {
        self.clear_room();
        self.amount = if self.params.enabled { 1.0 } else { 0.0 };
        let (w1, w2, d, m) = self.targets();
        self.wet1 = w1;
        self.wet2 = w2;
        self.dry_gain = d;
        self.mix = m;
    }

    fn clear_room(&mut self) {
        self.combs_l.iter_mut().for_each(Comb::clear);
        self.combs_r.iter_mut().for_each(Comb::clear);
        self.allpass_l.iter_mut().for_each(Allpass::clear);
        self.allpass_r.iter_mut().for_each(Allpass::clear);
        self.pre_delay.fill(0.0);
        self.low_cut_state = 0.0;
        self.low_cut_input = 0.0;
        self.room_empty = true;
    }

    pub fn is_bypassed(&self) -> bool {
        !self.params.enabled && self.amount == 0.0
    }

    /// Process stereo interleaved samples in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        let frames = buf.len() / 2;
        if frames == 0 {
            return;
        }
        let target = if self.params.enabled { 1.0 } else { 0.0 };
        if target == 0.0 && self.amount == 0.0 {
            if !self.room_empty {
                self.clear_room();
            }
            return;
        }
        self.room_empty = false;
        let p = self.params;
        let sr = self.sample_rate.max(1) as f32;
        let feedback = p.room_size.clamp(0.0, 1.0) * SCALE_ROOM + OFFSET_ROOM;
        let damp = p.damping.clamp(0.0, 1.0) * SCALE_DAMP;
        for c in self.combs_l.iter_mut().chain(self.combs_r.iter_mut()) {
            c.set(feedback, damp);
        }
        let delay_frames = ((p.pre_delay_ms.clamp(0.0, MAX_PRE_DELAY_MS) / 1000.0 * sr).round() as usize)
            .min(self.pre_delay.len() - 1);
        let low_cut = (-2.0 * std::f64::consts::PI * p.low_cut_hz.clamp(10.0, 2000.0) as f64 / sr as f64).exp() as f32;
        let (tw1, tw2, td, tm) = self.targets();
        let step = 1.0 / frames as f32;
        let toggle_step = 1.0 / (sr * TOGGLE_SECONDS);
        let (dw1, dw2, dd, dm) =
            ((tw1 - self.wet1) * step, (tw2 - self.wet2) * step, (td - self.dry_gain) * step, (tm - self.mix) * step);
        let plen = self.pre_delay.len();

        for frame in buf.as_chunks_mut::<2>().0 {
            let in_l = frame[0];
            let in_r = frame[1];
            self.amount = if target > self.amount {
                (self.amount + toggle_step).min(target)
            } else {
                (self.amount - toggle_step).max(target)
            };
            self.wet1 += dw1;
            self.wet2 += dw2;
            self.dry_gain += dd;
            self.mix += dm;

            // what goes into the room: mono, pre-delayed, low-cut
            let send = (in_l + in_r) * FIXED_GAIN;
            self.pre_delay[self.pre_index] = send;
            let read = (self.pre_index + plen - delay_frames) % plen;
            self.pre_index += 1;
            if self.pre_index == plen {
                self.pre_index = 0;
            }
            let delayed = self.pre_delay[read];
            self.low_cut_state = low_cut * (self.low_cut_state + delayed - self.low_cut_input);
            self.low_cut_input = delayed;
            let input = self.low_cut_state + ANTI_DENORMAL;

            let mut acc_l = 0.0;
            let mut acc_r = 0.0;
            for c in self.combs_l.iter_mut() {
                acc_l += c.process(input);
            }
            for a in self.allpass_l.iter_mut() {
                acc_l = a.process(acc_l);
            }
            for c in self.combs_r.iter_mut() {
                acc_r += c.process(input);
            }
            for a in self.allpass_r.iter_mut() {
                acc_r = a.process(acc_r);
            }
            let g = self.amount * self.mix;
            let room_l = acc_l * self.wet1 + acc_r * self.wet2 + in_l * self.dry_gain;
            let room_r = acc_r * self.wet1 + acc_l * self.wet2 + in_r * self.dry_gain;
            frame[0] = soft_knee(in_l + (room_l - in_l) * g, KNEE);
            frame[1] = soft_knee(in_r + (room_r - in_r) * g, KNEE);
        }
        self.wet1 = tw1;
        self.wet2 = tw2;
        self.dry_gain = td;
        self.mix = tm;
    }
}
