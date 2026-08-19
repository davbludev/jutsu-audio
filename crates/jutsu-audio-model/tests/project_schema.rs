use std::collections::BTreeMap;

use jutsu_audio_model::{
    Asset, AssetId, AudioAssetSource, BusId, CURRENT_PROJECT_SCHEMA_VERSION, Clip, ClipId, Layer,
    LayerId, MixerBus, ParameterValue, Project, ProjectId, ProjectMetadata, Track, TrackId,
    ValidationCode,
};

fn valid_project() -> Project {
    let asset_id = AssetId::new();
    let bus_id = BusId::new();

    Project {
        schema_version: CURRENT_PROJECT_SCHEMA_VERSION,
        id: ProjectId::new(),
        metadata: ProjectMetadata {
            name: "Impact test".into(),
            properties: BTreeMap::from([("game".into(), "Jutsu".into())]),
        },
        assets: vec![Asset {
            id: asset_id,
            name: "impact.wav".into(),
            source: AudioAssetSource::File {
                path: "audio/impact.wav".into(),
            },
        }],
        buses: vec![MixerBus {
            id: bus_id,
            name: "Master".into(),
            output_bus_id: None,
            parameters: BTreeMap::from([("gain_db".into(), ParameterValue::Float(0.0))]),
            effects: Vec::new(),
        }],
        master_bus_id: bus_id,
        markers: Vec::new(),
        loop_region: None,
        automation: Vec::new(),
        tempo: Vec::new(),
        tracks: vec![Track {
            id: TrackId::new(),
            name: "Impact".into(),
            output_bus_id: bus_id,
            parameters: BTreeMap::new(),
            layers: vec![Layer {
                id: LayerId::new(),
                name: "Body".into(),
                clips: vec![Clip {
                    id: ClipId::new(),
                    asset_id,
                    start_sample: 48_000,
                    source_start_sample: 0,
                    duration_samples: 24_000,
                    notes: Vec::new(),
                    parameters: BTreeMap::from([
                        ("gain_db".into(), ParameterValue::Float(-3.0)),
                        ("muted".into(), ParameterValue::Bool(false)),
                    ]),
                }],
            }],
            effects: Vec::new(),
        }],
    }
}

#[test]
fn json_round_trip_preserves_project_and_entity_ids() {
    let project = valid_project();

    let json = serde_json::to_string_pretty(&project).unwrap();
    let decoded: Project = serde_json::from_str(&json).unwrap();

    assert_eq!(decoded, project);
    assert_eq!(
        decoded.tracks[0].layers[0].clips[0].asset_id,
        project.assets[0].id
    );
}

#[test]
fn validation_reports_missing_asset_reference_with_machine_context() {
    let mut project = valid_project();
    let clip = &mut project.tracks[0].layers[0].clips[0];
    clip.asset_id = AssetId::new();
    let clip_id = clip.id.to_string();

    let diagnostics = project.validate();

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == ValidationCode::MissingAssetReference
            && diagnostic.entity_id.as_deref() == Some(clip_id.as_str())
            && diagnostic.path == "tracks[0].layers[0].clips[0].asset_id"
    }));
}

#[test]
fn validation_reports_missing_routing_references() {
    let mut project = valid_project();
    project.tracks[0].output_bus_id = BusId::new();

    let diagnostics = project.validate();

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == ValidationCode::MissingBusReference)
    );
}

#[test]
fn validation_reports_duplicate_typed_ids() {
    let mut project = valid_project();
    project.assets.push(project.assets[0].clone());

    let diagnostics = project.validate();

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == ValidationCode::DuplicateEntityId)
    );
}

#[test]
fn validation_reports_duplicate_clip_ids_across_tracks() {
    let mut project = valid_project();
    let mut duplicate_track = project.tracks[0].clone();
    duplicate_track.id = TrackId::new();
    duplicate_track.layers[0].id = LayerId::new();
    project.tracks.push(duplicate_track);

    let diagnostics = project.validate();

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == ValidationCode::DuplicateEntityId
            && diagnostic.path == "tracks[1].layers[0].clips[0].id"
    }));
}

#[test]
fn validation_rejects_unsupported_schema_versions() {
    let mut project = valid_project();
    project.schema_version = CURRENT_PROJECT_SCHEMA_VERSION + 1;

    let diagnostics = project.validate();

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == ValidationCode::UnsupportedSchemaVersion)
    );
}

#[test]
fn validation_rejects_zero_length_clips() {
    let mut project = valid_project();
    project.tracks[0].layers[0].clips[0].duration_samples = 0;

    let diagnostics = project.validate();

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == ValidationCode::InvalidClipRange)
    );
}
