//! The first game SFX generators.
//!
//! Each one is narrow on purpose: an impact is an impact, not a general
//! synthesis graph. Narrow generators take few parameters, are easy to judge by
//! ear, and compose — a hit plus a tail plus a whoosh is three clips, not one
//! generator with thirty knobs.
//!
//! All of them are deterministic in the same way: given a seed and parameters
//! they render the same samples, every run, on every machine.

pub mod ambience;
pub mod dsp;
pub mod explosion;
pub mod impact;
pub mod laser;
pub mod pickup;

use std::collections::BTreeMap;
use std::sync::Arc;

use jutsu_audio_model::ParameterValue;

use crate::{
    ExtensionDescriptor, ExtensionError, ExtensionKind, ExtensionRegistries, ExtensionTypeId,
    Generator, GeneratorFactory, ParameterDescriptor, ParameterType,
};

/// Registers every SFX generator this crate ships.
pub fn register_sfx_generators(registries: &mut ExtensionRegistries) -> Result<(), ExtensionError> {
    registries.register_generator(Arc::new(impact::factory()))?;
    registries.register_generator(Arc::new(explosion::factory()))?;
    registries.register_generator(Arc::new(laser::factory()))?;
    registries.register_generator(Arc::new(pickup::factory()))?;
    registries.register_generator(Arc::new(ambience::factory()))?;
    Ok(())
}

/// A named parameter set: somewhere useful to start, and the answer to "what
/// does this generator sound like" without reading its parameter list.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratorPreset {
    pub name: &'static str,
    pub parameters: BTreeMap<String, ParameterValue>,
}

/// The shape every SFX generator here shares: a descriptor, presets, and a
/// render function taking the seed and its resolved parameters.
pub struct SfxFactory {
    descriptor: ExtensionDescriptor,
    presets: Vec<GeneratorPreset>,
    render: fn(&Settings, u64, usize) -> Vec<f32>,
}

impl SfxFactory {
    #[must_use]
    pub fn new(
        descriptor: ExtensionDescriptor,
        presets: Vec<GeneratorPreset>,
        render: fn(&Settings, u64, usize) -> Vec<f32>,
    ) -> Self {
        Self {
            descriptor,
            presets,
            render,
        }
    }

    /// Named starting points for this generator.
    #[must_use]
    pub fn presets(&self) -> &[GeneratorPreset] {
        &self.presets
    }
}

impl GeneratorFactory for SfxFactory {
    fn descriptor(&self) -> &ExtensionDescriptor {
        &self.descriptor
    }

    fn presets(&self) -> &[GeneratorPreset] {
        &self.presets
    }

    fn instantiate(
        &self,
        parameters: &BTreeMap<String, ParameterValue>,
    ) -> Result<Box<dyn Generator>, ExtensionError> {
        Ok(Box::new(SfxGenerator {
            settings: Settings::resolve(&self.descriptor, parameters),
            render: self.render,
        }))
    }
}

struct SfxGenerator {
    settings: Settings,
    render: fn(&Settings, u64, usize) -> Vec<f32>,
}

impl Generator for SfxGenerator {
    fn generate_mono(&self, seed: u64, frame_count: usize) -> Vec<f32> {
        (self.render)(&self.settings, seed, frame_count)
    }
}

/// Parameters with the descriptor's defaults filled in and every value clamped
/// to its declared range, so a generator body can read them without checking.
pub struct Settings {
    values: BTreeMap<String, ParameterValue>,
    /// The rate a generator renders at. Generated audio is placed at the
    /// project rate, and the mix resamples nothing it does not have to.
    pub sample_rate: u32,
}

/// What a generator renders at unless it is told otherwise.
pub const GENERATOR_SAMPLE_RATE: u32 = 48_000;

impl Settings {
    fn resolve(
        descriptor: &ExtensionDescriptor,
        supplied: &BTreeMap<String, ParameterValue>,
    ) -> Self {
        let mut values = BTreeMap::new();
        for parameter in &descriptor.parameters {
            let value = supplied
                .get(&parameter.id)
                .cloned()
                .unwrap_or_else(|| parameter.default_value.clone());
            values.insert(parameter.id.clone(), clamp(parameter, value));
        }
        Self {
            values,
            sample_rate: GENERATOR_SAMPLE_RATE,
        }
    }

    /// A declared float parameter. Missing or mistyped reads as `0.0`, which
    /// cannot happen for a descriptor-declared parameter but keeps the
    /// generator bodies free of error handling.
    #[must_use]
    pub fn float(&self, id: &str) -> f64 {
        match self.values.get(id) {
            Some(ParameterValue::Float(value)) => *value,
            _ => 0.0,
        }
    }

    #[must_use]
    pub fn integer(&self, id: &str) -> i64 {
        match self.values.get(id) {
            Some(ParameterValue::Integer(value)) => *value,
            _ => 0,
        }
    }

    /// A float parameter in frames, from a value in milliseconds.
    #[must_use]
    pub fn frames_from_ms(&self, id: &str) -> f64 {
        self.float(id) * f64::from(self.sample_rate) / 1_000.0
    }
}

fn clamp(parameter: &ParameterDescriptor, value: ParameterValue) -> ParameterValue {
    match value {
        ParameterValue::Float(value) => ParameterValue::Float(
            value
                .max(parameter.minimum.unwrap_or(f64::NEG_INFINITY))
                .min(parameter.maximum.unwrap_or(f64::INFINITY)),
        ),
        ParameterValue::Integer(value) => {
            let low = parameter.minimum.unwrap_or(f64::NEG_INFINITY) as i64;
            let high = parameter.maximum.unwrap_or(f64::INFINITY) as i64;
            ParameterValue::Integer(value.clamp(low.min(high), high.max(low)))
        }
        other => other,
    }
}

/// A bounded float parameter, which is what almost every SFX knob is.
#[must_use]
pub fn ranged(
    id: &str,
    display_name: &str,
    default_value: f64,
    minimum: f64,
    maximum: f64,
) -> ParameterDescriptor {
    ParameterDescriptor {
        id: id.into(),
        display_name: display_name.into(),
        value_type: ParameterType::Float,
        default_value: ParameterValue::Float(default_value),
        introduced_in_state_version: 1,
        automatable: false,
        minimum: Some(minimum),
        maximum: Some(maximum),
    }
}

/// A bounded integer parameter, for counts.
#[must_use]
pub fn counted(
    id: &str,
    display_name: &str,
    default_value: i64,
    minimum: i64,
    maximum: i64,
) -> ParameterDescriptor {
    ParameterDescriptor {
        id: id.into(),
        display_name: display_name.into(),
        value_type: ParameterType::Integer,
        default_value: ParameterValue::Integer(default_value),
        introduced_in_state_version: 1,
        automatable: false,
        minimum: Some(minimum as f64),
        maximum: Some(maximum as f64),
    }
}

/// Builds a generator descriptor with the crate's conventions applied.
#[must_use]
pub fn descriptor(
    type_id: &str,
    display_name: &str,
    parameters: Vec<ParameterDescriptor>,
) -> ExtensionDescriptor {
    ExtensionDescriptor {
        type_id: ExtensionTypeId::new(type_id).expect("a valid built-in generator ID"),
        kind: ExtensionKind::Generator,
        display_name: display_name.into(),
        state_version: 1,
        parameters,
    }
}

/// One preset, from pairs of parameter ID and value.
#[must_use]
pub fn preset(name: &'static str, values: &[(&str, ParameterValue)]) -> GeneratorPreset {
    GeneratorPreset {
        name,
        parameters: values
            .iter()
            .map(|(id, value)| ((*id).to_string(), value.clone()))
            .collect(),
    }
}
