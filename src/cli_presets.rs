//! Presets on the machine surface: what exists, saving one from a project, and
//! applying one back.
//!
//! Built-in presets come from the registries and are read-only. User presets
//! come from a library directory the caller names, so an agent can keep its own
//! library beside a project without touching anyone else's.

use std::path::{Path, PathBuf};

use jutsu_audio_extensions::ExtensionRegistries;
use jutsu_audio_extensions::parameters::Preset as BuiltinPreset;
use jutsu_audio_model::{AudioAssetSource, EffectInsert, Project};
use jutsu_audio_project::presets::{
    ChainStep, Incompatibility, Preset, PresetKind, PresetLibrary, PresetPayload, check,
};
use serde_json::{Value, json};

use crate::cli_session::CliFailure;

/// Where user presets live when a caller does not say: beside the project, in a
/// directory it can copy or commit along with it.
#[must_use]
pub fn default_library(project_path: &Path) -> PathBuf {
    project_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("presets")
}

/// Built-in presets, from the extensions themselves.
#[must_use]
pub fn builtin(registries: &ExtensionRegistries) -> Vec<Value> {
    let mut listed = Vec::new();
    for type_id in registries.effect_type_ids() {
        let presets: &[BuiltinPreset] = registries.effect_presets(type_id).unwrap_or_default();
        listed.extend(presets.iter().map(|preset| {
            json!({
                "source": "builtin",
                "kind": "effect",
                "type_id": type_id.as_str(),
                "name": preset.name,
                "parameters": preset.parameters,
            })
        }));
    }
    for type_id in registries.generator_type_ids() {
        let presets: &[BuiltinPreset] = registries.generator_presets(type_id).unwrap_or_default();
        listed.extend(presets.iter().map(|preset| {
            json!({
                "source": "builtin",
                "kind": "generator",
                "type_id": type_id.as_str(),
                "name": preset.name,
                "parameters": preset.parameters,
            })
        }));
    }
    listed
}

/// A user preset as a caller reads it, with any reason it will not fit.
#[must_use]
pub fn describe(preset: &Preset, problems: &[Incompatibility]) -> Value {
    json!({
        "source": "user",
        "id": preset.id,
        "name": preset.name,
        "kind": kind_name(preset.kind),
        "schema_version": preset.schema_version,
        "tags": preset.tags,
        "type_id": preset.type_id(),
        "state_version": preset.state_version(),
        "payload": preset.payload,
        "incompatibilities": problems
            .iter()
            .map(|problem| json!({"code": code_name(problem.code), "message": problem.message}))
            .collect::<Vec<_>>(),
    })
}

/// Checks a preset against what this build has registered.
#[must_use]
pub fn incompatibilities(
    preset: &Preset,
    registries: &ExtensionRegistries,
) -> Vec<Incompatibility> {
    check(preset, |type_id| {
        let parsed = jutsu_audio_extensions::ExtensionTypeId::new(type_id).ok()?;
        registries
            .effect_descriptor(&parsed)
            .or_else(|| registries.synth_descriptor(&parsed))
            .or_else(|| registries.generator_descriptor(&parsed))
            .map(|descriptor| descriptor.state_version)
    })
}

/// Builds a preset from something in a project.
pub fn capture(
    project: &Project,
    kind: PresetKind,
    id: &str,
    name: &str,
    target: &CaptureTarget,
) -> Result<Preset, CliFailure> {
    let payload = match target {
        CaptureTarget::Asset { asset_id } => {
            let asset = project
                .assets
                .iter()
                .find(|asset| asset.id == *asset_id)
                .ok_or_else(|| {
                    (
                        4,
                        "command_failed",
                        format!("asset {asset_id} does not exist"),
                    )
                })?;
            match &asset.source {
                AudioAssetSource::Synth {
                    type_id,
                    state_version,
                    parameters,
                } => PresetPayload::Parameters {
                    type_id: type_id.clone(),
                    state_version: *state_version,
                    parameters: parameters.clone(),
                },
                AudioAssetSource::Generated {
                    generator_type,
                    algorithm_version,
                    parameters,
                    ..
                } => PresetPayload::Parameters {
                    type_id: generator_type.clone(),
                    state_version: *algorithm_version,
                    parameters: parameters.clone(),
                },
                AudioAssetSource::Sampler {
                    zones,
                    attack_ms,
                    release_ms,
                    max_voices,
                } => PresetPayload::Instrument {
                    zones: zones.clone(),
                    attack_ms: *attack_ms,
                    release_ms: *release_ms,
                    max_voices: *max_voices,
                },
                _ => {
                    return Err((
                        6,
                        "invalid_parameter",
                        format!("asset {asset_id} is a file, and a file is not a preset"),
                    ));
                }
            }
        }
        CaptureTarget::Chain { effects } => PresetPayload::Chain {
            steps: effects.iter().map(step_of).collect(),
        },
    };

    let mut preset = Preset::new(id, name, kind, payload);
    preset.tags.sort();
    Ok(preset)
}

/// What a preset is captured from.
pub enum CaptureTarget<'a> {
    /// A synth, generator or sampler asset.
    Asset {
        asset_id: jutsu_audio_model::AssetId,
    },
    /// A track or bus chain, as it stands.
    Chain { effects: &'a [EffectInsert] },
}

fn step_of(insert: &EffectInsert) -> ChainStep {
    ChainStep {
        type_id: insert.type_id.clone(),
        state_version: insert.state_version,
        parameters: insert.parameters.clone(),
        enabled: insert.enabled,
        wet: insert.wet,
    }
}

/// The inserts a chain preset becomes, with fresh IDs for this project.
#[must_use]
pub fn inserts_of(steps: &[ChainStep]) -> Vec<EffectInsert> {
    steps
        .iter()
        .map(|step| EffectInsert {
            id: jutsu_audio_model::EffectId::new(),
            type_id: step.type_id.clone(),
            state_version: step.state_version,
            parameters: step.parameters.clone(),
            enabled: step.enabled,
            wet: step.wet,
            // A preset describes a chain, not the mix it lands in: which track
            // keys a compressor is a property of this project, not of the
            // preset, so applying one never brings a routing decision with it.
            sidechain: None,
        })
        .collect()
}

/// Opens a library, defaulting to the one beside the project.
#[must_use]
pub fn library(project_path: &Path, requested: Option<&Path>) -> PresetLibrary {
    PresetLibrary::new(requested.map_or_else(|| default_library(project_path), Path::to_path_buf))
}

const fn kind_name(kind: PresetKind) -> &'static str {
    match kind {
        PresetKind::Synth => "synth",
        PresetKind::Effect => "effect",
        PresetKind::Chain => "chain",
        PresetKind::Generator => "generator",
        PresetKind::Instrument => "instrument",
    }
}

const fn code_name(code: jutsu_audio_project::presets::IncompatibilityCode) -> &'static str {
    use jutsu_audio_project::presets::IncompatibilityCode as Code;
    match code {
        Code::NewerSchema => "newer_schema",
        Code::UnavailableType => "unavailable_type",
        Code::StateVersionMismatch => "state_version_mismatch",
    }
}
