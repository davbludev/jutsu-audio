//! How the CLI reaches a project: through the editor that owns it, or through
//! the write lock when nobody does.
//!
//! Every mutating operation goes through [`apply`], so there is one place that
//! decides between the two routes and one shape of answer for both.

use std::path::Path;
use std::time::Duration;

use jutsu_audio_commands::{
    COMMAND_PROTOCOL_VERSION, CommandEnvelope, CommandId, ProjectCommand, ProjectCommandEngine,
};
use jutsu_audio_project::ProjectStore;
use jutsu_audio_session::{
    ClientError, ProjectLock, RequestPayload, ResponsePayload, SessionClient, SessionErrorCode,
    SessionResponse, TransportAction,
};
use serde::Serialize;

/// Exit code, machine-readable error code, human message — the shape
/// `cli::execute` already reports failures in.
pub type CliFailure = (i32, &'static str, String);

/// How long to wait for another offline writer to finish its batch. Long
/// enough to ride out a concurrent script on a loaded machine, short enough
/// not to look hung.
const LOCK_WAIT: Duration = Duration::from_secs(10);

/// Which route an operation took. Reported in every mutating response so a
/// caller can tell a live edit from a file edit.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    /// Applied by the editor that has the project open.
    Session,
    /// Applied under the project write lock, with no editor running.
    Offline,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Applied {
    pub revision: u64,
    pub delivery: Delivery,
}

/// Applies a command batch to the project at `path`.
///
/// Prefers the live session: an editor with unsaved work is ahead of the file,
/// so writing the file behind it would lose those edits. With no session, the
/// write lock makes this process the single writer for the length of the batch.
pub fn apply(path: &Path, commands: Vec<ProjectCommand>) -> Result<Applied, CliFailure> {
    match SessionClient::attach(path) {
        Ok(Some(mut client)) => {
            let response = client
                .request(RequestPayload::Apply {
                    // No precondition: the CLI has not read a revision to be
                    // stale against. A caller that has one sends it itself.
                    expected_revision: None,
                    commands,
                })
                .map_err(client_error)?;
            let revision = match session_result(response)? {
                ResponsePayload::Applied { revision, .. } => revision,
                other => {
                    return Err((
                        5,
                        "session_unavailable",
                        format!("the session answered an apply with {other:?}"),
                    ));
                }
            };
            Ok(Applied {
                revision,
                delivery: Delivery::Session,
            })
        }
        Ok(None) => apply_offline(path, commands),
        Err(error) => Err(client_error(error)),
    }
}

/// Sends a transport action to the live editor. With no session there is
/// nothing playing, so the action is acknowledged and dropped.
pub fn transport(
    path: &Path,
    action: TransportAction,
    position_frames: u64,
) -> Result<Delivery, CliFailure> {
    match SessionClient::attach(path) {
        Ok(Some(mut client)) => {
            let response = client
                .request(RequestPayload::Transport {
                    action,
                    position_frames,
                })
                .map_err(client_error)?;
            session_result(response)?;
            Ok(Delivery::Session)
        }
        Ok(None) => Ok(Delivery::Offline),
        Err(error) => Err(client_error(error)),
    }
}

/// Reports what owns the project right now, without changing anything.
pub fn status(path: &Path) -> Result<Option<ResponsePayload>, CliFailure> {
    match SessionClient::attach(path) {
        Ok(Some(mut client)) => {
            let response = client
                .request(RequestPayload::Status)
                .map_err(client_error)?;
            session_result(response).map(Some)
        }
        Ok(None) => Ok(None),
        Err(error) => Err(client_error(error)),
    }
}

/// The no-editor route: take the write lock, apply, save, release. The lock is
/// held across the read-modify-write so two CLI processes cannot interleave.
fn apply_offline(path: &Path, commands: Vec<ProjectCommand>) -> Result<Applied, CliFailure> {
    let _lock = ProjectLock::acquire_within(path, LOCK_WAIT).map_err(|error| {
        (
            5,
            "project_locked",
            format!("{} ({})", error.message, error.path.display()),
        )
    })?;
    let project = ProjectStore::open(path)
        .map_err(|error| (3, "project_io_failed", error.message))?
        .project;
    let mut engine =
        ProjectCommandEngine::new(project).map_err(|error| (4, "command_failed", error.message))?;
    let outcome = engine
        .apply(CommandEnvelope {
            protocol_version: COMMAND_PROTOCOL_VERSION,
            command_id: CommandId::new(),
            expected_revision: 0,
            commands,
        })
        .map_err(|error| (4, "command_failed", error.message))?;
    ProjectStore::save(path, engine.project())
        .map_err(|error| (3, "project_io_failed", error.message))?;
    Ok(Applied {
        revision: outcome.revision,
        delivery: Delivery::Offline,
    })
}

fn session_result(response: SessionResponse) -> Result<ResponsePayload, CliFailure> {
    match response {
        SessionResponse::Ok { payload, .. } => Ok(payload),
        SessionResponse::Error { error, .. } => {
            let (exit_code, code) = match error.code {
                SessionErrorCode::RevisionConflict => (5, "revision_conflict"),
                SessionErrorCode::CommandFailed => (4, "command_failed"),
                _ => (5, "session_unavailable"),
            };
            Err((exit_code, code, error.message))
        }
    }
}

fn client_error(error: ClientError) -> CliFailure {
    (5, "session_unavailable", error.message)
}

#[cfg(test)]
mod tests {
    use jutsu_audio_session::{SessionDescriptor, lock::lock_file_path};

    use super::*;

    fn rename(name: &str) -> Vec<ProjectCommand> {
        vec![ProjectCommand::SetProjectName { name: name.into() }]
    }

    #[test]
    fn with_no_editor_running_the_batch_is_written_to_the_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("song.jutsu-audio.json");
        ProjectStore::create(&path, "Before".to_string()).expect("create");

        let applied = apply(&path, rename("After")).expect("apply");
        assert_eq!(
            applied,
            Applied {
                revision: 1,
                delivery: Delivery::Offline
            }
        );
        assert_eq!(
            ProjectStore::open(&path)
                .expect("open")
                .project
                .metadata
                .name,
            "After"
        );
        assert!(
            !lock_file_path(&path).exists(),
            "the lock is released with the batch"
        );
    }

    #[test]
    fn a_session_file_whose_owner_is_gone_falls_back_to_the_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("song.jutsu-audio.json");
        ProjectStore::create(&path, "Before".to_string()).expect("create");
        // Port 1 on loopback answers nothing; this is a session file left
        // behind by a crashed editor.
        std::mem::forget(
            SessionDescriptor::new(&path, 1, "stale")
                .publish()
                .expect("publish"),
        );

        let applied = apply(&path, rename("After")).expect("apply");
        assert_eq!(applied.delivery, Delivery::Offline);
        assert!(
            !jutsu_audio_session::discovery::session_file_path(&path).exists(),
            "the stale session file is cleaned up"
        );
    }

    #[test]
    fn a_project_another_writer_holds_is_refused_rather_than_overwritten() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("song.jutsu-audio.json");
        ProjectStore::create(&path, "Before".to_string()).expect("create");
        let _held = ProjectLock::acquire(&path).expect("lock");

        let (exit_code, code, _) = apply(&path, rename("After")).expect_err("refused");
        assert_eq!(exit_code, 5);
        assert_eq!(code, "project_locked");
        assert_eq!(
            ProjectStore::open(&path)
                .expect("open")
                .project
                .metadata
                .name,
            "Before"
        );
    }

    #[test]
    fn status_reports_no_owner_when_no_editor_is_running() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("song.jutsu-audio.json");
        ProjectStore::create(&path, "Alone".to_string()).expect("create");

        assert!(status(&path).expect("status").is_none());
    }
}
