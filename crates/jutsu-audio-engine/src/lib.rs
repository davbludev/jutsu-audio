use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use jutsu_audio_extensions::ExtensionTypeId;
use jutsu_audio_model::{AssetId, BusId, ClipId, ParameterValue, ProjectId};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RenderNodeId(u64);

impl RenderNodeId {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProcessorSpec {
    SampleClip {
        clip_id: ClipId,
        asset_id: AssetId,
        start_sample: u64,
        source_start_sample: u64,
        duration_samples: u64,
        parameters: BTreeMap<String, ParameterValue>,
    },
    Synth {
        type_id: ExtensionTypeId,
        state_version: u32,
        parameters: BTreeMap<String, ParameterValue>,
    },
    Effect {
        type_id: ExtensionTypeId,
        state_version: u32,
        parameters: BTreeMap<String, ParameterValue>,
    },
    Mixer {
        bus_id: BusId,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderNode {
    pub id: RenderNodeId,
    pub processor: ProcessorSpec,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderConnection {
    pub from: RenderNodeId,
    pub to: RenderNodeId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotErrorCode {
    InvalidAudioFormat,
    DuplicateNodeId,
    MissingNodeReference,
    MissingOutputNode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotError {
    pub code: SnapshotErrorCode,
    pub message: String,
    pub node_id: Option<RenderNodeId>,
    pub connection_index: Option<usize>,
}

impl SnapshotError {
    fn new(code: SnapshotErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            node_id: None,
            connection_index: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RenderSnapshot {
    project_id: ProjectId,
    project_revision: u64,
    sample_rate: u32,
    channel_count: u16,
    nodes: Arc<[RenderNode]>,
    connections: Arc<[RenderConnection]>,
    output_node: RenderNodeId,
}

impl RenderSnapshot {
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        project_id: ProjectId,
        project_revision: u64,
        sample_rate: u32,
        channel_count: u16,
        nodes: Vec<RenderNode>,
        connections: Vec<RenderConnection>,
        output_node: RenderNodeId,
    ) -> Result<Self, SnapshotError> {
        if sample_rate == 0 || channel_count == 0 {
            return Err(SnapshotError::new(
                SnapshotErrorCode::InvalidAudioFormat,
                "sample rate and channel count must be positive",
            ));
        }

        let mut node_ids = HashSet::with_capacity(nodes.len());
        for node in &nodes {
            if !node_ids.insert(node.id) {
                return Err(SnapshotError {
                    code: SnapshotErrorCode::DuplicateNodeId,
                    message: format!("render node {} is duplicated", node.id.get()),
                    node_id: Some(node.id),
                    connection_index: None,
                });
            }
        }

        if !node_ids.contains(&output_node) {
            return Err(SnapshotError {
                code: SnapshotErrorCode::MissingOutputNode,
                message: format!("output node {} does not exist", output_node.get()),
                node_id: Some(output_node),
                connection_index: None,
            });
        }

        for (connection_index, connection) in connections.iter().enumerate() {
            let missing_node = if !node_ids.contains(&connection.from) {
                Some(connection.from)
            } else if !node_ids.contains(&connection.to) {
                Some(connection.to)
            } else {
                None
            };
            if let Some(node_id) = missing_node {
                return Err(SnapshotError {
                    code: SnapshotErrorCode::MissingNodeReference,
                    message: format!(
                        "connection {connection_index} references missing node {}",
                        node_id.get()
                    ),
                    node_id: Some(node_id),
                    connection_index: Some(connection_index),
                });
            }
        }

        Ok(Self {
            project_id,
            project_revision,
            sample_rate,
            channel_count,
            nodes: nodes.into(),
            connections: connections.into(),
            output_node,
        })
    }

    #[must_use]
    pub const fn project_id(&self) -> ProjectId {
        self.project_id
    }

    #[must_use]
    pub const fn project_revision(&self) -> u64 {
        self.project_revision
    }

    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    #[must_use]
    pub const fn channel_count(&self) -> u16 {
        self.channel_count
    }

    #[must_use]
    pub fn nodes(&self) -> &[RenderNode] {
        &self.nodes
    }

    #[must_use]
    pub fn connections(&self) -> &[RenderConnection] {
        &self.connections
    }

    #[must_use]
    pub const fn output_node(&self) -> RenderNodeId {
        self.output_node
    }
}
