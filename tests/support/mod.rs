//! Shared scaffolding for the integration tests: a live editor, the CLI, and
//! the small amount of WAV writing a scenario needs.
//!
//! Compiled into every test binary that declares it, so anything one binary
//! does not use is allowed to sit unused here.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use jutsu_audio::cli;
use jutsu_audio::session_host::SessionHost;
use jutsu_audio_commands::{CommandHistory, ProjectCommandEngine};
use jutsu_audio_model::Project;
use jutsu_audio_project::ProjectStore;
use serde_json::Value;

/// A running editor: owns the project, answers the session socket, and
/// publishes what it holds so a test can look without stopping it.
pub struct Editor {
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<Project>>,
    revision: Arc<Mutex<u64>>,
    path: PathBuf,
    thread: Option<JoinHandle<()>>,
}

impl Editor {
    /// Opens `path` the way the desktop shell does: engine, history, host.
    pub fn open(path: &Path) -> Self {
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
                // Drain what arrived while stopping, so a client waiting on an
                // answer is never left hanging.
                let _ = host.poll(&mut engine, &mut history, false);
            }
        });

        Self {
            stop,
            state,
            revision,
            path: path.to_path_buf(),
            thread: Some(thread),
        }
    }

    #[must_use]
    pub fn project(&self) -> Project {
        self.state.lock().expect("state").clone()
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        *self.revision.lock().expect("revision")
    }

    /// Writes what the editor holds, the way its debounced save does.
    pub fn save(&self) {
        ProjectStore::save(&self.path, &self.project()).expect("save");
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

/// Runs one CLI request in process and returns its exit code and response.
#[must_use]
pub fn call(request: Value) -> (i32, Value) {
    cli::execute_json(&request.to_string())
}

/// The `result` of a request that must succeed.
#[must_use]
pub fn ok(request: Value) -> Value {
    let (code, response) = call(request);
    assert_eq!(code, 0, "request failed: {response}");
    response["result"].clone()
}

#[must_use]
pub fn clip_count(project: &Project) -> usize {
    project
        .tracks
        .iter()
        .flat_map(|track| &track.layers)
        .map(|layer| layer.clips.len())
        .sum()
}

#[must_use]
pub fn clips_on_disk(path: &Path) -> usize {
    clip_count(&ProjectStore::open(path).expect("open").project)
}

/// A short mono ramp at full scale, deterministic sample for sample.
pub fn write_test_wav(path: &Path) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("wav");
    for frame in 0..480_i32 {
        writer
            .write_sample((frame * 16) as i16)
            .expect("write sample");
    }
    writer.finalize().expect("finalize");
}
