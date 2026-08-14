use std::collections::{BTreeMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const CURRENT_PROJECT_SCHEMA_VERSION: u32 = 1;

macro_rules! entity_id {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

entity_id!(ProjectId);
entity_id!(AssetId);
entity_id!(TrackId);
entity_id!(LayerId);
entity_id!(ClipId);
entity_id!(BusId);

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Project {
    pub schema_version: u32,
    pub id: ProjectId,
    pub metadata: ProjectMetadata,
    pub assets: Vec<Asset>,
    pub buses: Vec<MixerBus>,
    pub master_bus_id: BusId,
    pub tracks: Vec<Track>,
}

impl Project {
    #[must_use]
    pub fn validate(&self) -> Vec<ValidationDiagnostic> {
        let mut diagnostics = Vec::new();

        if self.schema_version != CURRENT_PROJECT_SCHEMA_VERSION {
            diagnostics.push(ValidationDiagnostic::new(
                ValidationCode::UnsupportedSchemaVersion,
                "schema_version",
                Some(self.id.to_string()),
                format!(
                    "project schema version {} is unsupported; expected {}",
                    self.schema_version, CURRENT_PROJECT_SCHEMA_VERSION
                ),
            ));
        }

        validate_unique_ids(
            self.assets.iter().map(|asset| asset.id),
            "assets",
            &mut diagnostics,
        );
        validate_unique_ids(
            self.buses.iter().map(|bus| bus.id),
            "buses",
            &mut diagnostics,
        );
        validate_unique_ids(
            self.tracks.iter().map(|track| track.id),
            "tracks",
            &mut diagnostics,
        );

        let asset_ids: HashSet<_> = self.assets.iter().map(|asset| asset.id).collect();
        let bus_ids: HashSet<_> = self.buses.iter().map(|bus| bus.id).collect();

        if !bus_ids.contains(&self.master_bus_id) {
            diagnostics.push(ValidationDiagnostic::new(
                ValidationCode::MissingBusReference,
                "master_bus_id",
                Some(self.master_bus_id.to_string()),
                "master bus does not exist",
            ));
        }

        for (bus_index, bus) in self.buses.iter().enumerate() {
            if let Some(output_bus_id) = bus.output_bus_id
                && !bus_ids.contains(&output_bus_id)
            {
                diagnostics.push(ValidationDiagnostic::new(
                    ValidationCode::MissingBusReference,
                    format!("buses[{bus_index}].output_bus_id"),
                    Some(bus.id.to_string()),
                    format!("output bus {output_bus_id} does not exist"),
                ));
            }
        }

        let mut clip_ids = HashSet::new();
        for (track_index, track) in self.tracks.iter().enumerate() {
            if !bus_ids.contains(&track.output_bus_id) {
                diagnostics.push(ValidationDiagnostic::new(
                    ValidationCode::MissingBusReference,
                    format!("tracks[{track_index}].output_bus_id"),
                    Some(track.id.to_string()),
                    format!("output bus {} does not exist", track.output_bus_id),
                ));
            }

            validate_unique_ids(
                track.layers.iter().map(|layer| layer.id),
                format!("tracks[{track_index}].layers"),
                &mut diagnostics,
            );

            for (layer_index, layer) in track.layers.iter().enumerate() {
                for (clip_index, clip) in layer.clips.iter().enumerate() {
                    let path =
                        format!("tracks[{track_index}].layers[{layer_index}].clips[{clip_index}]");

                    if !clip_ids.insert(clip.id) {
                        diagnostics.push(ValidationDiagnostic::new(
                            ValidationCode::DuplicateEntityId,
                            format!("{path}.id"),
                            Some(clip.id.to_string()),
                            "duplicate clip ID in track",
                        ));
                    }
                    if !asset_ids.contains(&clip.asset_id) {
                        diagnostics.push(ValidationDiagnostic::new(
                            ValidationCode::MissingAssetReference,
                            format!("{path}.asset_id"),
                            Some(clip.id.to_string()),
                            format!("asset {} does not exist", clip.asset_id),
                        ));
                    }
                    if clip.duration_samples == 0
                        || clip
                            .start_sample
                            .checked_add(clip.duration_samples)
                            .is_none()
                        || clip
                            .source_start_sample
                            .checked_add(clip.duration_samples)
                            .is_none()
                    {
                        diagnostics.push(ValidationDiagnostic::new(
                            ValidationCode::InvalidClipRange,
                            format!("{path}.duration_samples"),
                            Some(clip.id.to_string()),
                            "clip duration must be positive and sample ranges must not overflow",
                        ));
                    }
                }
            }
        }

        diagnostics
    }
}

fn validate_unique_ids<T>(
    ids: impl IntoIterator<Item = T>,
    path: impl Into<String>,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) where
    T: Copy + Eq + std::hash::Hash + fmt::Display,
{
    let path = path.into();
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(id) {
            diagnostics.push(ValidationDiagnostic::new(
                ValidationCode::DuplicateEntityId,
                format!("{path}.id"),
                Some(id.to_string()),
                "duplicate entity ID",
            ));
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ProjectMetadata {
    pub name: String,
    #[serde(default)]
    pub properties: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Asset {
    pub id: AssetId,
    pub name: String,
    pub source: AudioAssetSource,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AudioAssetSource {
    File {
        path: String,
    },
    Generated {
        generator_type: String,
        algorithm_version: u32,
        seed: u64,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Track {
    pub id: TrackId,
    pub name: String,
    pub output_bus_id: BusId,
    #[serde(default)]
    pub parameters: BTreeMap<String, ParameterValue>,
    #[serde(default)]
    pub layers: Vec<Layer>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    #[serde(default)]
    pub clips: Vec<Clip>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Clip {
    pub id: ClipId,
    pub asset_id: AssetId,
    pub start_sample: u64,
    pub source_start_sample: u64,
    pub duration_samples: u64,
    #[serde(default)]
    pub parameters: BTreeMap<String, ParameterValue>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MixerBus {
    pub id: BusId,
    pub name: String,
    pub output_bus_id: Option<BusId>,
    #[serde(default)]
    pub parameters: BTreeMap<String, ParameterValue>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ParameterValue {
    Float(f64),
    Integer(i64),
    Bool(bool),
    Text(String),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationCode {
    UnsupportedSchemaVersion,
    DuplicateEntityId,
    MissingAssetReference,
    MissingBusReference,
    InvalidClipRange,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ValidationDiagnostic {
    pub code: ValidationCode,
    pub path: String,
    pub entity_id: Option<String>,
    pub message: String,
}

impl ValidationDiagnostic {
    fn new(
        code: ValidationCode,
        path: impl Into<String>,
        entity_id: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            path: path.into(),
            entity_id,
            message: message.into(),
        }
    }
}
