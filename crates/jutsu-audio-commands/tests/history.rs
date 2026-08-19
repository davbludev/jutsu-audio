use std::collections::BTreeMap;

use jutsu_audio_commands::{
    COMMAND_PROTOCOL_VERSION, CommandEnvelope, CommandHistory, CommandId, ProjectCommand,
    ProjectCommandEngine, invert,
};
use jutsu_audio_model::{
    Asset, AssetId, AudioAssetSource, BusId, CURRENT_PROJECT_SCHEMA_VERSION, Clip, ClipId, Layer,
    LayerId, MixerBus, ParameterValue, Project, ProjectId, ProjectMetadata, Track, TrackId,
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
        }],
        master_bus_id: bus_id,
        markers: Vec::new(),
        loop_region: None,
        automation: Vec::new(),
        tracks: vec![Track {
            id: TrackId::new(),
            name: "SFX".into(),
            output_bus_id: bus_id,
            parameters: BTreeMap::new(),
            layers: vec![Layer {
                id: LayerId::new(),
                name: "Layer".into(),
                clips: vec![],
            }],
        }],
    }
}

fn asset() -> Asset {
    Asset {
        id: AssetId::new(),
        name: "Hit".into(),
        source: AudioAssetSource::File {
            path: "hit.wav".into(),
        },
    }
}

fn clip(asset_id: AssetId) -> Clip {
    Clip {
        id: ClipId::new(),
        asset_id,
        start_sample: 10,
        source_start_sample: 0,
        duration_samples: 100,
        notes: Vec::new(),
        parameters: [("gain_db".into(), ParameterValue::Float(-3.0))]
            .into_iter()
            .collect(),
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

fn lane(project: &Project) -> (TrackId, LayerId) {
    let track = &project.tracks[0];
    (track.id, track.layers[0].id)
}

/// The clip as the project currently holds it, for comparing before and after.
fn only_clip(project: &Project) -> Option<&Clip> {
    project
        .tracks
        .iter()
        .flat_map(|track| &track.layers)
        .flat_map(|layer| &layer.clips)
        .next()
}

#[test]
fn undo_restores_the_exact_state_a_batch_replaced() {
    let initial = project();
    let (track_id, layer_id) = lane(&initial);
    let asset = asset();
    let clip = clip(asset.id);
    let clip_id = clip.id;
    let before = initial.clone();

    let mut engine = ProjectCommandEngine::new(initial).expect("engine");
    let mut history = CommandHistory::new();
    history
        .apply(
            &mut engine,
            envelope(
                0,
                vec![
                    ProjectCommand::AddAsset {
                        asset: asset.clone(),
                    },
                    ProjectCommand::AddClip {
                        track_id,
                        layer_id,
                        clip,
                    },
                ],
            ),
        )
        .expect("batch applies");
    assert_eq!(engine.revision(), 1);
    assert!(only_clip(engine.project()).is_some());

    history
        .undo(&mut engine)
        .expect("a step to undo")
        .expect("the inverse applies");

    assert_eq!(*engine.project(), before, "undo restores the whole project");
    assert_eq!(engine.revision(), 2, "undo is an ordinary forward edit");
    assert!(!history.can_undo());
    assert!(history.can_redo());

    history
        .redo(&mut engine)
        .expect("a step to redo")
        .expect("the batch re-applies");
    assert!(only_clip(engine.project()).is_some());
    assert_eq!(
        only_clip(engine.project()).map(|clip| clip.id),
        Some(clip_id)
    );
}

#[test]
fn undoing_an_update_puts_the_old_values_back_including_gain() {
    let mut initial = project();
    let asset = asset();
    let clip = clip(asset.id);
    let clip_id = clip.id;
    initial.assets.push(asset);
    initial.tracks[0].layers[0].clips.push(clip);

    let mut engine = ProjectCommandEngine::new(initial).expect("engine");
    let mut history = CommandHistory::new();
    history
        .apply(
            &mut engine,
            envelope(
                0,
                vec![ProjectCommand::UpdateClip {
                    clip_id,
                    start_sample: 500,
                    source_start_sample: 20,
                    duration_samples: 40,
                    gain_db: 6.0,
                }],
            ),
        )
        .expect("update applies");

    history
        .undo(&mut engine)
        .expect("a step to undo")
        .expect("the inverse applies");

    let restored = only_clip(engine.project()).expect("clip");
    assert_eq!(restored.start_sample, 10);
    assert_eq!(restored.source_start_sample, 0);
    assert_eq!(restored.duration_samples, 100);
    assert_eq!(
        restored.parameters.get("gain_db"),
        Some(&ParameterValue::Float(-3.0))
    );
}

#[test]
fn undo_reverses_the_last_edit_whoever_made_it() {
    let initial = project();
    let mut engine = ProjectCommandEngine::new(initial).expect("engine");
    let mut history = CommandHistory::new();

    // The user renames, then an external client renames again through the same
    // history — undo must reverse the external edit first.
    for name in ["From the editor", "From the CLI"] {
        let batch = envelope(
            engine.revision(),
            vec![ProjectCommand::SetProjectName { name: name.into() }],
        );
        history.apply(&mut engine, batch).expect("rename applies");
    }
    assert_eq!(engine.project().metadata.name, "From the CLI");

    history.undo(&mut engine).expect("a step").expect("applies");
    assert_eq!(engine.project().metadata.name, "From the editor");

    history.undo(&mut engine).expect("a step").expect("applies");
    assert_eq!(engine.project().metadata.name, "Original");
    assert!(!history.can_undo());
}

#[test]
fn a_rejected_batch_records_no_step() {
    let initial = project();
    let mut engine = ProjectCommandEngine::new(initial).expect("engine");
    let mut history = CommandHistory::new();

    history
        .apply(
            &mut engine,
            envelope(
                0,
                vec![ProjectCommand::RemoveClip {
                    clip_id: ClipId::new(),
                }],
            ),
        )
        .expect_err("removing a clip that does not exist fails");

    assert!(!history.can_undo(), "a failed batch leaves nothing to undo");
    assert_eq!(engine.revision(), 0);
}

#[test]
fn a_new_edit_drops_the_redo_stack() {
    let initial = project();
    let mut engine = ProjectCommandEngine::new(initial).expect("engine");
    let mut history = CommandHistory::new();

    history
        .apply(
            &mut engine,
            envelope(
                0,
                vec![ProjectCommand::SetProjectName {
                    name: "First".into(),
                }],
            ),
        )
        .expect("applies");
    history.undo(&mut engine).expect("a step").expect("applies");
    assert!(history.can_redo());

    let batch = envelope(
        engine.revision(),
        vec![ProjectCommand::SetProjectName {
            name: "Second".into(),
        }],
    );
    history.apply(&mut engine, batch).expect("applies");
    assert!(!history.can_redo());
}

#[test]
fn inverting_a_batch_reverses_its_order() {
    let initial = project();
    let (track_id, layer_id) = lane(&initial);
    let asset = asset();
    let clip = clip(asset.id);
    let clip_id = clip.id;
    let asset_id = asset.id;

    let inverse = invert(
        &initial,
        &[
            ProjectCommand::AddAsset { asset },
            ProjectCommand::AddClip {
                track_id,
                layer_id,
                clip,
            },
        ],
    )
    .expect("invertible");

    assert_eq!(
        inverse,
        vec![
            ProjectCommand::RemoveClip { clip_id },
            ProjectCommand::RemoveAsset { asset_id },
        ]
    );
}
