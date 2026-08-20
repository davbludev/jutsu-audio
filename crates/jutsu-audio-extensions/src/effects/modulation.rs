//! `builtin.chorus` — a delay short enough to be heard as width rather than
//! as an echo, with its length moving.
//!
//! A delay of a few milliseconds is not heard as a repeat; it is heard as the
//! same sound arriving twice from slightly different places. Move that delay
//! slowly and the two copies drift in and out of tune with each other, which is
//! what a chorus is.
//!
//! The width comes from giving each channel a different point in the same
//! movement. The chain builds one instance per channel and tells each which it
//! is — without that, both sides would move identically and the result would be
//! wider in name only.

use jutsu_audio_model::ParameterValue;

use super::{BuiltinEffectFactory, DelayLine, descriptor, ranged, safe};
use crate::parameters::{Preset, UNIT_HERTZ, UNIT_MILLISECONDS, UNIT_NORMALISED};
use crate::{Effect, ExtensionDescriptor};

pub const TYPE_ID: &str = "builtin.chorus";

/// The longest the modulated delay can reach. The line is sized for this once,
/// so depth can move without ever needing a bigger buffer.
const MAXIMUM_DELAY_MS: f64 = 60.0;

#[must_use]
pub fn factory() -> BuiltinEffectFactory {
    BuiltinEffectFactory::new(
        chorus_descriptor(),
        vec![
            Preset::new(
                "Wide pad",
                &[
                    ("rate_hz", ParameterValue::Float(0.35)),
                    ("depth_ms", ParameterValue::Float(9.0)),
                    ("mix", ParameterValue::Float(0.5)),
                ],
            ),
            Preset::new(
                "Doubler",
                &[
                    ("rate_hz", ParameterValue::Float(0.9)),
                    ("depth_ms", ParameterValue::Float(3.0)),
                    ("mix", ParameterValue::Float(0.4)),
                ],
            ),
            Preset::new(
                "Seasick",
                &[
                    ("rate_hz", ParameterValue::Float(4.5)),
                    ("depth_ms", ParameterValue::Float(18.0)),
                    ("mix", ParameterValue::Float(0.7)),
                ],
            ),
        ],
        |settings| {
            Box::new(Chorus {
                rate_hz: settings.float("rate_hz"),
                depth_ms: settings.float("depth_ms"),
                centre_ms: settings.float("centre_ms"),
                mix: settings.float("mix"),
                spread: settings.float("spread"),
                line: DelayLine::new(),
                sample_rate: 48_000,
                phase: 0.0,
                channel: 0,
            })
        },
    )
}

fn chorus_descriptor() -> ExtensionDescriptor {
    descriptor(
        TYPE_ID,
        "Chorus",
        vec![
            ranged("rate_hz", "Rate", 0.4, 0.02, 8.0, UNIT_HERTZ),
            ranged("depth_ms", "Depth", 7.0, 0.1, 30.0, UNIT_MILLISECONDS),
            ranged(
                "centre_ms",
                "Centre",
                14.0,
                1.0,
                MAXIMUM_DELAY_MS - 30.0,
                UNIT_MILLISECONDS,
            ),
            ranged("mix", "Mix", 0.5, 0.0, 1.0, UNIT_NORMALISED),
            // How far apart the channels are in the same movement. At one they
            // are half a cycle apart, which is the widest they can be.
            ranged("spread", "Spread", 1.0, 0.0, 1.0, UNIT_NORMALISED),
        ],
    )
}

struct Chorus {
    rate_hz: f64,
    depth_ms: f64,
    centre_ms: f64,
    mix: f64,
    spread: f64,
    line: DelayLine,
    sample_rate: u32,
    /// Where the movement has got to, in cycles.
    phase: f64,
    channel: u16,
}

impl Effect for Chorus {
    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate.max(1);
        let longest = (f64::from(self.sample_rate) * MAXIMUM_DELAY_MS / 1_000.0).ceil() as usize;
        self.line.prepare(longest + 2);
        self.reset();
    }

    fn reset(&mut self) {
        self.line.reset();
        // The channel's own starting point, so the two sides begin apart rather
        // than drifting apart later.
        self.phase = f64::from(self.channel) * 0.5 * self.spread.clamp(0.0, 1.0);
    }

    fn set_channel(&mut self, channel: u16) {
        self.channel = channel;
        self.reset();
    }

    fn set_parameter(&mut self, id: &str, value: f64) {
        match id {
            "rate_hz" => self.rate_hz = value,
            "depth_ms" => self.depth_ms = value,
            "centre_ms" => self.centre_ms = value,
            "mix" => self.mix = value,
            "spread" => self.spread = value,
            _ => {}
        }
    }

    fn process(&mut self, samples: &mut [f32]) {
        let rate = self.rate_hz.max(0.0);
        let sample_rate = f64::from(self.sample_rate.max(1));
        let step = rate / sample_rate;
        let mix = self.mix.clamp(0.0, 1.0) as f32;
        let depth_frames = sample_rate * self.depth_ms.max(0.0) / 1_000.0;
        let centre_frames = sample_rate * self.centre_ms.max(0.1) / 1_000.0;

        for sample in samples {
            let movement = (self.phase * std::f64::consts::TAU).sin();
            self.phase = (self.phase + step).fract();

            let delay = (centre_frames + movement * depth_frames * 0.5).max(1.0);
            let wet = self.line.read(delay as usize);
            self.line.write(*sample);
            *sample = safe(*sample * (1.0 - mix) + wet * mix);
        }
    }
}
