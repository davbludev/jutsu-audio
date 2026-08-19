use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use jutsu_audio_commands::ProjectCommand;
use jutsu_audio_engine::{
    ExportEncoding, ExportRange, MIX_CHANNELS, OfflineExporter, PlaybackSnapshot, SourceAudio,
    mix_project,
};
use jutsu_audio_model::{
    AssetId, AudioAssetSource, Clip, ClipId, Layer, LayerId, ParameterValue, Project, Track,
    TrackId,
};
use jutsu_audio_project::{AssetManager, ImportMode, ImportStatus, ProjectStore};
use jutsu_audio_session::TransportAction as SessionTransportAction;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::cli_session::{self, Applied};

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
            } => *protocol_version,
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
        Request::DeleteClip { path, clip_id, .. } => {
            let applied = cli_session::apply(&path, vec![ProjectCommand::RemoveClip { clip_id }])?;
            Ok(clip_result("clip_deleted", clip_id, &applied))
        }
        Request::ExportWav {
            path,
            output,
            encoding,
            start_frame,
            frame_count,
            ..
        } => {
            let project = ProjectStore::open(&path).map_err(project_error)?.project;
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
            AudioAssetSource::Generated { .. } => continue,
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

    let mixed = mix_project(project, sample_rate, |asset_id| {
        decoded
            .get(&asset_id)
            .cloned()
            .ok_or_else(|| format!("asset {asset_id} has no readable WAV source"))
    })
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
