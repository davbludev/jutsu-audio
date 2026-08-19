//! `builtin.delay` — a feedback delay with a damped repeat.

use jutsu_audio_model::ParameterValue;

use super::{BuiltinEffectFactory, DelayLine, descriptor, ranged, safe};
use crate::parameters::{Preset, UNIT_MILLISECONDS, UNIT_NORMALISED};
use crate::{Effect, ExtensionDescriptor};

pub const TYPE_ID: &str = "builtin.delay";

/// The longest delay the line is sized for. Fixed so `prepare` allocates once
/// and a parameter change can never need a bigger buffer mid-render.
const MAXIMUM_DELAY_MS: f64 = 2_000.0;

#[must_use]
pub fn factory() -> BuiltinEffectFactory {
    BuiltinEffectFactory::new(
        delay_descriptor(),
        vec![
            Preset::new(
                "Slapback",
                &[
                    ("delay_ms", ParameterValue::Float(110.0)),
                    ("feedback", ParameterValue::Float(0.15)),
                    ("damping", ParameterValue::Float(0.3)),
                ],
            ),
            Preset::new(
                "Echo",
                &[
                    ("delay_ms", ParameterValue::Float(380.0)),
                    ("feedback", ParameterValue::Float(0.55)),
                    ("damping", ParameterValue::Float(0.5)),
                ],
            ),
        ],
        |settings| {
            Box::new(Delay {
                delay_ms: settings.float("delay_ms"),
                feedback: settings.float("feedback").clamp(0.0, 0.95) as f32,
                damping: settings.float("damping").clamp(0.0, 1.0) as f32,
                line: DelayLine::new(),
                sample_rate: 48_000,
                damped: 0.0,
            })
        },
    )
}

fn delay_descriptor() -> ExtensionDescriptor {
    descriptor(
        TYPE_ID,
        "Delay",
        vec![
            ranged(
                "delay_ms",
                "Time",
                250.0,
                1.0,
                MAXIMUM_DELAY_MS,
                UNIT_MILLISECONDS,
            ),
            // Capped below 1.0: a feedback of exactly one never decays, and an
            // effect that grows without bound is a bug however it is reached.
            ranged("feedback", "Feedback", 0.35, 0.0, 0.95, UNIT_NORMALISED),
            ranged("damping", "Damping", 0.4, 0.0, 1.0, UNIT_NORMALISED),
        ],
    )
}

struct Delay {
    delay_ms: f64,
    feedback: f32,
    damping: f32,
    line: DelayLine,
    sample_rate: u32,
    /// One-pole state that dulls each repeat.
    damped: f32,
}

impl Effect for Delay {
    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate.max(1);
        let frames = (MAXIMUM_DELAY_MS * f64::from(self.sample_rate) / 1_000.0) as usize;
        self.line.prepare(frames);
        self.damped = 0.0;
    }

    fn reset(&mut self) {
        self.line.reset();
        self.damped = 0.0;
    }

    fn process(&mut self, samples: &mut [f32]) {
        let delay_frames = (self.delay_ms * f64::from(self.sample_rate) / 1_000.0) as usize;
        for sample in samples {
            let delayed = self.line.read(delay_frames.max(1));
            // Each repeat passes through the damping filter, so a long tail
            // gets darker rather than louder.
            self.damped += (1.0 - self.damping) * (delayed - self.damped);
            self.line.write(safe(*sample + self.damped * self.feedback));
            *sample = safe(*sample + delayed);
        }
    }

    fn tail_frames(&self) -> u32 {
        // How long the repeats stay audible: the delay time, times how many
        // repeats it takes the feedback to fall below about -60 dB.
        let delay_frames = (self.delay_ms * f64::from(self.sample_rate) / 1_000.0) as u32;
        let repeats = if self.feedback <= 0.0 {
            1.0
        } else {
            (0.001_f32.ln() / self.feedback.max(0.001).ln()).clamp(1.0, 64.0)
        };
        delay_frames.saturating_mul(repeats as u32)
    }
}
