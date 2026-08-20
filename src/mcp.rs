//! The MCP surface: the same structured CLI, spoken over a connection that
//! stays open.
//!
//! `jutsu-audio-cli --mcp` serves the Model Context Protocol on stdin and
//! stdout — newline-delimited JSON-RPC, one message per line. It adds no
//! operations of its own. Every request is handed to `cli::execute_json`, so an
//! agent editing a project through MCP and a script piping JSON into the same
//! binary go through one code path, one session router and one command engine.
//!
//! Two tools rather than sixty: the protocol already describes itself, so
//! `describe_protocol` is the discovery surface and everything else is one
//! request object. A tool per operation would be a second copy of the operation
//! table, and the copy would drift.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

/// The revision of MCP this speaks. A client asking for a different one is
/// answered in its own version when we can, because the handshake is the one
/// message where disagreeing is fatal.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// Serves MCP until `input` reaches end of file.
///
/// # Errors
///
/// Fails only if writing a response fails; a malformed message is answered
/// with a JSON-RPC error rather than ending the session.
pub fn serve(input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Some(response) = handle_line(&line) else {
            continue;
        };
        writeln!(output, "{response}")?;
        output.flush()?;
    }
    Ok(())
}

/// One incoming line to at most one outgoing message. `None` is a
/// notification: the protocol forbids answering those.
#[must_use]
pub fn handle_line(line: &str) -> Option<Value> {
    let message: Value = match serde_json::from_str(line) {
        Ok(message) => message,
        Err(error) => return Some(error_for(&Value::Null, -32700, error.to_string())),
    };

    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    if id.is_null() {
        // A notification. `notifications/initialized` is the one that matters
        // and it wants silence, not an acknowledgement.
        return None;
    }
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));

    Some(match method {
        "initialize" => result_for(&id, initialize(&params)),
        "ping" => result_for(&id, json!({})),
        "tools/list" => result_for(&id, json!({ "tools": tools() })),
        "tools/call" => match call_tool(&params) {
            Ok(value) => result_for(&id, value),
            Err(message) => error_for(&id, -32602, message),
        },
        other => error_for(&id, -32601, format!("unknown method '{other}'")),
    })
}

fn initialize(params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(PROTOCOL_VERSION);
    json!({
        "protocolVersion": requested,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "jutsu-audio", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Call jutsu_audio_describe first: it lists every operation this \
                         build accepts, with the exit codes and the discovery operations for \
                         the parts that depend on which extensions are registered. Then send \
                         those operations through jutsu_audio_request. Editing needs no audio \
                         device, and exporting a WAV does not either.",
    })
}

fn tools() -> Value {
    json!([
        {
            "name": "jutsu_audio_describe",
            "title": "Describe the Jutsu Audio protocol",
            "description": "Lists every operation this build of Jutsu Audio accepts, the \
                            protocol and schema versions it speaks, the exit codes and what \
                            they mean. Takes no arguments and touches no project. Call this \
                            before the first edit of a session.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "jutsu_audio_request",
            "title": "Run one Jutsu Audio request",
            "description": "Runs one structured request against a Jutsu Audio project and \
                            returns the response envelope. `operation` names what to do and \
                            `path` names the project file; both come from \
                            jutsu_audio_describe. An editor with the project open is detected \
                            and the edit is routed through it, so the running window and this \
                            tool never disagree. Every edit is atomic: a refused one changes \
                            nothing.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "request": {
                        "type": "object",
                        "description": "The request object, exactly as docs/cli.md describes \
                                        it. `protocol_version` is filled in when absent.",
                    },
                },
                "required": ["request"],
            },
        },
    ])
}

fn call_tool(params: &Value) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or("tools/call needs a tool name")?;

    let request = match name {
        "jutsu_audio_describe" => json!({"protocol_version": 1, "operation": "describe_protocol"}),
        "jutsu_audio_request" => {
            let mut request = params
                .get("arguments")
                .and_then(|arguments| arguments.get("request"))
                .filter(|request| request.is_object())
                .cloned()
                .ok_or("jutsu_audio_request needs a `request` object")?;
            // The protocol version is this build's business, not the caller's.
            if let Some(object) = request.as_object_mut() {
                object.entry("protocol_version").or_insert_with(|| json!(1));
            }
            request
        }
        other => return Err(format!("unknown tool '{other}'")),
    };

    let (exit_code, response) = crate::cli::execute_json(&request.to_string());
    Ok(json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&response)
            .unwrap_or_else(|_| response.to_string()) }],
        // A refused edit is the tool reporting a result, not the transport
        // failing, so it comes back as an error the caller can read and retry.
        "isError": exit_code != 0,
    }))
}

fn result_for(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_for(id: &Value, code: i32, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}
