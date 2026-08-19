use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use jutsu_audio_commands::edits::{self, DeleteMode};
use jutsu_audio_commands::{CommandError, ProjectCommand};
use jutsu_audio_engine::{
    ExportEncoding, ExportRange, MIX_CHANNELS, OfflineExporter, PlaybackSnapshot, SourceAudio,
    mix_project,
};
use jutsu_audio_extensions::{ExtensionTypeId, RegenerateMode};
use jutsu_audio_model::{
    AssetId, AudioAssetSource, Clip, ClipId, ClipNote, Layer, LayerId, LoopRegion, Marker,
    MarkerId, ParameterValue, Project, Track, TrackId,
};
use jutsu_audio_project::{AssetManager, ImportMode, ImportStatus, ProjectStore};
use jutsu_audio_session::TransportAction as SessionTransportAction;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::cli_session::{self, Applied};
use crate::{cli_generator, cli_synth};

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
            } => *protocol_version,
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
            AudioAssetSource::Generated { .. } | AudioAssetSource::Synth { .. } => continue,
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
