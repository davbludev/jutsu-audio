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
