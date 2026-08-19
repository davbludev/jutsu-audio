//! `builtin.compressor` — a peak compressor with attack and release.
//!
//! Level detection is on the absolute sample rather than an RMS window: for
//! game SFX, catching the transient is the point.

use jutsu_audio_model::ParameterValue;

use super::{BuiltinEffectFactory, descriptor, from_decibels, ranged, safe};
use crate::parameters::{Preset, UNIT_DECIBELS, UNIT_MILLISECONDS, UNIT_NORMALISED};
use crate::{Effect, ExtensionDescriptor};

pub const TYPE_ID: &str = "builtin.compressor";

#[must_use]
pub fn compressor_factory() -> BuiltinEffectFactory {
    BuiltinEffectFactory::new(
        compressor_descriptor(),
        vec![
            Preset::new(
                "Glue",
                &[
                    ("threshold_db", ParameterValue::Float(-18.0)),
                    ("ratio", ParameterValue::Float(2.0)),
                    ("attack_ms", ParameterValue::Float(20.0)),
                    ("release_ms", ParameterValue::Float(180.0)),
                    ("makeup_db", ParameterValue::Float(2.0)),
                ],
            ),
            Preset::new(
                "Slam",
                &[
                    ("threshold_db", ParameterValue::Float(-30.0)),
                    ("ratio", ParameterValue::Float(12.0)),
                    ("attack_ms", ParameterValue::Float(1.0)),
                    ("release_ms", ParameterValue::Float(60.0)),
                    ("makeup_db", ParameterValue::Float(8.0)),
                ],
            ),
        ],
        |settings| {
            Box::new(Compressor {
                threshold_db: settings.float("threshold_db"),
                ratio: settings.float("ratio").max(1.0),
                attack_ms: settings.float("attack_ms"),
                release_ms: settings.float("release_ms"),
                makeup: from_decibels(settings.float("makeup_db")),
                envelope: 0.0,
                attack: 0.0,
                release: 0.0,
            })
        },
    )
}

fn compressor_descriptor() -> ExtensionDescriptor {
    descriptor(
        TYPE_ID,
        "Compressor",
        vec![
            ranged(
                "threshold_db",
                "Threshold",
                -18.0,
                -60.0,
                0.0,
                UNIT_DECIBELS,
            ),
            ranged("ratio", "Ratio", 4.0, 1.0, 20.0, UNIT_NORMALISED),
            ranged("attack_ms", "Attack", 10.0, 0.1, 200.0, UNIT_MILLISECONDS),
            ranged(
                "release_ms",
                "Release",
                120.0,
                5.0,
                2_000.0,
                UNIT_MILLISECONDS,
            ),
            ranged("makeup_db", "Makeup", 0.0, 0.0, 24.0, UNIT_DECIBELS),
        ],
    )
}

struct Compressor {
    threshold_db: f64,
    ratio: f64,
    attack_ms: f64,
    release_ms: f64,
    makeup: f32,
    /// The level the detector is tracking, linear.
    envelope: f32,
    attack: f32,
    release: f32,
}

impl Effect for Compressor {
    fn prepare(&mut self, sample_rate: u32) {
        self.attack = coefficient(self.attack_ms, sample_rate);
        self.release = coefficient(self.release_ms, sample_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.envelope = 0.0;
    }

    fn process(&mut self, samples: &mut [f32]) {
        let threshold = from_decibels(self.threshold_db);
        let ratio = self.ratio.max(1.0) as f32;

        for sample in samples {
            let level = sample.abs();
            // Rise fast on the attack coefficient, fall on the release one:
            // that asymmetry is what makes a compressor sound like one.
            let coefficient = if level > self.envelope {
                self.attack
            } else {
                self.release
            };
            self.envelope += coefficient * (level - self.envelope);

            let gain = if self.envelope > threshold && self.envelope > 0.0 {
                let over = self.envelope / threshold;
                // Above the threshold, only 1/ratio of the excess gets through.
                (over.powf(1.0 / ratio - 1.0)).clamp(0.0, 1.0)
            } else {
                1.0
            };
            *sample = safe(*sample * gain * self.makeup);
        }
    }
}

/// A one-pole coefficient for a time constant in milliseconds. A zero time
/// tracks instantly rather than dividing by zero.
fn coefficient(milliseconds: f64, sample_rate: u32) -> f32 {
    let frames = f64::from(sample_rate) * (milliseconds.max(0.0) / 1_000.0);
    if frames < 1.0 {
        1.0
    } else {
        (1.0 - (-1.0 / frames).exp()) as f32
    }
}
