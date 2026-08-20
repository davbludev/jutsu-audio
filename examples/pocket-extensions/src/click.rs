//! `pocket.click` — a seeded click.
//!
//! Short, dry, and reproducible: the same seed gives the same samples on any
//! machine, forever. That is not a nicety — a project stores the seed, not the
//! audio, so a generator that drifts changes a finished cue.

use std::collections::BTreeMap;

use jutsu_audio_extensions::{
    ExtensionDescriptor, ExtensionError, ExtensionKind, ExtensionTypeId, Generator,
    GeneratorFactory, ParameterDescriptor, ParameterType,
};
use jutsu_audio_model::ParameterValue;

pub const CLICK_TYPE_ID: &str = "pocket.click";

pub struct ClickFactory {
    descriptor: ExtensionDescriptor,
}

impl Default for ClickFactory {
    fn default() -> Self {
        Self {
            descriptor: descriptor(),
        }
    }
}

fn descriptor() -> ExtensionDescriptor {
    ExtensionDescriptor {
        type_id: ExtensionTypeId::new(CLICK_TYPE_ID).expect("a valid type ID"),
        kind: ExtensionKind::Generator,
        display_name: "Pocket Click".into(),
        state_version: 1,
        parameters: vec![
            ParameterDescriptor {
                id: "decay_ms".into(),
                display_name: "Decay".into(),
                value_type: ParameterType::Float,
                default_value: ParameterValue::Float(30.0),
                introduced_in_state_version: 1,
                automatable: false,
                minimum: Some(1.0),
                maximum: Some(500.0),
                unit: Some("ms".into()),
            },
            ParameterDescriptor {
                id: "noise".into(),
                display_name: "Noise".into(),
                value_type: ParameterType::Float,
                default_value: ParameterValue::Float(0.5),
                introduced_in_state_version: 1,
                automatable: false,
                minimum: Some(0.0),
                maximum: Some(1.0),
                unit: Some("ratio".into()),
            },
        ],
    }
}

impl GeneratorFactory for ClickFactory {
    fn descriptor(&self) -> &ExtensionDescriptor {
        &self.descriptor
    }

    fn instantiate(
        &self,
        parameters: &BTreeMap<String, ParameterValue>,
    ) -> Result<Box<dyn Generator>, ExtensionError> {
        Ok(Box::new(Click {
            decay_ms: crate::float(parameters, "decay_ms", 30.0),
            noise: crate::float(parameters, "noise", 0.5),
        }))
    }
}

struct Click {
    decay_ms: f64,
    noise: f64,
}

impl Generator for Click {
    fn generate_mono(&self, seed: u64, frame_count: usize) -> Vec<f32> {
        // The generator owns its randomness: seeded here, never from the
        // system, so two machines agree sample for sample.
        let mut state = seed ^ 0x9e37_79b9_7f4a_7c15;
        let sample_rate = 48_000.0;
        let decay_frames = (self.decay_ms / 1_000.0) * sample_rate;
        let decay = 0.001_f64.powf(1.0 / decay_frames.max(1.0));
        let mut level = 1.0;
        // Enough spread that "different seed, different click" is audible.
        let pitch_hz = 400.0 + f64::from((seed % 1_000) as u32);
        let step = std::f64::consts::TAU * pitch_hz / sample_rate;

        (0..frame_count)
            .map(|frame| {
                let tone = (step * frame as f64).sin();
                let value = (1.0 - self.noise) * tone + self.noise * next_noise(&mut state);
                let sample = value * level;
                level *= decay;
                #[allow(clippy::cast_possible_truncation)]
                {
                    sample.clamp(-1.0, 1.0) as f32
                }
            })
            .collect()
    }
}

/// splitmix64, in the eight lines it takes: good enough for a click, and the
/// same everywhere without a dependency.
fn next_noise(state: &mut u64) -> f64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut value = *state;
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^= value >> 31;
    // The top 24 bits, mapped to -1.0..1.0.
    f64::from((value >> 40) as u32) / 8_388_608.0 - 1.0
}
