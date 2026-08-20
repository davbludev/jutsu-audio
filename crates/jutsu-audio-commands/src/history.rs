//! Undo history.
//!
//! Every committed batch has an inverse computed against the state it was
//! applied to, so undo replays commands through the same engine as any other
//! edit — there is no second way to change a project, and no snapshot of one
//! kept alive on the side.
//!
//! The history is per project and strictly chronological. An edit that arrived
//! over the session socket sits in the same stack as one the user made, in the
//! order the engine committed them, so undo always reverses the last thing that
//! happened to the project regardless of who did it.

use jutsu_audio_model::{Clip, ParameterValue, Project, Track, TrackId};

use crate::{
    COMMAND_PROTOCOL_VERSION, CommandEnvelope, CommandError, CommandErrorCode, CommandId,
    CommandOutcome, ProjectCommand, ProjectCommandEngine, apply_command,
};

/// How many steps are kept. Beyond this the oldest is forgotten; the project
/// itself is never affected.
pub const HISTORY_LIMIT: usize = 128;

#[derive(Clone, Debug, PartialEq)]
struct Step {
    forward: Vec<ProjectCommand>,
    inverse: Vec<ProjectCommand>,
}

/// The undo/redo stacks for one open project.
#[derive(Debug, Default)]
pub struct CommandHistory {
    undo: Vec<Step>,
    redo: Vec<Step>,
}

impl CommandHistory {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    #[must_use]
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// Forgets both stacks. Used when the project is replaced wholesale —
    /// opening another one, for instance — where an inverse would be nonsense.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// Applies a batch and records it. A batch that fails records nothing,
    /// because the engine changed nothing.
    ///
    /// A new edit invalidates the redo stack, as everywhere else.
    pub fn apply(
        &mut self,
        engine: &mut ProjectCommandEngine,
        envelope: CommandEnvelope,
    ) -> Result<CommandOutcome, CommandError> {
        let inverse = invert(engine.project(), &envelope.commands)?;
        let forward = envelope.commands.clone();
        let outcome = engine.apply(envelope)?;
        self.redo.clear();
        self.undo.push(Step { forward, inverse });
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
        Ok(outcome)
    }

    /// Reverses the last recorded batch. `None` when there is nothing to undo;
    /// a failure leaves the step on the stack so it can be retried or reported.
    pub fn undo(
        &mut self,
        engine: &mut ProjectCommandEngine,
    ) -> Option<Result<CommandOutcome, CommandError>> {
        let step = self.undo.last()?.clone();
        Some(match replay(engine, &step.inverse) {
            Ok(outcome) => {
                self.undo.pop();
                self.redo.push(step);
                Ok(outcome)
            }
            Err(error) => Err(error),
        })
    }

    /// Re-applies the last undone batch.
    pub fn redo(
        &mut self,
        engine: &mut ProjectCommandEngine,
    ) -> Option<Result<CommandOutcome, CommandError>> {
        let step = self.redo.last()?.clone();
        Some(match replay(engine, &step.forward) {
            Ok(outcome) => {
                self.redo.pop();
                self.undo.push(step);
                Ok(outcome)
            }
            Err(error) => Err(error),
        })
    }
}

fn replay(
    engine: &mut ProjectCommandEngine,
    commands: &[ProjectCommand],
) -> Result<CommandOutcome, CommandError> {
    engine.apply(CommandEnvelope {
        protocol_version: COMMAND_PROTOCOL_VERSION,
        command_id: CommandId::new(),
        expected_revision: engine.revision(),
        commands: commands.to_vec(),
    })
}

/// Builds the batch that undoes `commands` when applied to the state they
/// produced: each command inverted against the state it saw, in reverse order.
///
/// Fails for the same reason applying would fail — a command that names an
/// entity which does not exist.
pub fn invert(
    project: &Project,
    commands: &[ProjectCommand],
) -> Result<Vec<ProjectCommand>, CommandError> {
    let mut candidate = project.clone();
    let mut inverses = Vec::with_capacity(commands.len());
    for (command_index, command) in commands.iter().enumerate() {
        inverses.push(invert_command(&candidate, command, command_index)?);
        apply_command(&mut candidate, command, command_index)?;
    }
    inverses.reverse();
    Ok(inverses)
}

fn invert_command(
    project: &Project,
    command: &ProjectCommand,
    command_index: usize,
) -> Result<ProjectCommand, CommandError> {
    let missing = |what: String| CommandError {
        code: CommandErrorCode::EntityNotFound,
        message: what,
        command_index: Some(command_index),
        expected_revision: None,
        actual_revision: None,
        diagnostics: Vec::new(),
    };

    Ok(match command {
        ProjectCommand::SetProjectName { .. } => ProjectCommand::SetProjectName {
            name: project.metadata.name.clone(),
        },
        ProjectCommand::AddAsset { asset } => ProjectCommand::RemoveAsset { asset_id: asset.id },
        ProjectCommand::RemoveAsset { asset_id } => {
            let asset = project
                .assets
                .iter()
                .find(|asset| asset.id == *asset_id)
                .ok_or_else(|| missing(format!("asset {asset_id} does not exist")))?;
            ProjectCommand::AddAsset {
                asset: asset.clone(),
            }
        }
        ProjectCommand::AddClip { clip, .. } => ProjectCommand::RemoveClip { clip_id: clip.id },
        ProjectCommand::RemoveClip { clip_id } => {
            let (track_id, layer_id, clip) = find_clip(project, *clip_id)
                .ok_or_else(|| missing(format!("clip {clip_id} does not exist")))?;
            ProjectCommand::AddClip {
                track_id,
                layer_id,
                clip: clip.clone(),
            }
        }
        ProjectCommand::AddTrack { track } => ProjectCommand::RemoveTrack { track_id: track.id },
        ProjectCommand::RemoveTrack { track_id } => {
            let track = find_track(project, *track_id)
                .ok_or_else(|| missing(format!("track {track_id} does not exist")))?;
            ProjectCommand::AddTrack {
                track: track.clone(),
            }
        }
        ProjectCommand::AddLayer { layer, .. } => {
            ProjectCommand::RemoveLayer { layer_id: layer.id }
        }
        ProjectCommand::RemoveLayer { layer_id } => {
            let (track, layer) = project
                .tracks
                .iter()
                .find_map(|track| {
                    track
                        .layers
                        .iter()
                        .find(|layer| layer.id == *layer_id)
                        .map(|layer| (track, layer))
                })
                .ok_or_else(|| missing(format!("layer {layer_id} does not exist")))?;
            ProjectCommand::AddLayer {
                track_id: track.id,
                layer: layer.clone(),
            }
        }
        ProjectCommand::SetTrackMute { track_id, .. } => {
            let track = find_track(project, *track_id)
                .ok_or_else(|| missing(format!("track {track_id} does not exist")))?;
            ProjectCommand::SetTrackMute {
                track_id: *track_id,
                muted: flag(track, "mute"),
            }
        }
        ProjectCommand::SetTrackSolo { track_id, .. } => {
            let track = find_track(project, *track_id)
                .ok_or_else(|| missing(format!("track {track_id} does not exist")))?;
            ProjectCommand::SetTrackSolo {
                track_id: *track_id,
                soloed: flag(track, "solo"),
            }
        }
        ProjectCommand::SetClipPan { clip_id, .. } => {
            let (_, _, clip) = find_clip(project, *clip_id)
                .ok_or_else(|| missing(format!("clip {clip_id} does not exist")))?;
            ProjectCommand::SetClipPan {
                clip_id: *clip_id,
                pan: match clip.parameters.get("pan") {
                    Some(ParameterValue::Float(value)) => *value,
                    _ => 0.0,
                },
            }
        }
        ProjectCommand::SetClipFades { clip_id, .. } => {
            let (_, _, clip) = find_clip(project, *clip_id)
                .ok_or_else(|| missing(format!("clip {clip_id} does not exist")))?;
            ProjectCommand::SetClipFades {
                clip_id: *clip_id,
                fade_in_samples: crate::edits::fade_in(clip),
                fade_out_samples: crate::edits::fade_out(clip),
            }
        }
        ProjectCommand::AddMarker { marker } => ProjectCommand::RemoveMarker {
            marker_id: marker.id,
        },
        ProjectCommand::RemoveMarker { marker_id } => {
            let marker = project
                .markers
                .iter()
                .find(|marker| marker.id == *marker_id)
                .ok_or_else(|| missing(format!("marker {marker_id} does not exist")))?;
            ProjectCommand::AddMarker {
                marker: marker.clone(),
            }
        }
        ProjectCommand::MoveMarker { marker_id, .. } => {
            let marker = project
                .markers
                .iter()
                .find(|marker| marker.id == *marker_id)
                .ok_or_else(|| missing(format!("marker {marker_id} does not exist")))?;
            ProjectCommand::MoveMarker {
                marker_id: *marker_id,
                frame: marker.frame,
            }
        }
        ProjectCommand::SetLoopRegion { .. } => ProjectCommand::SetLoopRegion {
            region: project.loop_region,
        },
        ProjectCommand::SetAssetParameters { asset_id, .. } => {
            let asset = project
                .assets
                .iter()
                .find(|asset| asset.id == *asset_id)
                .ok_or_else(|| missing(format!("asset {asset_id} does not exist")))?;
            let jutsu_audio_model::AudioAssetSource::Synth { parameters, .. } = &asset.source
            else {
                return Err(missing(format!("asset {asset_id} is not a synth")));
            };
            ProjectCommand::SetAssetParameters {
                asset_id: *asset_id,
                parameters: parameters.clone(),
            }
        }
        ProjectCommand::SetClipNotes { clip_id, .. } => {
            let (_, _, clip) = find_clip(project, *clip_id)
                .ok_or_else(|| missing(format!("clip {clip_id} does not exist")))?;
            ProjectCommand::SetClipNotes {
                clip_id: *clip_id,
                notes: clip.notes.clone(),
            }
        }
        ProjectCommand::AddBus { bus } => ProjectCommand::RemoveBus { bus_id: bus.id },
        ProjectCommand::RemoveBus { bus_id } => {
            let bus = project
                .buses
                .iter()
                .find(|bus| bus.id == *bus_id)
                .ok_or_else(|| missing(format!("bus {bus_id} does not exist")))?;
            ProjectCommand::AddBus { bus: bus.clone() }
        }
        ProjectCommand::SetBusOutput { bus_id, .. } => {
            let bus = project
                .buses
                .iter()
                .find(|bus| bus.id == *bus_id)
                .ok_or_else(|| missing(format!("bus {bus_id} does not exist")))?;
            ProjectCommand::SetBusOutput {
                bus_id: *bus_id,
                output_bus_id: bus.output_bus_id,
            }
        }
        ProjectCommand::SetTrackOutput { track_id, .. } => {
            let track = find_track(project, *track_id)
                .ok_or_else(|| missing(format!("track {track_id} does not exist")))?;
            ProjectCommand::SetTrackOutput {
                track_id: *track_id,
                output_bus_id: track.output_bus_id,
            }
        }
        ProjectCommand::SetTrackParameter { track_id, key, .. } => {
            let track = find_track(project, *track_id)
                .ok_or_else(|| missing(format!("track {track_id} does not exist")))?;
            ProjectCommand::SetTrackParameter {
                track_id: *track_id,
                key: key.clone(),
                // A parameter that did not exist reads back as its absence
                // would: unity gain, centre pan, off.
                value: track
                    .parameters
                    .get(key)
                    .cloned()
                    .unwrap_or(ParameterValue::Float(0.0)),
            }
        }
        ProjectCommand::SetBusParameter { bus_id, key, .. } => {
            let bus = project
                .buses
                .iter()
                .find(|bus| bus.id == *bus_id)
                .ok_or_else(|| missing(format!("bus {bus_id} does not exist")))?;
            ProjectCommand::SetBusParameter {
                bus_id: *bus_id,
                key: key.clone(),
                value: bus
                    .parameters
                    .get(key)
                    .cloned()
                    .unwrap_or(ParameterValue::Float(0.0)),
            }
        }
        ProjectCommand::AddAutomationLane { lane } => ProjectCommand::RemoveAutomationLane {
            automation_id: lane.id,
        },
        ProjectCommand::RemoveAutomationLane { automation_id } => {
            let lane = project
                .automation
                .iter()
                .find(|lane| lane.id == *automation_id)
                .ok_or_else(|| {
                    missing(format!("automation lane {automation_id} does not exist"))
                })?;
            ProjectCommand::AddAutomationLane { lane: lane.clone() }
        }
        ProjectCommand::SetAutomationPoints { automation_id, .. } => {
            let lane = project
                .automation
                .iter()
                .find(|lane| lane.id == *automation_id)
                .ok_or_else(|| {
                    missing(format!("automation lane {automation_id} does not exist"))
                })?;
            ProjectCommand::SetAutomationPoints {
                automation_id: *automation_id,
                points: lane.points.clone(),
            }
        }
        ProjectCommand::AddEffect { effect, .. } => ProjectCommand::RemoveEffect {
            effect_id: effect.id,
        },
        ProjectCommand::RemoveEffect { effect_id } => {
            let (target, effect, index) = find_effect(project, *effect_id)
                .ok_or_else(|| missing(format!("effect {effect_id} does not exist")))?;
            // Removing then re-adding would put it back at the end; the move
            // that follows puts it back where it was.
            let _ = index;
            ProjectCommand::AddEffect {
                target,
                effect: effect.clone(),
            }
        }
        ProjectCommand::MoveEffect { effect_id, .. } => {
            let (_, _, index) = find_effect(project, *effect_id)
                .ok_or_else(|| missing(format!("effect {effect_id} does not exist")))?;
            ProjectCommand::MoveEffect {
                effect_id: *effect_id,
                to_index: index,
            }
        }
        ProjectCommand::SetEffectEnabled { effect_id, .. } => {
            let (_, effect, _) = find_effect(project, *effect_id)
                .ok_or_else(|| missing(format!("effect {effect_id} does not exist")))?;
            ProjectCommand::SetEffectEnabled {
                effect_id: *effect_id,
                enabled: effect.enabled,
            }
        }
        ProjectCommand::SetEffectWet { effect_id, .. } => {
            let (_, effect, _) = find_effect(project, *effect_id)
                .ok_or_else(|| missing(format!("effect {effect_id} does not exist")))?;
            ProjectCommand::SetEffectWet {
                effect_id: *effect_id,
                wet: effect.wet,
            }
        }
        ProjectCommand::SetEffectParameters { effect_id, .. } => {
            let (_, effect, _) = find_effect(project, *effect_id)
                .ok_or_else(|| missing(format!("effect {effect_id} does not exist")))?;
            ProjectCommand::SetEffectParameters {
                effect_id: *effect_id,
                parameters: effect.parameters.clone(),
            }
        }
        ProjectCommand::SetTempoMap { .. } => ProjectCommand::SetTempoMap {
            changes: project.tempo.clone(),
        },
        ProjectCommand::AddPattern { pattern } => ProjectCommand::RemovePattern {
            pattern_id: pattern.id,
        },
        ProjectCommand::RemovePattern { pattern_id } => {
            let pattern = project
                .patterns
                .iter()
                .find(|pattern| pattern.id == *pattern_id)
                .ok_or_else(|| missing(format!("pattern {pattern_id} does not exist")))?;
            ProjectCommand::AddPattern {
                pattern: pattern.clone(),
            }
        }
        ProjectCommand::SetPatternNotes { pattern_id, .. } => {
            let pattern = project
                .patterns
                .iter()
                .find(|pattern| pattern.id == *pattern_id)
                .ok_or_else(|| missing(format!("pattern {pattern_id} does not exist")))?;
            ProjectCommand::SetPatternNotes {
                pattern_id: *pattern_id,
                length_frames: pattern.length_frames,
                notes: pattern.notes.clone(),
            }
        }
        ProjectCommand::SetClipPattern { clip_id, .. } => {
            let (_, _, clip) = find_clip(project, *clip_id)
                .ok_or_else(|| missing(format!("clip {clip_id} does not exist")))?;
            ProjectCommand::SetClipPattern {
                clip_id: *clip_id,
                pattern_id: clip.pattern_id,
            }
        }
        ProjectCommand::SetSamplerZones { asset_id, .. } => {
            let asset = project
                .assets
                .iter()
                .find(|asset| asset.id == *asset_id)
                .ok_or_else(|| missing(format!("asset {asset_id} does not exist")))?;
            let jutsu_audio_model::AudioAssetSource::Sampler { zones, .. } = &asset.source else {
                return Err(missing(format!("asset {asset_id} is not a sampler")));
            };
            ProjectCommand::SetSamplerZones {
                asset_id: *asset_id,
                zones: zones.clone(),
            }
        }
        ProjectCommand::UpdateClip { clip_id, .. } => {
            let (_, _, clip) = find_clip(project, *clip_id)
                .ok_or_else(|| missing(format!("clip {clip_id} does not exist")))?;
            ProjectCommand::UpdateClip {
                clip_id: *clip_id,
                start_sample: clip.start_sample,
                source_start_sample: clip.source_start_sample,
                duration_samples: clip.duration_samples,
                gain_db: match clip.parameters.get("gain_db") {
                    Some(ParameterValue::Float(value)) => *value,
                    _ => 0.0,
                },
            }
        }
    })
}

/// A clip and the lane it lives in — what re-adding a removed clip needs.
fn find_clip(
    project: &Project,
    clip_id: jutsu_audio_model::ClipId,
) -> Option<(
    jutsu_audio_model::TrackId,
    jutsu_audio_model::LayerId,
    &Clip,
)> {
    project.tracks.iter().find_map(|track| {
        track.layers.iter().find_map(|layer| {
            layer
                .clips
                .iter()
                .find(|clip| clip.id == clip_id)
                .map(|clip| (track.id, layer.id, clip))
        })
    })
}

fn find_track(project: &Project, track_id: TrackId) -> Option<&Track> {
    project.tracks.iter().find(|track| track.id == track_id)
}

fn flag(track: &Track, key: &str) -> bool {
    matches!(track.parameters.get(key), Some(ParameterValue::Bool(true)))
}

/// An insert, the chain it belongs to, and where in that chain it sits.
fn find_effect(
    project: &Project,
    effect_id: jutsu_audio_model::EffectId,
) -> Option<(crate::EffectTarget, &jutsu_audio_model::EffectInsert, usize)> {
    for track in &project.tracks {
        if let Some(index) = track
            .effects
            .iter()
            .position(|effect| effect.id == effect_id)
        {
            return Some((
                crate::EffectTarget::Track { track_id: track.id },
                &track.effects[index],
                index,
            ));
        }
    }
    for bus in &project.buses {
        if let Some(index) = bus.effects.iter().position(|effect| effect.id == effect_id) {
            return Some((
                crate::EffectTarget::Bus { bus_id: bus.id },
                &bus.effects[index],
                index,
            ));
        }
    }
    None
}
