use std::collections::BTreeMap;

use jutsu_audio_engine::{
    ProcessorSpec, RenderConnection, RenderNode, RenderNodeId, RenderSnapshot, SnapshotErrorCode,
};
use jutsu_audio_extensions::ExtensionTypeId;
use jutsu_audio_model::{BusId, ParameterValue, ProjectId};

fn nodes() -> Vec<RenderNode> {
    vec![
        RenderNode {
            id: RenderNodeId::new(1),
            processor: ProcessorSpec::Synth {
                type_id: ExtensionTypeId::new("builtin.mock_synth").unwrap(),
                state_version: 1,
                parameters: BTreeMap::from([("amount".into(), ParameterValue::Float(0.5))]),
            },
        },
        RenderNode {
            id: RenderNodeId::new(2),
            processor: ProcessorSpec::Mixer {
                bus_id: BusId::new(),
            },
        },
    ]
}

#[test]
fn builds_shareable_immutable_snapshot_with_valid_graph() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<RenderSnapshot>();

    let project_id = ProjectId::new();
    let snapshot = RenderSnapshot::build(
        project_id,
        8,
        48_000,
        2,
        nodes(),
        vec![RenderConnection {
            from: RenderNodeId::new(1),
            to: RenderNodeId::new(2),
        }],
        RenderNodeId::new(2),
    )
    .unwrap();

    assert_eq!(snapshot.project_id(), project_id);
    assert_eq!(snapshot.project_revision(), 8);
    assert_eq!(snapshot.sample_rate(), 48_000);
    assert_eq!(snapshot.channel_count(), 2);
    assert_eq!(snapshot.nodes().len(), 2);
    assert_eq!(snapshot.connections().len(), 1);
    assert_eq!(snapshot.output_node(), RenderNodeId::new(2));
}

#[test]
fn rejects_duplicate_node_ids() {
    let mut invalid_nodes = nodes();
    invalid_nodes[1].id = invalid_nodes[0].id;

    let error = RenderSnapshot::build(
        ProjectId::new(),
        0,
        48_000,
        2,
        invalid_nodes,
        vec![],
        RenderNodeId::new(1),
    )
    .unwrap_err();

    assert_eq!(error.code, SnapshotErrorCode::DuplicateNodeId);
}

#[test]
fn rejects_connections_or_output_that_reference_missing_nodes() {
    let missing_connection = RenderSnapshot::build(
        ProjectId::new(),
        0,
        48_000,
        2,
        nodes(),
        vec![RenderConnection {
            from: RenderNodeId::new(99),
            to: RenderNodeId::new(2),
        }],
        RenderNodeId::new(2),
    )
    .unwrap_err();
    assert_eq!(
        missing_connection.code,
        SnapshotErrorCode::MissingNodeReference
    );

    let missing_output = RenderSnapshot::build(
        ProjectId::new(),
        0,
        48_000,
        2,
        nodes(),
        vec![],
        RenderNodeId::new(99),
    )
    .unwrap_err();
    assert_eq!(missing_output.code, SnapshotErrorCode::MissingOutputNode);
}

#[test]
fn rejects_invalid_audio_format() {
    let error = RenderSnapshot::build(
        ProjectId::new(),
        0,
        0,
        2,
        nodes(),
        vec![],
        RenderNodeId::new(2),
    )
    .unwrap_err();

    assert_eq!(error.code, SnapshotErrorCode::InvalidAudioFormat);
}
