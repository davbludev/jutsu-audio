use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use tempfile::tempdir;

fn invoke(request: Value) -> (i32, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_jutsu-audio-cli"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    (
        output.status.code().unwrap(),
        serde_json::from_slice(&output.stdout).unwrap(),
    )
}

#[test]
fn create_and_inspect_return_stable_structured_results() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("agent.jutsu-audio.json");
    let (code, created) = invoke(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Agent SFX"
    }));
    assert_eq!(code, 0);
    assert_eq!(created["ok"], true);
    assert_eq!(created["result"]["type"], "project_created");
    assert!(created["result"]["project_id"].as_str().is_some());

    let (code, inspected) = invoke(json!({
        "protocol_version": 1,
        "operation": "inspect_project",
        "path": path
    }));
    assert_eq!(code, 0);
    assert_eq!(inspected["result"]["type"], "project_inspected");
    assert_eq!(
        inspected["result"]["project"]["metadata"]["name"],
        "Agent SFX"
    );
}

#[test]
fn malformed_requests_return_json_error_and_documented_exit_code() {
    let (code, response) = invoke(json!({"protocol_version": 99, "operation": "inspect_project"}));
    assert_eq!(code, 2);
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "invalid_request");
    assert!(response["error"]["message"].as_str().is_some());
}

#[test]
fn edits_report_the_route_they_took_and_the_revision_they_produced() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("agent.jutsu-audio.json");
    let source = directory.path().join("blip.wav");
    write_test_wav(&source);
    invoke(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Agent SFX"
    }));

    let (code, imported) = invoke(json!({
        "protocol_version": 1,
        "operation": "import_sample",
        "path": path,
        "source": source
    }));
    assert_eq!(code, 0, "{imported}");
    assert_eq!(imported["result"]["status"], "added");
    assert_eq!(imported["result"]["delivery"], "offline");
    assert_eq!(imported["result"]["revision"], 1);
}

#[test]
fn session_status_reports_that_no_editor_owns_the_project() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("agent.jutsu-audio.json");
    invoke(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Agent SFX"
    }));

    let (code, status) = invoke(json!({
        "protocol_version": 1,
        "operation": "session_status",
        "path": path
    }));
    assert_eq!(code, 0);
    assert_eq!(status["result"]["type"], "session_status");
    assert_eq!(status["result"]["attached"], false);
    assert!(status["result"]["session"].is_null());
}

fn write_test_wav(path: &std::path::Path) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for frame in 0..480_i32 {
        writer.write_sample((frame * 16) as i16).unwrap();
    }
    writer.finalize().unwrap();
}
