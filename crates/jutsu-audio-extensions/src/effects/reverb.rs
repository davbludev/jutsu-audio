//! `builtin.reverb` — a small Schroeder reverb: four combs into two all-passes.
//!
//! Not a hall simulation. It is the amount of room a game SFX usually needs,
//! from a few lines and a damping filter, with feedback bounded so it can never
//! run away however the parameters are set.

use jutsu_audio_model::ParameterValue;

use super::{BuiltinEffectFactory, DelayLine, descriptor, ranged, safe};
use crate::parameters::{Preset, UNIT_NORMALISED};
use crate::{Effect, ExtensionDescriptor};

pub const TYPE_ID: &str = "builtin.reverb";

/// Comb delays in milliseconds, mutually prime enough not to ring in step.
const COMB_MS: [f64; 4] = [29.7, 37.1, 41.1, 43.7];
/// All-pass delays, short and dense: they smear, they do not repeat.
const ALLPASS_MS: [f64; 2] = [5.0, 1.7];
/// How much of an all-pass is fed back. The classic Schroeder value.
const ALLPASS_GAIN: f32 = 0.5;
/// The most feedback a comb may have. Below one by enough that even the
/// longest room decays.
const MAXIMUM_FEEDBACK: f32 = 0.92;

#[must_use]
pub fn factory() -> BuiltinEffectFactory {
    BuiltinEffectFactory::new(
        reverb_descriptor(),
        vec![
            Preset::new(
                "Small room",
                &[
                    ("size", ParameterValue::Float(0.25)),
                    ("damping", ParameterValue::Float(0.6)),
                ],
            ),
            Preset::new(
                "Hall",
                &[
                    ("size", ParameterValue::Float(0.85)),
                    ("damping", ParameterValue::Float(0.3)),
                ],
            ),
        ],
        |settings| {
            Box::new(Reverb {
                size: settings.float("size").clamp(0.0, 1.0),
                damping: settings.float("damping").clamp(0.0, 1.0) as f32,
                combs: std::array::from_fn(|_| DelayLine::new()),
                comb_damping: [0.0; COMB_MS.len()],
                allpasses: std::array::from_fn(|_| DelayLine::new()),
                comb_frames: [1; COMB_MS.len()],
                allpass_frames: [1; ALLPASS_MS.len()],
                sample_rate: 48_000,
            })
        },
    )
}

fn reverb_descriptor() -> ExtensionDescriptor {
    descriptor(
        TYPE_ID,
        "Reverb",
        vec![
            ranged("size", "Size", 0.5, 0.0, 1.0, UNIT_NORMALISED),
            ranged("damping", "Damping", 0.5, 0.0, 1.0, UNIT_NORMALISED),
        ],
    )
}

struct Reverb {
    size: f64,
    damping: f32,
    combs: [DelayLine; COMB_MS.len()],
    comb_damping: [f32; COMB_MS.len()],
    allpasses: [DelayLine; ALLPASS_MS.len()],
    comb_frames: [usize; COMB_MS.len()],
    allpass_frames: [usize; ALLPASS_MS.len()],
    sample_rate: u32,
}

impl Reverb {
    /// How much each comb feeds back, from the size. Capped so the tail always
    /// decays, whatever the caller asked for.
    fn feedback(&self) -> f32 {
        (0.7 + 0.25 * self.size as f32).min(MAXIMUM_FEEDBACK)
    }
}

impl Effect for Reverb {
    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate.max(1);
        let frames_of = |milliseconds: f64| {
            (milliseconds * f64::from(self.sample_rate) / 1_000.0).max(1.0) as usize
        };
        // A bigger room is a longer set of delays, sized once here.
        let stretch = 0.7 + 0.6 * self.size;
        for (index, line) in self.combs.iter_mut().enumerate() {
            self.comb_frames[index] = frames_of(COMB_MS[index] * stretch);
            line.prepare(self.comb_frames[index]);
        }
        for (index, line) in self.allpasses.iter_mut().enumerate() {
            self.allpass_frames[index] = frames_of(ALLPASS_MS[index]);
            line.prepare(self.allpass_frames[index]);
        }
        self.comb_damping = [0.0; COMB_MS.len()];
    }

    fn reset(&mut self) {
        for line in &mut self.combs {
            line.reset();
        }
        for line in &mut self.allpasses {
            line.reset();
        }
        self.comb_damping = [0.0; COMB_MS.len()];
    }

    fn process(&mut self, samples: &mut [f32]) {
        let feedback = self.feedback();
        let damping = self.damping;

        for sample in samples {
            let input = *sample;
            let mut summed = 0.0;
            for index in 0..COMB_MS.len() {
                let delayed = self.combs[index].read(self.comb_frames[index]);
                // Damping inside the loop: each pass round the comb is duller
                // than the last, which is what a room does.
                self.comb_damping[index] += (1.0 - damping) * (delayed - self.comb_damping[index]);
                self.combs[index].write(safe(input + self.comb_damping[index] * feedback));
                summed += delayed;
            }
            let mut value = summed / COMB_MS.len() as f32;

            for index in 0..ALLPASS_MS.len() {
                let delayed = self.allpasses[index].read(self.allpass_frames[index]);
                let written = value + delayed * ALLPASS_GAIN;
                self.allpasses[index].write(safe(written));
                value = delayed - written * ALLPASS_GAIN;
            }
            *sample = safe(value);
        }
    }

    fn tail_frames(&self) -> u32 {
        // The longest comb, times how many passes it takes to fall below about
        // -60 dB at this feedback.
        let longest = self.comb_frames.iter().copied().max().unwrap_or(1) as u32;
        let feedback = self.feedback().clamp(0.001, 0.999);
        let passes = (0.001_f32.ln() / feedback.ln()).clamp(1.0, 256.0);
        longest.saturating_mul(passes as u32)
    }
}
