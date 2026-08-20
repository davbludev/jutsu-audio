//! The clean-machine check: does a packaged release actually work?
//!
//! ```bash
//! cargo smoke dist/jutsu-audio-0.1.0-x86_64-pc-windows-msvc
//! ```
//!
//! It runs the *packaged* binaries — not the ones in `target/` — the way a user
//! who just unpacked the download would: ask what the tool can do, make a
//! project, put a generated sound on the timeline, export it, and read the WAV
//! back. Nothing here touches the repository, so the same command is what a
//! release manager runs on a machine that has never built this.
//!
//! Playback is not covered: a clean machine may genuinely have no audio device,
//! and refusing to ship over that would be wrong. Export is the part that must
//! work everywhere.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Runs the checks against the release directory at `directory`.
///
/// # Errors
///
/// Returns the first check that failed, in the words needed to act on it.
pub fn run_smoke(directory: &Path) -> Result<(), String> {
    let cli = executable(directory, "jutsu-audio-cli");
    if !cli.exists() {
        return Err(format!("{} is not in the release", cli.display()));
    }
    for expected in ["INSTALL.md", "THIRD-PARTY-NOTICES.md", "SHA256SUMS"] {
        if !directory.join(expected).exists() {
            return Err(format!("{expected} is missing from the release"));
        }
    }

    // A user's first move after putting it on their PATH.
    let version = run(&cli, &["--version"], "")?;
    if !version.contains("jutsu-audio-cli") {
        return Err(format!("--version said something unexpected: {version}"));
    }

    let scratch = directory.join("smoke-scratch");
    if scratch.exists() {
        fs::remove_dir_all(&scratch).map_err(|error| format!("clearing scratch: {error}"))?;
    }
    fs::create_dir_all(&scratch).map_err(|error| format!("creating scratch: {error}"))?;
    let project = scratch.join("smoke.jutsu-audio.json");
    let output = scratch.join("smoke.wav");

    let protocol = request(
        &cli,
        &json_object(&[("operation", "\"describe_protocol\"")]),
    )?;
    if !protocol.contains("describe_protocol") {
        return Err("describe_protocol did not list itself".into());
    }

    let created = request(
        &cli,
        &json_object(&[
            ("operation", "\"create_project\""),
            ("path", &quote(&project)),
            ("name", "\"Smoke\""),
        ]),
    )?;
    let track_id = field(&created, "track_id")?;
    let layer_id = field(&created, "layer_id")?;

    request(
        &cli,
        &json_object(&[
            ("operation", "\"run_generator\""),
            ("path", &quote(&project)),
            ("type_id", "\"sfx.impact\""),
            ("seed", "1"),
            ("frame_count", "24000"),
            ("track_id", &format!("\"{track_id}\"")),
            ("layer_id", &format!("\"{layer_id}\"")),
        ]),
    )?;

    let exported = request(
        &cli,
        &json_object(&[
            ("operation", "\"export_wav\""),
            ("path", &quote(&project)),
            ("output", &quote(&output)),
            ("encoding", "\"float32\""),
        ]),
    )?;
    if !exported.contains("wav_exported") {
        return Err(format!("the export did not report success: {exported}"));
    }
    let written =
        fs::metadata(&output).map_err(|error| format!("the export is missing: {error}"))?;
    if written.len() < 1_000 {
        return Err(format!("the exported WAV is only {} bytes", written.len()));
    }

    fs::remove_dir_all(&scratch).map_err(|error| format!("clearing scratch: {error}"))?;
    eprintln!("smoke checks passed for {}", directory.display());
    Ok(())
}

fn executable(directory: &Path, name: &str) -> PathBuf {
    let name = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    directory.join(name)
}

/// One CLI request, with the protocol version the packaged build declares.
fn request(cli: &Path, body: &str) -> Result<String, String> {
    let response = run(cli, &[], body)?;
    if response.contains("\"ok\":false") {
        return Err(format!("request failed: {response}"));
    }
    Ok(response)
}

fn run(program: &Path, arguments: &[&str], stdin: &str) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("running {}: {error}", program.display()))?;
    child
        .stdin
        .take()
        .ok_or("no stdin")?
        .write_all(stdin.as_bytes())
        .map_err(|error| format!("writing to {}: {error}", program.display()))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("waiting for {}: {error}", program.display()))?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn json_object(fields: &[(&str, &str)]) -> String {
    let body: Vec<String> = std::iter::once("\"protocol_version\":1".to_owned())
        .chain(
            fields
                .iter()
                .map(|(name, value)| format!("\"{name}\":{value}")),
        )
        .collect();
    format!("{{{}}}", body.join(","))
}

/// A path as a JSON string. Backslashes are the only thing a Windows path
/// brings that JSON minds.
fn quote(path: &Path) -> String {
    format!("\"{}\"", path.display().to_string().replace('\\', "\\\\"))
}

/// Pulls a string field out of a response without a JSON parser: the smoke test
/// deliberately depends on as little as possible.
fn field(response: &str, name: &str) -> Result<String, String> {
    let key = format!("\"{name}\":\"");
    let start = response
        .find(&key)
        .ok_or_else(|| format!("no {name} in {response}"))?
        + key.len();
    let rest = &response[start..];
    let end = rest
        .find('"')
        .ok_or_else(|| format!("unterminated {name} in {response}"))?;
    Ok(rest[..end].to_owned())
}
