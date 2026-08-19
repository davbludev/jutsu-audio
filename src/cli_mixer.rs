//! The mixer half of the machine surface: strips, routing, effect racks and
//! automation.
//!
//! Values are validated the same way a synth's are — against a descriptor —
//! because a track's level is a parameter like any other. The descriptors for a
//! strip come from `jutsu-audio-extensions::parameters`, which is also what the
//! GUI validates against.

use std::collections::BTreeMap;

use jutsu_audio_extensions::parameters::{Preset, strip_parameters, validate_named};
use jutsu_audio_extensions::{ExtensionRegistries, ExtensionTypeId};
use jutsu_audio_model::ParameterValue;
use serde_json::{Value, json};

use crate::cli_session::CliFailure;
use crate::cli_synth;

/// Checks one strip parameter — a track's or a bus's — before it is applied.
pub fn validate_strip(id: &str, value: &ParameterValue) -> Result<(), CliFailure> {
    validate_named(&strip_parameters(), id, value).map_err(|error| {
        let code = if error.message.contains("is not a parameter here") {
            "unknown_parameter"
        } else {
            "invalid_parameter"
        };
        (6, code, error.message)
    })
}

/// The parameters every strip has, as a caller can discover them.
#[must_use]
pub fn describe_strip() -> Value {
    json!({
        "parameters": strip_parameters()
            .iter()
            .map(|parameter| json!({
                "id": parameter.id,
                "display_name": parameter.display_name,
                "value_type": value_type(parameter.value_type),
                "default_value": parameter.default_value,
                "minimum": parameter.minimum,
                "maximum": parameter.maximum,
                "unit": parameter.unit,
                "automatable": parameter.automatable,
            }))
            .collect::<Vec<_>>(),
    })
}

/// One effect's schema, with the presets it ships.
#[must_use]
pub fn describe_effect(
    registries: &ExtensionRegistries,
    type_id: &ExtensionTypeId,
) -> Option<Value> {
    let descriptor = registries.effect_descriptor(type_id)?;
    let presets: &[Preset] = registries.effect_presets(type_id).unwrap_or_default();
    let mut described = cli_synth::describe(descriptor);
    described["presets"] = presets
        .iter()
        .map(|preset| json!({"name": preset.name, "parameters": preset.parameters}))
        .collect();
    Some(described)
}

/// Looks an effect up and checks the parameters a caller sent for it.
///
/// Returns the descriptor's state version, which the project stores so a later
/// build can tell that the extension has moved on.
pub fn validate_effect(
    registries: &ExtensionRegistries,
    type_id: &str,
    parameters: &BTreeMap<String, ParameterValue>,
) -> Result<u32, CliFailure> {
    let parsed =
        ExtensionTypeId::new(type_id).map_err(|error| (6, "invalid_parameter", error.message))?;
    let Some(descriptor) = registries.effect_descriptor(&parsed) else {
        let available: Vec<&str> = registries
            .effect_type_ids()
            .map(ExtensionTypeId::as_str)
            .collect();
        return Err((
            6,
            "unknown_extension",
            format!(
                "no effect '{type_id}' is registered; this build has {}",
                available.join(", ")
            ),
        ));
    };
    for (id, value) in parameters {
        validate_named(&descriptor.parameters, id, value).map_err(|error| {
            let code = if error.message.contains("is not a parameter here") {
                "unknown_parameter"
            } else {
                "invalid_parameter"
            };
            (6, code, error.message)
        })?;
    }
    registries
        .instantiate_effect(&parsed, parameters)
        .map_err(|error| (6, "invalid_parameter", error.message))?;
    Ok(descriptor.state_version)
}

const fn value_type(value_type: jutsu_audio_extensions::ParameterType) -> &'static str {
    match value_type {
        jutsu_audio_extensions::ParameterType::Float => "float",
        jutsu_audio_extensions::ParameterType::Integer => "integer",
        jutsu_audio_extensions::ParameterType::Bool => "bool",
        jutsu_audio_extensions::ParameterType::Text => "text",
    }
}
