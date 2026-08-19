//! Concurrent editor-and-CLI workflows.
//!
//! The editor is stood up here the same way the desktop shell does it: a
//! command engine, a history, and a [`SessionHost`] polled in a loop. The CLI
//! side runs the real request handler in-process. What is being tested is the
//! seam between them — attach, conflict, replay, disconnect, crash, fallback —
//! not the UI.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use jutsu_audio::cli;
use jutsu_audio::session_host::SessionHost;
use jutsu_audio_commands::{CommandHistory, ProjectCommandEngine};
use jutsu_audio_model::Project;
use jutsu_audio_project::ProjectStore;
use jutsu_audio_session::{
    RequestPayload, ResponsePayload, SessionClient, SessionDescriptor, SessionErrorCode,
    SessionResponse,
};
use serde_json::{Value, json};
use tempfile::TempDir;

/// A running editor: owns the project, answers the socket, and publishes what
/// it currently holds so a test can look at it without stopping it.
struct Editor {
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<Project>>,
    revision: Arc<Mutex<u64>>,
    thread: Option<JoinHandle<()>>,
}

impl Editor {
    fn open(path: &Path) -> Self {
        let project = ProjectStore::open(path).expect("open").project;
        let state = Arc::new(Mutex::new(project.clone()));
        let revision = Arc::new(Mutex::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let host = SessionHost::start(path, || {}).expect("host");

        let thread = std::thread::spawn({
            let state = Arc::clone(&state);
            let revision = Arc::clone(&revision);
            let stop = Arc::clone(&stop);
            move || {
                let mut engine = ProjectCommandEngine::new(project).expect("engine");
                let mut history = CommandHistory::new();
                while !stop.load(Ordering::Acquire) {
                    if !host.poll(&mut engine, &mut history, false).is_empty() {
                        *state.lock().expect("state") = engine.project().clone();
                        *revision.lock().expect("revision") = engine.revision();
                    }
                    std::thread::yield_now();
                }
                // Drain anything that arrived while stopping, so a client
                // waiting on an answer is never left hanging.
                let _ = host.poll(&mut engine, &mut history, false);
            }
        });

        Self {
            stop,
            state,
            revision,
            thread: Some(thread),
        }
    }

    fn project(&self) -> Project {
        self.state.lock().expect("state").clone()
    }

    fn revision(&self) -> u64 {
        *self.revision.lock().expect("revision")
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Fixture {
    _directory: TempDir,
    path: PathBuf,
    asset: Value,
    track: Value,
    layer: Value,
}

/// A saved project with one imported sample, which is the least a clip needs.
fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("agent.jutsu-audio.json");
    let source = directory.path().join("blip.wav");
    write_test_wav(&source);

    let created = ok(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Concurrent"
    }));
    let imported = ok(json!({
        "protocol_version": 1,
        "operation": "import_sample",
        "path": path,
        "source": source
    }));

    Fixture {
        _directory: directory,
        path,
        asset: imported["asset_id"].clone(),
        track: created["track_id"].clone(),
        layer: created["layer_id"].clone(),
    }
}

impl Fixture {
    fn add_clip_request(&self, start_sample: u64) -> Value {
        json!({
            "protocol_version": 1,
            "operation": "add_clip",
            "path": self.path,
            "asset_id": self.asset,
            "track_id": self.track,
            "layer_id": self.layer,
            "start_sample": start_sample,
            "source_start_sample": 0,
            "duration_samples": 480
        })
    }
}

fn call(request: Value) -> (i32, Value) {
    cli::execute_json(&request.to_string())
}

/// The `result` of a request that must succeed.
fn ok(request: Value) -> Value {
    let (code, response) = call(request);
    assert_eq!(code, 0, "request failed: {response}");
    response["result"].clone()
}

fn clip_count(project: &Project) -> usize {
    project
        .tracks
        .iter()
        .flat_map(|track| &track.layers)
        .map(|layer| layer.clips.len())
        .sum()
}

fn clips_on_disk(path: &Path) -> usize {
    clip_count(&ProjectStore::open(path).expect("open").project)
}

#[test]
fn the_cli_attaches_to_a_live_editor_and_leaves_the_file_to_it() {
    let fixture = fixture();
    let editor = Editor::open(&fixture.path);

    let status = ok(json!({
        "protocol_version": 1,
        "operation": "session_status",
        "path": fixture.path
    }));
    assert_eq!(status["attached"], true);
    assert_eq!(status["session"]["revision"], 0);

    let added = ok(fixture.add_clip_request(0));
    assert_eq!(added["delivery"], "session");
    assert_eq!(added["revision"], 1);

    assert_eq!(
        clip_count(&editor.project()),
        1,
        "the editor holds the clip"
    );
    assert_eq!(
        clips_on_disk(&fixture.path),
        0,
        "an attached edit must not write the file behind the editor"
    );
}

#[test]
fn rapid_edits_from_two_clients_all_land_without_losing_updates() {
    let fixture = fixture();
    let editor = Editor::open(&fixture.path);

    const PER_CLIENT: u64 = 12;
    let clients: Vec<_> = (0..2_u64)
        .map(|client| {
            let requests: Vec<Value> = (0..PER_CLIENT)
                .map(|index| fixture.add_clip_request(1_000 * (client * PER_CLIENT + index)))
                .collect();
            std::thread::spawn(move || {
                for request in requests {
                    let (code, response) = call(request);
                    assert_eq!(code, 0, "edit failed: {response}");
                    assert_eq!(response["result"]["delivery"], "session");
                }
            })
        })
        .collect();
    for client in clients {
        client.join().expect("client thread");
    }

    let expected = usize::try_from(PER_CLIENT * 2).expect("small");
    assert_eq!(clip_count(&editor.project()), expected);
    assert_eq!(
        editor.revision(),
        PER_CLIENT * 2,
        "every batch committed exactly one revision"
    );
}

#[test]
fn a_stale_expected_revision_is_refused_and_leaves_the_project_alone() {
    let fixture = fixture();
    let editor = Editor::open(&fixture.path);
    ok(fixture.add_clip_request(0));

    let mut client = SessionClient::attach(&fixture.path)
        .expect("attach")
        .expect("a session is live");
    let response = client
        .request(RequestPayload::Apply {
            expected_revision: Some(0),
            commands: vec![jutsu_audio_commands::ProjectCommand::SetProjectName {
                name: "Stale".into(),
            }],
        })
        .expect("request");

    let SessionResponse::Error { error, .. } = response else {
        panic!("a stale revision must be refused, got {response:?}");
    };
    assert_eq!(error.code, SessionErrorCode::RevisionConflict);
    assert_eq!(error.expected_revision, Some(0));
    assert_eq!(error.actual_revision, Some(1));
    assert_eq!(editor.project().metadata.name, "Concurrent");
    assert_eq!(editor.revision(), 1);
}

#[test]
fn a_client_that_disconnects_mid_session_does_not_disturb_the_editor() {
    let fixture = fixture();
    let editor = Editor::open(&fixture.path);

    {
        let mut client = SessionClient::attach(&fixture.path)
            .expect("attach")
            .expect("a session is live");
        client.request(RequestPayload::Status).expect("status");
        // Dropped mid-conversation, exactly like a killed script.
    }

    let added = ok(fixture.add_clip_request(0));
    assert_eq!(added["delivery"], "session");
    assert_eq!(clip_count(&editor.project()), 1);
}

#[test]
fn a_session_file_left_by_a_crashed_editor_falls_back_to_an_offline_edit() {
    let fixture = fixture();
    // A port nobody listens on: what a crashed editor leaves behind, since a
    // clean exit removes the session file itself.
    let dead_port = {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        listener.local_addr().expect("addr").port()
    };
    let published = SessionDescriptor::new(&fixture.path, dead_port, "stale-token")
        .publish()
        .expect("publish");
    // The guard would delete the file on drop; this session is meant to look
    // abandoned, so let it leak for the length of the test.
    std::mem::forget(published);

    let added = ok(fixture.add_clip_request(0));
    assert_eq!(
        added["delivery"], "offline",
        "a dead session must not block editing"
    );
    assert_eq!(clips_on_disk(&fixture.path), 1);
    assert!(
        SessionDescriptor::read(&fixture.path).is_none(),
        "the stale session file is cleaned up rather than left to confuse the next client"
    );
}

#[test]
fn offline_writers_serialize_on_the_project_lock_and_keep_both_edits() {
    let fixture = fixture();
    let first = fixture.add_clip_request(0);
    let second = fixture.add_clip_request(5_000);

    let writers: Vec<_> = [first, second]
        .into_iter()
        .map(|request| {
            std::thread::spawn(move || {
                let (code, response) = call(request);
                assert_eq!(code, 0, "offline edit failed: {response}");
                assert_eq!(response["result"]["delivery"], "offline");
            })
        })
        .collect();
    for writer in writers {
        writer.join().expect("writer thread");
    }

    assert_eq!(
        clips_on_disk(&fixture.path),
        2,
        "a read-modify-write under the lock cannot drop the other writer's clip"
    );
}

#[test]
fn transport_reaches_a_live_editor_and_is_acknowledged_offline_otherwise() {
    let fixture = fixture();

    let offline = ok(json!({
        "protocol_version": 1,
        "operation": "transport_request",
        "path": fixture.path,
        "action": "play"
    }));
    assert_eq!(offline["delivery"], "offline");

    let editor = Editor::open(&fixture.path);
    let live = ok(json!({
        "protocol_version": 1,
        "operation": "transport_request",
        "path": fixture.path,
        "action": "seek",
        "position_frames": 240
    }));
    assert_eq!(live["delivery"], "session");
    drop(editor);
}

#[test]
fn a_replayed_request_id_is_answered_once_by_the_editor() {
    let fixture = fixture();
    let editor = Editor::open(&fixture.path);

    let mut client = SessionClient::attach(&fixture.path)
        .expect("attach")
        .expect("a session is live");
    let request = jutsu_audio_session::SessionRequest::new(
        client.token().to_string(),
        RequestPayload::Apply {
            expected_revision: Some(0),
            commands: vec![jutsu_audio_commands::ProjectCommand::SetProjectName {
                name: "Once".into(),
            }],
        },
    );
    client.send(&request).expect("first send");
    let replay = client.send(&request).expect("replayed send");

    let SessionResponse::Ok { payload, .. } = replay else {
        panic!("a replay must succeed, got {replay:?}");
    };
    assert!(
        matches!(payload, ResponsePayload::Applied { replayed: true, .. }),
        "got {payload:?}"
    );
    assert_eq!(editor.revision(), 1, "the batch applied exactly once");
    assert_eq!(editor.project().metadata.name, "Once");
}

fn write_test_wav(path: &Path) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("wav");
    for frame in 0..480_i32 {
        writer.write_sample((frame * 16) as i16).expect("sample");
    }
    writer.finalize().expect("finalize");
}
