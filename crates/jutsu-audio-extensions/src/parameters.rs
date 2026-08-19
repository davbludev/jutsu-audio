//! One parameter API for everything that has parameters.
//!
//! A synth's cutoff, an effect's mix, a track's level and a bus's pan are all
//! described the same way — type, unit, default, range — and validated by the
//! same function. That is what lets the GUI and the CLI reject the same values
//! for the same reasons, and what an automation lane needs in order to know
//! what it is allowed to write.

use jutsu_audio_model::ParameterValue;

use crate::{ExtensionError, ExtensionErrorCode, ParameterDescriptor, ParameterType};

/// Decibels, the unit for every level in the project.
pub const UNIT_DECIBELS: &str = "dB";
/// A stereo position from `-1.0` to `1.0`.
pub const UNIT_PAN: &str = "pan";
/// Milliseconds.
pub const UNIT_MILLISECONDS: &str = "ms";
/// Hertz.
pub const UNIT_HERTZ: &str = "Hz";
/// A plain `0.0..1.0` amount with no dimension.
pub const UNIT_NORMALISED: &str = "ratio";

/// Level on a track, bus or clip.
pub const GAIN_DB: &str = "gain_db";
/// Stereo position on a track, bus or clip.
pub const PAN: &str = "pan";
/// Whether a track is silent.
pub const MUTE: &str = "mute";
/// Whether a track plays to the exclusion of un-soloed ones.
pub const SOLO: &str = "solo";

/// The parameters every channel strip has. Mixer edits validate against these
/// exactly as a synth edit validates against its extension's descriptor.
#[must_use]
pub fn strip_parameters() -> Vec<ParameterDescriptor> {
    vec![
        ParameterDescriptor {
            id: GAIN_DB.into(),
            display_name: "Level".into(),
            value_type: ParameterType::Float,
            default_value: ParameterValue::Float(0.0),
            introduced_in_state_version: 1,
            automatable: true,
            minimum: Some(-60.0),
            maximum: Some(12.0),
            unit: Some(UNIT_DECIBELS.into()),
        },
        ParameterDescriptor {
            id: PAN.into(),
            display_name: "Pan".into(),
            value_type: ParameterType::Float,
            default_value: ParameterValue::Float(0.0),
            introduced_in_state_version: 1,
            automatable: true,
            minimum: Some(-1.0),
            maximum: Some(1.0),
            unit: Some(UNIT_PAN.into()),
        },
        ParameterDescriptor {
            id: MUTE.into(),
            display_name: "Mute".into(),
            value_type: ParameterType::Bool,
            default_value: ParameterValue::Bool(false),
            introduced_in_state_version: 1,
            automatable: false,
            minimum: None,
            maximum: None,
            unit: None,
        },
        ParameterDescriptor {
            id: SOLO.into(),
            display_name: "Solo".into(),
            value_type: ParameterType::Bool,
            default_value: ParameterValue::Bool(false),
            introduced_in_state_version: 1,
            automatable: false,
            minimum: None,
            maximum: None,
            unit: None,
        },
    ]
}

/// Finds a declared parameter by ID.
#[must_use]
pub fn find<'a>(
    parameters: &'a [ParameterDescriptor],
    id: &str,
) -> Option<&'a ParameterDescriptor> {
    parameters.iter().find(|parameter| parameter.id == id)
}

/// Checks one value against one declared parameter: right type, inside range.
///
/// The single place that answers "is this allowed?", so a value the CLI accepts
/// is a value the GUI accepts and vice versa.
pub fn validate_value(
    parameter: &ParameterDescriptor,
    value: &ParameterValue,
) -> Result<(), ExtensionError> {
    if !accepts(parameter.value_type, value) {
        return Err(ExtensionError {
            code: ExtensionErrorCode::InvalidParameters,
            message: format!(
                "parameter '{}' takes a {}",
                parameter.id,
                type_name(parameter.value_type)
            ),
            kind: None,
            type_id: None,
            parameter_id: Some(parameter.id.clone()),
        });
    }
    let Some(number) = numeric(value) else {
        return Ok(());
    };
    if let Some(minimum) = parameter.minimum
        && number < minimum
    {
        return Err(range_error(
            parameter,
            format!("below its minimum of {minimum}"),
        ));
    }
    if let Some(maximum) = parameter.maximum
        && number > maximum
    {
        return Err(range_error(
            parameter,
            format!("above its maximum of {maximum}"),
        ));
    }
    Ok(())
}

/// Checks a value against a named parameter of a set, reporting an unknown name
/// with the names that would have worked.
pub fn validate_named(
    parameters: &[ParameterDescriptor],
    id: &str,
    value: &ParameterValue,
) -> Result<(), ExtensionError> {
    let Some(parameter) = find(parameters, id) else {
        let declared: Vec<&str> = parameters
            .iter()
            .map(|parameter| parameter.id.as_str())
            .collect();
        return Err(ExtensionError {
            code: ExtensionErrorCode::InvalidParameters,
            message: format!(
                "'{id}' is not a parameter here; try {}",
                declared.join(", ")
            ),
            kind: None,
            type_id: None,
            parameter_id: Some(id.to_owned()),
        });
    };
    validate_value(parameter, value)
}

/// The value a parameter has when nothing has set it.
#[must_use]
pub fn default_of(parameters: &[ParameterDescriptor], id: &str) -> Option<ParameterValue> {
    find(parameters, id).map(|parameter| parameter.default_value.clone())
}

fn range_error(parameter: &ParameterDescriptor, what: String) -> ExtensionError {
    ExtensionError {
        code: ExtensionErrorCode::InvalidParameters,
        message: format!("parameter '{}' is {what}", parameter.id),
        kind: None,
        type_id: None,
        parameter_id: Some(parameter.id.clone()),
    }
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

fn numeric(value: &ParameterValue) -> Option<f64> {
    match value {
        ParameterValue::Float(value) => Some(*value),
        ParameterValue::Integer(value) => Some(*value as f64),
        _ => None,
    }
}

const fn type_name(value_type: ParameterType) -> &'static str {
    match value_type {
        ParameterType::Float => "float",
        ParameterType::Integer => "integer",
        ParameterType::Bool => "bool",
        ParameterType::Text => "text",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_level_outside_the_strip_range_is_refused_with_its_bound() {
        let parameters = strip_parameters();
        let error = validate_named(&parameters, GAIN_DB, &ParameterValue::Float(24.0))
            .expect_err("above the maximum");
        assert_eq!(error.parameter_id.as_deref(), Some(GAIN_DB));
        assert!(error.message.contains("12"), "{}", error.message);

        validate_named(&parameters, GAIN_DB, &ParameterValue::Float(-6.0)).expect("in range");
    }

    #[test]
    fn a_value_of_the_wrong_type_is_refused_by_name() {
        let parameters = strip_parameters();
        let error = validate_named(&parameters, MUTE, &ParameterValue::Float(1.0))
            .expect_err("mute is a switch");
        assert!(error.message.contains("bool"), "{}", error.message);
    }

    #[test]
    fn an_unknown_parameter_lists_the_ones_that_exist() {
        let parameters = strip_parameters();
        let error = validate_named(&parameters, "widh", &ParameterValue::Float(1.0))
            .expect_err("no such parameter");
        assert!(error.message.contains("pan"), "{}", error.message);
    }

    #[test]
    fn the_strip_publishes_units_and_defaults() {
        let parameters = strip_parameters();
        let gain = find(&parameters, GAIN_DB).expect("a level");
        assert_eq!(gain.unit.as_deref(), Some(UNIT_DECIBELS));
        assert!(gain.automatable, "levels are automatable");
        assert_eq!(
            default_of(&parameters, PAN),
            Some(ParameterValue::Float(0.0)),
            "centre is the default pan"
        );
    }
}
