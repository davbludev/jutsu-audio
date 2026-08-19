//! The small pieces of DSP the SFX generators share.
//!
//! Everything here is deterministic and allocation-light: given the same seed
//! and the same parameters it produces the same samples, which is the whole
//! promise a recipe makes.

use crate::voice::Noise;

/// A one-pole low-pass. Cheap, stable, and enough to take the edge off noise
/// or sweep a filter down over a tail.
#[derive(Clone, Copy, Debug)]
pub struct LowPass {
    state: f32,
}

impl LowPass {
    #[must_use]
    pub const fn new() -> Self {
        Self { state: 0.0 }
    }

    /// `cutoff_hz` is clamped into the audible range and to below Nyquist, so a
    /// parameter sweep can run to the edges without blowing up.
    pub fn process(&mut self, sample: f32, cutoff_hz: f32, sample_rate: u32) -> f32 {
        let nyquist = sample_rate as f32 * 0.5;
        let cutoff = cutoff_hz.clamp(20.0, nyquist * 0.98);
        let coefficient =
            (1.0 - (-std::f32::consts::TAU * cutoff / sample_rate as f32).exp()).clamp(0.0, 1.0);
        self.state += coefficient * (sample - self.state);
        self.state
    }
}

impl Default for LowPass {
    fn default() -> Self {
        Self::new()
    }
}

/// An exponential decay from 1.0, reaching about -60 dB after `decay_frames`.
/// Exponential rather than linear because that is what a real impact does.
#[must_use]
pub fn decay(frame: usize, decay_frames: f64) -> f32 {
    if decay_frames <= 0.0 {
        return 0.0;
    }
    let progress = frame as f64 / decay_frames;
    (-6.907 * progress).exp() as f32
}

/// A sine at `phase` turns, where one turn is a full cycle.
#[must_use]
pub fn sine(phase: f64) -> f32 {
    (phase * std::f64::consts::TAU).sin() as f32
}

/// Advances a phase by `frequency_hz` for one frame, wrapping at one turn.
#[must_use]
pub fn advance_phase(phase: f64, frequency_hz: f64, sample_rate: u32) -> f64 {
    (phase + frequency_hz / f64::from(sample_rate.max(1))).fract()
}

/// Interpolates between two values over a `0.0..1.0` progress.
#[must_use]
pub fn lerp(from: f64, to: f64, progress: f64) -> f64 {
    to.mul_add(progress, from * (1.0 - progress))
}

/// Normalises a buffer to just under full scale. A generator's parameters are
/// about character, not level; peak-matching keeps one recipe from being ten
/// times louder than the next.
pub fn normalise(samples: &mut [f32]) {
    let peak = samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    if peak <= f32::EPSILON {
        return;
    }
    let scale = 0.98 / peak;
    for sample in samples {
        *sample *= scale;
    }
}

/// A noise source seeded from a recipe seed and a label, so two parts of one
/// generator never share a stream.
#[must_use]
pub fn seeded_noise(seed: u64, label: &str) -> Noise {
    Noise::new(crate::recipe::GeneratorRecipe::new("", 0, seed).derive_seed(label))
}
