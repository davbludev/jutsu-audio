//! The MCP surface, driven the way a client drives it: one process, one open
//! connection, several messages in order.
//!
//! What matters here is the handshake, the silence a notification is owed, and
//! that a tool call reaches the same command engine every other surface uses.
//! The operations themselves are `cli_protocol.rs`'s job — this file must not
//! grow a second copy of them.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use tempfile::tempdir;

/// Runs one session and returns the messages that came back, in order.
fn session(messages: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_jutsu-audio-cli"))
        .arg("--mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    for message in messages {
        writeln!(stdin, "{message}").unwrap();
    }
    // End of file is how a client hangs up, and the server must exit on it
    // rather than waiting for a shutdown message it will never get.
    drop(stdin);

    let stdout = BufReader::new(child.stdout.take().unwrap());
    let answers = stdout
        .lines()
        .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
        .collect();
    assert!(child.wait().unwrap().success());
    answers
}

#[test]
fn a_session_handshakes_lists_its_tools_and_edits_a_project() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("mcp.jutsu-audio.json");

    let answers = session(&[
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
               "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                          "clientInfo": {"name": "test", "version": "0"}}}),
        // A notification is owed silence, so the next answer must be id 2.
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
               "params": {"name": "jutsu_audio_request",
                          "arguments": {"request": {"operation": "create_project",
                                                    "path": path, "name": "From MCP"}}}}),
    ]);

    let ids: Vec<&Value> = answers.iter().map(|answer| &answer["id"]).collect();
    assert_eq!(ids, vec![&json!(1), &json!(2), &json!(3)]);

    assert_eq!(answers[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(answers[0]["result"]["serverInfo"]["name"], "jutsu-audio");
    assert!(answers[0]["result"]["capabilities"]["tools"].is_object());

    let names: Vec<&str> = answers[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["jutsu_audio_describe", "jutsu_audio_request"]);

    assert_eq!(answers[2]["result"]["isError"], false);
    let text = answers[2]["result"]["content"][0]["text"].as_str().unwrap();
    let response: Value = serde_json::from_str(text).unwrap();
    assert_eq!(response["ok"], true);
    // The project is on disk, not merely reported: this went through the same
    // command engine and store as every other surface.
    assert!(path.exists());
}

#[test]
fn a_refused_request_comes_back_as_a_tool_error_and_changes_nothing() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("missing.jutsu-audio.json");

    let answers = session(&[json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "jutsu_audio_request",
                   "arguments": {"request": {"operation": "inspect_project", "path": path}}}})]);

    assert_eq!(answers[0]["result"]["isError"], true);
    let text = answers[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("\"ok\": false"), "{text}");
    assert!(!path.exists(), "a failed read must not create anything");
}

#[test]
fn the_protocol_says_no_rather_than_guessing() {
    let answers = session(&[
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "nope"}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "resources/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
               "params": {"name": "jutsu_audio_request", "arguments": {}}}),
        json!({"jsonrpc": "2.0", "id": 4, "method": "ping"}),
    ]);

    assert_eq!(answers[0]["error"]["code"], -32602);
    assert_eq!(answers[1]["error"]["code"], -32601);
    assert_eq!(answers[2]["error"]["code"], -32602);
    assert!(answers[3]["result"].is_object(), "ping is answered");
}
