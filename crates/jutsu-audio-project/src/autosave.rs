//! Crash recovery for unsaved work.
//!
//! An editor with unsaved edits parks them in a sidecar next to the project
//! file. A clean save removes the sidecar; a crash leaves it behind, and the
//! next open finds it. Recovery is always offered, never performed silently —
//! the file on disk is what the user last chose to keep.
//!
//! The sidecar is an ordinary project file, so it is written, migrated and
//! validated by exactly the same code as the real one.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use jutsu_audio_model::Project;

use crate::{OpenedProject, ProjectFileError, ProjectStore};

/// Appended to the whole project file name, like the session and lock sidecars.
pub const AUTOSAVE_SUFFIX: &str = ".autosave";

/// Where unsaved work for `project_path` is parked.
#[must_use]
pub fn autosave_path(project_path: impl AsRef<Path>) -> PathBuf {
    let project_path = project_path.as_ref();
    let mut name = project_path
        .file_name()
        .unwrap_or_else(|| "project".as_ref())
        .to_os_string();
    name.push(AUTOSAVE_SUFFIX);
    project_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(name)
}

/// Parks the current state. Atomic, like any other project write, so a crash
/// mid-autosave cannot leave a half-written recovery file.
pub fn write(project_path: impl AsRef<Path>, project: &Project) -> Result<(), ProjectFileError> {
    ProjectStore::save(autosave_path(project_path), project)
}

/// Removes the sidecar. Called after a successful save, and when the user
/// decides not to recover. Already gone counts as removed.
pub fn discard(project_path: impl AsRef<Path>) -> Result<(), ProjectFileError> {
    let path = autosave_path(project_path);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ProjectFileError {
            code: crate::ProjectFileErrorCode::Io,
            message: format!("failed to remove the recovery file: {error}"),
            path,
            diagnostics: Vec::new(),
        }),
    }
}

/// Loads parked work, if there is any.
///
/// `Ok(None)` means there is nothing to recover. A sidecar that cannot be read
/// is reported rather than swallowed: the user should be told their recovery
/// file is unusable, not left thinking there was none.
pub fn recover(project_path: impl AsRef<Path>) -> Result<Option<OpenedProject>, ProjectFileError> {
    let path = autosave_path(project_path);
    if !path.exists() {
        return Ok(None);
    }
    ProjectStore::open(&path).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_at(directory: &Path, name: &str) -> PathBuf {
        let path = directory.join("song.jutsu-audio.json");
        ProjectStore::save(&path, &ProjectStore::new_project(name)).expect("save");
        path
    }

    #[test]
    fn the_sidecar_sits_beside_the_project_and_keeps_its_whole_name() {
        let path = autosave_path(Path::new("/projects/song.jutsu-audio.json"));
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("song.jutsu-audio.json.autosave")
        );
    }

    #[test]
    fn parked_work_is_recovered_without_touching_the_saved_project() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = project_at(directory.path(), "Saved");

        let mut unsaved = ProjectStore::open(&path).expect("open").project;
        unsaved.metadata.name = "Unsaved".into();
        write(&path, &unsaved).expect("autosave");

        let recovered = recover(&path).expect("recover").expect("something parked");
        assert_eq!(recovered.project.metadata.name, "Unsaved");
        assert_eq!(
            ProjectStore::open(&path)
                .expect("open")
                .project
                .metadata
                .name,
            "Saved",
            "recovery must never overwrite the saved project on its own"
        );
    }

    #[test]
    fn a_project_with_nothing_parked_has_nothing_to_recover() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = project_at(directory.path(), "Saved");
        assert!(recover(&path).expect("recover").is_none());
    }

    #[test]
    fn discarding_removes_the_sidecar_and_is_safe_to_repeat() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = project_at(directory.path(), "Saved");
        write(&path, &ProjectStore::new_project("Unsaved")).expect("autosave");

        discard(&path).expect("discard");
        assert!(recover(&path).expect("recover").is_none());
        discard(&path).expect("discarding twice is not an error");
    }
}
