//! `pocket.tremolo` — amplitude wobble.
//!
//! One oscillator multiplying the signal. It shows the two things an effect
//! has to get right: state that `reset` clears, and no allocation in `process`.

use std::collections::BTreeMap;

use jutsu_audio_extensions::parameters::Preset;
use jutsu_audio_extensions::{
    Effect, EffectFactory, ExtensionDescriptor, ExtensionError, ExtensionKind, ExtensionTypeId,
    ParameterDescriptor, ParameterType,
};
use jutsu_audio_model::ParameterValue;

pub const TREMOLO_TYPE_ID: &str = "pocket.tremolo";

pub struct TremoloFactory {
    descriptor: ExtensionDescriptor,
    presets: Vec<Preset>,
}

impl Default for TremoloFactory {
    fn default() -> Self {
        Self {
            descriptor: descriptor(),
            presets: vec![
                Preset::new(
                    "Slow sway",
                    &[
                        ("rate_hz", ParameterValue::Float(1.5)),
                        ("depth", ParameterValue::Float(0.4)),
                    ],
                ),
                Preset::new(
                    "Chopper",
                    &[
                        ("rate_hz", ParameterValue::Float(12.0)),
                        ("depth", ParameterValue::Float(1.0)),
                    ],
                ),
            ],
        }
    }
}

fn descriptor() -> ExtensionDescriptor {
    ExtensionDescriptor {
        type_id: ExtensionTypeId::new(TREMOLO_TYPE_ID).expect("a valid type ID"),
        kind: ExtensionKind::Effect,
        display_name: "Pocket Tremolo".into(),
        state_version: 1,
        parameters: vec![
            ParameterDescriptor {
                id: "rate_hz".into(),
                display_name: "Rate".into(),
                value_type: ParameterType::Float,
                default_value: ParameterValue::Float(5.0),
                introduced_in_state_version: 1,
                automatable: true,
                minimum: Some(0.1),
                maximum: Some(20.0),
                unit: Some("Hz".into()),
            },
            ParameterDescriptor {
                id: "depth".into(),
                display_name: "Depth".into(),
                value_type: ParameterType::Float,
                default_value: ParameterValue::Float(0.5),
                introduced_in_state_version: 1,
                automatable: true,
                minimum: Some(0.0),
                maximum: Some(1.0),
                unit: Some("ratio".into()),
            },
        ],
    }
}

impl EffectFactory for TremoloFactory {
    fn descriptor(&self) -> &ExtensionDescriptor {
        &self.descriptor
    }

    fn presets(&self) -> &[Preset] {
        &self.presets
    }

    fn instantiate(
        &self,
        parameters: &BTreeMap<String, ParameterValue>,
    ) -> Result<Box<dyn Effect>, ExtensionError> {
        Ok(Box::new(Tremolo {
            rate_hz: crate::float(parameters, "rate_hz", 5.0),
            depth: crate::float(parameters, "depth", 0.5),
            sample_rate: 48_000.0,
            phase: 0.0,
        }))
    }
}

struct Tremolo {
    rate_hz: f64,
    depth: f64,
    sample_rate: f64,
    phase: f64,
}

impl Effect for Tremolo {
    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = f64::from(sample_rate);
        self.reset();
    }

    /// The phase is the whole of this effect's state. Clearing it is what makes
    /// an offline render match what playback just did.
    fn reset(&mut self) {
        self.phase = 0.0;
    }

    fn process(&mut self, samples: &mut [f32]) {
        let step = std::f64::consts::TAU * self.rate_hz / self.sample_rate;
        for sample in samples {
            // Between `1 - depth` and `1`: the effect only ever takes level
            // away, so it cannot push a mix past full scale.
            let gain = 1.0 - self.depth * 0.5 * (1.0 - self.phase.cos());
            #[allow(clippy::cast_possible_truncation)]
            {
                *sample *= gain as f32;
            }
            self.phase += step;
            if self.phase >= std::f64::consts::TAU {
                self.phase -= std::f64::consts::TAU;
            }
        }
    }
}
