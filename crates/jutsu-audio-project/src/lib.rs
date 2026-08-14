use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use atomic_write_file::AtomicWriteFile;
use jutsu_audio_model::{
    AudioAssetSource, BusId, CURRENT_PROJECT_SCHEMA_VERSION, MixerBus, Project, ProjectId,
    ProjectMetadata, ValidationDiagnostic,
};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectFileErrorCode {
    Io,
    InvalidJson,
    UnsupportedSchemaVersion,
    MigrationFailed,
    InvalidProject,
    InvalidAssetPath,
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
            tracks: Vec::new(),
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

fn validate_project(path: &Path, project: &Project) -> Result<(), ProjectFileError> {
    for asset in &project.assets {
        if let AudioAssetSource::File { path: asset_path } = &asset.source {
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
