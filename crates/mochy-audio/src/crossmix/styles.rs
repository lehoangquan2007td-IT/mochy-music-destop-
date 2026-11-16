//! Crossmix styles (port of `CrossmixStyles.kt`): each fills [`FxParams`]
//! for one side of the hand-over from the beat position alone, so the same
//! two songs always mix the same way.
//!
//! Desktop renders gain, high/low cuts (with resonance), low/mid/high bands,
//! the bass hand-off cut, echo, reverb, pan and width. Styles built on the
//! Android renderer's deck (backspin, tape stop, loop roll), gate, flanger,
//! bit-crush, orbit or noise riser are not ported.

use std::f32::consts::PI;

pub const OPEN_HZ: f32 = 20_000.0;

/// Every renderer knob at one moment (defaults = untouched track).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxParams {
    pub gain: f32,
    pub low_cut_hz: f32,
    pub high_cut_hz: f32,
    pub resonance: f32,
    pub low_band: f32,
    pub mid_band: f32,
    pub high_band: f32,
    /// 0 bass present, 1 gone behind a steep 200 Hz cut.
    pub bass_cut: f32,
    pub echo_send: f32,
    pub echo_beats: f32,
    pub echo_feedback: f32,
    /// 0 dark (dub), 1 bright repeats.
    pub echo_tone: f32,
    pub echo_ping_pong: bool,
    pub reverb_send: f32,
    pub reverb_size: f32,
    /// 1 holds the tail forever.
    pub reverb_freeze: f32,
    pub pan: f32,
    pub width: f32,
}

impl Default for FxParams {
    fn default() -> Self {
        FxParams {
            gain: 1.0,
            low_cut_hz: 0.0,
            high_cut_hz: OPEN_HZ,
            resonance: 0.707,
            low_band: 1.0,
            mid_band: 1.0,
            high_band: 1.0,
            bass_cut: 0.0,
            echo_send: 0.0,
            echo_beats: 0.75,
            echo_feedback: 0.45,
            echo_tone: 0.6,
            echo_ping_pong: false,
            reverb_send: 0.0,
            reverb_size: 0.84,
            reverb_freeze: 0.0,
            pan: 0.0,
            width: 1.0,
        }
    }
}

impl FxParams {
    pub fn reset(&mut self) {
        *self = FxParams::default();
    }

    /// Drift away: quieter, darker, narrower, to one side, into a room.
    pub fn recede(&mut self, distance: f32, side: f32, room: f32) {
        let d = distance.clamp(0.0, 1.0);
        if d <= 0.0 {
            return;
        }
        self.gain *= 10f32.powf(-12.0 * d.powf(1.25) / 20.0);
        self.high_cut_hz = self.high_cut_hz.min(sweep(d.powf(0.85), OPEN_HZ, 950.0));
        self.resonance += (0.62 - self.resonance) * d;
        self.mid_band *= 1.0 - 0.4 * d;
        self.width *= 1.0 - 0.8 * d;
        self.pan = (self.pan + side * 0.6 * d).clamp(-1.0, 1.0);
        self.reverb_send = self.reverb_send.max(0.3 * room * d);
    }
}

/// Which side of the transition a renderer plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Outgoing,
    Incoming,
}

/// When the planner may pick a style by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleUse {
    Everyday,
    Lift,
    Fall,
    Flavour,
}

/// The transitions desktop crossmix can play.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Style {
    SilkBlend,
    BassSwap,
    FilterSweep,
    HighPassLift,
    EchoOut,
    DubThrow,
    ReverbWash,
    VocalHandoff,
    DropCut,
    HarmonicGlide,
    BreakdownBridge,
    ThunderDrop,
    FarAway,
    Dampen,
    /// Desktop/Android plain crossfade (Auto / Manual): equal-power ramps.
    PlainFade,
}

impl Style {
    /// Styles the planner and the settings picker know (not `PlainFade`).
    pub const ALL: [Style; 14] = [
        Style::SilkBlend,
        Style::BassSwap,
        Style::FilterSweep,
        Style::HighPassLift,
        Style::EchoOut,
        Style::DubThrow,
        Style::ReverbWash,
        Style::VocalHandoff,
        Style::DropCut,
        Style::HarmonicGlide,
        Style::BreakdownBridge,
        Style::ThunderDrop,
        Style::FarAway,
        Style::Dampen,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Style::SilkBlend => "Silk blend",
            Style::BassSwap => "Bass swap",
            Style::FilterSweep => "Filter sweep",
            Style::HighPassLift => "High-pass lift",
            Style::EchoOut => "Echo out",
            Style::DubThrow => "Dub throw",
            Style::ReverbWash => "Reverb wash",
            Style::VocalHandoff => "Vocal hand-off",
            Style::DropCut => "Drop cut",
            Style::HarmonicGlide => "Harmonic glide",
            Style::BreakdownBridge => "Breakdown bridge",
            Style::ThunderDrop => "Thunder drop",
            Style::FarAway => "Fade to far",
            Style::Dampen => "Dampen",
            Style::PlainFade => "Crossfade",
        }
    }

    /// Stable id for settings (`CrossmixStyle.name`).
    pub fn id(self) -> &'static str {
        match self {
            Style::SilkBlend => "SilkBlend",
            Style::BassSwap => "BassSwap",
            Style::FilterSweep => "FilterSweep",
            Style::HighPassLift => "HighPassLift",
            Style::EchoOut => "EchoOut",
            Style::DubThrow => "DubThrow",
            Style::ReverbWash => "ReverbWash",
            Style::VocalHandoff => "VocalHandoff",
            Style::DropCut => "DropCut",
            Style::HarmonicGlide => "HarmonicGlide",
            Style::BreakdownBridge => "BreakdownBridge",
            Style::ThunderDrop => "ThunderDrop",
            Style::FarAway => "FarAway",
            Style::Dampen => "Dampen",
            Style::PlainFade => "PlainFade",
        }
    }

    pub fn from_id(id: &str) -> Option<Style> {
        Style::ALL.into_iter().find(|s| s.id() == id)
    }

    /// Overlaps on the beat.
    pub fn needs_beat_match(self) -> bool {
        matches!(self, Style::BassSwap | Style::DropCut | Style::HarmonicGlide)
    }

    /// Brings the next song in after the outgoing one is gone (a drop).
    pub fn enters_at_end(self) -> bool {
        matches!(
            self,
            Style::HighPassLift | Style::EchoOut | Style::DubThrow | Style::VocalHandoff | Style::DropCut | Style::ThunderDrop
        )
    }

    /// Lengths that suit it, in beats.
    pub fn lengths(self) -> &'static [u32] {
        match self {
            Style::SilkBlend | Style::FilterSweep | Style::HighPassLift | Style::DropCut | Style::ThunderDrop => &[8, 16],
            Style::FarAway | Style::Dampen => &[8, 16],
            Style::BassSwap => &[8, 16, 32],
            Style::EchoOut | Style::VocalHandoff => &[4, 8],
            Style::DubThrow => &[8],
            Style::ReverbWash | Style::BreakdownBridge => &[16, 24],
            Style::HarmonicGlide => &[16, 32],
            Style::PlainFade => &[16],
        }
    }

    /// Beats the outgoing side keeps ringing after the hand-over.
    pub fn tail_beats(self) -> u32 {
        match self {
            Style::EchoOut | Style::ThunderDrop => 12,
            Style::DubThrow | Style::ReverbWash => 16,
            Style::VocalHandoff | Style::FarAway => 8,
            Style::Dampen => 6,
            Style::BreakdownBridge => 4,
            _ => 0,
        }
    }

    pub fn style_use(self) -> StyleUse {
        match self {
            Style::SilkBlend
            | Style::BassSwap
            | Style::FilterSweep
            | Style::EchoOut
            | Style::VocalHandoff
            | Style::DropCut
            | Style::HarmonicGlide
            | Style::FarAway
            | Style::Dampen => StyleUse::Everyday,
            Style::HighPassLift | Style::ThunderDrop => StyleUse::Lift,
            _ => StyleUse::Flavour,
        }
    }

    /// The beat the low end changes hands and the next song lands.
    pub fn swap_at(self, length: u32) -> u32 {
        if self.enters_at_end() { length } else { length - 8.min(length / 2) }
    }

    /// Both songs really play together.
    pub fn blends_both(self) -> bool {
        !self.enters_at_end() && self != Style::FarAway && self != Style::Dampen
    }

    /// Fill `out` for one side at transition beat `beat`.
    pub fn evaluate(self, role: Role, beat: f32, length: f32, swap: f32, variation: u32, out: &mut FxParams) {
        out.reset();
        let p = (beat / length).clamp(0.0, 1.0);
        let swap_share = (swap / length).clamp(0.1, 1.0);
        let outgoing = role == Role::Outgoing;
        match self {
            Style::SilkBlend => {
                if outgoing {
                    out.gain = if beat <= swap { 1.0 } else { fade_out(ramp(beat, swap, length)) };
                    out.high_cut_hz = sweep(ramp(beat, swap, length).powf(0.8), OPEN_HZ, 1_600.0);
                } else {
                    out.gain = fade_in(ramp(beat, 0.0, swap));
                    out.low_cut_hz = entry_high_pass(p, 900.0, 0.8 * swap_share);
                }
            }
            Style::BassSwap => {
                if outgoing {
                    out.gain = if beat <= swap { 1.0 } else { fade_out(ramp(beat, swap, length)) };
                    out.high_cut_hz = sweep(ramp(p, 0.3, 1.0), OPEN_HZ, 2_200.0);
                } else {
                    out.gain = fade_in(ramp(beat, 0.0, swap));
                    out.low_cut_hz = entry_high_pass(p, 520.0, 0.9 * swap_share);
                }
            }
            Style::FilterSweep => {
                if outgoing {
                    out.high_cut_hz = sweep(p.powf(0.6), OPEN_HZ, 300.0);
                    out.resonance = 0.707 + 0.15 * (PI * p).sin();
                    out.gain = fade_out(ramp(p, 0.6, 1.0));
                } else {
                    out.low_cut_hz = entry_high_pass(p, 1_200.0, 0.6);
                    out.gain = fade_in(ramp(p, 0.0, 0.5));
                }
            }
            Style::HighPassLift => {
                if outgoing {
                    out.low_cut_hz = sweep(ramp(p, 0.0, 1.0).powf(1.4), 20.0, 1_900.0);
                    out.resonance = 0.9;
                    out.reverb_send = 0.25 * p;
                    out.gain = cut_at(beat, length);
                } else {
                    out.gain = entry(beat, length, 0.25);
                }
            }
            Style::EchoOut => {
                if outgoing {
                    out.echo_beats = 1.0;
                    out.echo_send = ramp(beat, length - 1.0, length - 0.05) * cut_at(beat, length + 0.01).max(0.0);
                    out.echo_feedback = 0.45;
                    out.echo_tone = 0.55;
                    out.high_cut_hz = sweep(ramp(beat, length - 1.0, length), OPEN_HZ, 9_000.0);
                    out.gain = cut_at(beat, length);
                } else {
                    out.gain = entry(beat, length, 0.5);
                    out.low_cut_hz = sweep(ramp(beat, length, length + 2.0), 420.0, 0.0);
                }
            }
            Style::DubThrow => {
                if outgoing {
                    out.echo_beats = 0.5;
                    out.echo_ping_pong = true;
                    out.echo_send = ramp(beat, length - 6.0, length - 1.0) * cut_at(beat, length + 0.01).max(0.0);
                    out.echo_feedback = 0.72;
                    out.echo_tone = 0.25;
                    out.reverb_send = 0.18 * ramp(beat, length - 4.0, length);
                    out.high_cut_hz = sweep(ramp(beat, length - 4.0, length), OPEN_HZ, 1_800.0);
                    out.gain = cut_at(beat, length);
                } else {
                    out.gain = entry(beat, length, 1.0);
                    out.high_cut_hz = sweep(ramp(beat, length, length + 4.0), 900.0, OPEN_HZ);
                }
            }
            Style::ReverbWash => {
                if outgoing {
                    out.reverb_send = 0.95 * smooth(p);
                    out.reverb_size = 0.9;
                    out.reverb_freeze = ramp(p, 0.8, 1.0) * 0.6;
                    out.high_cut_hz = sweep(ramp(p, 0.3, 1.0), OPEN_HZ, 2_200.0);
                    out.gain = fade_out(ramp(p, 0.15, 0.9));
                    out.reverb_send *= cut_at(beat, length + 0.01).max(0.0);
                } else {
                    out.gain = fade_in(ramp(p, 0.35, 1.0));
                    out.high_cut_hz = sweep(ramp(p, 0.35, 1.0), 1_200.0, OPEN_HZ);
                    out.reverb_send = 0.3 * (1.0 - ramp(p, 0.6, 1.0));
                }
            }
            Style::VocalHandoff => {
                if outgoing {
                    out.reverb_send = 0.7 * ramp(beat, length - 1.0, length) * cut_at(beat, length + 1.0).max(0.0);
                    out.reverb_size = 0.88;
                    out.gain = cut_at(beat, length + 0.5);
                    out.high_cut_hz = sweep(ramp(beat, length - 1.0, length + 0.5), OPEN_HZ, 3_000.0);
                } else {
                    out.gain = entry(beat, length, 1.0);
                    out.low_cut_hz = sweep(ramp(beat, length, length + 2.0), 300.0, 0.0);
                }
            }
            Style::DropCut => {
                if outgoing {
                    out.low_cut_hz = sweep(ramp(beat, length - 2.0, length), 20.0, 650.0);
                    out.gain = cut_at(beat, length);
                } else {
                    out.gain = entry(beat, length, 0.03);
                }
            }
            Style::HarmonicGlide => {
                if outgoing {
                    out.mid_band = 1.0 - 0.4 * ramp(p, 0.3, 0.7);
                    out.high_cut_hz = sweep(ramp(p, 0.4, 1.0), OPEN_HZ, 8_000.0);
                    out.gain = if beat <= swap { 1.0 } else { fade_out(ramp(beat, swap, length)) };
                } else {
                    out.mid_band = 0.5 + 0.5 * ramp(p, 0.3, 0.7);
                    out.gain = fade_in(ramp(beat, 0.0, swap));
                    out.low_cut_hz = entry_high_pass(p, 320.0, 0.8 * swap_share);
                }
            }
            Style::BreakdownBridge => {
                if outgoing {
                    out.high_cut_hz = sweep(ramp(p, 0.0, 0.35), OPEN_HZ, 480.0);
                    out.resonance = 0.85;
                    out.reverb_send = 0.3 * ramp(p, 0.2, 0.7);
                    out.gain = fade_out(ramp(p, 0.4, 0.85));
                } else {
                    out.high_cut_hz = sweep(ramp(p, 0.6, 1.0), 480.0, OPEN_HZ);
                    out.resonance = 0.85 - 0.15 * ramp(p, 0.6, 1.0);
                    out.gain = fade_in(ramp(p, 0.15, 0.55));
                }
            }
            Style::ThunderDrop => {
                if outgoing {
                    out.low_band = 1.0 - ramp(beat, length - 2.0, length - 1.0);
                    out.echo_beats = 0.75;
                    out.echo_send = 0.7 * ramp(beat, length - 2.0, length - 0.5) * cut_at(beat, length + 0.01).max(0.0);
                    out.echo_feedback = 0.55;
                    out.reverb_send = 0.45 * ramp(beat, length - 2.0, length) * cut_at(beat, length + 0.01).max(0.0);
                    out.gain = cut_at(beat, length);
                } else {
                    out.gain = entry(beat, length, 0.03);
                    out.low_band = 1.0 + 0.25 * (1.0 - ramp(beat, length, length + 1.0));
                }
            }
            Style::FarAway => {
                if outgoing {
                    out.recede(smooth(ramp(p, 0.0, 0.8)), if variation % 2 == 0 { -1.0 } else { 1.0 }, 1.0);
                    out.gain *= fade_out(ramp(p, 0.55, 1.0));
                } else {
                    out.gain = fade_in(ramp(p, 0.0, 0.3));
                    out.low_cut_hz = entry_high_pass(p, 700.0, 0.5);
                }
            }
            Style::Dampen => {
                if outgoing {
                    out.recede(0.85 * smooth(ramp(p, 0.0, 0.35)), 0.0, 0.6);
                    out.gain *= fade_out(ramp(p, 0.4, 1.0));
                } else {
                    out.gain = fade_in(ramp(p, 0.0, 0.3));
                    out.low_cut_hz = entry_high_pass(p, 500.0, 0.4);
                }
            }
            Style::PlainFade => {
                let (o, i) = crate::transition::equal_power(p);
                out.gain = if outgoing { o } else { i };
                // a plain crossfade keeps both songs whole
                if beat < 0.0 && !outgoing {
                    out.gain = 0.0;
                }
                return;
            }
        }
        // only one kick at a time: the low end changes hands over half a beat
        if !self.enters_at_end() {
            let handed = ramp(beat, swap - 0.5, swap);
            out.bass_cut = if outgoing { handed } else { 1.0 - handed };
        }
        if outgoing {
            let feed = cut_at(beat, length + 1.0);
            out.echo_send *= feed;
            out.reverb_send *= feed;
        } else if beat < 0.0 {
            out.gain = 0.0;
            out.echo_send = 0.0;
            out.reverb_send = 0.0;
        }
    }

    /// Give two singing voices bands of their own (`yieldVocals`).
    pub fn yield_vocals(self, role: Role, beat: f32, length: f32, swap: f32, amount: f32, out: &mut FxParams) {
        let a = amount.clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        let p = (beat / length).clamp(0.0, 1.0);
        if role == Role::Outgoing {
            out.high_cut_hz = out.high_cut_hz.min(sweep(a * smooth(ramp(p, 0.05, 0.5)), OPEN_HZ, 1_600.0));
            out.gain *= 1.0 - 0.25 * a * smooth(ramp(p, 0.1, 0.6));
            out.gain *= 1.0 - a * smooth(ramp(beat, swap, swap + (length - swap) * 0.5));
        } else if beat >= 0.0 {
            let open_by = (swap / length).clamp(0.1, 1.0);
            let top = 700.0 + 1_700.0 * a;
            out.low_cut_hz = out.low_cut_hz.max(sweep(ramp(p, open_by * (0.3 + 0.4 * a), open_by), top, 0.0));
        }
    }
}

/// The next song's body lifted out while it comes in, gone by `open_by`.
pub fn entry_high_pass(p: f32, top_hz: f32, open_by: f32) -> f32 {
    let remaining = 1.0 - ramp(p, 0.0, open_by);
    if remaining <= 0.0 {
        return 0.0;
    }
    20.0 * (top_hz / 20.0).powf(remaining.powf(0.35))
}

/// Two loud songs at once are brought down to the loudness of one.
pub fn headroom(own: f32, other: f32) -> f32 {
    let power = own * own + other * other;
    if power > 1.0 { 1.0 / power.sqrt() } else { 1.0 }
}

/// A 0..1 ramp from `a` to `b`.
#[inline]
pub fn ramp(x: f32, a: f32, b: f32) -> f32 {
    if b <= a {
        if x >= b { 1.0 } else { 0.0 }
    } else {
        ((x - a) / (b - a)).clamp(0.0, 1.0)
    }
}

#[inline]
pub fn smooth(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

/// Equal-power fades.
#[inline]
pub fn fade_out(x: f32) -> f32 {
    (x.clamp(0.0, 1.0) * PI / 2.0).cos()
}
#[inline]
pub fn fade_in(x: f32) -> f32 {
    (x.clamp(0.0, 1.0) * PI / 2.0).sin()
}

/// Full until `at`, gone 1/16 beat later.
#[inline]
pub fn cut_at(beat: f32, at: f32) -> f32 {
    1.0 - ramp(beat, at - 0.0625, at)
}

/// Silent until `at`, then in over `over_beats`.
#[inline]
pub fn entry(beat: f32, at: f32, over_beats: f32) -> f32 {
    fade_in(ramp(beat, at, at + over_beats))
}

/// Exponential filter sweep; a sweep to 0 Hz ends exactly there.
pub fn sweep(t: f32, from: f32, to: f32) -> f32 {
    if t <= 0.0 {
        return from;
    }
    if t >= 1.0 {
        return to;
    }
    let a = from.max(20.0);
    let b = to.max(20.0);
    let v = a * (b / a).powf(t);
    if to <= 0.0 && t >= 0.98 { 0.0 } else { v }
}
