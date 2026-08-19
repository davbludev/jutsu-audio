use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::sync::Arc;

use jutsu_audio_model::ParameterValue;
use serde::{Deserialize, Deserializer, Serialize};

pub mod builtin;
pub mod generators;
pub mod recipe;
pub mod voice;

pub use builtin::register_builtin;
pub use generators::{GeneratorPreset, register_sfx_generators};
pub use recipe::{GeneratorRecipe, RECIPE_CONTRACT_VERSION, RegenerateMode};
pub use voice::{Envelope, MAX_POLYPHONY, Noise, NoteEvent, NoteEventKind, VoiceStage};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ExtensionTypeId(String);

impl ExtensionTypeId {
    pub fn new(value: impl Into<String>) -> Result<Self, ExtensionError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.contains('.')
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
            });
        if !valid {
            return Err(ExtensionError::new(
                ExtensionErrorCode::InvalidTypeId,
                format!("extension type ID '{value}' must use lowercase dotted identifier syntax"),
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ExtensionTypeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl<'de> Deserialize<'de> for ExtensionTypeId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(|error| serde::de::Error::custom(error.message))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionKind {
    Synth,
    Effect,
    Generator,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterType {
    Float,
    Integer,
    Bool,
    Text,
}

impl ParameterType {
    fn accepts(self, value: &ParameterValue) -> bool {
        matches!(
            (self, value),
            (Self::Float, ParameterValue::Float(_))
                | (Self::Integer, ParameterValue::Integer(_))
                | (Self::Bool, ParameterValue::Bool(_))
                | (Self::Text, ParameterValue::Text(_))
        )
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ParameterDescriptor {
    pub id: String,
    pub display_name: String,
    pub value_type: ParameterType,
    pub default_value: ParameterValue,
    pub introduced_in_state_version: u32,
    pub automatable: bool,
    /// Bounds for numeric parameters. A value outside them is refused at
    /// instantiation, so a generator body can read its parameters without
    /// checking them, and a caller learns the range from the descriptor rather
    /// than from documentation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ExtensionDescriptor {
    pub type_id: ExtensionTypeId,
    pub kind: ExtensionKind,
    pub display_name: String,
    pub state_version: u32,
    pub parameters: Vec<ParameterDescriptor>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionErrorCode {
    InvalidTypeId,
    InvalidDescriptor,
    DuplicateTypeId,
    UnavailableType,
    InvalidParameters,
    InstantiationFailed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExtensionError {
    pub code: ExtensionErrorCode,
    pub message: String,
    pub kind: Option<ExtensionKind>,
    pub type_id: Option<ExtensionTypeId>,
    pub parameter_id: Option<String>,
}

impl ExtensionError {
    #[must_use]
    pub fn new(code: ExtensionErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            kind: None,
            type_id: None,
            parameter_id: None,
        }
    }

    fn for_type(
        code: ExtensionErrorCode,
        kind: ExtensionKind,
        type_id: ExtensionTypeId,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            kind: Some(kind),
            type_id: Some(type_id),
            parameter_id: None,
        }
    }
}

/// A polyphonic sound source.
///
/// `prepare` is called before the first render and whenever the rate changes;
/// `reset` returns the instance to the state a fresh one would be in, so a
/// transport stop or an offline render starts from silence and produces the
/// same samples every time. `render` fills the block and applies each event on
/// the frame it names. Neither method may allocate or block: they run on the
/// audio callback.
pub trait Synth: Send {
    fn prepare(&mut self, sample_rate: u32);
    fn reset(&mut self);
    fn render(&mut self, events: &[NoteEvent], output: &mut [f32]);
}

pub trait Effect: Send {
    fn process_mono(&mut self, samples: &mut [f32]);
}

pub trait Generator: Send {
    fn generate_mono(&self, seed: u64, frame_count: usize) -> Vec<f32>;
}

pub trait SynthFactory: Send + Sync {
    fn descriptor(&self) -> &ExtensionDescriptor;
    fn instantiate(
        &self,
        parameters: &BTreeMap<String, ParameterValue>,
    ) -> Result<Box<dyn Synth>, ExtensionError>;
}

pub trait EffectFactory: Send + Sync {
    fn descriptor(&self) -> &ExtensionDescriptor;
    fn instantiate(
        &self,
        parameters: &BTreeMap<String, ParameterValue>,
    ) -> Result<Box<dyn Effect>, ExtensionError>;
}

pub trait GeneratorFactory: Send + Sync {
    fn descriptor(&self) -> &ExtensionDescriptor;
    fn instantiate(
        &self,
        parameters: &BTreeMap<String, ParameterValue>,
    ) -> Result<Box<dyn Generator>, ExtensionError>;
}

#[derive(Default)]
pub struct ExtensionRegistries {
    synths: BTreeMap<ExtensionTypeId, Arc<dyn SynthFactory>>,
    effects: BTreeMap<ExtensionTypeId, Arc<dyn EffectFactory>>,
    generators: BTreeMap<ExtensionTypeId, Arc<dyn GeneratorFactory>>,
}

impl ExtensionRegistries {
    pub fn register_synth(&mut self, factory: Arc<dyn SynthFactory>) -> Result<(), ExtensionError> {
        validate_descriptor(factory.descriptor(), ExtensionKind::Synth)?;
        let type_id = factory.descriptor().type_id.clone();
        if self.synths.contains_key(&type_id) {
            return Err(duplicate_error(ExtensionKind::Synth, type_id));
        }
        self.synths.insert(type_id, factory);
        Ok(())
    }

    pub fn register_effect(
        &mut self,
        factory: Arc<dyn EffectFactory>,
    ) -> Result<(), ExtensionError> {
        validate_descriptor(factory.descriptor(), ExtensionKind::Effect)?;
        let type_id = factory.descriptor().type_id.clone();
        if self.effects.contains_key(&type_id) {
            return Err(duplicate_error(ExtensionKind::Effect, type_id));
        }
        self.effects.insert(type_id, factory);
        Ok(())
    }

    pub fn register_generator(
        &mut self,
        factory: Arc<dyn GeneratorFactory>,
    ) -> Result<(), ExtensionError> {
        validate_descriptor(factory.descriptor(), ExtensionKind::Generator)?;
        let type_id = factory.descriptor().type_id.clone();
        if self.generators.contains_key(&type_id) {
            return Err(duplicate_error(ExtensionKind::Generator, type_id));
        }
        self.generators.insert(type_id, factory);
        Ok(())
    }

    #[must_use]
    pub fn synth_descriptor(&self, type_id: &ExtensionTypeId) -> Option<&ExtensionDescriptor> {
        self.synths.get(type_id).map(|factory| factory.descriptor())
    }

    #[must_use]
    pub fn effect_descriptor(&self, type_id: &ExtensionTypeId) -> Option<&ExtensionDescriptor> {
        self.effects
            .get(type_id)
            .map(|factory| factory.descriptor())
    }

    #[must_use]
    pub fn generator_descriptor(&self, type_id: &ExtensionTypeId) -> Option<&ExtensionDescriptor> {
        self.generators
            .get(type_id)
            .map(|factory| factory.descriptor())
    }

    /// Every registered synth type, in ID order. Discovery for a caller that
    /// has no idea what this build ships with.
    pub fn synth_type_ids(&self) -> impl Iterator<Item = &ExtensionTypeId> {
        self.synths.keys()
    }

    pub fn effect_type_ids(&self) -> impl Iterator<Item = &ExtensionTypeId> {
        self.effects.keys()
    }

    pub fn generator_type_ids(&self) -> impl Iterator<Item = &ExtensionTypeId> {
        self.generators.keys()
    }

    pub fn instantiate_synth(
        &self,
        type_id: &ExtensionTypeId,
        parameters: &BTreeMap<String, ParameterValue>,
    ) -> Result<Box<dyn Synth>, ExtensionError> {
        let factory = self
            .synths
            .get(type_id)
            .ok_or_else(|| unavailable_error(ExtensionKind::Synth, type_id))?;
        validate_parameters(factory.descriptor(), parameters)?;
        factory.instantiate(parameters)
    }

    pub fn instantiate_effect(
        &self,
        type_id: &ExtensionTypeId,
        parameters: &BTreeMap<String, ParameterValue>,
    ) -> Result<Box<dyn Effect>, ExtensionError> {
        let factory = self
            .effects
            .get(type_id)
            .ok_or_else(|| unavailable_error(ExtensionKind::Effect, type_id))?;
        validate_parameters(factory.descriptor(), parameters)?;
        factory.instantiate(parameters)
    }

    pub fn instantiate_generator(
        &self,
        type_id: &ExtensionTypeId,
        parameters: &BTreeMap<String, ParameterValue>,
    ) -> Result<Box<dyn Generator>, ExtensionError> {
        let factory = self
            .generators
            .get(type_id)
            .ok_or_else(|| unavailable_error(ExtensionKind::Generator, type_id))?;
        validate_parameters(factory.descriptor(), parameters)?;
        factory.instantiate(parameters)
    }
}

fn validate_descriptor(
    descriptor: &ExtensionDescriptor,
    expected_kind: ExtensionKind,
) -> Result<(), ExtensionError> {
    if descriptor.kind != expected_kind || descriptor.state_version == 0 {
        return Err(ExtensionError::for_type(
            ExtensionErrorCode::InvalidDescriptor,
            expected_kind,
            descriptor.type_id.clone(),
            "extension descriptor kind must match registry and state version must be positive",
        ));
    }

    let mut parameter_ids = HashSet::new();
    for parameter in &descriptor.parameters {
        let valid_id = !parameter.id.is_empty()
            && parameter.id.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte)
            });
        let valid_version = parameter.introduced_in_state_version > 0
            && parameter.introduced_in_state_version <= descriptor.state_version;
        if !valid_id
            || !parameter_ids.insert(&parameter.id)
            || !valid_version
            || !parameter.value_type.accepts(&parameter.default_value)
        {
            return Err(ExtensionError {
                code: ExtensionErrorCode::InvalidDescriptor,
                message: format!(
                    "parameter '{}' has invalid ID, version, duplicate ID, or default value type",
                    parameter.id
                ),
                kind: Some(expected_kind),
                type_id: Some(descriptor.type_id.clone()),
                parameter_id: Some(parameter.id.clone()),
            });
        }
    }
    Ok(())
}

fn duplicate_error(kind: ExtensionKind, type_id: ExtensionTypeId) -> ExtensionError {
    ExtensionError::for_type(
        ExtensionErrorCode::DuplicateTypeId,
        kind,
        type_id.clone(),
        format!("{kind:?} extension '{type_id}' is already registered"),
    )
}

fn validate_parameters(
    descriptor: &ExtensionDescriptor,
    parameters: &BTreeMap<String, ParameterValue>,
) -> Result<(), ExtensionError> {
    for (parameter_id, value) in parameters {
        let Some(parameter) = descriptor
            .parameters
            .iter()
            .find(|parameter| parameter.id == *parameter_id)
        else {
            return Err(ExtensionError {
                code: ExtensionErrorCode::InvalidParameters,
                message: format!("parameter '{parameter_id}' is not declared"),
                kind: Some(descriptor.kind),
                type_id: Some(descriptor.type_id.clone()),
                parameter_id: Some(parameter_id.clone()),
            });
        };
        if !parameter.value_type.accepts(value) {
            return Err(ExtensionError {
                code: ExtensionErrorCode::InvalidParameters,
                message: format!("parameter '{parameter_id}' has wrong value type"),
                kind: Some(descriptor.kind),
                type_id: Some(descriptor.type_id.clone()),
                parameter_id: Some(parameter_id.clone()),
            });
        }
        if let Some(number) = numeric(value)
            && let Some(message) = out_of_range(parameter, number)
        {
            return Err(ExtensionError {
                code: ExtensionErrorCode::InvalidParameters,
                message,
                kind: Some(descriptor.kind),
                type_id: Some(descriptor.type_id.clone()),
                parameter_id: Some(parameter_id.clone()),
            });
        }
    }
    Ok(())
}

fn numeric(value: &ParameterValue) -> Option<f64> {
    match value {
        ParameterValue::Float(value) => Some(*value),
        ParameterValue::Integer(value) => Some(*value as f64),
        _ => None,
    }
}

/// The complaint to make about a numeric parameter, if any.
fn out_of_range(parameter: &ParameterDescriptor, value: f64) -> Option<String> {
    match (parameter.minimum, parameter.maximum) {
        (Some(minimum), _) if value < minimum => Some(format!(
            "parameter '{}' is below its minimum of {minimum}",
            parameter.id
        )),
        (_, Some(maximum)) if value > maximum => Some(format!(
            "parameter '{}' is above its maximum of {maximum}",
            parameter.id
        )),
        _ => None,
    }
}

fn unavailable_error(kind: ExtensionKind, type_id: &ExtensionTypeId) -> ExtensionError {
    ExtensionError::for_type(
        ExtensionErrorCode::UnavailableType,
        kind,
        type_id.clone(),
        format!("{kind:?} extension '{type_id}' is not available"),
    )
}
