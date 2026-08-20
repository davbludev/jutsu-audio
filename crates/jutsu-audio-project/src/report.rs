//! A diagnostic report: everything worth knowing about a project file that a
//! user cannot see by opening it.
//!
//! The point is a bug report that arrives complete. When a project will not
//! open, or opens wrong, this collects the state around it — what the file
//! claims, what actually reads back, which sources are missing, what recovery
//! material is lying about — into one JSON document that can be attached to an
//! issue without a round trip asking for more.
//!
//! Nothing here mutates the project. A report can always be taken, including
//! from a file that fails to open, which is exactly when it is needed.

use std::fs;
use std::path::{Path, PathBuf};

use jutsu_audio_model::ValidationDiagnostic;
use serde::{Deserialize, Serialize};

use crate::{AssetManager, atomic_write, autosave};

/// The file name the report is written under inside a bundle directory.
pub const REPORT_FILE_NAME: &str = "diagnostics.json";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DiagnosticReport {
    /// The crate version that produced the report.
    pub tool_version: String,
    /// The schema version this build writes.
    pub supported_schema_version: u32,
    pub project_path: String,
    /// Size in bytes, or `None` when the file could not be read at all.
    pub project_bytes: Option<u64>,
    /// What the file claims, read straight out of the JSON without opening the
    /// project — available even when opening fails.
    pub declared_schema_version: Option<u32>,
    /// `Ok` when the project opens, otherwise why it does not.
    pub open_status: OpenStatus,
    /// The schema version the file was migrated from, if opening migrated it.
    pub migrated_from: Option<u32>,
    pub validation: Vec<ValidationDiagnostic>,
    pub assets: Vec<AssetReport>,
    pub counts: Option<ProjectCounts>,
    /// Extension type IDs the project references. Ones this build does not know
    /// are listed here all the same — the report is what makes an unknown
    /// extension visible.
    pub extension_type_ids: Vec<String>,
    pub recovery: RecoveryReport,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OpenStatus {
    Ok,
    Failed { code: String, message: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AssetReport {
    pub asset_id: String,
    pub name: String,
    /// `file`, `managed_file`, `synth`, `sampler` or `generated`.
    pub kind: String,
    /// The stored relative path, for the source kinds that have one.
    pub path: Option<String>,
    /// `None` when the asset has no file behind it.
    pub present: Option<bool>,
    pub bytes: Option<u64>,
    /// Whether the bytes still hash to the recorded fingerprint.
    pub fingerprint_matches: Option<bool>,
    /// Set when the file is present but will not decode.
    pub decode_error: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct ProjectCounts {
    pub tracks: usize,
    pub clips: usize,
    pub assets: usize,
    pub buses: usize,
    pub patterns: usize,
    pub automation_lanes: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct RecoveryReport {
    /// Unsaved work is parked and has not been claimed.
    pub autosave_present: bool,
    pub previous_autosave_present: bool,
    /// Whether the parked state reads back. `false` here with a saved project
    /// still on disk is the case worth knowing about.
    pub autosave_readable: Option<bool>,
    /// Backups left behind by migrations, newest name last.
    pub backups: Vec<String>,
    /// A `.lock` sidecar exists: either a live editor, or a crashed one.
    pub lock_present: bool,
}

/// Collects everything about `project_path`, opening it if it will open.
#[must_use]
pub fn collect(project_path: impl AsRef<Path>) -> DiagnosticReport {
    let project_path = project_path.as_ref();
    let raw = fs::read(project_path).ok();

    let mut report = DiagnosticReport {
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        supported_schema_version: jutsu_audio_model::CURRENT_PROJECT_SCHEMA_VERSION,
        project_path: project_path.display().to_string(),
        project_bytes: raw.as_ref().map(|bytes| bytes.len() as u64),
        declared_schema_version: raw.as_deref().and_then(declared_schema_version),
        open_status: OpenStatus::Ok,
        migrated_from: None,
        validation: Vec::new(),
        assets: Vec::new(),
        counts: None,
        extension_type_ids: Vec::new(),
        recovery: recovery(project_path),
    };

    // Opening a project migrates it, which writes. Reading the file directly
    // keeps a report side-effect free: a diagnostic pass must never be the
    // thing that changes the file being diagnosed.
    match raw.as_deref().map(read_only_open) {
        Some(Ok(project)) => {
            report.migrated_from = report
                .declared_schema_version
                .filter(|version| *version < jutsu_audio_model::CURRENT_PROJECT_SCHEMA_VERSION);
            report.validation = project.validate();
            report.assets = assets(&project, project_path);
            report.counts = Some(counts(&project));
            report.extension_type_ids = extension_type_ids(&project);
        }
        Some(Err(message)) => {
            report.open_status = OpenStatus::Failed {
                code: "invalid_project".into(),
                message,
            };
        }
        None => {
            report.open_status = OpenStatus::Failed {
                code: "io".into(),
                message: "project file could not be read".into(),
            };
        }
    }
    report
}

/// Writes the report, and a copy of the project beside it, into `destination`.
///
/// A copy rather than the original: whoever reads the bundle can open it,
/// break it, and lose nothing.
pub fn write_bundle(
    project_path: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> Result<DiagnosticReport, crate::ProjectFileError> {
    let project_path = project_path.as_ref();
    let destination = destination.as_ref();
    fs::create_dir_all(destination)
        .map_err(|error| crate::ProjectFileError::io(destination, "create bundle", error))?;

    let report = collect(project_path);
    let encoded = serde_json::to_vec_pretty(&report)
        .map_err(|error| {
            crate::ProjectFileError::new(
                crate::ProjectFileErrorCode::InvalidJson,
                destination,
                format!("diagnostic report could not be encoded: {error}"),
            )
        })
        .map(|mut bytes| {
            bytes.push(b'\n');
            bytes
        })?;
    atomic_write(&destination.join(REPORT_FILE_NAME), &encoded)?;

    // Best effort: an unreadable project is the most useful one to have a copy
    // of, and also the one most likely to fail here.
    if let Ok(contents) = fs::read(project_path) {
        let name = project_path
            .file_name()
            .unwrap_or_else(|| "project.json".as_ref());
        atomic_write(&destination.join(name), &contents)?;
    }
    Ok(report)
}

fn declared_schema_version(raw: &[u8]) -> Option<u32> {
    serde_json::from_slice::<serde_json::Value>(raw)
        .ok()?
        .get("schema_version")?
        .as_u64()?
        .try_into()
        .ok()
}

/// Parses without writing anything, migrating in memory the way `open` does.
fn read_only_open(raw: &[u8]) -> Result<jutsu_audio_model::Project, String> {
    let mut value: serde_json::Value = serde_json::from_slice(raw)
        .map_err(|error| format!("project is not valid JSON: {error}"))?;
    if value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        == Some(0)
    {
        value["schema_version"] =
            serde_json::Value::from(jutsu_audio_model::CURRENT_PROJECT_SCHEMA_VERSION);
    }
    serde_json::from_value(value).map_err(|error| format!("project fields are invalid: {error}"))
}

fn recovery(project_path: &Path) -> RecoveryReport {
    let autosave = autosave::autosave_path(project_path);
    let autosave_present = autosave.exists();
    RecoveryReport {
        autosave_present,
        previous_autosave_present: autosave::previous_autosave_path(project_path).exists(),
        autosave_readable: autosave_present
            .then(|| fs::read(&autosave).is_ok_and(|raw| read_only_open(&raw).is_ok())),
        backups: backups(project_path),
        lock_present: sibling(project_path, ".lock").exists(),
    }
}

fn sibling(project_path: &Path, suffix: &str) -> PathBuf {
    let mut name = project_path
        .file_name()
        .unwrap_or_else(|| "project".as_ref())
        .to_os_string();
    name.push(suffix);
    project_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(name)
}

fn backups(project_path: &Path) -> Vec<String> {
    let Some(stem) = project_path.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    let directory = project_path.parent().unwrap_or_else(|| Path::new("."));
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut found: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with(stem) && name.contains(".backup"))
        .collect();
    // Directory order is not defined; a report has to read the same twice.
    found.sort();
    found
}

fn assets(project: &jutsu_audio_model::Project, project_path: &Path) -> Vec<AssetReport> {
    use jutsu_audio_model::AudioAssetSource;

    let directory = project_path.parent().unwrap_or_else(|| Path::new("."));
    project
        .assets
        .iter()
        .map(|asset| {
            let mut report = AssetReport {
                asset_id: asset.id.to_string(),
                name: asset.name.clone(),
                kind: match &asset.source {
                    AudioAssetSource::File { .. } => "file",
                    AudioAssetSource::ManagedFile { .. } => "managed_file",
                    AudioAssetSource::Synth { .. } => "synth",
                    AudioAssetSource::Sampler { .. } => "sampler",
                    AudioAssetSource::Generated { .. } => "generated",
                }
                .into(),
                path: None,
                present: None,
                bytes: None,
                fingerprint_matches: None,
                decode_error: None,
            };
            let (path, fingerprint) = match &asset.source {
                AudioAssetSource::File { path } => (path, None),
                AudioAssetSource::ManagedFile {
                    path, fingerprint, ..
                } => (path, Some(fingerprint)),
                _ => return report,
            };
            report.path = Some(path.clone());
            let full = directory.join(path);
            match fs::read(&full) {
                Ok(contents) => {
                    report.present = Some(true);
                    report.bytes = Some(contents.len() as u64);
                    report.fingerprint_matches =
                        fingerprint.map(|expected| crate::sha256_hex(&contents) == *expected);
                    // A file that is present and hashes right can still be one
                    // this build cannot decode, which is a different bug.
                    if let Err(error) = AssetManager::decode_wav_samples(&full) {
                        report.decode_error = Some(error.message);
                    }
                }
                Err(_) => report.present = Some(false),
            }
            report
        })
        .collect()
}

fn counts(project: &jutsu_audio_model::Project) -> ProjectCounts {
    ProjectCounts {
        tracks: project.tracks.len(),
        clips: project
            .tracks
            .iter()
            .flat_map(|track| &track.layers)
            .map(|layer| layer.clips.len())
            .sum(),
        assets: project.assets.len(),
        buses: project.buses.len(),
        patterns: project.patterns.len(),
        automation_lanes: project.automation.len(),
    }
}

fn extension_type_ids(project: &jutsu_audio_model::Project) -> Vec<String> {
    use jutsu_audio_model::AudioAssetSource;

    let mut found: Vec<String> = project
        .assets
        .iter()
        .filter_map(|asset| match &asset.source {
            AudioAssetSource::Synth { type_id, .. } => Some(type_id.clone()),
            AudioAssetSource::Generated { generator_type, .. } => Some(generator_type.clone()),
            AudioAssetSource::File { .. }
            | AudioAssetSource::ManagedFile { .. }
            | AudioAssetSource::Sampler { .. } => None,
        })
        .chain(
            project
                .tracks
                .iter()
                .flat_map(|track| &track.effects)
                .chain(project.buses.iter().flat_map(|bus| &bus.effects))
                .map(|effect| effect.type_id.clone()),
        )
        .collect();
    found.sort();
    found.dedup();
    found
}
