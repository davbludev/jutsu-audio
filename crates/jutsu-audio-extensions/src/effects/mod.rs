//! The built-in effects: filters, dynamics, delay and reverb.
//!
//! Enough to finish a game SFX or a short cue without leaving the editor, and
//! narrow enough that each one is judgeable by ear. Every effect here is
//! deterministic, allocation-free once prepared, and safe with any parameter
//! value its descriptor allows.

pub mod convolution;
pub mod delay;
pub mod dynamics;
pub mod eq;
pub mod filters;
pub mod limiter;
pub mod modulation;
pub mod reverb;
pub mod saturation;

use std::collections::BTreeMap;
use std::sync::Arc;

use jutsu_audio_model::ParameterValue;

use crate::parameters::Preset;
use crate::{
    Effect, EffectFactory, ExtensionDescriptor, ExtensionError, ExtensionKind, ExtensionRegistries,
    ExtensionTypeId, ParameterDescriptor, ParameterType,
};

/// Registers every built-in effect.
pub fn register_builtin_effects(
    registries: &mut ExtensionRegistries,
) -> Result<(), ExtensionError> {
    registries.register_effect(Arc::new(filters::low_pass_factory()))?;
    registries.register_effect(Arc::new(filters::high_pass_factory()))?;
    registries.register_effect(Arc::new(dynamics::compressor_factory()))?;
    registries.register_effect(Arc::new(delay::factory()))?;
    registries.register_effect(Arc::new(reverb::factory()))?;
    registries.register_effect(Arc::new(eq::factory()))?;
    registries.register_effect(Arc::new(saturation::factory()))?;
    registries.register_effect(Arc::new(modulation::factory()))?;
    registries.register_effect(Arc::new(limiter::factory()))?;
    registries.register_effect(Arc::new(convolution::factory()))?;
    Ok(())
}

/// The shape every built-in effect shares: a descriptor, presets, and a
/// constructor taking its resolved settings.
pub struct BuiltinEffectFactory {
    descriptor: ExtensionDescriptor,
    presets: Vec<Preset>,
    build: fn(&Settings) -> Box<dyn Effect>,
}

impl BuiltinEffectFactory {
    #[must_use]
    pub fn new(
        descriptor: ExtensionDescriptor,
        presets: Vec<Preset>,
        build: fn(&Settings) -> Box<dyn Effect>,
    ) -> Self {
        Self {
            descriptor,
            presets,
            build,
        }
    }
}

impl EffectFactory for BuiltinEffectFactory {
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
        Ok((self.build)(&Settings::resolve(
            &self.descriptor,
            parameters,
        )))
    }
}

/// Parameters with defaults filled in and every value clamped to its declared
/// range, so an effect body can read them without checking.
pub struct Settings {
    values: BTreeMap<String, ParameterValue>,
}

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
        Self { values }
    }

    #[must_use]
    pub fn float(&self, id: &str) -> f64 {
        match self.values.get(id) {
            Some(ParameterValue::Float(value)) => *value,
            _ => 0.0,
        }
    }
}

fn clamp(parameter: &ParameterDescriptor, value: ParameterValue) -> ParameterValue {
    match value {
        ParameterValue::Float(value) => ParameterValue::Float(
            value
                .max(parameter.minimum.unwrap_or(f64::NEG_INFINITY))
                .min(parameter.maximum.unwrap_or(f64::INFINITY)),
        ),
        other => other,
    }
}

/// A bounded float parameter with a unit.
#[must_use]
pub fn ranged(
    id: &str,
    display_name: &str,
    default_value: f64,
    minimum: f64,
    maximum: f64,
    unit: &str,
) -> ParameterDescriptor {
    ParameterDescriptor {
        id: id.into(),
        display_name: display_name.into(),
        value_type: ParameterType::Float,
        default_value: ParameterValue::Float(default_value),
        introduced_in_state_version: 1,
        automatable: true,
        minimum: Some(minimum),
        maximum: Some(maximum),
        unit: Some(unit.into()),
    }
}

/// Builds an effect descriptor with the crate's conventions applied.
#[must_use]
pub fn descriptor(
    type_id: &str,
    display_name: &str,
    parameters: Vec<ParameterDescriptor>,
) -> ExtensionDescriptor {
    ExtensionDescriptor {
        type_id: ExtensionTypeId::new(type_id).expect("a valid built-in effect ID"),
        kind: ExtensionKind::Effect,
        display_name: display_name.into(),
        state_version: 1,
        parameters,
    }
}

/// Turns decibels into a linear gain.
#[must_use]
pub fn from_decibels(decibels: f64) -> f32 {
    10_f32.powf(decibels as f32 / 20.0)
}

/// A delay line sized once, at `prepare`, and never resized while processing.
pub struct DelayLine {
    buffer: Vec<f32>,
    write: usize,
}

impl DelayLine {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buffer: Vec::new(),
            write: 0,
        }
    }

    /// Sizes the line for its longest delay. Allocation happens here, never in
    /// `process`.
    pub fn prepare(&mut self, frames: usize) {
        self.buffer = vec![0.0; frames.max(1)];
        self.write = 0;
    }

    pub fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write = 0;
    }

    /// The sample written `delay_frames` ago.
    #[must_use]
    pub fn read(&self, delay_frames: usize) -> f32 {
        if self.buffer.is_empty() {
            return 0.0;
        }
        let length = self.buffer.len();
        let delay = delay_frames.clamp(1, length);
        self.buffer[(self.write + length - delay) % length]
    }

    /// Writes one sample and advances. Read before writing: a feedback path
    /// needs what the line held, not what it is about to hold.
    pub fn write(&mut self, sample: f32) {
        if self.buffer.is_empty() {
            return;
        }
        self.buffer[self.write] = sample;
        self.write = (self.write + 1) % self.buffer.len();
    }
}

impl Default for DelayLine {
    fn default() -> Self {
        Self::new()
    }
}

/// Keeps a sample finite and inside full scale.
///
/// Every built-in ends with this. A parameter sweep, a feedback path or a
/// denormal should never be able to hand the mixer a NaN.
#[must_use]
pub fn safe(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}
