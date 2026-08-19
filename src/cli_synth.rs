//! The synth half of the machine surface: what extensions exist, what
//! parameters they take, and whether the ones a caller sent are acceptable.
//!
//! Validation happens here rather than in the command engine, because the
//! registry is what knows: the engine stays extension-agnostic and the caller
//! gets a diagnostic naming the parameter it got wrong.

use std::collections::BTreeMap;

use jutsu_audio_extensions::{
    ExtensionDescriptor, ExtensionKind, ExtensionRegistries, ExtensionTypeId, ParameterType,
};
use jutsu_audio_model::ParameterValue;
use serde_json::{Value, json};

use crate::cli_session::CliFailure;

/// Everything registered, as a caller can consume it without reading prose.
#[must_use]
pub fn describe_all(registries: &ExtensionRegistries) -> Value {
    json!({
        "synths": registries
            .synth_type_ids()
            .filter_map(|type_id| registries.synth_descriptor(type_id))
            .map(describe)
            .collect::<Vec<_>>(),
        "effects": registries
            .effect_type_ids()
            .filter_map(|type_id| registries.effect_descriptor(type_id))
            .map(describe)
            .collect::<Vec<_>>(),
        "generators": registries
            .generator_type_ids()
            .filter_map(|type_id| registries.generator_descriptor(type_id))
            .map(describe)
            .collect::<Vec<_>>(),
    })
}

/// One extension, with the parameters it declares and their defaults.
#[must_use]
pub fn describe(descriptor: &ExtensionDescriptor) -> Value {
    json!({
        "type_id": descriptor.type_id.as_str(),
        "kind": kind_name(descriptor.kind),
        "display_name": descriptor.display_name,
        "state_version": descriptor.state_version,
        "parameters": descriptor
            .parameters
            .iter()
            .map(|parameter| json!({
                "id": parameter.id,
                "display_name": parameter.display_name,
                "value_type": value_type_name(parameter.value_type),
                "default_value": parameter.default_value,
                "introduced_in_state_version": parameter.introduced_in_state_version,
                "automatable": parameter.automatable,
            }))
            .collect::<Vec<_>>(),
    })
}

/// Looks a synth up and checks the parameters against its descriptor.
///
/// Returns the descriptor's state version, which is what a project stores
/// alongside the parameters so a later build knows what they were written for.
pub fn validate_synth(
    registries: &ExtensionRegistries,
    type_id: &str,
    parameters: &BTreeMap<String, ParameterValue>,
) -> Result<u32, CliFailure> {
    let type_id = ExtensionTypeId::new(type_id).map_err(extension_failure)?;
    let descriptor = registries
        .synth_descriptor(&type_id)
        .ok_or_else(|| available_synths(registries, &type_id))?;

    for (id, value) in parameters {
        let Some(parameter) = descriptor
            .parameters
            .iter()
            .find(|parameter| parameter.id == *id)
        else {
            let declared: Vec<&str> = descriptor
                .parameters
                .iter()
                .map(|parameter| parameter.id.as_str())
                .collect();
            return Err((
                6,
                "unknown_parameter",
                format!(
                    "'{}' has no parameter '{id}'; it takes {}",
                    descriptor.type_id,
                    declared.join(", ")
                ),
            ));
        };
        if !accepts(parameter.value_type, value) {
            return Err((
                6,
                "invalid_parameter",
                format!(
                    "parameter '{id}' of '{}' is a {}",
                    descriptor.type_id,
                    value_type_name(parameter.value_type)
                ),
            ));
        }
    }

    // The registry has the final word: it also rejects values a factory
    // refuses, such as a waveform name that does not exist.
    registries
        .instantiate_synth(&type_id, parameters)
        .map_err(extension_failure)?;
    Ok(descriptor.state_version)
}

fn accepts(value_type: ParameterType, value: &ParameterValue) -> bool {
    matches!(
        (value_type, value),
        (ParameterType::Float, ParameterValue::Float(_))
            | (ParameterType::Integer, ParameterValue::Integer(_))
            | (ParameterType::Bool, ParameterValue::Bool(_))
            | (ParameterType::Text, ParameterValue::Text(_))
    )
}

fn available_synths(registries: &ExtensionRegistries, wanted: &ExtensionTypeId) -> CliFailure {
    let available: Vec<&str> = registries
        .synth_type_ids()
        .map(ExtensionTypeId::as_str)
        .collect();
    (
        6,
        "unknown_extension",
        format!(
            "no synth '{wanted}' is registered; this build has {}",
            available.join(", ")
        ),
    )
}

/// Registry errors keep their own message: it already names the parameter.
fn extension_failure(error: jutsu_audio_extensions::ExtensionError) -> CliFailure {
    (6, "invalid_parameter", error.message)
}

const fn kind_name(kind: ExtensionKind) -> &'static str {
    match kind {
        ExtensionKind::Synth => "synth",
        ExtensionKind::Effect => "effect",
        ExtensionKind::Generator => "generator",
    }
}

const fn value_type_name(value_type: ParameterType) -> &'static str {
    match value_type {
        ParameterType::Float => "float",
        ParameterType::Integer => "integer",
        ParameterType::Bool => "bool",
        ParameterType::Text => "text",
    }
}
