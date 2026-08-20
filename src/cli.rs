use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use jutsu_audio_commands::edits::{self, DeleteMode};
use jutsu_audio_commands::notes;
use jutsu_audio_commands::{CommandError, EffectTarget, ProjectCommand};
use jutsu_audio_engine::{
    ExportEncoding, ExportRange, MIX_CHANNELS, OfflineExporter, PlaybackSnapshot, SourceAudio,
    mix_project,
};
use jutsu_audio_extensions::{ExtensionTypeId, RegenerateMode};
use jutsu_audio_model::{
    AssetId, AudioAssetSource, AutomationId, AutomationTarget, Breakpoint, BusId, Clip, ClipId,
    ClipNote, Curve, EffectId, Layer, LayerId, LoopRegion, Marker, MarkerId, MusicalPosition,
    ParameterValue, PatternId, Project, SampleLoopMode, SamplerZone, TempoChange, Track, TrackId,
};
use jutsu_audio_project::presets::{IncompatibilityCode, Preset, PresetKind, PresetPayload};
use jutsu_audio_project::{AssetManager, ImportMode, ImportStatus, ProjectStore};
use jutsu_audio_session::TransportAction as SessionTransportAction;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::cli_session::{self, Applied};
use crate::{cli_generator, cli_mixer, cli_presets, cli_synth};

pub const CLI_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum Request {
    CreateProject {
        protocol_version: u32,
        path: PathBuf,
        name: String,
    },
    InspectProject {
        protocol_version: u32,
        path: PathBuf,
    },
    ImportSample {
        protocol_version: u32,
        path: PathBuf,
        source: PathBuf,
    },
    AddClip {
        protocol_version: u32,
        path: PathBuf,
        asset_id: AssetId,
        track_id: TrackId,
        layer_id: LayerId,
        start_sample: u64,
        source_start_sample: u64,
        duration_samples: u64,
        #[serde(default)]
        gain_db: f64,
    },
    UpdateClip {
        protocol_version: u32,
        path: PathBuf,
        clip_id: ClipId,
        start_sample: u64,
        source_start_sample: u64,
        duration_samples: u64,
        gain_db: f64,
    },
    DeleteClip {
        protocol_version: u32,
        path: PathBuf,
        clip_id: ClipId,
        /// Close the gap the clip leaves behind. Defaults to leaving it.
        #[serde(default)]
        ripple: bool,
    },
    ExportWav {
        protocol_version: u32,
        path: PathBuf,
        output: PathBuf,
        encoding: CliExportEncoding,
        #[serde(default)]
        start_frame: u64,
        #[serde(default = "full_frame_count")]
        frame_count: u64,
        /// Render the project's active loop instead of the whole timeline, so
        /// an exported loop matches what playback repeats.
        #[serde(default)]
        use_loop_region: bool,
    },
    #[serde(rename = "transport_request")]
    Transport {
        protocol_version: u32,
        path: PathBuf,
        action: TransportAction,
        #[serde(default)]
        position_frames: u64,
    },
    SessionStatus {
        protocol_version: u32,
        path: PathBuf,
    },
    AddTrack {
        protocol_version: u32,
        path: PathBuf,
        name: String,
    },
    AddLayer {
        protocol_version: u32,
        path: PathBuf,
        track_id: TrackId,
        name: String,
    },
    SetTrackMute {
        protocol_version: u32,
        path: PathBuf,
        track_id: TrackId,
        muted: bool,
    },
    SetTrackSolo {
        protocol_version: u32,
        path: PathBuf,
        track_id: TrackId,
        soloed: bool,
    },
    SetClipPan {
        protocol_version: u32,
        path: PathBuf,
        clip_id: ClipId,
        pan: f64,
    },
    SplitClip {
        protocol_version: u32,
        path: PathBuf,
        clip_id: ClipId,
        at_frame: u64,
    },
    DuplicateClip {
        protocol_version: u32,
        path: PathBuf,
        clip_id: ClipId,
        offset_frames: u64,
    },
    SlipClip {
        protocol_version: u32,
        path: PathBuf,
        clip_id: ClipId,
        delta_frames: i64,
    },
    SetClipFades {
        protocol_version: u32,
        path: PathBuf,
        clip_id: ClipId,
        fade_in_samples: u64,
        fade_out_samples: u64,
    },
    CrossfadeClips {
        protocol_version: u32,
        path: PathBuf,
        first_clip_id: ClipId,
        second_clip_id: ClipId,
    },
    AddMarker {
        protocol_version: u32,
        path: PathBuf,
        name: String,
        frame: u64,
    },
    MoveMarker {
        protocol_version: u32,
        path: PathBuf,
        marker_id: MarkerId,
        frame: u64,
    },
    RemoveMarker {
        protocol_version: u32,
        path: PathBuf,
        marker_id: MarkerId,
    },
    SetLoopRegion {
        protocol_version: u32,
        path: PathBuf,
        start_frame: u64,
        end_frame: u64,
        #[serde(default = "enabled_by_default")]
        enabled: bool,
    },
    ClearLoopRegion {
        protocol_version: u32,
        path: PathBuf,
    },
    /// Discovery: every registered synth, effect and generator with its
    /// parameters. Takes no project, because it describes the build.
    ListExtensions { protocol_version: u32 },
    AddSynthClip {
        protocol_version: u32,
        path: PathBuf,
        track_id: TrackId,
        layer_id: LayerId,
        type_id: String,
        start_sample: u64,
        duration_samples: u64,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        parameters: BTreeMap<String, ParameterValue>,
        #[serde(default)]
        notes: Vec<CliNote>,
    },
    SetSynthParameters {
        protocol_version: u32,
        path: PathBuf,
        asset_id: AssetId,
        parameters: BTreeMap<String, ParameterValue>,
    },
    SetClipNotes {
        protocol_version: u32,
        path: PathBuf,
        clip_id: ClipId,
        notes: Vec<CliNote>,
    },
    /// One generator's full schema: parameters, bounds and presets.
    DescribeGenerator {
        protocol_version: u32,
        type_id: String,
    },
    /// Renders a recipe and reports what it sounds like, without touching a
    /// project. Writes a WAV when `output` is given.
    PreviewGenerator {
        protocol_version: u32,
        type_id: String,
        seed: u64,
        frame_count: u64,
        #[serde(default)]
        parameters: BTreeMap<String, ParameterValue>,
        #[serde(default)]
        output: Option<PathBuf>,
    },
    AddBus {
        protocol_version: u32,
        path: PathBuf,
        name: String,
        /// Where the new bus sends. Defaults to the master.
        #[serde(default)]
        output_bus_id: Option<BusId>,
    },
    SetTrackOutput {
        protocol_version: u32,
        path: PathBuf,
        track_id: TrackId,
        output_bus_id: BusId,
    },
    SetBusOutput {
        protocol_version: u32,
        path: PathBuf,
        bus_id: BusId,
        #[serde(default)]
        output_bus_id: Option<BusId>,
    },
    SetTrackParameter {
        protocol_version: u32,
        path: PathBuf,
        track_id: TrackId,
        key: String,
        value: ParameterValue,
    },
    SetBusParameter {
        protocol_version: u32,
        path: PathBuf,
        bus_id: BusId,
        key: String,
        value: ParameterValue,
    },
    /// The parameters every track and bus strip has.
    DescribeStrip { protocol_version: u32 },
    /// Every preset this build can offer: the built-in ones from the
    /// extensions, and the user ones in the library.
    ListPresets {
        protocol_version: u32,
        path: PathBuf,
        #[serde(default)]
        library: Option<PathBuf>,
    },
    /// Saves what something in the project is set to, as a user preset.
    SavePreset {
        protocol_version: u32,
        path: PathBuf,
        name: String,
        #[serde(default)]
        tags: Vec<String>,
        #[serde(flatten)]
        target: CliPresetTarget,
        #[serde(default)]
        library: Option<PathBuf>,
    },
    /// Applies a user preset to something in the project.
    ApplyPreset {
        protocol_version: u32,
        path: PathBuf,
        preset_id: String,
        #[serde(flatten)]
        target: CliPresetTarget,
        #[serde(default)]
        library: Option<PathBuf>,
    },
    ImportPreset {
        protocol_version: u32,
        path: PathBuf,
        from: PathBuf,
        #[serde(default)]
        library: Option<PathBuf>,
    },
    ExportPreset {
        protocol_version: u32,
        path: PathBuf,
        preset_id: String,
        kind: CliPresetKind,
        to: PathBuf,
        #[serde(default)]
        library: Option<PathBuf>,
    },
    /// Creates a sampler instrument from a mapping of the project's samples.
    AddSampler {
        protocol_version: u32,
        path: PathBuf,
        name: String,
        zones: Vec<CliZone>,
        #[serde(default)]
        attack_ms: f64,
        #[serde(default = "default_release_ms")]
        release_ms: f64,
        #[serde(default = "default_max_voices")]
        max_voices: u32,
    },
    SetSamplerZones {
        protocol_version: u32,
        path: PathBuf,
        asset_id: AssetId,
        zones: Vec<CliZone>,
    },
    AddPattern {
        protocol_version: u32,
        path: PathBuf,
        name: String,
        length_frames: u64,
        #[serde(default)]
        notes: Vec<CliNote>,
    },
    SetPatternNotes {
        protocol_version: u32,
        path: PathBuf,
        pattern_id: PatternId,
        length_frames: u64,
        notes: Vec<CliNote>,
    },
    RemovePattern {
        protocol_version: u32,
        path: PathBuf,
        pattern_id: PatternId,
    },
    /// Points a clip at a pattern, or unlinks it when `pattern_id` is absent.
    SetClipPattern {
        protocol_version: u32,
        path: PathBuf,
        clip_id: ClipId,
        #[serde(default)]
        pattern_id: Option<PatternId>,
    },
    QuantiseClip {
        protocol_version: u32,
        path: PathBuf,
        clip_id: ClipId,
        #[serde(default = "four_divisions")]
        divisions_per_beat: u32,
    },
    TransposeClip {
        protocol_version: u32,
        path: PathBuf,
        clip_id: ClipId,
        semitones: f64,
    },
    HumaniseClip {
        protocol_version: u32,
        path: PathBuf,
        clip_id: ClipId,
        seed: u64,
        #[serde(default)]
        timing_frames: u64,
        #[serde(default)]
        velocity_amount: f64,
    },
    LoopClipNotes {
        protocol_version: u32,
        path: PathBuf,
        clip_id: ClipId,
        period_frames: u64,
        repeats: u32,
    },
    /// Replaces the tempo map. An empty list means the default: 120 BPM, 4/4.
    SetTempoMap {
        protocol_version: u32,
        path: PathBuf,
        changes: Vec<CliTempoChange>,
    },
    /// Converts between frames and musical time, both ways, using the
    /// project's own tempo — the same conversion the editor displays.
    ConvertTime {
        protocol_version: u32,
        path: PathBuf,
        #[serde(default)]
        frame: Option<u64>,
        #[serde(default)]
        position: Option<CliPosition>,
    },
    DescribeEffect {
        protocol_version: u32,
        type_id: String,
    },
    AddEffect {
        protocol_version: u32,
        path: PathBuf,
        #[serde(flatten)]
        target: CliEffectTarget,
        type_id: String,
        #[serde(default)]
        parameters: BTreeMap<String, ParameterValue>,
    },
    RemoveEffect {
        protocol_version: u32,
        path: PathBuf,
        effect_id: EffectId,
    },
    MoveEffect {
        protocol_version: u32,
        path: PathBuf,
        effect_id: EffectId,
        to_index: usize,
    },
    SetEffectEnabled {
        protocol_version: u32,
        path: PathBuf,
        effect_id: EffectId,
        enabled: bool,
    },
    SetEffectWet {
        protocol_version: u32,
        path: PathBuf,
        effect_id: EffectId,
        wet: f64,
    },
    SetEffectParameters {
        protocol_version: u32,
        path: PathBuf,
        effect_id: EffectId,
        parameters: BTreeMap<String, ParameterValue>,
    },
    AddAutomationLane {
        protocol_version: u32,
        path: PathBuf,
        target: AutomationTarget,
        parameter: String,
        #[serde(default)]
        points: Vec<CliBreakpoint>,
    },
    SetAutomationPoints {
        protocol_version: u32,
        path: PathBuf,
        automation_id: AutomationId,
        points: Vec<CliBreakpoint>,
    },
    RemoveAutomationLane {
        protocol_version: u32,
        path: PathBuf,
        automation_id: AutomationId,
    },
    /// Runs a recipe into a project: a generated asset and the clip that plays
    /// it, with IDs derived from the recipe.
    RunGenerator {
        protocol_version: u32,
        path: PathBuf,
        track_id: TrackId,
        layer_id: LayerId,
        type_id: String,
        seed: u64,
        frame_count: u64,
        #[serde(default)]
        start_sample: u64,
        #[serde(default)]
        parameters: BTreeMap<String, ParameterValue>,
        /// `replace` reuses the IDs this recipe produced before, so every clip
        /// already using the asset follows the new version. `new` adds a
        /// variant beside it.
        #[serde(default = "replace_by_default")]
        mode: RegenerateMode,
        /// Distinguishes one variant from the next under `new`, and keeps that
        /// variant reproducible.
        #[serde(default)]
        variant: u64,
    },
}

const fn replace_by_default() -> RegenerateMode {
    RegenerateMode::Replace
}

/// Which chain an insert goes into, as a caller writes it.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliEffectTarget {
    Track { track_id: TrackId },
    Bus { bus_id: BusId },
}

impl From<CliEffectTarget> for EffectTarget {
    fn from(target: CliEffectTarget) -> Self {
        match target {
            CliEffectTarget::Track { track_id } => Self::Track { track_id },
            CliEffectTarget::Bus { bus_id } => Self::Bus { bus_id },
        }
    }
}

/// One sampler zone as a caller writes it. Ranges default to "everything", so
/// a single-sample instrument needs only an asset and its root pitch.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct CliZone {
    pub asset_id: AssetId,
    pub root_pitch_hz: f64,
    #[serde(default = "lowest_pitch")]
    pub low_pitch_hz: f64,
    #[serde(default = "highest_pitch")]
    pub high_pitch_hz: f64,
    #[serde(default)]
    pub low_velocity: f32,
    #[serde(default = "full_velocity")]
    pub high_velocity: f32,
    #[serde(default)]
    pub gain_db: f64,
    /// Loop points in source frames. Absent means the zone plays once.
    #[serde(default)]
    pub loop_start_frame: Option<u64>,
    #[serde(default)]
    pub loop_end_frame: Option<u64>,
}

const fn lowest_pitch() -> f64 {
    8.0
}

const fn highest_pitch() -> f64 {
    20_000.0
}

const fn default_release_ms() -> f64 {
    80.0
}

const fn default_max_voices() -> u32 {
    16
}

impl From<CliZone> for SamplerZone {
    fn from(zone: CliZone) -> Self {
        Self {
            asset_id: zone.asset_id,
            root_pitch_hz: zone.root_pitch_hz,
            low_pitch_hz: zone.low_pitch_hz,
            high_pitch_hz: zone.high_pitch_hz,
            low_velocity: zone.low_velocity,
            high_velocity: zone.high_velocity,
            gain_db: zone.gain_db,
            loop_mode: match (zone.loop_start_frame, zone.loop_end_frame) {
                (Some(start_frame), Some(end_frame)) => SampleLoopMode::Loop {
                    start_frame,
                    end_frame,
                },
                _ => SampleLoopMode::OneShot,
            },
        }
    }
}

/// What a preset is saved from or applied to.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliPresetTarget {
    /// A synth, generator or sampler asset.
    Asset { asset_id: AssetId },
    /// A track's effect chain.
    TrackChain { track_id: TrackId },
    /// A bus's effect chain.
    BusChain { bus_id: BusId },
}

/// The kind of a preset, as a caller names it.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliPresetKind {
    Synth,
    Effect,
    Chain,
    Generator,
    Instrument,
}

impl From<CliPresetKind> for PresetKind {
    fn from(kind: CliPresetKind) -> Self {
        match kind {
            CliPresetKind::Synth => Self::Synth,
            CliPresetKind::Effect => Self::Effect,
            CliPresetKind::Chain => Self::Chain,
            CliPresetKind::Generator => Self::Generator,
            CliPresetKind::Instrument => Self::Instrument,
        }
    }
}

/// A tempo change as a caller writes it.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct CliTempoChange {
    pub frame: u64,
    pub beats_per_minute: f64,
    #[serde(default = "four")]
    pub beats_per_bar: u32,
    #[serde(default = "four")]
    pub beat_unit: u32,
}

const fn four() -> u32 {
    4
}

/// Sixteenths: the division a caller almost always means.
const fn four_divisions() -> u32 {
    4
}

impl From<CliTempoChange> for TempoChange {
    fn from(change: CliTempoChange) -> Self {
        Self {
            frame: change.frame,
            beats_per_minute: change.beats_per_minute,
            beats_per_bar: change.beats_per_bar,
            beat_unit: change.beat_unit,
        }
    }
}

/// A musical position as a caller writes it: bars and beats from one, ticks
/// from zero.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct CliPosition {
    pub bar: u64,
    #[serde(default = "one")]
    pub beat: u64,
    #[serde(default)]
    pub tick: u32,
}

const fn one() -> u64 {
    1
}

/// A breakpoint as a caller writes it.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct CliBreakpoint {
    pub frame: u64,
    pub value: f64,
    #[serde(default)]
    pub curve: Curve,
}

impl From<CliBreakpoint> for Breakpoint {
    fn from(point: CliBreakpoint) -> Self {
        Self {
            frame: point.frame,
            value: point.value,
            curve: point.curve,
        }
    }
}

/// A note as a caller writes it: frames from the clip's own start.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct CliNote {
    pub start_frame: u64,
    pub duration_frames: u64,
    pub pitch_hz: f64,
    #[serde(default = "full_velocity")]
    pub velocity: f32,
}

const fn full_velocity() -> f32 {
    1.0
}

impl From<CliNote> for ClipNote {
    fn from(note: CliNote) -> Self {
        Self {
            start_frame: note.start_frame,
            duration_frames: note.duration_frames,
            pitch_hz: note.pitch_hz,
            velocity: note.velocity,
        }
    }
}

const fn enabled_by_default() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TransportAction {
    Play,
    Pause,
    Stop,
    Seek,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CliExportEncoding {
    Pcm16,
    Float32,
}

const fn full_frame_count() -> u64 {
    u64::MAX
}

/// Generators render at this rate, so a preview WAV is written at it too.
const PREVIEW_SAMPLE_RATE: u32 = 48_000;

impl Request {
    const fn protocol_version(&self) -> u32 {
        match self {
            Self::CreateProject {
                protocol_version, ..
            }
            | Self::InspectProject {
                protocol_version, ..
            }
            | Self::ImportSample {
                protocol_version, ..
            }
            | Self::AddClip {
                protocol_version, ..
            }
            | Self::UpdateClip {
                protocol_version, ..
            }
            | Self::DeleteClip {
                protocol_version, ..
            }
            | Self::ExportWav {
                protocol_version, ..
            }
            | Self::Transport {
                protocol_version, ..
            }
            | Self::SessionStatus {
                protocol_version, ..
            }
            | Self::AddTrack {
                protocol_version, ..
            }
            | Self::AddLayer {
                protocol_version, ..
            }
            | Self::SetTrackMute {
                protocol_version, ..
            }
            | Self::SetTrackSolo {
                protocol_version, ..
            }
            | Self::SetClipPan {
                protocol_version, ..
            }
            | Self::SplitClip {
                protocol_version, ..
            }
            | Self::DuplicateClip {
                protocol_version, ..
            }
            | Self::SlipClip {
                protocol_version, ..
            }
            | Self::SetClipFades {
                protocol_version, ..
            }
            | Self::CrossfadeClips {
                protocol_version, ..
            }
            | Self::AddMarker {
                protocol_version, ..
            }
            | Self::MoveMarker {
                protocol_version, ..
            }
            | Self::RemoveMarker {
                protocol_version, ..
            }
            | Self::SetLoopRegion {
                protocol_version, ..
            }
            | Self::ClearLoopRegion {
                protocol_version, ..
            }
            | Self::AddSynthClip {
                protocol_version, ..
            }
            | Self::SetSynthParameters {
                protocol_version, ..
            }
            | Self::SetClipNotes {
                protocol_version, ..
            }
            | Self::DescribeGenerator {
                protocol_version, ..
            }
            | Self::PreviewGenerator {
                protocol_version, ..
            }
            | Self::RunGenerator {
                protocol_version, ..
            }
            | Self::AddBus {
                protocol_version, ..
            }
            | Self::SetTrackOutput {
                protocol_version, ..
            }
            | Self::SetBusOutput {
                protocol_version, ..
            }
            | Self::SetTrackParameter {
                protocol_version, ..
            }
            | Self::SetBusParameter {
                protocol_version, ..
            }
            | Self::DescribeEffect {
                protocol_version, ..
            }
            | Self::AddEffect {
                protocol_version, ..
            }
            | Self::RemoveEffect {
                protocol_version, ..
            }
            | Self::MoveEffect {
                protocol_version, ..
            }
            | Self::SetEffectEnabled {
                protocol_version, ..
            }
            | Self::SetEffectWet {
                protocol_version, ..
            }
            | Self::SetEffectParameters {
                protocol_version, ..
            }
            | Self::AddAutomationLane {
                protocol_version, ..
            }
            | Self::SetAutomationPoints {
                protocol_version, ..
            }
            | Self::RemoveAutomationLane {
                protocol_version, ..
            } => *protocol_version,
            Self::SetTempoMap {
                protocol_version, ..
            }
            | Self::ConvertTime {
                protocol_version, ..
            } => *protocol_version,
            Self::AddPattern {
                protocol_version, ..
            }
            | Self::SetPatternNotes {
                protocol_version, ..
            }
            | Self::RemovePattern {
                protocol_version, ..
            }
            | Self::SetClipPattern {
                protocol_version, ..
            }
            | Self::QuantiseClip {
                protocol_version, ..
            }
            | Self::TransposeClip {
                protocol_version, ..
            }
            | Self::HumaniseClip {
                protocol_version, ..
            }
            | Self::LoopClipNotes {
                protocol_version, ..
            } => *protocol_version,
            Self::AddSampler {
                protocol_version, ..
            }
            | Self::SetSamplerZones {
                protocol_version, ..
            } => *protocol_version,
            Self::ListPresets {
                protocol_version, ..
            }
            | Self::SavePreset {
                protocol_version, ..
            }
            | Self::ApplyPreset {
                protocol_version, ..
            }
            | Self::ImportPreset {
                protocol_version, ..
            }
            | Self::ExportPreset {
                protocol_version, ..
            } => *protocol_version,
            Self::DescribeStrip { protocol_version } => *protocol_version,
            Self::ListExtensions { protocol_version } => *protocol_version,
        }
    }
}

pub fn execute_json(input: &str) -> (i32, Value) {
    let request: Request = match serde_json::from_str(input) {
        Ok(request) => request,
        Err(error) => return (2, error_response("invalid_request", error.to_string())),
    };
    if request.protocol_version() != CLI_PROTOCOL_VERSION {
        return (
            2,
            error_response(
                "invalid_request",
                format!(
                    "unsupported CLI protocol version {}; expected {CLI_PROTOCOL_VERSION}",
                    request.protocol_version()
                ),
            ),
        );
    }
    match execute(request) {
        Ok(result) => (
            0,
            json!({"ok": true, "protocol_version": CLI_PROTOCOL_VERSION, "result": result}),
        ),
        Err((exit_code, code, message)) => (exit_code, error_response(code, message)),
    }
}

#[must_use]
pub fn error_response(code: &str, message: impl Into<String>) -> Value {
    json!({"ok": false, "protocol_version": CLI_PROTOCOL_VERSION, "error": {"code": code, "message": message.into()}})
}

fn execute(request: Request) -> Result<Value, (i32, &'static str, String)> {
    match request {
        Request::CreateProject { path, name, .. } => {
            let project = ProjectStore::create(&path, name).map_err(project_error)?;
            Ok(
                json!({"type": "project_created", "path": path, "project_id": project.id, "track_id": project.tracks[0].id, "layer_id": project.tracks[0].layers[0].id}),
            )
        }
        Request::InspectProject { path, .. } => {
            let opened = ProjectStore::open(&path).map_err(project_error)?;
            Ok(
                json!({"type": "project_inspected", "path": path, "project": opened.project, "migrated_from": opened.migrated_from}),
            )
        }
        Request::ImportSample { path, source, .. } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let prepared = AssetManager::prepare_wav_import(
                &project,
                &path,
                source,
                ImportMode::CopyIntoProject,
            )
            .map_err(project_error)?;
            match prepared.status {
                ImportStatus::Duplicate(asset_id) => Ok(
                    json!({"type": "sample_imported", "status": "duplicate", "asset_id": asset_id, "metadata": prepared.metadata}),
                ),
                ImportStatus::Prepared => {
                    let asset = prepared.asset.expect("prepared import contains asset");
                    let asset_id = asset.id;
                    let applied =
                        cli_session::apply(&path, vec![ProjectCommand::AddAsset { asset }])?;
                    Ok(
                        json!({"type": "sample_imported", "status": "added", "asset_id": asset_id, "metadata": prepared.metadata, "revision": applied.revision, "delivery": applied.delivery}),
                    )
                }
            }
        }
        Request::AddClip {
            path,
            asset_id,
            track_id,
            layer_id,
            start_sample,
            source_start_sample,
            duration_samples,
            gain_db,
            ..
        } => {
            let clip = Clip {
                id: ClipId::new(),
                asset_id,
                start_sample,
                source_start_sample,
                duration_samples,
                notes: Vec::new(),
                pattern_id: None,
                parameters: [("gain_db".into(), ParameterValue::Float(gain_db))]
                    .into_iter()
                    .collect(),
            };
            let clip_id = clip.id;
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::AddClip {
                    track_id,
                    layer_id,
                    clip,
                }],
            )?;
            Ok(clip_result("clip_added", clip_id, &applied))
        }
        Request::UpdateClip {
            path,
            clip_id,
            start_sample,
            source_start_sample,
            duration_samples,
            gain_db,
            ..
        } => {
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::UpdateClip {
                    clip_id,
                    start_sample,
                    source_start_sample,
                    duration_samples,
                    gain_db,
                }],
            )?;
            Ok(clip_result("clip_updated", clip_id, &applied))
        }
        Request::DeleteClip {
            path,
            clip_id,
            ripple,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let mode = if ripple {
                DeleteMode::Ripple
            } else {
                DeleteMode::Leave
            };
            let commands = edits::delete(&project, &[clip_id], mode).map_err(command_failed)?;
            let applied = cli_session::apply(&path, commands)?;
            Ok(
                json!({"type": "clip_deleted", "clip_id": clip_id, "ripple": ripple, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::ExportWav {
            path,
            output,
            encoding,
            start_frame,
            frame_count,
            use_loop_region,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let (start_frame, frame_count) = if use_loop_region {
                let region = project.loop_region.filter(LoopRegion::is_active).ok_or((
                    3,
                    "export_failed",
                    "this project has no active loop region to export".to_owned(),
                ))?;
                (region.start_frame, region.frame_count())
            } else {
                (start_frame, frame_count)
            };
            let snapshot = build_master_snapshot(&project, &path)?;
            let encoding = match encoding {
                CliExportEncoding::Pcm16 => ExportEncoding::Pcm16,
                CliExportEncoding::Float32 => ExportEncoding::Float32,
            };
            let report = OfflineExporter::export_wav(
                snapshot,
                &output,
                ExportRange {
                    start_frame,
                    frame_count,
                },
                encoding,
            )
            .map_err(|error| (3, "export_failed", error.message))?;
            Ok(
                json!({"type": "wav_exported", "output": output, "sample_rate": report.sample_rate, "channel_count": report.channel_count, "frame_count": report.frame_count}),
            )
        }
        Request::Transport {
            path,
            action,
            position_frames,
            ..
        } => {
            let delivery = cli_session::transport(&path, action.into(), position_frames)?;
            Ok(
                json!({"type": "transport_requested", "action": format!("{action:?}").to_lowercase(), "position_frames": position_frames, "delivery": delivery}),
            )
        }
        Request::AddTrack { path, name, .. } => {
            let track = Track {
                id: TrackId::new(),
                name,
                // Read from the file rather than the session: the master bus is
                // fixed when a project is created and no command moves it.
                output_bus_id: ProjectStore::open(&path)
                    .map_err(project_error)?
                    .project
                    .master_bus_id,
                parameters: BTreeMap::new(),
                layers: vec![Layer {
                    id: LayerId::new(),
                    name: "Layer 1".into(),
                    clips: Vec::new(),
                }],
                effects: Vec::new(),
            };
            let track_id = track.id;
            let layer_id = track.layers[0].id;
            let applied = cli_session::apply(&path, vec![ProjectCommand::AddTrack { track }])?;
            Ok(
                json!({"type": "track_added", "track_id": track_id, "layer_id": layer_id, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::AddLayer {
            path,
            track_id,
            name,
            ..
        } => {
            let layer = Layer {
                id: LayerId::new(),
                name,
                clips: Vec::new(),
            };
            let layer_id = layer.id;
            let applied =
                cli_session::apply(&path, vec![ProjectCommand::AddLayer { track_id, layer }])?;
            Ok(
                json!({"type": "layer_added", "track_id": track_id, "layer_id": layer_id, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SetTrackMute {
            path,
            track_id,
            muted,
            ..
        } => {
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::SetTrackMute { track_id, muted }],
            )?;
            Ok(
                json!({"type": "track_mute_set", "track_id": track_id, "muted": muted, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SetTrackSolo {
            path,
            track_id,
            soloed,
            ..
        } => {
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::SetTrackSolo { track_id, soloed }],
            )?;
            Ok(
                json!({"type": "track_solo_set", "track_id": track_id, "soloed": soloed, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SetClipPan {
            path, clip_id, pan, ..
        } => {
            let applied =
                cli_session::apply(&path, vec![ProjectCommand::SetClipPan { clip_id, pan }])?;
            Ok(
                json!({"type": "clip_pan_set", "clip_id": clip_id, "pan": pan, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SplitClip {
            path,
            clip_id,
            at_frame,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let commands = edits::split(&project, clip_id, at_frame).map_err(command_failed)?;
            let applied = cli_session::apply(&path, commands)?;
            Ok(
                json!({"type": "clip_split", "clip_id": clip_id, "at_frame": at_frame, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::DuplicateClip {
            path,
            clip_id,
            offset_frames,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let commands =
                edits::duplicate(&project, &[clip_id], offset_frames).map_err(command_failed)?;
            let copy_id = match commands.first() {
                Some(ProjectCommand::AddClip { clip, .. }) => clip.id,
                _ => return Err((4, "command_failed", "duplicate produced no clip".into())),
            };
            let applied = cli_session::apply(&path, commands)?;
            Ok(
                json!({"type": "clip_duplicated", "clip_id": clip_id, "copy_id": copy_id, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SlipClip {
            path,
            clip_id,
            delta_frames,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let commands =
                edits::slip(&project, &[clip_id], delta_frames).map_err(command_failed)?;
            let applied = cli_session::apply(&path, commands)?;
            Ok(
                json!({"type": "clip_slipped", "clip_id": clip_id, "delta_frames": delta_frames, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SetClipFades {
            path,
            clip_id,
            fade_in_samples,
            fade_out_samples,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let commands = edits::set_fades(&project, clip_id, fade_in_samples, fade_out_samples)
                .map_err(command_failed)?;
            let applied = cli_session::apply(&path, commands)?;
            Ok(
                json!({"type": "clip_fades_set", "clip_id": clip_id, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::CrossfadeClips {
            path,
            first_clip_id,
            second_clip_id,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let commands = edits::crossfade(&project, first_clip_id, second_clip_id)
                .map_err(command_failed)?;
            let applied = cli_session::apply(&path, commands)?;
            Ok(
                json!({"type": "clips_crossfaded", "first_clip_id": first_clip_id, "second_clip_id": second_clip_id, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::AddMarker {
            path, name, frame, ..
        } => {
            let marker = Marker {
                id: MarkerId::new(),
                name,
                frame,
            };
            let marker_id = marker.id;
            let applied = cli_session::apply(&path, vec![ProjectCommand::AddMarker { marker }])?;
            Ok(
                json!({"type": "marker_added", "marker_id": marker_id, "frame": frame, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::MoveMarker {
            path,
            marker_id,
            frame,
            ..
        } => {
            let applied =
                cli_session::apply(&path, vec![ProjectCommand::MoveMarker { marker_id, frame }])?;
            Ok(
                json!({"type": "marker_moved", "marker_id": marker_id, "frame": frame, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::RemoveMarker {
            path, marker_id, ..
        } => {
            let applied =
                cli_session::apply(&path, vec![ProjectCommand::RemoveMarker { marker_id }])?;
            Ok(
                json!({"type": "marker_removed", "marker_id": marker_id, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SetLoopRegion {
            path,
            start_frame,
            end_frame,
            enabled,
            ..
        } => {
            let region = LoopRegion {
                start_frame,
                end_frame,
                enabled,
            };
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::SetLoopRegion {
                    region: Some(region),
                }],
            )?;
            Ok(
                json!({"type": "loop_region_set", "start_frame": start_frame, "end_frame": end_frame, "enabled": enabled, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::ClearLoopRegion { path, .. } => {
            let applied =
                cli_session::apply(&path, vec![ProjectCommand::SetLoopRegion { region: None }])?;
            Ok(
                json!({"type": "loop_region_cleared", "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::ListExtensions { .. } => Ok(json!({
            "type": "extensions_listed",
            "extensions": cli_synth::describe_all(crate::extensions::registries()),
        })),
        Request::AddSynthClip {
            path,
            track_id,
            layer_id,
            type_id,
            start_sample,
            duration_samples,
            name,
            parameters,
            notes,
            ..
        } => {
            let state_version =
                cli_synth::validate_synth(crate::extensions::registries(), &type_id, &parameters)?;
            let asset = jutsu_audio_model::Asset {
                id: AssetId::new(),
                name: name.unwrap_or_else(|| type_id.clone()),
                source: AudioAssetSource::Synth {
                    type_id,
                    state_version,
                    parameters,
                },
            };
            let asset_id = asset.id;
            let clip = Clip {
                id: ClipId::new(),
                asset_id,
                start_sample,
                source_start_sample: 0,
                duration_samples,
                parameters: BTreeMap::new(),
                notes: notes.into_iter().map(ClipNote::from).collect(),
                pattern_id: None,
            };
            let clip_id = clip.id;
            // One batch: the asset and the clip that needs it arrive together.
            let applied = cli_session::apply(
                &path,
                vec![
                    ProjectCommand::AddAsset { asset },
                    ProjectCommand::AddClip {
                        track_id,
                        layer_id,
                        clip,
                    },
                ],
            )?;
            Ok(
                json!({"type": "synth_clip_added", "asset_id": asset_id, "clip_id": clip_id, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SetSynthParameters {
            path,
            asset_id,
            parameters,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let asset = project
                .assets
                .iter()
                .find(|asset| asset.id == asset_id)
                .ok_or_else(|| {
                    (
                        4,
                        "command_failed",
                        format!("asset {asset_id} does not exist"),
                    )
                })?;
            let AudioAssetSource::Synth { type_id, .. } = &asset.source else {
                return Err((
                    4,
                    "command_failed",
                    format!("asset {asset_id} is not a synth"),
                ));
            };
            cli_synth::validate_synth(crate::extensions::registries(), type_id, &parameters)?;
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::SetAssetParameters {
                    asset_id,
                    parameters,
                }],
            )?;
            Ok(
                json!({"type": "synth_parameters_set", "asset_id": asset_id, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SetClipNotes {
            path,
            clip_id,
            notes,
            ..
        } => {
            let count = notes.len();
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::SetClipNotes {
                    clip_id,
                    notes: notes.into_iter().map(ClipNote::from).collect(),
                }],
            )?;
            Ok(
                json!({"type": "clip_notes_set", "clip_id": clip_id, "note_count": count, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::DescribeGenerator { type_id, .. } => {
            let registries = crate::extensions::registries();
            let parsed = ExtensionTypeId::new(type_id.clone())
                .map_err(|error| (6, "invalid_parameter", error.message))?;
            let descriptor = registries.generator_descriptor(&parsed).ok_or_else(|| {
                let available: Vec<&str> = registries
                    .generator_type_ids()
                    .map(ExtensionTypeId::as_str)
                    .collect();
                (
                    6,
                    "unknown_extension",
                    format!(
                        "no generator '{type_id}' is registered; this build has {}",
                        available.join(", ")
                    ),
                )
            })?;
            let presets = registries.generator_presets(&parsed).unwrap_or_default();
            Ok(json!({
                "type": "generator_described",
                "generator": cli_generator::describe(descriptor, presets),
            }))
        }
        Request::PreviewGenerator {
            type_id,
            seed,
            frame_count,
            parameters,
            output,
            ..
        } => {
            let recipe = cli_generator::recipe(type_id, 1, seed, frame_count, parameters);
            let samples = cli_generator::render(crate::extensions::registries(), &recipe)?;
            let mut result = cli_generator::summarise(&samples);
            result["type"] = json!("generator_previewed");
            result["seed"] = json!(seed);
            if let Some(output) = output {
                let snapshot =
                    PlaybackSnapshot::new(PREVIEW_SAMPLE_RATE, 1, std::sync::Arc::from(samples))
                        .map_err(|error| (3, "export_failed", error.message))?;
                OfflineExporter::export_wav(
                    std::sync::Arc::new(snapshot),
                    &output,
                    ExportRange::full(),
                    ExportEncoding::Float32,
                )
                .map_err(|error| (3, "export_failed", error.message))?;
                result["output"] = json!(output);
            }
            Ok(result)
        }
        Request::RunGenerator {
            path,
            track_id,
            layer_id,
            type_id,
            seed,
            frame_count,
            start_sample,
            parameters,
            mode,
            variant,
            ..
        } => {
            let recipe = cli_generator::recipe(type_id, 1, seed, frame_count, parameters);
            cli_generator::validate(crate::extensions::registries(), &recipe)?;
            let (asset_id, clip_id) = cli_generator::identity(&recipe, mode, variant);

            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let existing = project.assets.iter().any(|asset| asset.id == asset_id);
            let asset = jutsu_audio_model::Asset {
                id: asset_id,
                name: recipe.generator_type.clone(),
                source: recipe.asset_source(),
            };
            let clip = Clip {
                id: clip_id,
                asset_id,
                start_sample,
                source_start_sample: 0,
                duration_samples: frame_count,
                parameters: BTreeMap::new(),
                notes: Vec::new(),
                pattern_id: None,
            };

            // Replacing means removing what this recipe produced before, in the
            // same batch, so the project is never briefly without it.
            let mut commands = Vec::new();
            if existing {
                commands.push(ProjectCommand::RemoveClip { clip_id });
                commands.push(ProjectCommand::RemoveAsset { asset_id });
            }
            commands.push(ProjectCommand::AddAsset { asset });
            commands.push(ProjectCommand::AddClip {
                track_id,
                layer_id,
                clip,
            });
            let applied = cli_session::apply(&path, commands)?;
            Ok(json!({
                "type": "generator_ran",
                "asset_id": asset_id,
                "clip_id": clip_id,
                "seed": seed,
                "mode": mode,
                "replaced": existing,
                "revision": applied.revision,
                "delivery": applied.delivery,
            }))
        }
        Request::AddBus {
            path,
            name,
            output_bus_id,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let bus = jutsu_audio_model::MixerBus {
                id: BusId::new(),
                name,
                // Defaulting to the master is what a new bus almost always
                // wants, and it keeps a project renderable straight away.
                output_bus_id: Some(output_bus_id.unwrap_or(project.master_bus_id)),
                parameters: BTreeMap::new(),
                effects: Vec::new(),
            };
            let bus_id = bus.id;
            let applied = cli_session::apply(&path, vec![ProjectCommand::AddBus { bus }])?;
            Ok(
                json!({"type": "bus_added", "bus_id": bus_id, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SetTrackOutput {
            path,
            track_id,
            output_bus_id,
            ..
        } => {
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::SetTrackOutput {
                    track_id,
                    output_bus_id,
                }],
            )?;
            Ok(
                json!({"type": "track_output_set", "track_id": track_id, "output_bus_id": output_bus_id, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SetBusOutput {
            path,
            bus_id,
            output_bus_id,
            ..
        } => {
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::SetBusOutput {
                    bus_id,
                    output_bus_id,
                }],
            )?;
            Ok(
                json!({"type": "bus_output_set", "bus_id": bus_id, "output_bus_id": output_bus_id, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SetTrackParameter {
            path,
            track_id,
            key,
            value,
            ..
        } => {
            cli_mixer::validate_strip(&key, &value)?;
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::SetTrackParameter {
                    track_id,
                    key: key.clone(),
                    value: value.clone(),
                }],
            )?;
            Ok(
                json!({"type": "track_parameter_set", "track_id": track_id, "key": key, "value": value, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SetBusParameter {
            path,
            bus_id,
            key,
            value,
            ..
        } => {
            cli_mixer::validate_strip(&key, &value)?;
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::SetBusParameter {
                    bus_id,
                    key: key.clone(),
                    value: value.clone(),
                }],
            )?;
            Ok(
                json!({"type": "bus_parameter_set", "bus_id": bus_id, "key": key, "value": value, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::DescribeStrip { .. } => Ok(json!({
            "type": "strip_described",
            "strip": cli_mixer::describe_strip(),
        })),
        Request::DescribeEffect { type_id, .. } => {
            let registries = crate::extensions::registries();
            let parsed = ExtensionTypeId::new(type_id.clone())
                .map_err(|error| (6, "invalid_parameter", error.message))?;
            let described = cli_mixer::describe_effect(registries, &parsed).ok_or_else(|| {
                let available: Vec<&str> = registries
                    .effect_type_ids()
                    .map(ExtensionTypeId::as_str)
                    .collect();
                (
                    6,
                    "unknown_extension",
                    format!(
                        "no effect '{type_id}' is registered; this build has {}",
                        available.join(", ")
                    ),
                )
            })?;
            Ok(json!({"type": "effect_described", "effect": described}))
        }
        Request::AddEffect {
            path,
            target,
            type_id,
            parameters,
            ..
        } => {
            let state_version =
                cli_mixer::validate_effect(crate::extensions::registries(), &type_id, &parameters)?;
            let effect = jutsu_audio_model::EffectInsert {
                id: EffectId::new(),
                type_id,
                state_version,
                parameters,
                enabled: true,
                wet: 1.0,
            };
            let effect_id = effect.id;
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::AddEffect {
                    target: target.into(),
                    effect,
                }],
            )?;
            Ok(
                json!({"type": "effect_added", "effect_id": effect_id, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::RemoveEffect {
            path, effect_id, ..
        } => {
            let applied =
                cli_session::apply(&path, vec![ProjectCommand::RemoveEffect { effect_id }])?;
            Ok(
                json!({"type": "effect_removed", "effect_id": effect_id, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::MoveEffect {
            path,
            effect_id,
            to_index,
            ..
        } => {
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::MoveEffect {
                    effect_id,
                    to_index,
                }],
            )?;
            Ok(
                json!({"type": "effect_moved", "effect_id": effect_id, "to_index": to_index, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SetEffectEnabled {
            path,
            effect_id,
            enabled,
            ..
        } => {
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::SetEffectEnabled { effect_id, enabled }],
            )?;
            Ok(
                json!({"type": "effect_enabled_set", "effect_id": effect_id, "enabled": enabled, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SetEffectWet {
            path,
            effect_id,
            wet,
            ..
        } => {
            let applied =
                cli_session::apply(&path, vec![ProjectCommand::SetEffectWet { effect_id, wet }])?;
            Ok(
                json!({"type": "effect_wet_set", "effect_id": effect_id, "wet": wet, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SetEffectParameters {
            path,
            effect_id,
            parameters,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let insert = project
                .tracks
                .iter()
                .flat_map(|track| &track.effects)
                .chain(project.buses.iter().flat_map(|bus| &bus.effects))
                .find(|effect| effect.id == effect_id)
                .ok_or_else(|| {
                    (
                        4,
                        "command_failed",
                        format!("effect {effect_id} does not exist"),
                    )
                })?;
            cli_mixer::validate_effect(
                crate::extensions::registries(),
                &insert.type_id,
                &parameters,
            )?;
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::SetEffectParameters {
                    effect_id,
                    parameters,
                }],
            )?;
            Ok(
                json!({"type": "effect_parameters_set", "effect_id": effect_id, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::AddAutomationLane {
            path,
            target,
            parameter,
            points,
            ..
        } => {
            // Only the name is checked here: a lane's values are bounded by the
            // same descriptor when they are rendered, and a caller drawing a
            // curve should not be stopped mid-draw by one out-of-range point.
            cli_mixer::validate_strip(&parameter, &ParameterValue::Float(0.0))?;
            let mut lane = jutsu_audio_model::AutomationLane {
                id: AutomationId::new(),
                target,
                parameter: parameter.clone(),
                points: points.into_iter().map(Breakpoint::from).collect(),
            };
            lane.points.sort_by_key(|point| point.frame);
            let automation_id = lane.id;
            let applied =
                cli_session::apply(&path, vec![ProjectCommand::AddAutomationLane { lane }])?;
            Ok(
                json!({"type": "automation_lane_added", "automation_id": automation_id, "parameter": parameter, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SetAutomationPoints {
            path,
            automation_id,
            points,
            ..
        } => {
            let count = points.len();
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::SetAutomationPoints {
                    automation_id,
                    points: points.into_iter().map(Breakpoint::from).collect(),
                }],
            )?;
            Ok(
                json!({"type": "automation_points_set", "automation_id": automation_id, "point_count": count, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::RemoveAutomationLane {
            path,
            automation_id,
            ..
        } => {
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::RemoveAutomationLane { automation_id }],
            )?;
            Ok(
                json!({"type": "automation_lane_removed", "automation_id": automation_id, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SetTempoMap { path, changes, .. } => {
            let changes: Vec<TempoChange> = changes.into_iter().map(TempoChange::from).collect();
            if let Some(bad) = changes
                .iter()
                .find(|change| change.beats_per_minute <= 0.0 || change.beats_per_bar == 0)
            {
                return Err((
                    6,
                    "invalid_parameter",
                    format!(
                        "a tempo change needs a positive tempo and at least one beat per bar; got {} BPM in {}/{}",
                        bad.beats_per_minute, bad.beats_per_bar, bad.beat_unit
                    ),
                ));
            }
            let count = changes.len();
            let applied = cli_session::apply(&path, vec![ProjectCommand::SetTempoMap { changes }])?;
            Ok(
                json!({"type": "tempo_map_set", "change_count": count, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::ConvertTime {
            path,
            frame,
            position,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let rate = project_sample_rate(&project);
            let map = project.tempo_map();

            let frame = match (frame, position) {
                (Some(frame), _) => frame,
                (None, Some(position)) => map.frame_at_position(
                    MusicalPosition {
                        bar: position.bar,
                        beat: position.beat,
                        tick: position.tick,
                    },
                    rate,
                ),
                (None, None) => {
                    return Err((
                        2,
                        "invalid_request",
                        "convert_time needs either a frame or a position".into(),
                    ));
                }
            };
            let musical = map.position_at(frame, rate);
            let tempo = map.at(frame);
            Ok(json!({
                "type": "time_converted",
                "frame": frame,
                "seconds": frame as f64 / f64::from(rate.max(1)),
                "beats": map.beats_at(frame, rate),
                "position": {"bar": musical.bar, "beat": musical.beat, "tick": musical.tick},
                "formatted": musical.format(),
                "beats_per_minute": tempo.beats_per_minute,
                "time_signature": format!("{}/{}", tempo.beats_per_bar, tempo.beat_unit),
                "sample_rate": rate,
            }))
        }
        Request::AddPattern {
            path,
            name,
            length_frames,
            notes,
            ..
        } => {
            let mut pattern = jutsu_audio_model::Pattern {
                id: PatternId::new(),
                name,
                length_frames,
                notes: notes.into_iter().map(ClipNote::from).collect(),
            };
            pattern.notes.sort_by_key(|note| note.start_frame);
            let pattern_id = pattern.id;
            let applied = cli_session::apply(&path, vec![ProjectCommand::AddPattern { pattern }])?;
            Ok(
                json!({"type": "pattern_added", "pattern_id": pattern_id, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SetPatternNotes {
            path,
            pattern_id,
            length_frames,
            notes,
            ..
        } => {
            let count = notes.len();
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::SetPatternNotes {
                    pattern_id,
                    length_frames,
                    notes: notes.into_iter().map(ClipNote::from).collect(),
                }],
            )?;
            Ok(
                json!({"type": "pattern_notes_set", "pattern_id": pattern_id, "note_count": count, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::RemovePattern {
            path, pattern_id, ..
        } => {
            let applied =
                cli_session::apply(&path, vec![ProjectCommand::RemovePattern { pattern_id }])?;
            Ok(
                json!({"type": "pattern_removed", "pattern_id": pattern_id, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SetClipPattern {
            path,
            clip_id,
            pattern_id,
            ..
        } => {
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::SetClipPattern {
                    clip_id,
                    pattern_id,
                }],
            )?;
            Ok(
                json!({"type": "clip_pattern_set", "clip_id": clip_id, "pattern_id": pattern_id, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::QuantiseClip {
            path,
            clip_id,
            divisions_per_beat,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let rate = project_sample_rate(&project);
            let commands = notes::quantise(&project, clip_id, divisions_per_beat, rate)
                .map_err(command_failed)?;
            let applied = cli_session::apply(&path, commands)?;
            Ok(
                json!({"type": "clip_quantised", "clip_id": clip_id, "divisions_per_beat": divisions_per_beat, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::TransposeClip {
            path,
            clip_id,
            semitones,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let commands =
                notes::transpose(&project, clip_id, semitones).map_err(command_failed)?;
            let applied = cli_session::apply(&path, commands)?;
            Ok(
                json!({"type": "clip_transposed", "clip_id": clip_id, "semitones": semitones, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::HumaniseClip {
            path,
            clip_id,
            seed,
            timing_frames,
            velocity_amount,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let commands = notes::humanise(&project, clip_id, seed, timing_frames, velocity_amount)
                .map_err(command_failed)?;
            let applied = cli_session::apply(&path, commands)?;
            Ok(
                json!({"type": "clip_humanised", "clip_id": clip_id, "seed": seed, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::LoopClipNotes {
            path,
            clip_id,
            period_frames,
            repeats,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let commands = notes::loop_notes(&project, clip_id, period_frames, repeats)
                .map_err(command_failed)?;
            let applied = cli_session::apply(&path, commands)?;
            Ok(
                json!({"type": "clip_notes_looped", "clip_id": clip_id, "repeats": repeats, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::AddSampler {
            path,
            name,
            zones,
            attack_ms,
            release_ms,
            max_voices,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let zones: Vec<SamplerZone> = zones.into_iter().map(SamplerZone::from).collect();
            check_zones(&project, &zones)?;
            let asset = jutsu_audio_model::Asset {
                id: AssetId::new(),
                name,
                source: AudioAssetSource::Sampler {
                    zones,
                    attack_ms,
                    release_ms,
                    max_voices,
                },
            };
            let asset_id = asset.id;
            let applied = cli_session::apply(&path, vec![ProjectCommand::AddAsset { asset }])?;
            Ok(
                json!({"type": "sampler_added", "asset_id": asset_id, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::SetSamplerZones {
            path,
            asset_id,
            zones,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let zones: Vec<SamplerZone> = zones.into_iter().map(SamplerZone::from).collect();
            check_zones(&project, &zones)?;
            let count = zones.len();
            let applied = cli_session::apply(
                &path,
                vec![ProjectCommand::SetSamplerZones { asset_id, zones }],
            )?;
            Ok(
                json!({"type": "sampler_zones_set", "asset_id": asset_id, "zone_count": count, "revision": applied.revision, "delivery": applied.delivery}),
            )
        }
        Request::ListPresets { path, library, .. } => {
            let registries = crate::extensions::registries();
            let library = cli_presets::library(&path, library.as_deref());
            let user: Vec<Value> = library
                .list()
                .iter()
                .map(|preset| {
                    cli_presets::describe(
                        preset,
                        &cli_presets::incompatibilities(preset, registries),
                    )
                })
                .collect();
            Ok(json!({
                "type": "presets_listed",
                "library": library.root(),
                "builtin": cli_presets::builtin(registries),
                "user": user,
            }))
        }
        Request::SavePreset {
            path,
            name,
            tags,
            target,
            library,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let (kind, capture) = preset_target(&project, target)?;
            let mut preset = cli_presets::capture(&project, kind, &name, &name, &capture)?;
            preset.tags = tags;
            preset.tags.sort();
            let library = cli_presets::library(&path, library.as_deref());
            let file = library.save(&preset).map_err(project_error)?;
            Ok(json!({
                "type": "preset_saved",
                "preset_id": preset.id,
                "kind": kind_name(kind),
                "file": file,
            }))
        }
        Request::ApplyPreset {
            path,
            preset_id,
            target,
            library,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
            let (kind, _) = preset_target(&project, target)?;
            let library = cli_presets::library(&path, library.as_deref());
            let preset = library.load(kind, &preset_id).map_err(project_error)?;
            let registries = crate::extensions::registries();
            let problems = cli_presets::incompatibilities(&preset, registries);
            if problems
                .iter()
                .any(|problem| problem.code == IncompatibilityCode::NewerSchema)
            {
                return Err((
                    6,
                    "incompatible_preset",
                    problems
                        .iter()
                        .map(|problem| problem.message.clone())
                        .collect::<Vec<_>>()
                        .join("; "),
                ));
            }

            let commands = apply_preset_commands(&project, &preset, target)?;
            let applied = cli_session::apply(&path, commands)?;
            Ok(json!({
                "type": "preset_applied",
                "preset_id": preset.id,
                "revision": applied.revision,
                "delivery": applied.delivery,
                // Applied anyway, and reported: a moved state version usually
                // still means what it said.
                "incompatibilities": problems
                    .iter()
                    .map(|problem| problem.message.clone())
                    .collect::<Vec<_>>(),
            }))
        }
        Request::ImportPreset {
            path,
            from,
            library,
            ..
        } => {
            let library = cli_presets::library(&path, library.as_deref());
            let preset = library.import(&from).map_err(project_error)?;
            Ok(
                json!({"type": "preset_imported", "preset_id": preset.id, "kind": kind_name(preset.kind)}),
            )
        }
        Request::ExportPreset {
            path,
            preset_id,
            kind,
            to,
            library,
            ..
        } => {
            let library = cli_presets::library(&path, library.as_deref());
            let preset = library
                .load(PresetKind::from(kind), &preset_id)
                .map_err(project_error)?;
            library.export(&preset, &to).map_err(project_error)?;
            Ok(json!({"type": "preset_exported", "preset_id": preset.id, "file": to}))
        }
        Request::SessionStatus { path, .. } => {
            let live = cli_session::status(&path)?;
            Ok(match live {
                Some(payload) => {
                    json!({"type": "session_status", "attached": true, "session": payload})
                }
                None => {
                    json!({"type": "session_status", "attached": false, "session": Value::Null})
                }
            })
        }
    }
}

/// Every clip operation answers in the same shape, whichever route it took.
fn clip_result(kind: &str, clip_id: ClipId, applied: &Applied) -> Value {
    json!({"type": kind, "clip_id": clip_id, "revision": applied.revision, "delivery": applied.delivery})
}

impl From<TransportAction> for SessionTransportAction {
    fn from(action: TransportAction) -> Self {
        match action {
            TransportAction::Play => Self::Play,
            TransportAction::Pause => Self::Pause,
            TransportAction::Stop => Self::Stop,
            TransportAction::Seek => Self::Seek,
        }
    }
}

/// Builds the master mix the same way playback does — through
/// `jutsu-audio-engine` — so an exported WAV is what the editor plays.
///
/// The project's rate is taken from its first decodable source, because the
/// schema has no rate field yet; a clip at another rate is resampled onto it.
fn build_master_snapshot(
    project: &Project,
    project_path: &Path,
) -> Result<std::sync::Arc<PlaybackSnapshot>, (i32, &'static str, String)> {
    let project_directory = project_path.parent().unwrap_or_else(|| Path::new("."));
    let mut decoded: HashMap<AssetId, SourceAudio> = HashMap::new();
    for asset in &project.assets {
        let path = match &asset.source {
            AudioAssetSource::ManagedFile { path, .. } | AudioAssetSource::File { path } => path,
            // Rendered rather than read: a generator runs, a synth is played,
            // and a sampler plays the project's other assets.
            AudioAssetSource::Generated { .. }
            | AudioAssetSource::Synth { .. }
            | AudioAssetSource::Sampler { .. } => continue,
        };
        let Ok((metadata, samples)) =
            AssetManager::decode_wav_samples(project_directory.join(path))
        else {
            // Left out of the map: a clip that needs it fails the mix with a
            // structured error naming the clip, which is more use than naming
            // the file here.
            continue;
        };
        decoded.insert(
            asset.id,
            SourceAudio {
                sample_rate: metadata.sample_rate,
                channels: metadata.channels,
                samples: samples.into(),
            },
        );
    }

    let sample_rate = project
        .assets
        .iter()
        .find_map(|asset| decoded.get(&asset.id))
        .map_or(48_000, |source| source.sample_rate);

    let mixed = mix_project(
        project,
        sample_rate,
        crate::extensions::registries(),
        |asset_id| {
            decoded
                .get(&asset_id)
                .cloned()
                .ok_or_else(|| format!("asset {asset_id} has no readable WAV source"))
        },
    )
    .map_err(|error| (3, "export_failed", error.message))?;

    let snapshot = match mixed {
        Some(snapshot) => snapshot,
        // Exporting silence beats failing: the caller asked for a file.
        None => PlaybackSnapshot::new(sample_rate, MIX_CHANNELS, std::sync::Arc::from(Vec::new()))
            .map_err(|error| (3, "export_failed", error.message))?,
    };
    Ok(std::sync::Arc::new(snapshot))
}

fn project_error(error: jutsu_audio_project::ProjectFileError) -> (i32, &'static str, String) {
    (3, "project_io_failed", error.message)
}

/// An edit that cannot be built — a clip that has gone, a split outside its
/// clip — is a command failure, the same shape the engine reports.
fn command_failed(error: CommandError) -> (i32, &'static str, String) {
    (4, "command_failed", error.message)
}

/// The rate a project counts frames in: its first managed asset's, or 48 kHz
/// when it has none. The editor infers it the same way.
fn project_sample_rate(project: &Project) -> u32 {
    project
        .assets
        .iter()
        .find_map(|asset| match &asset.source {
            AudioAssetSource::ManagedFile { sample_rate, .. } => Some(*sample_rate),
            _ => None,
        })
        .unwrap_or(48_000)
}

/// Checks a sampler mapping before it is stored: every zone must name an asset
/// the project has, and cover a range that can match something.
fn check_zones(
    project: &Project,
    zones: &[SamplerZone],
) -> Result<(), (i32, &'static str, String)> {
    for zone in zones {
        if !project.assets.iter().any(|asset| asset.id == zone.asset_id) {
            return Err((
                4,
                "command_failed",
                format!(
                    "sampler zone names asset {} which does not exist",
                    zone.asset_id
                ),
            ));
        }
        if zone.high_pitch_hz < zone.low_pitch_hz || zone.high_velocity < zone.low_velocity {
            return Err((
                6,
                "invalid_parameter",
                format!(
                    "sampler zone for asset {} has a range that covers nothing",
                    zone.asset_id
                ),
            ));
        }
        if zone.root_pitch_hz <= 0.0 {
            return Err((
                6,
                "invalid_parameter",
                "a sampler zone needs a positive root pitch".to_owned(),
            ));
        }
    }
    Ok(())
}

/// The preset kind a target implies, and how to capture from it.
fn preset_target<'a>(
    project: &'a Project,
    target: CliPresetTarget,
) -> Result<(PresetKind, cli_presets::CaptureTarget<'a>), (i32, &'static str, String)> {
    match target {
        CliPresetTarget::Asset { asset_id } => {
            let asset = project
                .assets
                .iter()
                .find(|asset| asset.id == asset_id)
                .ok_or_else(|| {
                    (
                        4,
                        "command_failed",
                        format!("asset {asset_id} does not exist"),
                    )
                })?;
            let kind = match &asset.source {
                AudioAssetSource::Synth { .. } => PresetKind::Synth,
                AudioAssetSource::Generated { .. } => PresetKind::Generator,
                AudioAssetSource::Sampler { .. } => PresetKind::Instrument,
                _ => {
                    return Err((
                        6,
                        "invalid_parameter",
                        format!("asset {asset_id} is a file, and a file is not a preset"),
                    ));
                }
            };
            Ok((kind, cli_presets::CaptureTarget::Asset { asset_id }))
        }
        CliPresetTarget::TrackChain { track_id } => {
            let track = project
                .tracks
                .iter()
                .find(|track| track.id == track_id)
                .ok_or_else(|| {
                    (
                        4,
                        "command_failed",
                        format!("track {track_id} does not exist"),
                    )
                })?;
            Ok((
                PresetKind::Chain,
                cli_presets::CaptureTarget::Chain {
                    effects: &track.effects,
                },
            ))
        }
        CliPresetTarget::BusChain { bus_id } => {
            let bus = project
                .buses
                .iter()
                .find(|bus| bus.id == bus_id)
                .ok_or_else(|| (4, "command_failed", format!("bus {bus_id} does not exist")))?;
            Ok((
                PresetKind::Chain,
                cli_presets::CaptureTarget::Chain {
                    effects: &bus.effects,
                },
            ))
        }
    }
}

/// The commands that put a preset into a project.
fn apply_preset_commands(
    project: &Project,
    preset: &Preset,
    target: CliPresetTarget,
) -> Result<Vec<ProjectCommand>, (i32, &'static str, String)> {
    match (&preset.payload, target) {
        (PresetPayload::Parameters { parameters, .. }, CliPresetTarget::Asset { asset_id }) => {
            Ok(vec![ProjectCommand::SetAssetParameters {
                asset_id,
                parameters: parameters.clone(),
            }])
        }
        (PresetPayload::Instrument { zones, .. }, CliPresetTarget::Asset { asset_id }) => {
            Ok(vec![ProjectCommand::SetSamplerZones {
                asset_id,
                zones: zones.clone(),
            }])
        }
        (PresetPayload::Chain { steps }, target) => {
            // Replacing a chain: the old inserts go, the preset's arrive, all in
            // one batch so the strip is never briefly half-configured.
            let (existing, effect_target) = match target {
                CliPresetTarget::TrackChain { track_id } => (
                    project
                        .tracks
                        .iter()
                        .find(|track| track.id == track_id)
                        .map(|track| track.effects.clone())
                        .unwrap_or_default(),
                    EffectTarget::Track { track_id },
                ),
                CliPresetTarget::BusChain { bus_id } => (
                    project
                        .buses
                        .iter()
                        .find(|bus| bus.id == bus_id)
                        .map(|bus| bus.effects.clone())
                        .unwrap_or_default(),
                    EffectTarget::Bus { bus_id },
                ),
                CliPresetTarget::Asset { .. } => {
                    return Err((
                        6,
                        "invalid_parameter",
                        "a chain preset applies to a track or a bus, not to an asset".to_owned(),
                    ));
                }
            };
            let mut commands: Vec<ProjectCommand> = existing
                .iter()
                .map(|insert| ProjectCommand::RemoveEffect {
                    effect_id: insert.id,
                })
                .collect();
            commands.extend(cli_presets::inserts_of(steps).into_iter().map(|effect| {
                ProjectCommand::AddEffect {
                    target: effect_target,
                    effect,
                }
            }));
            Ok(commands)
        }
        _ => Err((
            6,
            "invalid_parameter",
            format!("preset '{}' does not fit what it was applied to", preset.id),
        )),
    }
}

const fn kind_name(kind: PresetKind) -> &'static str {
    match kind {
        PresetKind::Synth => "synth",
        PresetKind::Effect => "effect",
        PresetKind::Chain => "chain",
        PresetKind::Generator => "generator",
        PresetKind::Instrument => "instrument",
    }
}
