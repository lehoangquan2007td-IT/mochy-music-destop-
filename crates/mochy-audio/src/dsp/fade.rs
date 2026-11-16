//! Transport fader: the volume fades of pause / play / skip / seek
//! (`TransportTransitions.kt`: `level = from + (to − from)·smoothstep(p)`),
//! run per sample inside the output callback. Allocation-free.

use crate::transition::smoothstep;

/// A smoothstep ramp between two levels, stepped one frame at a time.
#[derive(Debug, Clone, Copy)]
pub struct Fader {
    from: f32,
    to: f32,
    /// Frames into the current ramp.
    pos: u32,
    /// Ramp length in frames (0 = settled at `to`).
    len: u32,
    /// Only advance the ramp on frames carrying real audio (a skip's swell
    /// waits for the new song to be heard, `awaitNewSong`; an underrun
    /// mid-swell doesn't eat into it).
    wait: bool,
    level: f32,
}

impl Default for Fader {
    fn default() -> Self {
        Fader { from: 1.0, to: 1.0, pos: 0, len: 0, wait: false, level: 1.0 }
    }
}

impl Fader {
    /// Current gain.
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Settled (no ramp running) at the target.
    pub fn is_settled(&self) -> bool {
        self.pos >= self.len && !self.wait
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    /// Jump to `level` at once.
    pub fn set(&mut self, level: f32) {
        let l = level.clamp(0.0, 1.0);
        *self = Fader { from: l, to: l, pos: 0, len: 0, wait: false, level: l };
    }

    /// Ramp from the current level to `target` over `frames`.
    pub fn ramp_to(&mut self, target: f32, frames: u32, wait_for_audio: bool) {
        let t = target.clamp(0.0, 1.0);
        self.from = self.level;
        self.to = t;
        self.pos = 0;
        self.len = frames;
        self.wait = wait_for_audio;
        if frames == 0 {
            self.level = t;
        }
    }

    /// Gain for the next frame; `audible` = this frame carries real audio
    /// (not an underrun / paused silence).
    #[inline]
    pub fn next(&mut self, audible: bool) -> f32 {
        if self.pos < self.len {
            if self.wait && !audible {
                return self.level;
            }
            self.pos += 1;
            let p = self.pos as f32 / self.len as f32;
            self.level = self.from + (self.to - self.from) * smoothstep(p);
        } else {
            self.level = self.to;
            self.wait = false;
        }
        self.level
    }

    /// Apply to an interleaved stereo block whose first `audible` frames are
    /// real audio.
    pub fn process(&mut self, buf: &mut [f32], audible: usize) {
        if self.is_settled() && self.level == 1.0 {
            return;
        }
        for (i, f) in buf.chunks_exact_mut(2).enumerate() {
            let g = self.next(i < audible);
            f[0] *= g;
            f[1] *= g;
        }
    }
}
