//! `builtin.saturator` — soft clipping, the cheap kind of loud.
//!
//! Driving a signal into a curve that flattens near full scale adds harmonics
//! that were not there, mostly odd ones, mostly close to the fundamental. The
//! ear hears that as weight and presence rather than as distortion, which is
//! why a thin saw becomes a thick one without getting any louder.
//!
//! The curve is normalised by its own drive, so turning drive up changes the
//! tone rather than the level — a saturator that just got louder would be
//! indistinguishable from a fader, and would flatter itself in every A/B.

use jutsu_audio_model::ParameterValue;

use super::{BuiltinEffectFactory, descriptor, from_decibels, ranged, safe};
use crate::parameters::{Preset, UNIT_DECIBELS, UNIT_NORMALISED};
use crate::{Effect, ExtensionDescriptor};

pub const TYPE_ID: &str = "builtin.saturator";

#[must_use]
pub fn factory() -> BuiltinEffectFactory {
    BuiltinEffectFactory::new(
        saturator_descriptor(),
        vec![
            Preset::new(
                "Warmth",
                &[
                    ("drive_db", ParameterValue::Float(6.0)),
                    ("bias", ParameterValue::Float(0.0)),
                ],
            ),
            Preset::new(
                "Grit",
                &[
                    ("drive_db", ParameterValue::Float(18.0)),
                    ("bias", ParameterValue::Float(0.25)),
                ],
            ),
            Preset::new(
                "Destroyed",
                &[
                    ("drive_db", ParameterValue::Float(30.0)),
                    ("bias", ParameterValue::Float(0.5)),
                    ("output_db", ParameterValue::Float(-6.0)),
                ],
            ),
        ],
        |settings| {
            Box::new(Saturator {
                drive_db: settings.float("drive_db"),
                bias: settings.float("bias"),
                output_db: settings.float("output_db"),
            })
        },
    )
}

fn saturator_descriptor() -> ExtensionDescriptor {
    descriptor(
        TYPE_ID,
        "Saturator",
        vec![
            ranged("drive_db", "Drive", 6.0, 0.0, 36.0, UNIT_DECIBELS),
            // Asymmetry: a curve that treats the two halves of a wave
            // differently adds even harmonics as well as odd, which is the
            // difference between valve warmth and transistor edge.
            ranged("bias", "Bias", 0.0, 0.0, 1.0, UNIT_NORMALISED),
            ranged("output_db", "Output", 0.0, -24.0, 12.0, UNIT_DECIBELS),
        ],
    )
}

struct Saturator {
    drive_db: f64,
    bias: f64,
    output_db: f64,
}

impl Effect for Saturator {
    fn prepare(&mut self, _sample_rate: u32) {}

    fn reset(&mut self) {}

    fn set_parameter(&mut self, id: &str, value: f64) {
        match id {
            "drive_db" => self.drive_db = value,
            "bias" => self.bias = value,
            "output_db" => self.output_db = value,
            _ => {}
        }
    }

    fn process(&mut self, samples: &mut [f32]) {
        let drive = from_decibels(self.drive_db).max(1.0);
        let bias = self.bias.clamp(0.0, 1.0) as f32 * 0.5;
        let output = from_decibels(self.output_db);
        // What the curve does to a full-scale input, so the result comes back
        // to the level it went in at.
        let normalise = 1.0 / (drive + bias).tanh().max(f32::EPSILON);

        for sample in samples {
            let shaped = (*sample * drive + bias).tanh() - bias.tanh();
            *sample = safe(shaped * normalise * output);
        }
    }
}
