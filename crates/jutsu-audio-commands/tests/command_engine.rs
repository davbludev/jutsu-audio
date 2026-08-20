use std::collections::BTreeMap;

use jutsu_audio_commands::{
    COMMAND_PROTOCOL_VERSION, ChangeKind, CommandEnvelope, CommandErrorCode, CommandId, EntityKind,
    ProjectCommand, ProjectCommandEngine,
};
use jutsu_audio_model::{
    Asset, AssetId, AudioAssetSource, BusId, CURRENT_PROJECT_SCHEMA_VERSION, Clip, ClipId, Layer,
    LayerId, MixerBus, Project, ProjectId, ProjectMetadata, Track, TrackId,
};

fn project() -> Project {
    let bus_id = BusId::new();
    Project {
        schema_version: CURRENT_PROJECT_SCHEMA_VERSION,
        id: ProjectId::new(),
        metadata: ProjectMetadata {
            name: "Original".into(),
            properties: BTreeMap::new(),
        },
        assets: vec![],
        buses: vec![MixerBus {
            id: bus_id,
            name: "Master".into(),
            output_bus_id: None,
            parameters: BTreeMap::new(),
            effects: Vec::new(),
        }],
        master_bus_id: bus_id,
        markers: Vec::new(),
        loop_region: None,
        automation: Vec::new(),
        tempo: Vec::new(),
        patterns: Vec::new(),
        tracks: vec![Track {
            id: TrackId::new(),
            name: "SFX".into(),
            output_bus_id: bus_id,
            sends: Vec::new(),
            parameters: BTreeMap::new(),
            layers: vec![Layer {
                id: LayerId::new(),
                name: "Layer".into(),
                clips: vec![],
            }],
            effects: Vec::new(),
        }],
    }
}

fn envelope(expected_revision: u64, commands: Vec<ProjectCommand>) -> CommandEnvelope {
    CommandEnvelope {
        protocol_version: COMMAND_PROTOCOL_VERSION,
        command_id: CommandId::new(),
        expected_revision,
        commands,
    }
}

#[test]
fn updates_and_removes_clip_through_shared_commands() {
    let mut initial = project();
    let asset = Asset {
        id: AssetId::new(),
        name: "Hit".into(),
        source: AudioAssetSource::File {
            path: "hit.wav".into(),
        },
    };
    let clip = Clip {
        id: ClipId::new(),
        asset_id: asset.id,
        start_sample: 0,
        source_start_sample: 0,
        duration_samples: 100,
        parameters: BTreeMap::new(),
        notes: Vec::new(),
        pattern_id: None,
    };
    initial.assets.push(asset);
    initial.tracks[0].layers[0].clips.push(clip.clone());
    let mut engine = ProjectCommandEngine::new(initial).unwrap();

    engine
        .apply(envelope(
            0,
            vec![ProjectCommand::UpdateClip {
                clip_id: clip.id,
                start_sample: 25,
                source_start_sample: 5,
                duration_samples: 50,
                gain_db: -3.0,
            }],
        ))
        .unwrap();
    let updated = &engine.project().tracks[0].layers[0].clips[0];
    assert_eq!(
        (
            updated.start_sample,
            updated.source_start_sample,
            updated.duration_samples
        ),
        (25, 5, 50)
    );

    engine
        .apply(envelope(
            1,
            vec![ProjectCommand::RemoveClip { clip_id: clip.id }],
        ))
        .unwrap();
    assert!(engine.project().tracks[0].layers[0].clips.is_empty());
}

#[test]
fn applies_command_and_publishes_ordered_change_at_new_revision() {
    let initial = project();
    let project_id = initial.id.to_string();
    let mut engine = ProjectCommandEngine::new(initial).unwrap();
    let command = envelope(
        0,
        vec![ProjectCommand::SetProjectName {
            name: "Updated".into(),
        }],
    );

    let outcome = engine.apply(command.clone()).unwrap();

    assert_eq!(engine.project().metadata.name, "Updated");
    assert_eq!(engine.revision(), 1);
    assert_eq!(outcome.command_id, command.command_id);
    assert_eq!(outcome.revision, 1);
    assert_eq!(outcome.changes.len(), 1);
    assert_eq!(outcome.changes[0].sequence, 0);
    assert_eq!(outcome.changes[0].kind, ChangeKind::Updated);
    assert_eq!(outcome.changes[0].entity_kind, EntityKind::Project);
    assert_eq!(outcome.changes[0].entity_id, project_id);
}

#[test]
fn rejects_stale_revision_without_mutating_state() {
    let initial = project();
    let mut engine = ProjectCommandEngine::new(initial.clone()).unwrap();

    let error = engine
        .apply(envelope(
            9,
            vec![ProjectCommand::SetProjectName {
                name: "Must not apply".into(),
            }],
        ))
        .unwrap_err();

    assert_eq!(error.code, CommandErrorCode::RevisionConflict);
    assert_eq!(error.expected_revision, Some(9));
    assert_eq!(error.actual_revision, Some(0));
    assert_eq!(engine.project(), &initial);
    assert_eq!(engine.revision(), 0);
}

#[test]
fn rolls_back_entire_batch_when_later_command_fails() {
    let initial = project();
    let mut engine = ProjectCommandEngine::new(initial.clone()).unwrap();

    let error = engine
        .apply(envelope(
            0,
            vec![
                ProjectCommand::SetProjectName {
                    name: "Must roll back".into(),
                },
                ProjectCommand::RemoveAsset {
                    asset_id: AssetId::new(),
                },
            ],
        ))
        .unwrap_err();

    assert_eq!(error.code, CommandErrorCode::EntityNotFound);
    assert_eq!(error.command_index, Some(1));
    assert_eq!(engine.project(), &initial);
    assert_eq!(engine.revision(), 0);
}

#[test]
fn validates_batch_final_state_and_commits_references_atomically() {
    let initial = project();
    let track_id = initial.tracks[0].id;
    let layer_id = initial.tracks[0].layers[0].id;
    let asset_id = AssetId::new();
    let clip_id = ClipId::new();
    let mut engine = ProjectCommandEngine::new(initial).unwrap();

    let outcome = engine
        .apply(envelope(
            0,
            vec![
                ProjectCommand::AddAsset {
                    asset: Asset {
                        id: asset_id,
                        name: "impact.wav".into(),
                        source: AudioAssetSource::File {
                            path: "audio/impact.wav".into(),
                        },
                    },
                },
                ProjectCommand::AddClip {
                    track_id,
                    layer_id,
                    clip: Clip {
                        id: clip_id,
                        asset_id,
                        start_sample: 0,
                        source_start_sample: 0,
                        duration_samples: 48_000,
                        parameters: BTreeMap::new(),
                        notes: Vec::new(),
                        pattern_id: None,
                    },
                },
            ],
        ))
        .unwrap();

    assert_eq!(engine.project().assets[0].id, asset_id);
    assert_eq!(engine.project().tracks[0].layers[0].clips[0].id, clip_id);
    assert_eq!(outcome.changes[0].sequence, 0);
    assert_eq!(outcome.changes[1].sequence, 1);
    assert_eq!(outcome.revision, 1);
}

#[test]
fn rejects_batch_that_leaves_invalid_project_references() {
    let mut initial = project();
    let asset_id = AssetId::new();
    initial.assets.push(Asset {
        id: asset_id,
        name: "used.wav".into(),
        source: AudioAssetSource::File {
            path: "used.wav".into(),
        },
    });
    initial.tracks[0].layers[0].clips.push(Clip {
        id: ClipId::new(),
        asset_id,
        start_sample: 0,
        source_start_sample: 0,
        duration_samples: 1,
        parameters: BTreeMap::new(),
        notes: Vec::new(),
        pattern_id: None,
    });
    let mut engine = ProjectCommandEngine::new(initial.clone()).unwrap();

    let error = engine
        .apply(envelope(0, vec![ProjectCommand::RemoveAsset { asset_id }]))
        .unwrap_err();

    assert_eq!(error.code, CommandErrorCode::ProjectValidationFailed);
    assert!(!error.diagnostics.is_empty());
    assert_eq!(engine.project(), &initial);
}

#[test]
fn command_envelope_has_stable_tagged_json_shape() {
    let command_id = CommandId::new();
    let command = CommandEnvelope {
        protocol_version: COMMAND_PROTOCOL_VERSION,
        command_id,
        expected_revision: 42,
        commands: vec![ProjectCommand::SetProjectName {
            name: "Machine readable".into(),
        }],
    };

    let value = serde_json::to_value(&command).unwrap();
    let decoded: CommandEnvelope = serde_json::from_value(value.clone()).unwrap();

    assert_eq!(decoded, command);
    assert_eq!(value["protocol_version"], 1);
    assert_eq!(value["command_id"], command_id.to_string());
    assert_eq!(value["commands"][0]["type"], "set_project_name");
    assert_eq!(value["commands"][0]["name"], "Machine readable");
}

#[test]
fn same_state_and_envelope_produce_same_outcome() {
    let initial = project();
    let command = envelope(
        0,
        vec![ProjectCommand::SetProjectName {
            name: "Deterministic".into(),
        }],
    );
    let mut first = ProjectCommandEngine::new(initial.clone()).unwrap();
    let mut second = ProjectCommandEngine::new(initial).unwrap();

    let first_outcome = first.apply(command.clone()).unwrap();
    let second_outcome = second.apply(command).unwrap();

    assert_eq!(first.project(), second.project());
    assert_eq!(first_outcome, second_outcome);
}
