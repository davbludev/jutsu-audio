use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use arc_swap::ArcSwapOption;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use jutsu_audio_extensions::ExtensionTypeId;
use jutsu_audio_model::{AssetId, BusId, ClipId, ParameterValue, ProjectId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TransportState {
    Stopped = 0,
    Playing = 1,
    Paused = 2,
}

#[derive(Debug)]
struct TransportShared {
    state: AtomicU8,
    position_frames: AtomicU64,
    underruns: AtomicU64,
}

#[derive(Clone, Debug)]
pub struct TransportController(Arc<TransportShared>);

#[derive(Clone, Debug)]
pub struct TransportReader(Arc<TransportShared>);

impl TransportController {
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(TransportShared {
            state: AtomicU8::new(TransportState::Stopped as u8),
            position_frames: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
        }))
    }

    #[must_use]
    pub fn reader(&self) -> TransportReader {
        TransportReader(Arc::clone(&self.0))
    }

    pub fn play(&self) {
        self.0
            .state
            .store(TransportState::Playing as u8, Ordering::Release);
    }

    pub fn pause(&self) {
        self.0
            .state
            .store(TransportState::Paused as u8, Ordering::Release);
    }

    pub fn stop(&self) {
        self.0
            .state
            .store(TransportState::Stopped as u8, Ordering::Release);
        self.0.position_frames.store(0, Ordering::Release);
    }

    pub fn seek(&self, frame: u64) {
        self.0.position_frames.store(frame, Ordering::Release);
    }

    #[must_use]
    pub fn state(&self) -> TransportState {
        load_transport_state(&self.0.state)
    }

    #[must_use]
    pub fn position_frames(&self) -> u64 {
        self.0.position_frames.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn underrun_count(&self) -> u64 {
        self.0.underruns.load(Ordering::Relaxed)
    }
}

impl Default for TransportController {
    fn default() -> Self {
        Self::new()
    }
}

fn load_transport_state(state: &AtomicU8) -> TransportState {
    match state.load(Ordering::Acquire) {
        1 => TransportState::Playing,
        2 => TransportState::Paused,
        _ => TransportState::Stopped,
    }
}

#[derive(Clone, Debug)]
pub struct PlaybackSnapshot {
    sample_rate: u32,
    channel_count: u16,
    samples: Arc<[f32]>,
}

impl PlaybackSnapshot {
    pub fn new(
        sample_rate: u32,
        channel_count: u16,
        samples: Arc<[f32]>,
    ) -> Result<Self, SnapshotError> {
        if sample_rate == 0
            || channel_count == 0
            || !samples.len().is_multiple_of(usize::from(channel_count))
        {
            return Err(SnapshotError::new(
                SnapshotErrorCode::InvalidAudioFormat,
                "playback audio must have a valid format and complete interleaved frames",
            ));
        }
        Ok(Self {
            sample_rate,
            channel_count,
            samples,
        })
    }

    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    #[must_use]
    pub const fn channel_count(&self) -> u16 {
        self.channel_count
    }
}

#[derive(Clone)]
pub struct SnapshotExchange(Arc<ArcSwapOption<PlaybackSnapshot>>);

#[derive(Clone)]
pub struct SnapshotReader(Arc<ArcSwapOption<PlaybackSnapshot>>);

impl SnapshotExchange {
    #[must_use]
    pub fn new(snapshot: Option<Arc<PlaybackSnapshot>>) -> Self {
        Self(Arc::new(ArcSwapOption::new(snapshot)))
    }

    #[must_use]
    pub fn reader(&self) -> SnapshotReader {
        SnapshotReader(Arc::clone(&self.0))
    }

    pub fn publish(&self, snapshot: Arc<PlaybackSnapshot>) {
        self.0.store(Some(snapshot));
    }

    pub fn clear(&self) {
        self.0.store(None);
    }

    #[must_use]
    pub fn current(&self) -> Option<Arc<PlaybackSnapshot>> {
        self.0.load_full()
    }
}

pub struct PlaybackRenderer {
    snapshots: SnapshotReader,
    transport: TransportReader,
}

impl PlaybackRenderer {
    #[must_use]
    pub const fn new(snapshots: SnapshotReader, transport: TransportReader) -> Self {
        Self {
            snapshots,
            transport,
        }
    }

    pub fn render(&mut self, output: &mut [f32]) {
        output.fill(0.0);
        if load_transport_state(&self.transport.0.state) != TransportState::Playing {
            return;
        }
        let Some(snapshot) = self.snapshots.0.load_full() else {
            self.transport.0.underruns.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let channels = usize::from(snapshot.channel_count);
        if !output.len().is_multiple_of(channels) {
            self.transport.0.underruns.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let frame = self.transport.0.position_frames.load(Ordering::Acquire);
        let start = usize::try_from(frame)
            .ok()
            .and_then(|value| value.checked_mul(channels));
        let Some(source) = start.and_then(|start| snapshot.samples.get(start..)) else {
            self.transport.0.underruns.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let copied = output.len().min(source.len());
        output[..copied].copy_from_slice(&source[..copied]);
        let rendered_frames = copied / channels;
        self.transport
            .0
            .position_frames
            .fetch_add(rendered_frames as u64, Ordering::Release);
        if copied < output.len() {
            self.transport.0.underruns.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Debug)]
pub enum AudioOutputError {
    NoOutputDevice,
    DefaultConfig(String),
    UnsupportedSampleFormat(String),
    BuildStream(String),
    StartStream(String),
}

pub struct SystemAudioOutput {
    _stream: cpal::Stream,
    pub sample_rate: u32,
    pub channel_count: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportEncoding {
    Pcm16,
    Float32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportRange {
    pub start_frame: u64,
    pub frame_count: u64,
}

impl ExportRange {
    #[must_use]
    pub const fn full() -> Self {
        Self {
            start_frame: 0,
            frame_count: u64::MAX,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportReport {
    pub sample_rate: u32,
    pub channel_count: u16,
    pub frame_count: u64,
}

#[derive(Debug)]
pub struct ExportError {
    pub message: String,
}

pub struct OfflineExporter;

impl OfflineExporter {
    pub fn export_wav(
        snapshot: Arc<PlaybackSnapshot>,
        path: impl AsRef<std::path::Path>,
        range: ExportRange,
        encoding: ExportEncoding,
    ) -> Result<ExportReport, ExportError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|error| ExportError {
                message: format!("failed to create export directory: {error}"),
            })?;
        }
        let channels = usize::from(snapshot.channel_count);
        let total_frames = snapshot.samples.len() / channels;
        let start = usize::try_from(range.start_frame)
            .unwrap_or(usize::MAX)
            .min(total_frames);
        let requested = usize::try_from(range.frame_count).unwrap_or(usize::MAX);
        let end = start.saturating_add(requested).min(total_frames);
        let samples = &snapshot.samples[start * channels..end * channels];
        let (bits_per_sample, sample_format) = match encoding {
            ExportEncoding::Pcm16 => (16, hound::SampleFormat::Int),
            ExportEncoding::Float32 => (32, hound::SampleFormat::Float),
        };
        let spec = hound::WavSpec {
            channels: snapshot.channel_count,
            sample_rate: snapshot.sample_rate,
            bits_per_sample,
            sample_format,
        };
        let mut writer = hound::WavWriter::create(path, spec).map_err(|error| ExportError {
            message: format!("failed to create WAV export: {error}"),
        })?;
        for &sample in samples {
            let result = match encoding {
                ExportEncoding::Float32 => writer.write_sample(sample.clamp(-1.0, 1.0)),
                ExportEncoding::Pcm16 => {
                    let normalized = sample.clamp(-1.0, 1.0);
                    let quantized = if normalized <= -1.0 {
                        i16::MIN
                    } else {
                        (normalized * f32::from(i16::MAX)).round() as i16
                    };
                    writer.write_sample(quantized)
                }
            };
            result.map_err(|error| ExportError {
                message: format!("failed to write WAV sample: {error}"),
            })?;
        }
        writer.finalize().map_err(|error| ExportError {
            message: format!("failed to finalize WAV export: {error}"),
        })?;
        Ok(ExportReport {
            sample_rate: snapshot.sample_rate,
            channel_count: snapshot.channel_count,
            frame_count: (end - start) as u64,
        })
    }
}

impl SystemAudioOutput {
    pub fn open_default(mut renderer: PlaybackRenderer) -> Result<Self, AudioOutputError> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or(AudioOutputError::NoOutputDevice)?;
        let supported = device
            .default_output_config()
            .map_err(|error| AudioOutputError::DefaultConfig(error.to_string()))?;
        if supported.sample_format() != cpal::SampleFormat::F32 {
            return Err(AudioOutputError::UnsupportedSampleFormat(format!(
                "{:?}",
                supported.sample_format()
            )));
        }
        let sample_rate = supported.sample_rate();
        let channel_count = supported.channels();
        let config = supported.config();
        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _| renderer.render(data),
                |_| {},
                None,
            )
            .map_err(|error| AudioOutputError::BuildStream(error.to_string()))?;
        stream
            .play()
            .map_err(|error| AudioOutputError::StartStream(error.to_string()))?;
        Ok(Self {
            _stream: stream,
            sample_rate,
            channel_count,
        })
    }
}

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
