//! The generator half of the machine surface: discovery, validation, and
//! running a recipe into commands.
//!
//! A caller that has never seen this build can list the generators, read their
//! parameter schemas and presets, preview a seed, and only then commit the
//! result to a project — without parsing a word of prose.

use std::collections::BTreeMap;

use jutsu_audio_extensions::generators::GeneratorPreset;
use jutsu_audio_extensions::{
    ExtensionDescriptor, ExtensionRegistries, ExtensionTypeId, GeneratorRecipe, RegenerateMode,
};
use jutsu_audio_model::{AssetId, ClipId, ParameterValue};
use serde_json::{Value, json};

use crate::cli_session::CliFailure;
use crate::cli_synth;

/// One generator's full schema: what it takes, within what bounds, and where to
/// start.
#[must_use]
pub fn describe(descriptor: &ExtensionDescriptor, presets: &[GeneratorPreset]) -> Value {
    let mut described = cli_synth::describe(descriptor);
    described["presets"] = presets
        .iter()
        .map(|preset| json!({"name": preset.name, "parameters": preset.parameters}))
        .collect();
    described
}

/// Looks a generator up and checks a recipe's parameters against its
/// descriptor, including declared bounds.
pub fn validate(
    registries: &ExtensionRegistries,
    recipe: &GeneratorRecipe,
) -> Result<(), CliFailure> {
    let type_id = ExtensionTypeId::new(recipe.generator_type.clone())
        .map_err(|error| (6, "invalid_parameter", error.message))?;
    if registries.generator_descriptor(&type_id).is_none() {
        let available: Vec<&str> = registries
            .generator_type_ids()
            .map(ExtensionTypeId::as_str)
            .collect();
        return Err((
            6,
            "unknown_extension",
            format!(
                "no generator '{}' is registered; this build has {}",
                recipe.generator_type,
                available.join(", ")
            ),
        ));
    }
    registries
        .instantiate_generator(&type_id, &recipe.parameters)
        .map(|_| ())
        .map_err(|error| {
            let code = if error.parameter_id.is_some() {
                "invalid_parameter"
            } else {
                "unknown_extension"
            };
            (6, code, error.message)
        })
}

/// Renders a recipe without touching a project — what "preview" means for a
/// caller that wants to hear a seed before committing to it.
pub fn render(
    registries: &ExtensionRegistries,
    recipe: &GeneratorRecipe,
) -> Result<Vec<f32>, CliFailure> {
    validate(registries, recipe)?;
    let type_id = ExtensionTypeId::new(recipe.generator_type.clone())
        .map_err(|error| (6, "invalid_parameter", error.message))?;
    let frames = usize::try_from(recipe.frame_count).map_err(|_| {
        (
            6,
            "invalid_parameter",
            "frame_count is too large".to_owned(),
        )
    })?;
    let generator = registries
        .instantiate_generator(&type_id, &recipe.parameters)
        .map_err(|error| (6, "invalid_parameter", error.message))?;
    Ok(generator.generate_mono(recipe.seed, frames))
}

/// The entities a recipe produces. Derived from the recipe, so two runs of the
/// same recipe name the same asset and clip — which is what `Replace` needs and
/// what makes a rerun comparable byte for byte.
#[must_use]
pub fn identity(recipe: &GeneratorRecipe, mode: RegenerateMode, salt: u64) -> (AssetId, ClipId) {
    match mode {
        RegenerateMode::Replace => (
            AssetId::from_uuid(recipe.derive_uuid("asset")),
            ClipId::from_uuid(recipe.derive_uuid("clip")),
        ),
        // A variant is a new entity by definition, so the caller's salt joins
        // the derivation. Same salt, same IDs.
        RegenerateMode::New => (
            AssetId::from_uuid(recipe.derive_uuid(&format!("asset.{salt}"))),
            ClipId::from_uuid(recipe.derive_uuid(&format!("clip.{salt}"))),
        ),
    }
}

/// Peak, RMS and a fingerprint of the samples: enough for a caller to tell two
/// renders apart, or to prove they are the same, without keeping the audio.
#[must_use]
pub fn summarise(samples: &[f32]) -> Value {
    let peak = samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    let sum_squares: f64 = samples
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum();
    let rms = if samples.is_empty() {
        0.0
    } else {
        (sum_squares / samples.len() as f64).sqrt()
    };
    json!({
        "frame_count": samples.len(),
        "peak": peak,
        "rms": rms,
        "fingerprint": fingerprint(samples),
    })
}

/// FNV-1a over the sample bit patterns. Specified here rather than taken from a
/// standard hasher, because a caller compares these across builds.
fn fingerprint(samples: &[f32]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for sample in samples {
        for byte in sample.to_bits().to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

/// Turns the parameters a caller sent into the map a recipe holds.
#[must_use]
pub fn recipe(
    generator_type: String,
    algorithm_version: u32,
    seed: u64,
    frame_count: u64,
    parameters: BTreeMap<String, ParameterValue>,
) -> GeneratorRecipe {
    GeneratorRecipe {
        generator_type,
        algorithm_version,
        seed,
        frame_count,
        parameters,
    }
}
