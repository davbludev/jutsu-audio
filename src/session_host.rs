//! The editor's side of the session protocol.
//!
//! While the GUI has a project open on disk it is that project's single
//! writer, and this module is how everyone else reaches it. Requests are
//! answered on the UI thread from the same command engine the user's own edits
//! go through — there is no second mutation path.

use std::path::{Path, PathBuf};

use jutsu_audio_commands::{
    COMMAND_PROTOCOL_VERSION, ChangeEvent, CommandEnvelope, CommandError, CommandErrorCode,
    CommandHistory, CommandId, ProjectCommandEngine,
};
use jutsu_audio_session::{
    RequestPayload, ResponsePayload, SessionCall, SessionClient, SessionError, SessionErrorCode,
    SessionResponse, SessionServer, TransportAction,
};

/// What an answered request asks the editor to do next. The host itself only
/// owns the command engine; transport and repaint stay with the app.
#[derive(Clone, Debug, PartialEq)]
pub enum ExternalEffect {
    Applied {
        revision: u64,
        changes: Vec<ChangeEvent>,
    },
    Transport {
        action: TransportAction,
        position_frames: u64,
    },
}

pub struct SessionHost {
    server: SessionServer,
    path: PathBuf,
}

impl SessionHost {
    /// Starts hosting `project_path`. `wake` is called whenever a request
    /// arrives, so an owner that only runs on repaint can be nudged.
    ///
    /// Refuses when another editor already answers for that project: taking it
    /// over would leave two writers, which is exactly what the protocol exists
    /// to prevent.
    pub fn start(
        project_path: &Path,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, String> {
        match SessionClient::attach(project_path) {
            Ok(Some(_)) => {
                return Err("another Jutsu Audio window already owns this project".into());
            }
            Ok(None) => {}
            Err(error) => return Err(error.message),
        }
        let server = SessionServer::start(project_path, wake)
            .map_err(|error| format!("could not host this project: {error}"))?;
        Ok(Self {
            server,
            path: project_path.to_path_buf(),
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Answers every queued request and reports what the editor still has to do
    /// about them. Called once per frame; never blocks.
    pub fn poll(
        &self,
        engine: &mut ProjectCommandEngine,
        history: &mut CommandHistory,
        unsaved: bool,
    ) -> Vec<ExternalEffect> {
        let mut effects = Vec::new();
        while let Some(call) = self.server.try_recv() {
            let (response, effect) = self.answer(&call, engine, history, unsaved);
            call.respond(response);
            if let Some(effect) = effect {
                effects.push(effect);
            }
        }
        effects
    }

    fn answer(
        &self,
        call: &SessionCall,
        engine: &mut ProjectCommandEngine,
        history: &mut CommandHistory,
        unsaved: bool,
    ) -> (SessionResponse, Option<ExternalEffect>) {
        let request = call.request();
        let request_id = request.request_id;
        match &request.payload {
            RequestPayload::Status => (
                SessionResponse::ok(
                    request_id,
                    ResponsePayload::Status {
                        project_path: Some(self.path.display().to_string()),
                        project_name: engine.project().metadata.name.clone(),
                        revision: engine.revision(),
                        unsaved,
                    },
                ),
                None,
            ),
            RequestPayload::Apply {
                expected_revision,
                commands,
            } => {
                let envelope = CommandEnvelope {
                    protocol_version: COMMAND_PROTOCOL_VERSION,
                    // The request ID doubles as the command ID, so one edit can
                    // be followed from the wire into the engine's history.
                    command_id: CommandId::from_uuid(request_id.as_uuid()),
                    expected_revision: expected_revision.unwrap_or_else(|| engine.revision()),
                    commands: commands.clone(),
                };
                // Recorded in the same history as edits made here, so undo
                // reverses whatever happened to the project last.
                match history.apply(engine, envelope) {
                    Ok(outcome) => (
                        SessionResponse::ok(
                            request_id,
                            ResponsePayload::Applied {
                                revision: outcome.revision,
                                changes: outcome.changes.clone(),
                                replayed: false,
                            },
                        ),
                        Some(ExternalEffect::Applied {
                            revision: outcome.revision,
                            changes: outcome.changes,
                        }),
                    ),
                    Err(error) => (
                        SessionResponse::failed(request_id, command_error(&error)),
                        None,
                    ),
                }
            }
            RequestPayload::Transport {
                action,
                position_frames,
            } => (
                SessionResponse::ok(
                    request_id,
                    ResponsePayload::TransportAck {
                        action: *action,
                        position_frames: *position_frames,
                    },
                ),
                Some(ExternalEffect::Transport {
                    action: *action,
                    position_frames: *position_frames,
                }),
            ),
        }
    }
}

/// Command failures cross the wire as structured session errors; a stale writer
/// gets both revisions back so it can re-read and retry.
fn command_error(error: &CommandError) -> SessionError {
    let code = if error.code == CommandErrorCode::RevisionConflict {
        SessionErrorCode::RevisionConflict
    } else {
        SessionErrorCode::CommandFailed
    };
    SessionError {
        code,
        message: error.message.clone(),
        expected_revision: error.expected_revision,
        actual_revision: error.actual_revision,
    }
}

#[cfg(test)]
mod tests {
    use jutsu_audio_model::Project;
    use jutsu_audio_project::ProjectStore;
    use jutsu_audio_session::{RequestId, SessionRequest};

    use super::*;

    /// Runs `client` against a host whose engine is polled between requests,
    /// the way the UI thread polls it between frames.
    fn hosted(
        project: Project,
        exchange: impl FnOnce(&mut SessionClient) + Send + 'static,
    ) -> (ProjectCommandEngine, Vec<ExternalEffect>) {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("song.jutsu-audio.json");
        ProjectStore::save(&path, &project).expect("save");
        let mut engine = ProjectCommandEngine::new(project).expect("engine");
        let mut history = CommandHistory::new();
        let host = SessionHost::start(&path, || {}).expect("host");

        let client_path = path.clone();
        let client = std::thread::spawn(move || {
            let mut client = SessionClient::attach(&client_path)
                .expect("attach")
                .expect("a session is live");
            exchange(&mut client);
        });

        let mut effects = Vec::new();
        while !client.is_finished() {
            effects.extend(host.poll(&mut engine, &mut history, false));
            std::thread::yield_now();
        }
        effects.extend(host.poll(&mut engine, &mut history, false));
        client.join().expect("client thread");
        (engine, effects)
    }

    fn name_command(name: &str) -> jutsu_audio_commands::ProjectCommand {
        jutsu_audio_commands::ProjectCommand::SetProjectName { name: name.into() }
    }

    #[test]
    fn an_external_batch_applies_through_the_same_engine_the_user_edits_with() {
        let project = ProjectStore::new_project("Before");
        let (engine, effects) = hosted(project, |client| {
            let response = client
                .request(RequestPayload::Apply {
                    expected_revision: Some(0),
                    commands: vec![name_command("After")],
                })
                .expect("request");
            let SessionResponse::Ok { payload, .. } = response else {
                panic!("expected the batch to apply, got {response:?}");
            };
            assert!(matches!(
                payload,
                ResponsePayload::Applied {
                    revision: 1,
                    replayed: false,
                    ..
                }
            ));
        });

        assert_eq!(engine.project().metadata.name, "After");
        assert_eq!(engine.revision(), 1);
        assert!(matches!(
            effects.as_slice(),
            [ExternalEffect::Applied { revision: 1, .. }]
        ));
    }

    #[test]
    fn a_repeated_request_id_applies_once_and_replays_its_answer() {
        let project = ProjectStore::new_project("Before");
        let (engine, effects) = hosted(project, |client| {
            let request = SessionRequest {
                protocol_version: jutsu_audio_session::SESSION_PROTOCOL_VERSION,
                token: client.token().to_string(),
                request_id: RequestId::new(),
                payload: RequestPayload::Apply {
                    expected_revision: Some(0),
                    commands: vec![name_command("After")],
                },
            };
            client.send(&request).expect("first send");
            let second = client.send(&request).expect("second send");
            let SessionResponse::Ok { payload, .. } = second else {
                panic!("a replay must succeed, got {second:?}");
            };
            assert!(
                matches!(
                    payload,
                    ResponsePayload::Applied {
                        revision: 1,
                        replayed: true,
                        ..
                    }
                ),
                "got {payload:?}"
            );
        });

        assert_eq!(engine.revision(), 1, "the batch applied exactly once");
        assert_eq!(effects.len(), 1);
    }

    #[test]
    fn a_stale_writer_is_told_both_revisions_and_changes_nothing() {
        let project = ProjectStore::new_project("Before");
        let (engine, effects) = hosted(project, |client| {
            client
                .request(RequestPayload::Apply {
                    expected_revision: Some(0),
                    commands: vec![name_command("First")],
                })
                .expect("first batch");
            let response = client
                .request(RequestPayload::Apply {
                    expected_revision: Some(0),
                    commands: vec![name_command("Stale")],
                })
                .expect("second batch");
            let SessionResponse::Error { error, .. } = response else {
                panic!("a stale revision must be refused, got {response:?}");
            };
            assert_eq!(error.code, SessionErrorCode::RevisionConflict);
            assert_eq!(error.expected_revision, Some(0));
            assert_eq!(error.actual_revision, Some(1));
        });

        assert_eq!(engine.project().metadata.name, "First");
        assert_eq!(engine.revision(), 1);
        assert_eq!(effects.len(), 1, "the refused batch produced no effect");
    }

    #[test]
    fn a_wrong_token_is_refused_before_the_payload_is_looked_at() {
        let project = ProjectStore::new_project("Before");
        let (engine, effects) = hosted(project, |client| {
            let request = SessionRequest {
                protocol_version: jutsu_audio_session::SESSION_PROTOCOL_VERSION,
                token: "not the session token".into(),
                request_id: RequestId::new(),
                payload: RequestPayload::Apply {
                    expected_revision: Some(0),
                    commands: vec![name_command("Intruder")],
                },
            };
            let response = client.send(&request).expect("send");
            let SessionResponse::Error { error, .. } = response else {
                panic!("a bad token must be refused, got {response:?}");
            };
            assert_eq!(error.code, SessionErrorCode::Unauthorized);
        });

        assert_eq!(engine.project().metadata.name, "Before");
        assert_eq!(engine.revision(), 0);
        assert!(effects.is_empty());
    }

    #[test]
    fn status_reports_the_hosted_project_without_touching_it() {
        let project = ProjectStore::new_project("Reported");
        let (engine, effects) = hosted(project, |client| {
            let response = client.request(RequestPayload::Status).expect("request");
            let SessionResponse::Ok { payload, .. } = response else {
                panic!("status must succeed, got {response:?}");
            };
            let ResponsePayload::Status {
                project_name,
                revision,
                unsaved,
                project_path,
            } = payload
            else {
                panic!("expected a status payload, got {payload:?}");
            };
            assert_eq!(project_name, "Reported");
            assert_eq!(revision, 0);
            assert!(!unsaved);
            assert!(project_path.is_some_and(|path| path.ends_with("song.jutsu-audio.json")));
        });

        assert_eq!(engine.revision(), 0);
        assert!(effects.is_empty());
    }

    #[test]
    fn a_second_editor_refuses_to_take_over_a_hosted_project() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("song.jutsu-audio.json");
        ProjectStore::save(&path, &ProjectStore::new_project("Owned")).expect("save");

        let _first = SessionHost::start(&path, || {}).expect("first host");
        let Err(second) = SessionHost::start(&path, || {}) else {
            panic!("a second editor must not take over a hosted project");
        };
        assert!(second.contains("already owns"), "got {second}");
    }
}
