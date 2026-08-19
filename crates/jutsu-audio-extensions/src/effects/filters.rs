//! `builtin.lowpass` and `builtin.highpass` — one-pole filters.
//!
//! One pole rather than a biquad: gentle, unconditionally stable at every
//! cutoff, and exactly what a game SFX usually wants — take the top off, or
//! take the weight out.

use jutsu_audio_model::ParameterValue;

use super::{BuiltinEffectFactory, descriptor, ranged, safe};
use crate::parameters::{Preset, UNIT_HERTZ};
use crate::{Effect, ExtensionDescriptor};

pub const LOW_PASS_TYPE_ID: &str = "builtin.lowpass";
pub const HIGH_PASS_TYPE_ID: &str = "builtin.highpass";

#[must_use]
pub fn low_pass_factory() -> BuiltinEffectFactory {
    BuiltinEffectFactory::new(
        low_pass_descriptor(),
        vec![
            Preset::new("Muffled", &[("cutoff_hz", ParameterValue::Float(600.0))]),
            Preset::new("Warm", &[("cutoff_hz", ParameterValue::Float(4_000.0))]),
        ],
        |settings| {
            Box::new(OnePole {
                cutoff_hz: settings.float("cutoff_hz"),
                sample_rate: 48_000,
                state: 0.0,
                high_pass: false,
            })
        },
    )
}

#[must_use]
pub fn high_pass_factory() -> BuiltinEffectFactory {
    BuiltinEffectFactory::new(
        high_pass_descriptor(),
        vec![
            Preset::new("Thin", &[("cutoff_hz", ParameterValue::Float(1_200.0))]),
            Preset::new("Rumble cut", &[("cutoff_hz", ParameterValue::Float(90.0))]),
        ],
        |settings| {
            Box::new(OnePole {
                cutoff_hz: settings.float("cutoff_hz"),
                sample_rate: 48_000,
                state: 0.0,
                high_pass: true,
            })
        },
    )
}

fn low_pass_descriptor() -> ExtensionDescriptor {
    descriptor(
        LOW_PASS_TYPE_ID,
        "Low-pass",
        vec![ranged(
            "cutoff_hz",
            "Cutoff",
            8_000.0,
            20.0,
            20_000.0,
            UNIT_HERTZ,
        )],
    )
}

fn high_pass_descriptor() -> ExtensionDescriptor {
    descriptor(
        HIGH_PASS_TYPE_ID,
        "High-pass",
        vec![ranged(
            "cutoff_hz",
            "Cutoff",
            200.0,
            20.0,
            20_000.0,
            UNIT_HERTZ,
        )],
    )
}

/// One pole, used as a low-pass directly and as a high-pass by subtraction.
struct OnePole {
    cutoff_hz: f64,
    sample_rate: u32,
    state: f32,
    high_pass: bool,
}

impl Effect for OnePole {
    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate.max(1);
        self.reset();
    }

    fn reset(&mut self) {
        self.state = 0.0;
    }

    fn process(&mut self, samples: &mut [f32]) {
        // Clamped below Nyquist: a cutoff at or above it would make the
        // coefficient meaningless rather than merely extreme.
        let nyquist = f64::from(self.sample_rate) * 0.5;
        let cutoff = self.cutoff_hz.clamp(20.0, nyquist * 0.98);
        let coefficient =
            (1.0 - (-std::f64::consts::TAU * cutoff / f64::from(self.sample_rate)).exp()) as f32;
        let coefficient = coefficient.clamp(0.0, 1.0);

        for sample in samples {
            self.state += coefficient * (*sample - self.state);
            *sample = safe(if self.high_pass {
                *sample - self.state
            } else {
                self.state
            });
        }
    }
}
