use std::path::{Path, PathBuf};

use jutsu_audio_commands::ProjectCommand;
use jutsu_audio_engine::{ExportEncoding, ExportRange, OfflineExporter, PlaybackSnapshot};
use jutsu_audio_model::{AssetId, Clip, ClipId, LayerId, ParameterValue, Project, TrackId};
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

fn build_master_snapshot(
    project: &Project,
    project_path: &Path,
) -> Result<std::sync::Arc<PlaybackSnapshot>, (i32, &'static str, String)> {
    let project_directory = project_path.parent().unwrap_or_else(|| Path::new("."));
    let clips: Vec<_> = project
        .tracks
        .iter()
        .flat_map(|track| &track.layers)
        .flat_map(|layer| &layer.clips)
        .collect();
    let end_frame = clips
        .iter()
        .map(|clip| clip.start_sample.saturating_add(clip.duration_samples))
        .max()
        .unwrap_or(0);
    let mut format = None;
    let mut decoded = Vec::new();
    for clip in clips {
        let asset = project
            .assets
            .iter()
            .find(|asset| asset.id == clip.asset_id)
            .ok_or_else(|| {
                (
                    4,
                    "command_failed",
                    format!("asset {} is missing", clip.asset_id),
                )
            })?;
        let jutsu_audio_model::AudioAssetSource::ManagedFile { path, .. } = &asset.source else {
            return Err((
                3,
                "export_failed",
                "only managed WAV clips can be exported in MVP".into(),
            ));
        };
        let (metadata, samples) = AssetManager::decode_wav_samples(project_directory.join(path))
            .map_err(project_error)?;
        let current = (metadata.sample_rate, metadata.channels);
        if format.is_some_and(|value| value != current) {
            return Err((
                3,
                "export_failed",
                "all MVP clips must share sample rate and channel count".into(),
            ));
        }
        format = Some(current);
        decoded.push((clip, metadata, samples));
    }
    let (sample_rate, channels) = format.unwrap_or((48_000, 2));
    let sample_count = usize::try_from(end_frame)
        .unwrap_or(usize::MAX)
        .checked_mul(usize::from(channels))
        .ok_or_else(|| (3, "export_failed", "project duration is too large".into()))?;
    let mut master = vec![0.0_f32; sample_count];
    for (clip, metadata, samples) in decoded {
        let channels = usize::from(metadata.channels);
        let source_start = usize::try_from(clip.source_start_sample)
            .unwrap_or(usize::MAX)
            .saturating_mul(channels);
        let length = usize::try_from(clip.duration_samples)
            .unwrap_or(usize::MAX)
            .saturating_mul(channels);
        let destination = usize::try_from(clip.start_sample)
            .unwrap_or(usize::MAX)
            .saturating_mul(channels);
        let available = samples
            .len()
            .saturating_sub(source_start)
            .min(length)
            .min(master.len().saturating_sub(destination));
        let gain_db = clip
            .parameters
            .get("gain_db")
            .and_then(|value| match value {
                ParameterValue::Float(value) => Some(*value),
                _ => None,
            })
            .unwrap_or(0.0);
        let gain = 10_f32.powf(gain_db as f32 / 20.0);
        for index in 0..available {
            master[destination + index] += samples[source_start + index] * gain;
        }
    }
    PlaybackSnapshot::new(sample_rate, channels, master.into())
        .map(std::sync::Arc::new)
        .map_err(|error| (3, "export_failed", error.message))
}

fn project_error(error: jutsu_audio_project::ProjectFileError) -> (i32, &'static str, String) {
    (3, "project_io_failed", error.message)
}
