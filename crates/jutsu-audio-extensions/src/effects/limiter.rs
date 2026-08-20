//! `builtin.limiter` — a ceiling nothing gets past.
//!
//! The compressor bends a signal towards a threshold; this one stops it at a
//! line. The difference is lookahead: the gain is already down by the time the
//! peak arrives, so the transient is held rather than clipped, and nothing has
//! to be turned down in advance to leave room for it.
//!
//! Lookahead means latency, and latency is declared rather than hidden — the
//! chain reports it, so an offline render and live playback agree about where
//! the audio actually is.

use jutsu_audio_model::ParameterValue;

use super::{BuiltinEffectFactory, descriptor, from_decibels, ranged, safe};
use crate::parameters::{Preset, UNIT_DECIBELS, UNIT_MILLISECONDS};
use crate::{Effect, ExtensionDescriptor};

pub const TYPE_ID: &str = "builtin.limiter";

/// How far ahead the detector looks. Two milliseconds is long enough to catch a
/// drum transient and short enough that the delay it adds is inaudible.
const LOOKAHEAD_MS: f64 = 2.0;

#[must_use]
pub fn factory() -> BuiltinEffectFactory {
    BuiltinEffectFactory::new(
        limiter_descriptor(),
        vec![
            Preset::new(
                "Master ceiling",
                &[
                    ("ceiling_db", ParameterValue::Float(-1.0)),
                    ("release_ms", ParameterValue::Float(120.0)),
                ],
            ),
            Preset::new(
                "Loud",
                &[
                    ("ceiling_db", ParameterValue::Float(-0.3)),
                    ("gain_db", ParameterValue::Float(6.0)),
                    ("release_ms", ParameterValue::Float(60.0)),
                ],
            ),
        ],
        |settings| {
            Box::new(Limiter {
                ceiling_db: settings.float("ceiling_db"),
                gain_db: settings.float("gain_db"),
                release_ms: settings.float("release_ms"),
                sample_rate: 48_000,
                buffer: Vec::new(),
                write: 0,
                lookahead: 0,
                envelope: 1.0,
                release_coefficient: 0.0,
            })
        },
    )
}

fn limiter_descriptor() -> ExtensionDescriptor {
    descriptor(
        TYPE_ID,
        "Limiter",
        vec![
            ranged("ceiling_db", "Ceiling", -1.0, -24.0, 0.0, UNIT_DECIBELS),
            // Gain before the ceiling: this is how a limiter makes something
            // louder rather than merely safe.
            ranged("gain_db", "Gain", 0.0, 0.0, 24.0, UNIT_DECIBELS),
            ranged(
                "release_ms",
                "Release",
                120.0,
                5.0,
                1_000.0,
                UNIT_MILLISECONDS,
            ),
        ],
    )
}

struct Limiter {
    ceiling_db: f64,
    gain_db: f64,
    release_ms: f64,
    sample_rate: u32,
    /// The delayed signal, so the gain can move before the peak arrives.
    buffer: Vec<f32>,
    write: usize,
    lookahead: usize,
    /// Current gain reduction, linear, one means untouched.
    envelope: f32,
    release_coefficient: f32,
}

impl Effect for Limiter {
    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate.max(1);
        self.lookahead =
            ((f64::from(self.sample_rate) * LOOKAHEAD_MS / 1_000.0).round() as usize).max(1);
        self.buffer = vec![0.0; self.lookahead + 1];
        self.reset();
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write = 0;
        self.envelope = 1.0;
        let frames = f64::from(self.sample_rate) * (self.release_ms.max(1.0) / 1_000.0);
        self.release_coefficient = if frames < 1.0 {
            1.0
        } else {
            (1.0 - (-1.0 / frames).exp()) as f32
        };
    }

    fn latency_frames(&self) -> u32 {
        u32::try_from(self.lookahead).unwrap_or(0)
    }

    fn set_parameter(&mut self, id: &str, value: f64) {
        match id {
            "ceiling_db" => self.ceiling_db = value,
            "gain_db" => self.gain_db = value,
            "release_ms" => {
                self.release_ms = value;
                let frames = f64::from(self.sample_rate) * (self.release_ms.max(1.0) / 1_000.0);
                self.release_coefficient = if frames < 1.0 {
                    1.0
                } else {
                    (1.0 - (-1.0 / frames).exp()) as f32
                };
            }
            _ => {}
        }
    }

    fn process(&mut self, samples: &mut [f32]) {
        if self.buffer.is_empty() {
            return;
        }
        let ceiling = from_decibels(self.ceiling_db);
        let gain = from_decibels(self.gain_db);
        let length = self.buffer.len();

        for sample in samples {
            let driven = *sample * gain;
            // Read the sample that is about to leave the delay, write the one
            // that just arrived: the detector sees the future of what is heard.
            let delayed = self.buffer[self.write];
            self.buffer[self.write] = driven;
            self.write = (self.write + 1) % length;

            let peak = driven.abs().max(delayed.abs());
            let target = if peak > ceiling && peak > 0.0 {
                ceiling / peak
            } else {
                1.0
            };
            // Instant on the way down, eased on the way up: a limiter that let
            // go as fast as it grabbed would breathe on every transient.
            self.envelope = if target < self.envelope {
                target
            } else {
                self.envelope + self.release_coefficient * (target - self.envelope)
            };

            *sample = safe((delayed * self.envelope).clamp(-ceiling, ceiling));
        }
    }
}
