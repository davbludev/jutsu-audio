//! The note lifecycle every synth speaks, and the small pieces of DSP state a
//! polyphonic voice needs.
//!
//! Events are sample-accurate: each carries the frame *inside the block* it
//! takes effect on, so a note landing mid-buffer is not rounded to the block
//! boundary. Rendering never allocates and never reads a clock — the same
//! events at the same rate always produce the same samples, which is what lets
//! offline export match real-time playback.

use serde::{Deserialize, Serialize};

/// What happens to a note.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NoteEventKind {
    /// Starts a note. `pitch_hz` rather than a MIDI number: a synth should not
    /// have to know about tuning to make a sound.
    NoteOn { pitch_hz: f64, velocity: f32 },
    /// Releases the note at this pitch. Releasing a note that is not sounding
    /// is legal and does nothing.
    NoteOff { pitch_hz: f64 },
    /// Releases everything, for a transport stop or a loop wrap.
    AllNotesOff,
}

/// One event and where in the block it lands.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct NoteEvent {
    /// Frames from the start of the block being rendered.
    pub frame_offset: usize,
    pub kind: NoteEventKind,
}

impl NoteEvent {
    #[must_use]
    pub const fn note_on(frame_offset: usize, pitch_hz: f64, velocity: f32) -> Self {
        Self {
            frame_offset,
            kind: NoteEventKind::NoteOn { pitch_hz, velocity },
        }
    }

    #[must_use]
    pub const fn note_off(frame_offset: usize, pitch_hz: f64) -> Self {
        Self {
            frame_offset,
            kind: NoteEventKind::NoteOff { pitch_hz },
        }
    }
}

/// How many notes a built-in synth sounds at once. A new note past the limit
/// steals the quietest voice, which is less noticeable than stealing the
/// oldest when a long tail is still ringing.
pub const MAX_POLYPHONY: usize = 16;

/// Where a voice is in its attack/release shape.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VoiceStage {
    Idle,
    Attack,
    Sustain,
    Release,
}

/// A linear attack/release envelope. Simple on purpose: the shapes a game SFX
/// needs come from the generator recipes on top, not from one clever envelope.
#[derive(Clone, Copy, Debug)]
pub struct Envelope {
    pub stage: VoiceStage,
    pub level: f32,
    attack_per_frame: f32,
    release_per_frame: f32,
}

impl Envelope {
    /// Times in milliseconds, at `sample_rate`. A zero time means "instantly",
    /// which is what a click track or an impact wants.
    #[must_use]
    pub fn new(sample_rate: u32, attack_ms: f64, release_ms: f64) -> Self {
        Self {
            stage: VoiceStage::Idle,
            level: 0.0,
            attack_per_frame: per_frame(sample_rate, attack_ms),
            release_per_frame: per_frame(sample_rate, release_ms),
        }
    }

    pub fn trigger(&mut self) {
        self.stage = VoiceStage::Attack;
    }

    pub fn release(&mut self) {
        if self.stage != VoiceStage::Idle {
            self.stage = VoiceStage::Release;
        }
    }

    pub fn silence(&mut self) {
        self.stage = VoiceStage::Idle;
        self.level = 0.0;
    }

    #[must_use]
    pub const fn is_active(&self) -> bool {
        !matches!(self.stage, VoiceStage::Idle)
    }

    /// Advances one frame and returns the gain to apply to it.
    pub fn advance(&mut self) -> f32 {
        match self.stage {
            VoiceStage::Idle => 0.0,
            VoiceStage::Attack => {
                self.level = (self.level + self.attack_per_frame).min(1.0);
                if self.level >= 1.0 {
                    self.stage = VoiceStage::Sustain;
                }
                self.level
            }
            VoiceStage::Sustain => self.level,
            VoiceStage::Release => {
                self.level -= self.release_per_frame;
                if self.level <= 0.0 {
                    self.silence();
                    return 0.0;
                }
                self.level
            }
        }
    }
}

/// Per-frame step for a ramp of `milliseconds`. A zero or negative time ramps
/// in one frame rather than dividing by zero.
fn per_frame(sample_rate: u32, milliseconds: f64) -> f32 {
    let frames = f64::from(sample_rate) * (milliseconds.max(0.0) / 1_000.0);
    if frames < 1.0 {
        1.0
    } else {
        (1.0 / frames) as f32
    }
}

/// Deterministic white noise. Seeded per instance and re-seeded on reset, so a
/// noise synth renders the same samples every run — offline export included.
#[derive(Clone, Copy, Debug)]
pub struct Noise {
    seed: u64,
    state: u64,
}

impl Noise {
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        // A zero state would stick at zero forever.
        let state = if seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            seed
        };
        Self { seed, state }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.seed);
    }

    /// xorshift64*: cheap, allocation free, and the same on every platform.
    pub fn next_sample(&mut self) -> f32 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        let value = self.state.wrapping_mul(0x2545_f491_4f6c_dd1d);
        // Top 24 bits, mapped onto -1.0..1.0.
        const HALF_RANGE: f32 = 8_388_608.0;
        ((value >> 40) as f32 / HALF_RANGE) - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_envelope_rises_holds_and_falls_to_silence() {
        let mut envelope = Envelope::new(1_000, 2.0, 2.0);
        envelope.trigger();
        assert_eq!(envelope.advance(), 0.5);
        assert_eq!(envelope.advance(), 1.0);
        assert_eq!(envelope.stage, VoiceStage::Sustain);
        assert_eq!(envelope.advance(), 1.0);

        envelope.release();
        assert_eq!(envelope.advance(), 0.5);
        assert_eq!(envelope.advance(), 0.0);
        assert_eq!(envelope.stage, VoiceStage::Idle);
        assert!(!envelope.is_active());
    }

    #[test]
    fn a_zero_length_attack_reaches_full_level_on_its_first_frame() {
        let mut envelope = Envelope::new(48_000, 0.0, 0.0);
        envelope.trigger();
        assert_eq!(envelope.advance(), 1.0);
    }

    #[test]
    fn releasing_a_silent_voice_leaves_it_silent() {
        let mut envelope = Envelope::new(48_000, 1.0, 1.0);
        envelope.release();
        assert_eq!(envelope.stage, VoiceStage::Idle);
        assert_eq!(envelope.advance(), 0.0);
    }

    #[test]
    fn noise_is_the_same_sequence_every_time_it_is_reset() {
        let mut noise = Noise::new(7);
        let first: Vec<f32> = (0..8).map(|_| noise.next_sample()).collect();
        noise.reset();
        let again: Vec<f32> = (0..8).map(|_| noise.next_sample()).collect();
        assert_eq!(first, again);

        let mut other = Noise::new(8);
        let different: Vec<f32> = (0..8).map(|_| other.next_sample()).collect();
        assert_ne!(first, different, "a different seed is different noise");
    }

    #[test]
    fn noise_stays_inside_full_scale() {
        let mut noise = Noise::new(0);
        for _ in 0..10_000 {
            let sample = noise.next_sample();
            assert!((-1.0..=1.0).contains(&sample), "{sample} is out of range");
        }
    }
}
