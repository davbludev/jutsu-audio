//! Two things a script needs that one-request-at-a-time cannot give it:
//! finding out what this build can do, and changing several things at once
//! without leaving a half-edited project behind if step four fails.
//!
//! Both are deliberately dumb. Discovery answers from what the request enum
//! already declares. A batch runs ordinary requests in order and restores the
//! project file if any of them fails — no second execution path, so a batched
//! edit behaves exactly like the same edit typed out one at a time.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use jutsu_audio_session::SessionClient;
use serde_json::{Value, json};

use crate::cli_session::CliFailure;

/// Every operation this build accepts, with the one line of what it does that a
/// caller needs before reading `docs/cli.md`.
///
/// Hand-written, and kept honest by `describe_protocol_lists_every_operation`,
/// which compares it against what the request enum actually accepts.
pub const OPERATIONS: &[(&str, &str)] = &[
    (
        "create_project",
        "Creates a project file with one track and one layer.",
    ),
    (
        "inspect_project",
        "The whole project, plus its default track and layer.",
    ),
    (
        "describe_protocol",
        "This: operations, exit codes, and what else to ask.",
    ),
    (
        "diagnose",
        "Everything a bug report needs about a project file.",
    ),
    (
        "batch",
        "Runs several requests as one all-or-nothing change.",
    ),
    (
        "import_sample",
        "Copies a WAV into the project and adds an asset for it.",
    ),
    ("add_clip", "Places an asset on a track's layer."),
    ("update_clip", "Retimes, trims or re-gains a clip."),
    ("delete_clip", "Removes a clip, optionally closing the gap."),
    (
        "export_stems",
        "Writes one WAV per track from a single render.",
    ),
    ("export_wav", "Renders the master mix to a WAV file."),
    (
        "transport_request",
        "Play, pause, stop or locate the running editor.",
    ),
    (
        "session_status",
        "Whether an editor holds this project, and at what revision.",
    ),
    ("add_track", "Adds a track routed to a bus."),
    ("remove_track", "Removes a track, its layers and its clips."),
    ("add_layer", "Adds a lane to a track."),
    ("set_track_mute", "Mutes or unmutes a track."),
    ("set_track_solo", "Solos or unsolos a track."),
    ("set_clip_pan", "Pans one clip."),
    ("split_clip", "Splits a clip at a frame."),
    ("duplicate_clip", "Copies a clip to another position."),
    (
        "slip_clip",
        "Moves the audio inside a clip without moving the clip.",
    ),
    ("set_clip_fades", "Sets a clip's fade in and out."),
    (
        "crossfade_clips",
        "Fades two overlapping clips into each other.",
    ),
    ("add_marker", "Names a position on the timeline."),
    ("move_marker", "Moves a marker."),
    ("remove_marker", "Removes a marker."),
    (
        "set_loop_region",
        "Sets the loop, for playback and for export.",
    ),
    ("clear_loop_region", "Removes the loop region."),
    (
        "list_extensions",
        "Every synth, effect and generator this build registers.",
    ),
    (
        "add_synth_clip",
        "Adds a synth asset and a clip that plays it.",
    ),
    (
        "set_synth_parameters",
        "Changes a synth asset's parameters.",
    ),
    (
        "set_clip_notes",
        "Replaces the notes on a synth or sampler clip.",
    ),
    (
        "describe_generator",
        "One generator's parameters, ranges and defaults.",
    ),
    (
        "preview_generator",
        "Renders a generator without touching the project.",
    ),
    (
        "run_generator_variations",
        "Runs one recipe as several variations and places them in turn.",
    ),
    (
        "run_generator",
        "Renders a generator into the project as an asset and clip.",
    ),
    ("add_bus", "Adds a mixer bus."),
    ("set_track_output", "Routes a track to a bus."),
    ("set_bus_output", "Routes a bus to another bus."),
    (
        "set_track_parameter",
        "Sets gain, pan, mute or solo on a track.",
    ),
    (
        "set_bus_parameter",
        "Sets gain, pan, mute or solo on a bus.",
    ),
    (
        "describe_strip",
        "The parameters every track and bus strip has.",
    ),
    (
        "bundle_project",
        "Packs the project and its audio into a portable directory.",
    ),
    (
        "check_assets",
        "Every source the project names but cannot read.",
    ),
    (
        "relink_assets",
        "Finds moved audio by fingerprint and repoints the project.",
    ),
    (
        "list_presets",
        "Built-in and user presets, with any incompatibilities.",
    ),
    (
        "save_preset",
        "Captures a synth, instrument or effect rack as a preset.",
    ),
    ("apply_preset", "Puts a preset back onto a target."),
    ("import_preset", "Adds a preset file to the library."),
    ("export_preset", "Writes a preset out as a file."),
    ("add_sampler", "Adds a sampler instrument asset."),
    (
        "set_sampler_zones",
        "Maps samples across pitch and velocity.",
    ),
    ("add_pattern", "Adds a reusable note pattern."),
    ("set_pattern_notes", "Replaces a pattern's notes."),
    ("remove_pattern", "Removes a pattern."),
    (
        "set_clip_pattern",
        "Points a clip at a pattern, or back at its own notes.",
    ),
    ("quantise_clip", "Snaps notes to a musical grid."),
    ("transpose_clip", "Shifts notes in semitones."),
    ("humanise_clip", "Seeded timing and velocity variation."),
    ("loop_clip_notes", "Repeats a clip's notes to fill it."),
    ("set_tempo_map", "Replaces the tempo and signature changes."),
    ("convert_time", "Frames to bars and beats, and back."),
    (
        "describe_effect",
        "One effect's parameters, ranges and defaults.",
    ),
    ("add_effect", "Inserts an effect on a track or bus."),
    (
        "set_track_sends",
        "Replaces a track's parallel sends to buses.",
    ),
    (
        "set_effect_sidechain",
        "Points an insert at a key track, or removes the key.",
    ),
    ("remove_effect", "Removes an effect insert."),
    ("move_effect", "Reorders an effect within its chain."),
    ("set_effect_enabled", "Bypasses or re-enables an effect."),
    ("set_effect_wet", "Sets an effect's wet mix."),
    ("set_effect_parameters", "Changes an effect's parameters."),
    (
        "add_automation_lane",
        "Adds an automation lane for a target parameter.",
    ),
    ("set_automation_points", "Replaces a lane's points."),
    ("remove_automation_lane", "Removes an automation lane."),
];

/// What `describe_protocol` answers. Everything here is fixed for a build, so
/// two calls always agree.
#[must_use]
pub fn describe_protocol(protocol_version: u32) -> Value {
    json!({
        "type": "protocol_described",
        "protocol_version": protocol_version,
        "project_schema_version": jutsu_audio_model::CURRENT_PROJECT_SCHEMA_VERSION,
        "command_protocol_version": jutsu_audio_commands::COMMAND_PROTOCOL_VERSION,
        "session_protocol_version": jutsu_audio_session::SESSION_PROTOCOL_VERSION,
        "operations": OPERATIONS
            .iter()
            .map(|(name, summary)| json!({"name": name, "summary": summary}))
            .collect::<Vec<_>>(),
        "exit_codes": [
            {"code": 0, "meaning": "structured success"},
            {"code": 2, "meaning": "malformed request or unsupported protocol version"},
            {"code": 3, "meaning": "project file, asset or WAV failure"},
            {"code": 4, "meaning": "command validation or entity failure"},
            {"code": 5, "meaning": "a live session or another writer refused the edit"},
        ],
        // What to ask next, rather than repeating those answers here: they
        // depend on which extensions a build registers.
        "discovery": {
            "extensions": "list_extensions",
            "strip_parameters": "describe_strip",
            "effect_parameters": "describe_effect",
            "generator_parameters": "describe_generator",
            "presets": "list_presets",
        },
        "documentation": "docs/cli.md",
    })
}

/// One request inside a batch, and how it went.
struct StepOutcome {
    index: usize,
    operation: String,
    exit_code: i32,
    response: Value,
}

impl StepOutcome {
    fn to_json(&self) -> Value {
        let mut step = json!({
            "index": self.index,
            "operation": self.operation,
            "ok": self.exit_code == 0,
        });
        if self.exit_code == 0 {
            step["result"] = self.response["result"].clone();
        } else {
            step["exit_code"] = json!(self.exit_code);
            step["error"] = self.response["error"].clone();
        }
        step
    }
}

/// Runs `requests` against the project at `path` as one change.
///
/// All of them land or none do: the project file is restored if any step fails,
/// if the deadline passes, or if `dry_run` asked to see the outcome without
/// keeping it. A dry run applies and then restores rather than working on a
/// copy, because a copy would not have the project's assets beside it. `progress` writes one JSON object per step to stderr as it goes,
/// so a caller watching a long batch does not have to wait for the answer.
pub fn run(
    path: &Path,
    requests: &[Value],
    dry_run: bool,
    progress: bool,
    timeout: Option<Duration>,
) -> Result<Value, CliFailure> {
    // A live editor is ahead of the file, so restoring the file would throw
    // away its unsaved work. Batching is for scripts working on their own.
    if matches!(SessionClient::attach(path), Ok(Some(_))) {
        return Err((
            5,
            "session_unavailable",
            "an editor has this project open; send the requests individually so \
             the editor applies them"
                .to_owned(),
        ));
    }

    let before = fs::read(path).map_err(|error| {
        (
            3,
            "project_io_failed",
            format!("batch could not read the project: {error}"),
        )
    })?;

    let started = Instant::now();
    let mut outcomes = Vec::with_capacity(requests.len());
    let mut failure: Option<CliFailure> = None;

    for (index, request) in requests.iter().enumerate() {
        if let Some(timeout) = timeout
            && started.elapsed() > timeout
        {
            failure = Some((
                4,
                "batch_cancelled",
                format!(
                    "batch stopped at step {index} of {}: out of time",
                    requests.len()
                ),
            ));
            break;
        }

        // The batch names the project once; a step that named a different one
        // would edit a file outside the transaction this restores.
        let mut request = request.clone();
        request["protocol_version"] = json!(crate::cli::CLI_PROTOCOL_VERSION);
        request["path"] = json!(path);
        let operation = request["operation"].as_str().unwrap_or("").to_owned();

        let (exit_code, response) = crate::cli::execute_json(&request.to_string());
        let outcome = StepOutcome {
            index,
            operation,
            exit_code,
            response,
        };
        if progress {
            report_progress(&outcome, requests.len());
        }
        let failed = exit_code != 0;
        outcomes.push(outcome);
        if failed {
            failure = Some((
                exit_code,
                "batch_failed",
                format!("batch step {index} failed; no change was kept"),
            ));
            break;
        }
    }

    let steps: Vec<Value> = outcomes.iter().map(StepOutcome::to_json).collect();

    if failure.is_some() || dry_run {
        restore(path, &before)?;
    }

    if let Some((exit_code, code, message)) = failure {
        return Err((exit_code, code, format!("{message}: {}", json!(steps))));
    }

    Ok(json!({
        "type": "batch_applied",
        "dry_run": dry_run,
        "steps": steps,
        "revision": steps
            .iter()
            .rev()
            .find_map(|step| step["result"]["revision"].as_u64()),
    }))
}

/// Puts the project file back as it was. Assets a step copied in are left —
/// unreferenced files are harmless, and deleting files a batch did not create
/// is not.
fn restore(path: &Path, before: &[u8]) -> Result<(), CliFailure> {
    jutsu_audio_project::write_bytes(path, before).map_err(|error| {
        (
            3,
            "project_io_failed",
            format!("batch could not restore the project: {}", error.message),
        )
    })
}

fn report_progress(outcome: &StepOutcome, total: usize) {
    let line = json!({
        "type": "progress",
        "index": outcome.index,
        "total": total,
        "operation": outcome.operation,
        "ok": outcome.exit_code == 0,
    });
    eprintln!("{line}");
}
