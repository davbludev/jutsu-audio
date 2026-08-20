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
/// The generation before the current one. Kept so a bad autosave — state the
/// user would not want back — is not the only thing left after a crash.
pub const PREVIOUS_AUTOSAVE_SUFFIX: &str = ".autosave.1";

/// Where unsaved work for `project_path` is parked.
#[must_use]
pub fn autosave_path(project_path: impl AsRef<Path>) -> PathBuf {
    sidecar(project_path.as_ref(), AUTOSAVE_SUFFIX)
}

/// Where the generation before the current one is kept.
#[must_use]
pub fn previous_autosave_path(project_path: impl AsRef<Path>) -> PathBuf {
    sidecar(project_path.as_ref(), PREVIOUS_AUTOSAVE_SUFFIX)
}

fn sidecar(project_path: &Path, suffix: &str) -> PathBuf {
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

/// Parks the current state. Atomic, like any other project write, so a crash
/// mid-autosave cannot leave a half-written recovery file.
///
/// The generation before this one is kept alongside it. Atomicity protects
/// against a torn write; keeping a previous generation protects against a
/// *complete* write of state the user would not want back.
pub fn write(project_path: impl AsRef<Path>, project: &Project) -> Result<(), ProjectFileError> {
    let project_path = project_path.as_ref();
    let current = autosave_path(project_path);
    if current.exists() {
        // Best effort: failing to rotate is not a reason to skip the autosave.
        let _ = fs::rename(&current, previous_autosave_path(project_path));
    }
    ProjectStore::save(current, project)
}

/// Removes both generations. Called after a successful save, and when the user
/// decides not to recover. Already gone counts as removed.
pub fn discard(project_path: impl AsRef<Path>) -> Result<(), ProjectFileError> {
    let project_path = project_path.as_ref();
    remove(&autosave_path(project_path))?;
    remove(&previous_autosave_path(project_path))
}

fn remove(path: &Path) -> Result<(), ProjectFileError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ProjectFileError {
            code: crate::ProjectFileErrorCode::Io,
            message: format!("failed to remove the recovery file: {error}"),
            path: path.to_path_buf(),
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
    let project_path = project_path.as_ref();
    let current = autosave_path(project_path);
    if !current.exists() {
        return Ok(None);
    }
    match ProjectStore::open(&current) {
        Ok(opened) => Ok(Some(opened)),
        Err(error) => {
            // The newest generation is unreadable. Fall back to the one before
            // it rather than reporting nothing to recover — losing an edit
            // beats losing the session.
            let previous = previous_autosave_path(project_path);
            if previous.exists() {
                ProjectStore::open(&previous).map(Some)
            } else {
                Err(error)
            }
        }
    }
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

#[cfg(test)]
mod retention_tests {
    use super::*;

    fn saved(directory: &Path, name: &str) -> PathBuf {
        let path = directory.join("song.jutsu-audio.json");
        ProjectStore::save(&path, &ProjectStore::new_project(name)).expect("save");
        path
    }

    #[test]
    fn a_second_autosave_keeps_the_first_beside_it() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = saved(directory.path(), "Saved");

        write(&path, &ProjectStore::new_project("First")).expect("first");
        write(&path, &ProjectStore::new_project("Second")).expect("second");

        assert_eq!(
            recover(&path)
                .expect("recover")
                .expect("something parked")
                .project
                .metadata
                .name,
            "Second"
        );
        assert_eq!(
            ProjectStore::open(previous_autosave_path(&path))
                .expect("previous")
                .project
                .metadata
                .name,
            "First",
            "the generation before is still there"
        );
    }

    #[test]
    fn an_unreadable_autosave_falls_back_to_the_one_before_it() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = saved(directory.path(), "Saved");
        write(&path, &ProjectStore::new_project("Good")).expect("first");
        write(&path, &ProjectStore::new_project("Also good")).expect("second");

        // A crash mid-write cannot produce this — the write is atomic — but a
        // damaged disk can, and losing one edit beats losing the session.
        fs::write(autosave_path(&path), b"{ truncated").expect("corrupt");

        let recovered = recover(&path).expect("recover").expect("the older one");
        assert_eq!(recovered.project.metadata.name, "Good");
    }

    #[test]
    fn discarding_clears_every_generation() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = saved(directory.path(), "Saved");
        write(&path, &ProjectStore::new_project("First")).expect("first");
        write(&path, &ProjectStore::new_project("Second")).expect("second");

        discard(&path).expect("discard");
        assert!(recover(&path).expect("recover").is_none());
        assert!(!previous_autosave_path(&path).exists());
    }
}
