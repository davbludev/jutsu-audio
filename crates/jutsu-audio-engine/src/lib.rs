use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

use arc_swap::ArcSwapOption;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use jutsu_audio_extensions::ExtensionTypeId;
use jutsu_audio_model::{AssetId, BusId, ClipId, LoopRegion, ParameterValue, ProjectId};

pub mod mixdown;

pub use mixdown::{
    MIX_CHANNELS, Meters, MixError, MixErrorCode, MixOutput, SourceAudio, mix_project,
    mix_project_metered,
};

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
    /// Peak absolute sample of the last rendered block, as `f32::to_bits`.
    /// Written by the audio callback, read by whoever draws a meter.
    peak_level: AtomicU32,
    /// Loop bounds in frames, half-open. `loop_end == 0` means "not looping",
    /// which keeps the callback free of a third atomic to check.
    loop_start: AtomicU64,
    loop_end: AtomicU64,
}

impl TransportShared {
    /// The active loop, already ordered. Read once per callback.
    fn loop_bounds(&self) -> Option<(u64, u64)> {
        let end = self.loop_end.load(Ordering::Acquire);
        if end == 0 {
            return None;
        }
        let start = self.loop_start.load(Ordering::Acquire);
        (end > start).then_some((start, end))
    }
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
            peak_level: AtomicU32::new(0),
            loop_start: AtomicU64::new(0),
            loop_end: AtomicU64::new(0),
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

    /// Sets the region playback repeats over, or clears it. A disabled or empty
    /// region clears it too: the project remembers where the loop was, the
    /// transport only needs to know whether to wrap.
    pub fn set_loop(&self, region: Option<LoopRegion>) {
        match region.filter(LoopRegion::is_active) {
            Some(region) => {
                self.0
                    .loop_start
                    .store(region.start_frame, Ordering::Release);
                self.0.loop_end.store(region.end_frame, Ordering::Release);
            }
            None => self.0.loop_end.store(0, Ordering::Release),
        }
    }

    /// The bounds the renderer is wrapping between, if any.
    #[must_use]
    pub fn loop_bounds(&self) -> Option<(u64, u64)> {
        self.0.loop_bounds()
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

    /// Peak absolute sample of the most recently rendered block, 0.0 when
    /// nothing is playing. Meters should apply their own decay on top.
    #[must_use]
    pub fn peak_level(&self) -> f32 {
        f32::from_bits(self.0.peak_level.load(Ordering::Relaxed))
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

    #[must_use]
    pub fn frame_count(&self) -> u64 {
        (self.samples.len() / usize::from(self.channel_count)) as u64
    }

    #[must_use]
    pub fn samples(&self) -> &[f32] {
        &self.samples
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

/// Real-time renderer: pulls the published snapshot, adapts it to the output
/// device format, and advances the transport.
///
/// The audio callback runs this, so it never allocates, locks, or does I/O.
pub struct PlaybackRenderer {
    snapshots: SnapshotReader,
    transport: TransportReader,
    output_sample_rate: u32,
    output_channels: u16,
    /// Sub-frame read position carried between callbacks. Audio thread only.
    fraction: f64,
    /// Last position this renderer wrote, so an external seek can be detected.
    last_position: u64,
    /// The mix that was playing before the current one, and how many frames of
    /// blending are left. Holding the `Arc` keeps the old audio alive for the
    /// length of the fade and nothing longer.
    crossfade: Option<(Arc<PlaybackSnapshot>, u32)>,
    /// What the renderer last played, so a published mix can be noticed.
    current: Option<Arc<PlaybackSnapshot>>,
}

/// How long a newly published mix takes to replace the old one. Short enough
/// not to smear an edit, long enough to hide the step.
const CROSSFADE_FRAMES: u32 = 240;

impl PlaybackRenderer {
    #[must_use]
    pub fn new(
        snapshots: SnapshotReader,
        transport: TransportReader,
        output_sample_rate: u32,
        output_channels: u16,
    ) -> Self {
        Self {
            snapshots,
            transport,
            output_sample_rate,
            output_channels,
            fraction: 0.0,
            last_position: 0,
            crossfade: None,
            current: None,
        }
    }

    fn underrun(&self) {
        self.transport.0.underruns.fetch_add(1, Ordering::Relaxed);
    }

    fn publish_peak(&self, level: f32) {
        self.transport
            .0
            .peak_level
            .store(level.to_bits(), Ordering::Relaxed);
    }

    /// Material ran out. Stop and rewind, so the next Play starts from the top
    /// instead of sitting past the end reporting underruns forever.
    fn finish(&mut self) {
        self.transport
            .0
            .state
            .store(TransportState::Stopped as u8, Ordering::Release);
        self.transport.0.position_frames.store(0, Ordering::Release);
        self.fraction = 0.0;
        self.last_position = 0;
    }

    /// Renders one device block, wrapping at the loop if there is one.
    ///
    /// Looping is done by rendering in segments that stop exactly on the loop
    /// end, so the wrap lands on a frame rather than on a block boundary.
    pub fn render(&mut self, output: &mut [f32]) {
        output.fill(0.0);
        if load_transport_state(&self.transport.0.state) != TransportState::Playing {
            self.publish_peak(0.0);
            return;
        }
        let Some((start, end)) = self.transport.0.loop_bounds() else {
            self.render_block(output);
            return;
        };
        let channels = usize::from(self.output_channels).max(1);
        let mut filled = 0;
        while filled < output.len() {
            let position = self.transport.0.position_frames.load(Ordering::Acquire);
            if position < start || position >= end {
                self.transport
                    .0
                    .position_frames
                    .store(start, Ordering::Release);
                self.fraction = 0.0;
            }
            let position = self.transport.0.position_frames.load(Ordering::Acquire);
            let frames_left = usize::try_from(end.saturating_sub(position)).unwrap_or(usize::MAX);
            let segment = (output.len() - filled).min(frames_left.saturating_mul(channels));
            if segment == 0 {
                break;
            }
            self.render_block(&mut output[filled..filled + segment]);
            filled += segment;
            if load_transport_state(&self.transport.0.state) != TransportState::Playing {
                // The material ran out inside the loop; the block already
                // stopped the transport, and there is nothing left to wrap to.
                return;
            }
        }
    }

    fn render_block(&mut self, output: &mut [f32]) {
        let Some(snapshot) = self.snapshots.0.load_full() else {
            self.underrun();
            return;
        };
        // A different mix than last block means someone edited while playing.
        // Keep the old one for a few milliseconds to fade out from under it.
        match &self.current {
            Some(current) if Arc::ptr_eq(current, &snapshot) => {}
            Some(current) => {
                self.crossfade = Some((Arc::clone(current), CROSSFADE_FRAMES));
                self.current = Some(Arc::clone(&snapshot));
            }
            None => self.current = Some(Arc::clone(&snapshot)),
        }
        let source_channels = usize::from(snapshot.channel_count);
        let output_channels = usize::from(self.output_channels);
        if output_channels == 0 || !output.len().is_multiple_of(output_channels) {
            self.underrun();
            return;
        }
        let total_frames = snapshot.samples.len() / source_channels;
        let position = self.transport.0.position_frames.load(Ordering::Acquire);
        if position != self.last_position {
            // Someone seeked or stopped between callbacks; drop the stale phase,
            // and the fade with it — blending across a jump would mix two
            // unrelated moments together.
            self.fraction = 0.0;
            self.crossfade = None;
        }
        if position >= total_frames as u64 {
            self.finish();
            return;
        }

        if snapshot.sample_rate == self.output_sample_rate && source_channels == output_channels {
            self.render_direct(&snapshot, output, position, output_channels);
        } else {
            self.render_converted(
                &snapshot,
                output,
                position,
                total_frames,
                source_channels,
                output_channels,
            );
        }
    }

    /// Device format already matches the snapshot: copy verbatim, so real-time
    /// output stays bit-identical to what `OfflineExporter` writes.
    ///
    /// When a new mix has just been published, the first few milliseconds are
    /// blended out of the old one. A gain or routing change during playback
    /// would otherwise step the waveform mid-note, which is audible as a click.
    fn render_direct(
        &mut self,
        snapshot: &PlaybackSnapshot,
        output: &mut [f32],
        position: u64,
        channels: usize,
    ) {
        let start = usize::try_from(position)
            .ok()
            .and_then(|value| value.checked_mul(channels));
        let Some(source) = start.and_then(|start| snapshot.samples.get(start..)) else {
            self.underrun();
            return;
        };
        let copied = output.len().min(source.len());
        output[..copied].copy_from_slice(&source[..copied]);
        self.blend_previous(output, position, channels, copied);
        self.publish_peak(block_peak(&output[..copied]));
        let advanced = position + (copied / channels) as u64;
        self.transport
            .0
            .position_frames
            .store(advanced, Ordering::Release);
        self.last_position = advanced;
        if copied < output.len() {
            self.finish();
        }
    }

    /// Sample rates or channel counts differ. Linear interpolation on the time
    /// axis, `channel % source_channels` fan-out on the channel axis, with a
    /// straight average when folding down to mono.
    ///
    /// ponytail: linear interpolation aliases above ~0.4 Nyquist; swap in a
    /// windowed-sinc resampler if export quality ever depends on this path.
    fn render_converted(
        &mut self,
        snapshot: &PlaybackSnapshot,
        output: &mut [f32],
        position: u64,
        total_frames: usize,
        source_channels: usize,
        output_channels: usize,
    ) {
        let ratio = f64::from(snapshot.sample_rate) / f64::from(self.output_sample_rate);
        let samples = &snapshot.samples;
        let downmix = output_channels == 1 && source_channels > 1;
        let mut fraction = self.fraction;
        let mut frame = position;
        let mut ended = false;

        for block in output.chunks_exact_mut(output_channels) {
            let Ok(index) = usize::try_from(frame) else {
                ended = true;
                break;
            };
            if index >= total_frames {
                ended = true;
                break;
            }
            let next = (index + 1).min(total_frames - 1);
            let blend = fraction as f32;
            let base = index * source_channels;
            let next_base = next * source_channels;

            if downmix {
                let scale = 1.0 / source_channels as f32;
                let mut current = 0.0;
                let mut upcoming = 0.0;
                for channel in 0..source_channels {
                    current += samples[base + channel];
                    upcoming += samples[next_base + channel];
                }
                block[0] = lerp(current * scale, upcoming * scale, blend);
            } else {
                for (channel, slot) in block.iter_mut().enumerate() {
                    let source = channel % source_channels;
                    *slot = lerp(samples[base + source], samples[next_base + source], blend);
                }
            }

            fraction += ratio;
            let whole = fraction.floor();
            frame = frame.saturating_add(whole as u64);
            fraction -= whole;
        }

        self.fraction = fraction;
        self.publish_peak(block_peak(output));
        self.transport
            .0
            .position_frames
            .store(frame, Ordering::Release);
        self.last_position = frame;
        if ended {
            self.finish();
        }
    }
}

impl PlaybackRenderer {
    /// Fades the outgoing mix under the incoming one for [`CROSSFADE_FRAMES`].
    ///
    /// Only the verbatim path does this: it is the one that plays while the
    /// user edits, and the one where a hard swap is most audible. A converted
    /// path swaps outright.
    ///
    /// ponytail: a linear blend over a few milliseconds. Enough to hide a level
    /// change; a longer, equal-power fade would be the next step if an edit to
    /// a dense mix still ticks.
    fn blend_previous(
        &mut self,
        output: &mut [f32],
        position: u64,
        channels: usize,
        copied: usize,
    ) {
        let Some((previous, remaining)) = self.crossfade.as_mut() else {
            return;
        };
        let Some(start) = usize::try_from(position)
            .ok()
            .and_then(|value| value.checked_mul(channels))
        else {
            self.crossfade = None;
            return;
        };
        let Some(old) = previous.samples.get(start..) else {
            self.crossfade = None;
            return;
        };

        let frames = copied / channels.max(1);
        let mut faded = 0;
        for frame in 0..frames {
            if *remaining == 0 {
                break;
            }
            // 0.0 at the moment of the swap, 1.0 once the fade is done.
            let blend = 1.0 - (*remaining as f32 / CROSSFADE_FRAMES as f32);
            for channel in 0..channels {
                let index = frame * channels + channel;
                let Some(old_sample) = old.get(index) else {
                    break;
                };
                output[index] = lerp(*old_sample, output[index], blend);
            }
            *remaining -= 1;
            faded += 1;
        }
        if *remaining == 0 || faded == 0 {
            self.crossfade = None;
        }
    }
}

fn lerp(from: f32, to: f32, blend: f32) -> f32 {
    from + (to - from) * blend
}

fn block_peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
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
    /// Opens the default device and builds a renderer bound to *that device's*
    /// format. The renderer cannot be supplied from outside: it has to know the
    /// real output rate and channel count to convert the snapshot correctly.
    pub fn open_default(
        snapshots: SnapshotReader,
        transport: TransportReader,
    ) -> Result<Self, AudioOutputError> {
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
        let mut renderer = PlaybackRenderer::new(snapshots, transport, sample_rate, channel_count);
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
