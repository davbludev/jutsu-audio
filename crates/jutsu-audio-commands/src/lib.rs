use std::fmt;

use jutsu_audio_model::{
    Asset, AssetId, AudioAssetSource, Clip, ClipId, ClipNote, Layer, LayerId, LoopRegion, Marker,
    MarkerId, ParameterValue, Project, Track, TrackId, ValidationDiagnostic,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod edits;
mod history;

pub use history::{CommandHistory, HISTORY_LIMIT, invert};

pub const COMMAND_PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct CommandId(Uuid);

impl CommandId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }
}

impl Default for CommandId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CommandEnvelope {
    pub protocol_version: u32,
    pub command_id: CommandId,
    pub expected_revision: u64,
    pub commands: Vec<ProjectCommand>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProjectCommand {
    SetProjectName {
        name: String,
    },
    AddAsset {
        asset: Asset,
    },
    RemoveAsset {
        asset_id: AssetId,
    },
    AddClip {
        track_id: TrackId,
        layer_id: LayerId,
        clip: Clip,
    },
    UpdateClip {
        clip_id: ClipId,
        start_sample: u64,
        source_start_sample: u64,
        duration_samples: u64,
        gain_db: f64,
    },
    RemoveClip {
        clip_id: ClipId,
    },
    /// Appends a track. Order is the project's own order, which is also the
    /// order the mix sums in.
    AddTrack {
        track: Track,
    },
    RemoveTrack {
        track_id: TrackId,
    },
    /// Appends a layer to a track. Layers are lanes within one track.
    AddLayer {
        track_id: TrackId,
        layer: Layer,
    },
    RemoveLayer {
        layer_id: LayerId,
    },
    SetTrackMute {
        track_id: TrackId,
        muted: bool,
    },
    /// Solo wins over mute: with any track soloed, only soloed tracks play.
    SetTrackSolo {
        track_id: TrackId,
        soloed: bool,
    },
    /// Stereo position, `-1.0` hard left to `1.0` hard right.
    SetClipPan {
        clip_id: ClipId,
        pan: f64,
    },
    /// Fade lengths in project frames, measured from each end of the clip.
    SetClipFades {
        clip_id: ClipId,
        fade_in_samples: u64,
        fade_out_samples: u64,
    },
    AddMarker {
        marker: Marker,
    },
    RemoveMarker {
        marker_id: MarkerId,
    },
    MoveMarker {
        marker_id: MarkerId,
        frame: u64,
    },
    /// `None` clears the loop entirely; a disabled region is remembered.
    SetLoopRegion {
        region: Option<LoopRegion>,
    },
    /// Replaces a synth asset's parameters. Whether the values make sense to
    /// the extension is decided where the registry lives, before this is sent.
    SetAssetParameters {
        asset_id: AssetId,
        parameters: std::collections::BTreeMap<String, ParameterValue>,
    },
    /// Replaces everything a synth clip plays. One command rather than
    /// add/remove-note pairs, so an edit to a phrase is one undo step.
    SetClipNotes {
        clip_id: ClipId,
        notes: Vec<ClipNote>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Updated,
    Removed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Project,
    Asset,
    Clip,
    Track,
    Layer,
    Marker,
    LoopRegion,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChangeEvent {
    pub sequence: u32,
    pub kind: ChangeKind,
    pub entity_kind: EntityKind,
    pub entity_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandOutcome {
    pub command_id: CommandId,
    pub revision: u64,
    pub changes: Vec<ChangeEvent>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandErrorCode {
    UnsupportedProtocolVersion,
    RevisionConflict,
    EmptyBatch,
    EntityNotFound,
    ProjectValidationFailed,
    RevisionOverflow,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandError {
    pub code: CommandErrorCode,
    pub message: String,
    pub command_index: Option<usize>,
    pub expected_revision: Option<u64>,
    pub actual_revision: Option<u64>,
    #[serde(default)]
    pub diagnostics: Vec<ValidationDiagnostic>,
}

impl CommandError {
    fn simple(code: CommandErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            command_index: None,
            expected_revision: None,
            actual_revision: None,
            diagnostics: Vec::new(),
        }
    }

    fn at_command(
        code: CommandErrorCode,
        command_index: usize,
        message: impl Into<String>,
    ) -> Self {
        Self {
            command_index: Some(command_index),
            ..Self::simple(code, message)
        }
    }
}

pub struct ProjectCommandEngine {
    project: Project,
    revision: u64,
}

impl ProjectCommandEngine {
    pub fn new(project: Project) -> Result<Self, CommandError> {
        let diagnostics = project.validate();
        if !diagnostics.is_empty() {
            return Err(CommandError {
                code: CommandErrorCode::ProjectValidationFailed,
                message: "initial project state is invalid".into(),
                command_index: None,
                expected_revision: None,
                actual_revision: None,
                diagnostics,
            });
        }

        Ok(Self {
            project,
            revision: 0,
        })
    }

    #[must_use]
    pub const fn project(&self) -> &Project {
        &self.project
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub fn apply(&mut self, envelope: CommandEnvelope) -> Result<CommandOutcome, CommandError> {
        if envelope.protocol_version != COMMAND_PROTOCOL_VERSION {
            return Err(CommandError::simple(
                CommandErrorCode::UnsupportedProtocolVersion,
                format!(
                    "command protocol version {} is unsupported; expected {}",
                    envelope.protocol_version, COMMAND_PROTOCOL_VERSION
                ),
            ));
        }
        if envelope.expected_revision != self.revision {
            return Err(CommandError {
                code: CommandErrorCode::RevisionConflict,
                message: format!(
                    "expected project revision {}, current revision is {}",
                    envelope.expected_revision, self.revision
                ),
                command_index: None,
                expected_revision: Some(envelope.expected_revision),
                actual_revision: Some(self.revision),
                diagnostics: Vec::new(),
            });
        }
        if envelope.commands.is_empty() {
            return Err(CommandError::simple(
                CommandErrorCode::EmptyBatch,
                "command batch must contain at least one command",
            ));
        }

        let next_revision = self.revision.checked_add(1).ok_or_else(|| {
            CommandError::simple(
                CommandErrorCode::RevisionOverflow,
                "project revision cannot be incremented",
            )
        })?;
        let mut candidate = self.project.clone();
        let mut changes = Vec::with_capacity(envelope.commands.len());

        for (command_index, command) in envelope.commands.iter().enumerate() {
            let change = apply_command(&mut candidate, command, command_index)?;
            changes.push(ChangeEvent {
                sequence: command_index as u32,
                ..change
            });
        }

        let diagnostics = candidate.validate();
        if !diagnostics.is_empty() {
            return Err(CommandError {
                code: CommandErrorCode::ProjectValidationFailed,
                message: "command batch would leave project state invalid".into(),
                command_index: None,
                expected_revision: None,
                actual_revision: Some(self.revision),
                diagnostics,
            });
        }

        self.project = candidate;
        self.revision = next_revision;

        Ok(CommandOutcome {
            command_id: envelope.command_id,
            revision: next_revision,
            changes,
        })
    }
}

pub(crate) fn apply_command(
    project: &mut Project,
    command: &ProjectCommand,
    command_index: usize,
) -> Result<ChangeEvent, CommandError> {
    let change = match command {
        ProjectCommand::SetProjectName { name } => {
            project.metadata.name.clone_from(name);
            ChangeEvent {
                sequence: 0,
                kind: ChangeKind::Updated,
                entity_kind: EntityKind::Project,
                entity_id: project.id.to_string(),
            }
        }
        ProjectCommand::AddAsset { asset } => {
            project.assets.push(asset.clone());
            ChangeEvent {
                sequence: 0,
                kind: ChangeKind::Added,
                entity_kind: EntityKind::Asset,
                entity_id: asset.id.to_string(),
            }
        }
        ProjectCommand::RemoveAsset { asset_id } => {
            let asset_index = project
                .assets
                .iter()
                .position(|asset| asset.id == *asset_id)
                .ok_or_else(|| {
                    CommandError::at_command(
                        CommandErrorCode::EntityNotFound,
                        command_index,
                        format!("asset {asset_id} does not exist"),
                    )
                })?;
            project.assets.remove(asset_index);
            ChangeEvent {
                sequence: 0,
                kind: ChangeKind::Removed,
                entity_kind: EntityKind::Asset,
                entity_id: asset_id.to_string(),
            }
        }
        ProjectCommand::AddClip {
            track_id,
            layer_id,
            clip,
        } => {
            let track = project
                .tracks
                .iter_mut()
                .find(|track| track.id == *track_id)
                .ok_or_else(|| {
                    CommandError::at_command(
                        CommandErrorCode::EntityNotFound,
                        command_index,
                        format!("track {track_id} does not exist"),
                    )
                })?;
            let layer = track
                .layers
                .iter_mut()
                .find(|layer| layer.id == *layer_id)
                .ok_or_else(|| {
                    CommandError::at_command(
                        CommandErrorCode::EntityNotFound,
                        command_index,
                        format!("layer {layer_id} does not exist in track {track_id}"),
                    )
                })?;
            layer.clips.push(clip.clone());
            ChangeEvent {
                sequence: 0,
                kind: ChangeKind::Added,
                entity_kind: EntityKind::Clip,
                entity_id: clip.id.to_string(),
            }
        }
        ProjectCommand::UpdateClip {
            clip_id,
            start_sample,
            source_start_sample,
            duration_samples,
            gain_db,
        } => {
            let clip = project
                .tracks
                .iter_mut()
                .flat_map(|track| &mut track.layers)
                .flat_map(|layer| &mut layer.clips)
                .find(|clip| clip.id == *clip_id)
                .ok_or_else(|| {
                    CommandError::at_command(
                        CommandErrorCode::EntityNotFound,
                        command_index,
                        format!("clip {clip_id} does not exist"),
                    )
                })?;
            clip.start_sample = *start_sample;
            clip.source_start_sample = *source_start_sample;
            clip.duration_samples = *duration_samples;
            clip.parameters.insert(
                "gain_db".into(),
                jutsu_audio_model::ParameterValue::Float(*gain_db),
            );
            ChangeEvent {
                sequence: 0,
                kind: ChangeKind::Updated,
                entity_kind: EntityKind::Clip,
                entity_id: clip_id.to_string(),
            }
        }
        ProjectCommand::AddTrack { track } => {
            project.tracks.push(track.clone());
            ChangeEvent {
                sequence: 0,
                kind: ChangeKind::Added,
                entity_kind: EntityKind::Track,
                entity_id: track.id.to_string(),
            }
        }
        ProjectCommand::RemoveTrack { track_id } => {
            let index = project
                .tracks
                .iter()
                .position(|track| track.id == *track_id)
                .ok_or_else(|| {
                    CommandError::at_command(
                        CommandErrorCode::EntityNotFound,
                        command_index,
                        format!("track {track_id} does not exist"),
                    )
                })?;
            project.tracks.remove(index);
            ChangeEvent {
                sequence: 0,
                kind: ChangeKind::Removed,
                entity_kind: EntityKind::Track,
                entity_id: track_id.to_string(),
            }
        }
        ProjectCommand::AddLayer { track_id, layer } => {
            let track = find_track(project, *track_id, command_index)?;
            track.layers.push(layer.clone());
            ChangeEvent {
                sequence: 0,
                kind: ChangeKind::Added,
                entity_kind: EntityKind::Layer,
                entity_id: layer.id.to_string(),
            }
        }
        ProjectCommand::RemoveLayer { layer_id } => {
            let layers = project
                .tracks
                .iter_mut()
                .map(|track| &mut track.layers)
                .find(|layers| layers.iter().any(|layer| layer.id == *layer_id))
                .ok_or_else(|| {
                    CommandError::at_command(
                        CommandErrorCode::EntityNotFound,
                        command_index,
                        format!("layer {layer_id} does not exist"),
                    )
                })?;
            let index = layers
                .iter()
                .position(|layer| layer.id == *layer_id)
                .expect("found");
            layers.remove(index);
            ChangeEvent {
                sequence: 0,
                kind: ChangeKind::Removed,
                entity_kind: EntityKind::Layer,
                entity_id: layer_id.to_string(),
            }
        }
        ProjectCommand::SetTrackMute { track_id, muted } => {
            let track = find_track(project, *track_id, command_index)?;
            track
                .parameters
                .insert("mute".into(), ParameterValue::Bool(*muted));
            ChangeEvent {
                sequence: 0,
                kind: ChangeKind::Updated,
                entity_kind: EntityKind::Track,
                entity_id: track_id.to_string(),
            }
        }
        ProjectCommand::SetTrackSolo { track_id, soloed } => {
            let track = find_track(project, *track_id, command_index)?;
            track
                .parameters
                .insert("solo".into(), ParameterValue::Bool(*soloed));
            ChangeEvent {
                sequence: 0,
                kind: ChangeKind::Updated,
                entity_kind: EntityKind::Track,
                entity_id: track_id.to_string(),
            }
        }
        ProjectCommand::SetClipPan { clip_id, pan } => {
            let clip = find_clip_mut(project, *clip_id, command_index)?;
            clip.parameters
                .insert("pan".into(), ParameterValue::Float(pan.clamp(-1.0, 1.0)));
            ChangeEvent {
                sequence: 0,
                kind: ChangeKind::Updated,
                entity_kind: EntityKind::Clip,
                entity_id: clip_id.to_string(),
            }
        }
        ProjectCommand::SetClipFades {
            clip_id,
            fade_in_samples,
            fade_out_samples,
        } => {
            let clip = find_clip_mut(project, *clip_id, command_index)?;
            let (fade_in, fade_out) =
                edits::clamp_fades(clip.duration_samples, *fade_in_samples, *fade_out_samples);
            clip.parameters.insert(
                edits::FADE_IN_KEY.into(),
                ParameterValue::Integer(i64::try_from(fade_in).unwrap_or(i64::MAX)),
            );
            clip.parameters.insert(
                edits::FADE_OUT_KEY.into(),
                ParameterValue::Integer(i64::try_from(fade_out).unwrap_or(i64::MAX)),
            );
            ChangeEvent {
                sequence: 0,
                kind: ChangeKind::Updated,
                entity_kind: EntityKind::Clip,
                entity_id: clip_id.to_string(),
            }
        }
        ProjectCommand::AddMarker { marker } => {
            project.markers.push(marker.clone());
            ChangeEvent {
                sequence: 0,
                kind: ChangeKind::Added,
                entity_kind: EntityKind::Marker,
                entity_id: marker.id.to_string(),
            }
        }
        ProjectCommand::RemoveMarker { marker_id } => {
            let index = project
                .markers
                .iter()
                .position(|marker| marker.id == *marker_id)
                .ok_or_else(|| {
                    CommandError::at_command(
                        CommandErrorCode::EntityNotFound,
                        command_index,
                        format!("marker {marker_id} does not exist"),
                    )
                })?;
            project.markers.remove(index);
            ChangeEvent {
                sequence: 0,
                kind: ChangeKind::Removed,
                entity_kind: EntityKind::Marker,
                entity_id: marker_id.to_string(),
            }
        }
        ProjectCommand::MoveMarker { marker_id, frame } => {
            let marker = project
                .markers
                .iter_mut()
                .find(|marker| marker.id == *marker_id)
                .ok_or_else(|| {
                    CommandError::at_command(
                        CommandErrorCode::EntityNotFound,
                        command_index,
                        format!("marker {marker_id} does not exist"),
                    )
                })?;
            marker.frame = *frame;
            ChangeEvent {
                sequence: 0,
                kind: ChangeKind::Updated,
                entity_kind: EntityKind::Marker,
                entity_id: marker_id.to_string(),
            }
        }
        ProjectCommand::SetLoopRegion { region } => {
            project.loop_region = *region;
            ChangeEvent {
                sequence: 0,
                kind: if region.is_some() {
                    ChangeKind::Updated
                } else {
                    ChangeKind::Removed
                },
                entity_kind: EntityKind::LoopRegion,
                entity_id: project.id.to_string(),
            }
        }
        ProjectCommand::SetAssetParameters {
            asset_id,
            parameters,
        } => {
            let asset = project
                .assets
                .iter_mut()
                .find(|asset| asset.id == *asset_id)
                .ok_or_else(|| {
                    CommandError::at_command(
                        CommandErrorCode::EntityNotFound,
                        command_index,
                        format!("asset {asset_id} does not exist"),
                    )
                })?;
            let AudioAssetSource::Synth {
                parameters: stored, ..
            } = &mut asset.source
            else {
                return Err(CommandError::at_command(
                    CommandErrorCode::EntityNotFound,
                    command_index,
                    format!("asset {asset_id} is not a synth"),
                ));
            };
            stored.clone_from(parameters);
            ChangeEvent {
                sequence: 0,
                kind: ChangeKind::Updated,
                entity_kind: EntityKind::Asset,
                entity_id: asset_id.to_string(),
            }
        }
        ProjectCommand::SetClipNotes { clip_id, notes } => {
            let clip = find_clip_mut(project, *clip_id, command_index)?;
            clip.notes.clone_from(notes);
            ChangeEvent {
                sequence: 0,
                kind: ChangeKind::Updated,
                entity_kind: EntityKind::Clip,
                entity_id: clip_id.to_string(),
            }
        }
        ProjectCommand::RemoveClip { clip_id } => {
            let clips = project
                .tracks
                .iter_mut()
                .flat_map(|track| &mut track.layers)
                .map(|layer| &mut layer.clips)
                .find(|clips| clips.iter().any(|clip| clip.id == *clip_id))
                .ok_or_else(|| {
                    CommandError::at_command(
                        CommandErrorCode::EntityNotFound,
                        command_index,
                        format!("clip {clip_id} does not exist"),
                    )
                })?;
            let index = clips
                .iter()
                .position(|clip| clip.id == *clip_id)
                .expect("found");
            clips.remove(index);
            ChangeEvent {
                sequence: 0,
                kind: ChangeKind::Removed,
                entity_kind: EntityKind::Clip,
                entity_id: clip_id.to_string(),
            }
        }
    };

    Ok(change)
}

/// The track a command names, or a structured "does not exist".
fn find_track(
    project: &mut Project,
    track_id: TrackId,
    command_index: usize,
) -> Result<&mut Track, CommandError> {
    project
        .tracks
        .iter_mut()
        .find(|track| track.id == track_id)
        .ok_or_else(|| {
            CommandError::at_command(
                CommandErrorCode::EntityNotFound,
                command_index,
                format!("track {track_id} does not exist"),
            )
        })
}

/// The clip a command names, or a structured "does not exist".
fn find_clip_mut(
    project: &mut Project,
    clip_id: ClipId,
    command_index: usize,
) -> Result<&mut Clip, CommandError> {
    project
        .tracks
        .iter_mut()
        .flat_map(|track| &mut track.layers)
        .flat_map(|layer| &mut layer.clips)
        .find(|clip| clip.id == clip_id)
        .ok_or_else(|| {
            CommandError::at_command(
                CommandErrorCode::EntityNotFound,
                command_index,
                format!("clip {clip_id} does not exist"),
            )
        })
}
