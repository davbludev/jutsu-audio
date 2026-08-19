use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use atomic_write_file::AtomicWriteFile;
use hound::{SampleFormat, WavReader};
use jutsu_audio_model::{
    Asset, AssetId, AudioAssetSource, BusId, CURRENT_PROJECT_SCHEMA_VERSION, Layer, LayerId,
    MixerBus, Project, ProjectId, ProjectMetadata, Track, TrackId, ValidationDiagnostic,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub mod autosave;
pub mod waveform;

pub use waveform::{
    BASE_WINDOW_FRAMES, CACHE_FORMAT_VERSION, CachedWaveform, PeakLevel, WaveformPeak,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectFileErrorCode {
    Io,
    InvalidJson,
    UnsupportedSchemaVersion,
    MigrationFailed,
    InvalidProject,
    InvalidAssetPath,
    InvalidWav,
    UnsupportedAudioFormat,
}

#[derive(Debug)]
pub struct ProjectFileError {
    pub code: ProjectFileErrorCode,
    pub path: PathBuf,
    pub message: String,
    pub diagnostics: Vec<ValidationDiagnostic>,
}

impl ProjectFileError {
    fn new(code: ProjectFileErrorCode, path: &Path, message: impl Into<String>) -> Self {
        Self {
            code,
            path: path.to_path_buf(),
            message: message.into(),
            diagnostics: Vec::new(),
        }
    }

    fn io(path: &Path, operation: &str, error: std::io::Error) -> Self {
        Self::new(
            ProjectFileErrorCode::Io,
            path,
            format!("failed to {operation}: {error}"),
        )
    }
}

#[derive(Debug)]
pub struct OpenedProject {
    pub project: Project,
    pub migrated_from: Option<u32>,
    pub backup_path: Option<PathBuf>,
}

pub struct ProjectStore;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportMode {
    CopyIntoProject,
    ReferenceInPlace,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportStatus {
    Prepared,
    Duplicate(AssetId),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AudioMetadata {
    pub sample_rate: u32,
    pub channels: u16,
    pub frame_count: u64,
    pub bits_per_sample: u16,
    pub sample_format: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedImport {
    pub status: ImportStatus,
    pub asset: Option<Asset>,
    pub metadata: AudioMetadata,
    pub waveform: CachedWaveform,
    pub cache_path: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssetDiagnosticCode {
    MissingSource,
    ChangedSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetDiagnostic {
    pub code: AssetDiagnosticCode,
    pub asset_id: AssetId,
    pub path: PathBuf,
    pub message: String,
}

pub struct AssetManager;

impl ProjectStore {
    pub fn create(
        path: impl AsRef<Path>,
        name: impl Into<String>,
    ) -> Result<Project, ProjectFileError> {
        let project = Self::new_project(name);
        Self::save(path, &project)?;
        Ok(project)
    }

    #[must_use]
    pub fn new_project(name: impl Into<String>) -> Project {
        let master_bus_id = BusId::new();
        Project {
            schema_version: CURRENT_PROJECT_SCHEMA_VERSION,
            id: ProjectId::new(),
            metadata: ProjectMetadata {
                name: name.into(),
                properties: BTreeMap::new(),
            },
            assets: Vec::new(),
            buses: vec![MixerBus {
                id: master_bus_id,
                name: "Master".into(),
                output_bus_id: None,
                parameters: BTreeMap::new(),
            }],
            master_bus_id,
            tracks: vec![Track {
                id: TrackId::new(),
                name: "Track 1".into(),
                output_bus_id: master_bus_id,
                parameters: BTreeMap::new(),
                layers: vec![Layer {
                    id: LayerId::new(),
                    name: "Layer 1".into(),
                    clips: Vec::new(),
                }],
            }],
            markers: Vec::new(),
            loop_region: None,
            automation: Vec::new(),
        }
    }

    pub fn open(path: impl AsRef<Path>) -> Result<OpenedProject, ProjectFileError> {
        let path = path.as_ref();
        let original =
            fs::read(path).map_err(|error| ProjectFileError::io(path, "read project", error))?;
        let mut value: Value = serde_json::from_slice(&original).map_err(|error| {
            ProjectFileError::new(
                ProjectFileErrorCode::InvalidJson,
                path,
                format!("project is not valid JSON: {error}"),
            )
        })?;
        let schema_version = value
            .get("schema_version")
            .and_then(Value::as_u64)
            .and_then(|version| u32::try_from(version).ok())
            .ok_or_else(|| {
                ProjectFileError::new(
                    ProjectFileErrorCode::InvalidJson,
                    path,
                    "project schema_version must be an unsigned 32-bit integer",
                )
            })?;

        if schema_version > CURRENT_PROJECT_SCHEMA_VERSION {
            return Err(ProjectFileError::new(
                ProjectFileErrorCode::UnsupportedSchemaVersion,
                path,
                format!(
                    "project schema version {schema_version} is newer than supported version {CURRENT_PROJECT_SCHEMA_VERSION}"
                ),
            ));
        }

        let migrated_from =
            (schema_version < CURRENT_PROJECT_SCHEMA_VERSION).then_some(schema_version);
        if schema_version == 0 {
            value["schema_version"] = Value::from(CURRENT_PROJECT_SCHEMA_VERSION);
        } else if schema_version != CURRENT_PROJECT_SCHEMA_VERSION {
            return Err(ProjectFileError::new(
                ProjectFileErrorCode::MigrationFailed,
                path,
                format!("no migration path exists from schema version {schema_version}"),
            ));
        }

        let project: Project = serde_json::from_value(value).map_err(|error| {
            ProjectFileError::new(
                ProjectFileErrorCode::InvalidJson,
                path,
                format!("project fields are invalid: {error}"),
            )
        })?;
        validate_project(path, &project)?;

        let backup_path = if let Some(source_version) = migrated_from {
            let backup_path = next_backup_path(path, source_version);
            atomic_write(&backup_path, &original)?;
            Self::save(path, &project)?;
            Some(backup_path)
        } else {
            None
        };

        Ok(OpenedProject {
            project,
            migrated_from,
            backup_path,
        })
    }

    pub fn save(path: impl AsRef<Path>, project: &Project) -> Result<(), ProjectFileError> {
        let path = path.as_ref();
        validate_project(path, project)?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .map_err(|error| ProjectFileError::io(parent, "create project directory", error))?;
        }
        let mut encoded = serde_json::to_vec_pretty(project).map_err(|error| {
            ProjectFileError::new(
                ProjectFileErrorCode::InvalidProject,
                path,
                format!("project cannot be serialized: {error}"),
            )
        })?;
        encoded.push(b'\n');
        atomic_write(path, &encoded)
    }
}

impl AssetManager {
    pub fn decode_wav_samples(
        path: impl AsRef<Path>,
    ) -> Result<(AudioMetadata, Vec<f32>), ProjectFileError> {
        decode_wav(path.as_ref())
    }

    /// Where `prepare_wav_import` parks the peak cache for a source
    /// fingerprint. Callers that want to draw a waveform look here first.
    #[must_use]
    pub fn waveform_cache_path(project_path: impl AsRef<Path>, fingerprint: &str) -> PathBuf {
        project_path
            .as_ref()
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(".jutsu-audio-cache")
            .join("waveforms")
            .join(format!("{fingerprint}.json"))
    }

    /// Reads the cached peaks for an already-imported asset. Cheap enough to
    /// call per asset on a worker thread: the cache holds one peak per 1024
    /// frames, not the audio itself.
    pub fn load_waveform(
        project_path: impl AsRef<Path>,
        fingerprint: &str,
    ) -> Result<CachedWaveform, ProjectFileError> {
        let cache_path = Self::waveform_cache_path(project_path, fingerprint);
        let contents = fs::read(&cache_path)
            .map_err(|error| ProjectFileError::io(&cache_path, "read waveform cache", error))?;
        let waveform: CachedWaveform = serde_json::from_slice(&contents).map_err(|error| {
            ProjectFileError::new(
                ProjectFileErrorCode::InvalidJson,
                &cache_path,
                format!("waveform cache is not valid: {error}"),
            )
        })?;
        if !waveform.is_current() {
            // Readable, but written before the zoom levels existed. Reporting
            // it as unusable is what makes the caller rebuild it.
            return Err(ProjectFileError::new(
                ProjectFileErrorCode::InvalidJson,
                &cache_path,
                format!(
                    "waveform cache is format {} and this build reads {CACHE_FORMAT_VERSION}",
                    waveform.format_version
                ),
            ));
        }
        Ok(waveform)
    }

    /// Rebuilds and rewrites the peak cache for a source file. Used when a
    /// project is opened whose cache was never written or has been deleted.
    pub fn rebuild_waveform(
        project_path: impl AsRef<Path>,
        source_path: impl AsRef<Path>,
        fingerprint: &str,
    ) -> Result<CachedWaveform, ProjectFileError> {
        let (metadata, samples) = decode_wav(source_path.as_ref())?;
        let waveform = build_waveform(metadata, &samples);
        let cache_path = Self::waveform_cache_path(project_path, fingerprint);
        write_waveform_cache(&cache_path, &waveform)?;
        Ok(waveform)
    }

    pub fn prepare_wav_import(
        project: &Project,
        project_path: impl AsRef<Path>,
        source_path: impl AsRef<Path>,
        mode: ImportMode,
    ) -> Result<PreparedImport, ProjectFileError> {
        let project_path = project_path.as_ref();
        let source_path = source_path.as_ref();
        let project_directory = project_path.parent().unwrap_or_else(|| Path::new("."));
        let contents = fs::read(source_path)
            .map_err(|error| ProjectFileError::io(source_path, "read WAV source", error))?;
        let fingerprint = sha256_hex(&contents);
        // Decode the bytes already in memory instead of reopening the file.
        let (metadata, samples) = decode_wav_bytes(&contents, source_path)?;
        let waveform = build_waveform(metadata.clone(), &samples);
        let cache_path = Self::waveform_cache_path(project_path, &fingerprint);
        write_waveform_cache(&cache_path, &waveform)?;

        if let Some(existing) = project.assets.iter().find(|asset| {
            matches!(&asset.source, AudioAssetSource::ManagedFile { fingerprint: value, .. } if value == &fingerprint)
        }) {
            return Ok(PreparedImport {
                status: ImportStatus::Duplicate(existing.id),
                asset: None,
                metadata,
                waveform,
                cache_path,
            });
        }

        let portable_path = match mode {
            ImportMode::CopyIntoProject => {
                let relative = PathBuf::from("assets").join(format!("{fingerprint}.wav"));
                let destination = project_directory.join(&relative);
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent).map_err(|error| {
                        ProjectFileError::io(parent, "create asset directory", error)
                    })?;
                }
                if !destination.exists() {
                    atomic_write(&destination, &contents)?;
                }
                path_to_portable_string(&relative, project_path)?
            }
            ImportMode::ReferenceInPlace => {
                let relative = source_path.strip_prefix(project_directory).map_err(|_| {
                    ProjectFileError::new(
                        ProjectFileErrorCode::InvalidAssetPath,
                        source_path,
                        "referenced WAV must be inside the project directory",
                    )
                })?;
                path_to_portable_string(relative, project_path)?
            }
        };
        let asset = Asset {
            id: AssetId::new(),
            name: source_path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("Imported WAV")
                .to_owned(),
            source: AudioAssetSource::ManagedFile {
                path: portable_path,
                fingerprint,
                sample_rate: metadata.sample_rate,
                channels: metadata.channels,
                frame_count: metadata.frame_count,
            },
        };
        Ok(PreparedImport {
            status: ImportStatus::Prepared,
            asset: Some(asset),
            metadata,
            waveform,
            cache_path,
        })
    }

    #[must_use]
    pub fn verify_sources(
        project: &Project,
        project_path: impl AsRef<Path>,
    ) -> Vec<AssetDiagnostic> {
        let project_directory = project_path
            .as_ref()
            .parent()
            .unwrap_or_else(|| Path::new("."));
        project
            .assets
            .iter()
            .filter_map(|asset| {
                let AudioAssetSource::ManagedFile {
                    path, fingerprint, ..
                } = &asset.source
                else {
                    return None;
                };
                let source_path = project_directory.join(path);
                let contents = match fs::read(&source_path) {
                    Ok(contents) => contents,
                    Err(_) => {
                        return Some(AssetDiagnostic {
                            code: AssetDiagnosticCode::MissingSource,
                            asset_id: asset.id,
                            path: source_path,
                            message: "managed audio source is missing".into(),
                        });
                    }
                };
                (sha256_hex(&contents) != *fingerprint).then(|| AssetDiagnostic {
                    code: AssetDiagnosticCode::ChangedSource,
                    asset_id: asset.id,
                    path: source_path,
                    message: "managed audio source fingerprint changed".into(),
                })
            })
            .collect()
    }
}

fn validate_project(path: &Path, project: &Project) -> Result<(), ProjectFileError> {
    for asset in &project.assets {
        if let AudioAssetSource::File { path: asset_path }
        | AudioAssetSource::ManagedFile {
            path: asset_path, ..
        } = &asset.source
        {
            let portable_path = Path::new(asset_path);
            let has_forbidden_component = portable_path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::Prefix(_)
                        | std::path::Component::RootDir
                        | std::path::Component::ParentDir
                )
            });
            if asset_path.is_empty() || portable_path.is_absolute() || has_forbidden_component {
                return Err(ProjectFileError::new(
                    ProjectFileErrorCode::InvalidAssetPath,
                    path,
                    format!(
                        "asset {} file path must be non-empty and project-relative",
                        asset.id
                    ),
                ));
            }
        }
    }
    let diagnostics = project.validate();
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(ProjectFileError {
            code: ProjectFileErrorCode::InvalidProject,
            path: path.to_path_buf(),
            message: "project validation failed".into(),
            diagnostics,
        })
    }
}

fn sha256_hex(contents: &[u8]) -> String {
    Sha256::digest(contents)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn decode_wav(path: &Path) -> Result<(AudioMetadata, Vec<f32>), ProjectFileError> {
    let reader = WavReader::open(path).map_err(|error| {
        ProjectFileError::new(
            ProjectFileErrorCode::InvalidWav,
            path,
            format!("cannot decode WAV: {error}"),
        )
    })?;
    decode_wav_reader(reader, path)
}

fn decode_wav_bytes(
    contents: &[u8],
    path: &Path,
) -> Result<(AudioMetadata, Vec<f32>), ProjectFileError> {
    let reader = WavReader::new(std::io::Cursor::new(contents)).map_err(|error| {
        ProjectFileError::new(
            ProjectFileErrorCode::InvalidWav,
            path,
            format!("cannot decode WAV: {error}"),
        )
    })?;
    decode_wav_reader(reader, path)
}

fn decode_wav_reader<R: std::io::Read>(
    mut reader: WavReader<R>,
    path: &Path,
) -> Result<(AudioMetadata, Vec<f32>), ProjectFileError> {
    let spec = reader.spec();
    if spec.channels == 0 {
        return Err(ProjectFileError::new(
            ProjectFileErrorCode::InvalidWav,
            path,
            "WAV must contain at least one channel",
        ));
    }
    let samples = match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .map(|sample| sample.map(|value| value.clamp(-1.0, 1.0)))
            .collect::<Result<Vec<_>, _>>(),
        (SampleFormat::Int, bits @ 1..=8) => {
            let scale = 2_f32.powi(i32::from(bits) - 1);
            reader
                .samples::<i8>()
                .map(|sample| sample.map(|value| (f32::from(value) / scale).clamp(-1.0, 1.0)))
                .collect::<Result<Vec<_>, _>>()
        }
        (SampleFormat::Int, bits @ 9..=16) => {
            let scale = 2_f32.powi(i32::from(bits) - 1);
            reader
                .samples::<i16>()
                .map(|sample| sample.map(|value| (f32::from(value) / scale).clamp(-1.0, 1.0)))
                .collect::<Result<Vec<_>, _>>()
        }
        (SampleFormat::Int, bits @ 17..=32) => {
            let scale = 2_f32.powi(i32::from(bits) - 1);
            reader
                .samples::<i32>()
                .map(|sample| sample.map(|value| ((value as f32) / scale).clamp(-1.0, 1.0)))
                .collect::<Result<Vec<_>, _>>()
        }
        _ => {
            return Err(ProjectFileError::new(
                ProjectFileErrorCode::UnsupportedAudioFormat,
                path,
                format!(
                    "unsupported WAV encoding: {:?} {}-bit",
                    spec.sample_format, spec.bits_per_sample
                ),
            ));
        }
    }
    .map_err(|error| {
        ProjectFileError::new(
            ProjectFileErrorCode::InvalidWav,
            path,
            format!("cannot decode WAV samples: {error}"),
        )
    })?;
    let metadata = AudioMetadata {
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        frame_count: u64::try_from(samples.len() / usize::from(spec.channels)).unwrap_or(u64::MAX),
        bits_per_sample: spec.bits_per_sample,
        sample_format: match spec.sample_format {
            SampleFormat::Float => "float",
            SampleFormat::Int => "pcm",
        }
        .into(),
    };
    Ok((metadata, samples))
}

fn write_waveform_cache(
    cache_path: &Path,
    waveform: &CachedWaveform,
) -> Result<(), ProjectFileError> {
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| ProjectFileError::io(parent, "create waveform cache", error))?;
    }
    // Compact, not pretty: this file is machine-read only and gets one entry
    // per 1024 frames, so pretty-printing triples it for no reader.
    let mut encoded = serde_json::to_vec(waveform).map_err(|error| {
        ProjectFileError::new(
            ProjectFileErrorCode::InvalidProject,
            cache_path,
            format!("waveform cache cannot be serialized: {error}"),
        )
    })?;
    encoded.push(b'\n');
    atomic_write(cache_path, &encoded)
}

fn build_waveform(metadata: AudioMetadata, samples: &[f32]) -> CachedWaveform {
    CachedWaveform::build(metadata, samples)
}

fn path_to_portable_string(path: &Path, error_path: &Path) -> Result<String, ProjectFileError> {
    let parts = path
        .components()
        .map(|component| match component {
            std::path::Component::Normal(value) => value.to_str().map(str::to_owned),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            ProjectFileError::new(
                ProjectFileErrorCode::InvalidAssetPath,
                error_path,
                "asset path must be project-relative UTF-8",
            )
        })?;
    if parts.is_empty() {
        return Err(ProjectFileError::new(
            ProjectFileErrorCode::InvalidAssetPath,
            error_path,
            "asset path must not be empty",
        ));
    }
    Ok(parts.join("/"))
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), ProjectFileError> {
    let mut file = AtomicWriteFile::open(path)
        .map_err(|error| ProjectFileError::io(path, "open atomic project write", error))?;
    file.write_all(contents)
        .map_err(|error| ProjectFileError::io(path, "write project", error))?;
    file.commit()
        .map_err(|error| ProjectFileError::io(path, "commit atomic project write", error))
}

fn next_backup_path(path: &Path, source_version: u32) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project");
    for suffix in 0_u32.. {
        let suffix = if suffix == 0 {
            String::new()
        } else {
            format!(".{suffix}")
        };
        let candidate = path.with_file_name(format!("{file_name}.v{source_version}.bak{suffix}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("u32 backup suffix space exhausted")
}
