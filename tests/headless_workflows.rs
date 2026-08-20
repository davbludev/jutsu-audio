//! The workflow a script runs: find out what the build can do, then change
//! several things at once and know exactly what happened.
//!
//! No GUI, no screen scraping, no prose to interpret. Every answer here is a
//! field a program can branch on.

mod support;

use std::fs;

use jutsu_audio_project::ProjectStore;
use serde_json::{Value, json};
use support::{Editor, call, ok, write_test_wav};

/// The operation names the request enum actually accepts, taken from what serde
/// says when it is handed one it does not know.
fn accepted_operations() -> Vec<String> {
    let (_, response) = call(json!({"protocol_version": 1, "operation": "no_such_operation"}));
    let message = response["error"]["message"]
        .as_str()
        .expect("an unknown operation is rejected by name");
    message
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|name| *name != "no_such_operation")
        .map(str::to_owned)
        .collect()
}

#[test]
fn describe_protocol_lists_every_operation_the_build_accepts() {
    let described = ok(json!({"protocol_version": 1, "operation": "describe_protocol"}));

    let mut documented: Vec<String> = described["operations"]
        .as_array()
        .expect("operations")
        .iter()
        .map(|operation| {
            let name = operation["name"].as_str().expect("name").to_owned();
            assert!(
                operation["summary"]
                    .as_str()
                    .is_some_and(|summary| !summary.is_empty()),
                "{name} has no summary"
            );
            name
        })
        .collect();
    let mut accepted = accepted_operations();
    documented.sort();
    accepted.sort();
    assert_eq!(
        documented, accepted,
        "describe_protocol must list exactly what the CLI accepts"
    );

    assert_eq!(described["protocol_version"], 1);
    assert!(
        described["exit_codes"]
            .as_array()
            .expect("exit codes")
            .len()
            >= 5
    );
    // The parts that depend on which extensions a build has are named, not
    // guessed at: a caller asks those operations rather than assuming.
    assert_eq!(described["discovery"]["extensions"], "list_extensions");
}

#[test]
fn describe_protocol_answers_the_same_thing_twice() {
    let first = ok(json!({"protocol_version": 1, "operation": "describe_protocol"}));
    let second = ok(json!({"protocol_version": 1, "operation": "describe_protocol"}));
    assert_eq!(first, second, "discovery carries nothing time-dependent");
}

#[test]
fn a_batch_lands_as_one_change_and_reports_every_step() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("cue.jutsu-audio.json");
    let source = directory.path().join("blip.wav");
    write_test_wav(&source);

    let created = ok(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Cue"
    }));
    let imported = ok(json!({
        "protocol_version": 1,
        "operation": "import_sample",
        "path": path,
        "source": source
    }));

    let applied = ok(json!({
        "protocol_version": 1,
        "operation": "batch",
        "path": path,
        "requests": [
            {"operation": "add_track", "name": "Layers"},
            {
                "operation": "add_clip",
                "asset_id": imported["asset_id"],
                "track_id": created["track_id"],
                "layer_id": created["layer_id"],
                "start_sample": 0,
                "source_start_sample": 0,
                "duration_samples": 480
            },
            {
                "operation": "set_track_parameter",
                "track_id": created["track_id"],
                "key": "gain_db",
                "value": {"type": "float", "value": -6.0}
            }
        ]
    }));

    assert_eq!(applied["dry_run"], false);
    let steps = applied["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 3);
    for (index, step) in steps.iter().enumerate() {
        assert_eq!(step["index"], index);
        assert_eq!(step["ok"], true, "{step}");
        assert!(step["result"].is_object(), "every step answers: {step}");
    }
    assert_eq!(
        applied["revision"], steps[2]["result"]["revision"],
        "the batch reports the revision it left the project at"
    );

    let project = ProjectStore::open(&path).expect("open").project;
    assert_eq!(project.tracks.len(), 2);
    assert_eq!(support::clip_count(&project), 1);
}

#[test]
fn a_batch_that_fails_partway_leaves_the_project_exactly_as_it_was() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("cue.jutsu-audio.json");
    let created = ok(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Cue"
    }));
    let before = fs::read(&path).expect("read");

    let (code, response) = call(json!({
        "protocol_version": 1,
        "operation": "batch",
        "path": path,
        "requests": [
            {"operation": "add_track", "name": "First"},
            {"operation": "add_track", "name": "Second"},
            {
                "operation": "set_clip_pan",
                "clip_id": "00000000-0000-4000-8000-000000000000",
                "pan": 0.5
            },
            {"operation": "add_track", "name": "Never runs"}
        ]
    }));

    assert_ne!(code, 0, "{response}");
    assert_eq!(response["error"]["code"], "batch_failed");
    let message = response["error"]["message"].as_str().expect("message");
    assert!(
        message.contains("step 2") && message.contains("no change was kept"),
        "the failing step is named: {message}"
    );
    assert!(
        message.contains("set_clip_pan"),
        "and the steps that ran are reported: {message}"
    );

    assert_eq!(
        fs::read(&path).expect("read"),
        before,
        "the project is byte for byte what it was before the batch"
    );
    assert_eq!(
        ProjectStore::open(&path)
            .expect("open")
            .project
            .tracks
            .len(),
        1,
        "including the two tracks that had already been added"
    );
    // The project it created is still usable afterwards.
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "add_track",
        "path": path,
        "name": "After"
    }));
    assert_eq!(created["path"], serde_json::to_value(&path).expect("path"));
}

#[test]
fn a_dry_run_reports_what_would_happen_and_keeps_none_of_it() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("cue.jutsu-audio.json");
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Cue"
    }));
    let before = fs::read(&path).expect("read");

    let applied = ok(json!({
        "protocol_version": 1,
        "operation": "batch",
        "path": path,
        "dry_run": true,
        "requests": [
            {"operation": "add_track", "name": "Would be added"},
            {"operation": "add_bus", "name": "Would be added too"}
        ]
    }));
    assert_eq!(applied["dry_run"], true);
    assert_eq!(applied["steps"].as_array().expect("steps").len(), 2);
    assert!(
        applied["steps"][0]["result"]["track_id"].is_string(),
        "a dry run still says what the edit would produce: {applied}"
    );

    assert_eq!(
        fs::read(&path).expect("read"),
        before,
        "and the project is untouched"
    );
}

#[test]
fn a_batch_that_runs_out_of_time_rolls_back_and_says_so() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("cue.jutsu-audio.json");
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Cue"
    }));
    let before = fs::read(&path).expect("read");

    // Zero milliseconds: the deadline has passed before the first step, which
    // is the same code path a caller uses to cut a long batch short.
    let (code, response) = call(json!({
        "protocol_version": 1,
        "operation": "batch",
        "path": path,
        "timeout_ms": 0,
        "requests": [
            {"operation": "add_track", "name": "Too late"},
            {"operation": "add_track", "name": "Also too late"}
        ]
    }));
    assert_ne!(code, 0, "{response}");
    assert_eq!(response["error"]["code"], "batch_cancelled");
    assert_eq!(fs::read(&path).expect("read"), before);
}

#[test]
fn a_batch_refuses_to_run_behind_a_live_editor() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("cue.jutsu-audio.json");
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Cue"
    }));

    let editor = Editor::open(&path);
    let (code, response) = call(json!({
        "protocol_version": 1,
        "operation": "batch",
        "path": path,
        "requests": [{"operation": "add_track", "name": "Behind the editor"}]
    }));
    assert_eq!(code, 5, "{response}");
    assert_eq!(response["error"]["code"], "session_unavailable");
    assert!(
        response["error"]["message"]
            .as_str()
            .expect("message")
            .contains("individually"),
        "and says what to do instead: {response}"
    );

    // Sent one at a time, the same edit lands in the editor.
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "add_track",
        "path": path,
        "name": "Through the editor"
    }));
    assert_eq!(editor.revision(), 1);
    drop(editor);
}

#[test]
fn a_batch_step_cannot_reach_a_different_project() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("cue.jutsu-audio.json");
    let other = directory.path().join("other.jutsu-audio.json");
    for target in [&path, &other] {
        let _ = ok(json!({
            "protocol_version": 1,
            "operation": "create_project",
            "path": target,
            "name": "Cue"
        }));
    }
    let other_before = fs::read(&other).expect("read");

    // A step naming another project is redirected at the batch's own project:
    // a transaction can only roll back what it owns.
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "batch",
        "path": path,
        "requests": [{"operation": "add_track", "path": other, "name": "Redirected"}]
    }));

    assert_eq!(fs::read(&other).expect("read"), other_before);
    assert_eq!(
        ProjectStore::open(&path)
            .expect("open")
            .project
            .tracks
            .len(),
        2
    );
}

#[test]
fn a_representative_script_works_from_discovery_alone() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("cue.jutsu-audio.json");
    let output = directory.path().join("cue.wav");

    // 1. What can this build do?
    let protocol = ok(json!({"protocol_version": 1, "operation": "describe_protocol"}));
    let has = |name: &str| {
        protocol["operations"]
            .as_array()
            .expect("operations")
            .iter()
            .any(|operation| operation["name"] == name)
    };
    assert!(has("run_generator") && has("add_effect") && has("export_wav"));

    // 2. Which generators and effects, specifically?
    let extensions = ok(json!({"protocol_version": 1, "operation": "list_extensions"}));
    let generator = extensions["extensions"]["generators"]
        .as_array()
        .expect("generators")
        .first()
        .expect("at least one generator")["type_id"]
        .clone();
    let effect = extensions["extensions"]["effects"]
        .as_array()
        .expect("effects")
        .iter()
        .find(|effect| effect["type_id"] == "builtin.lowpass")
        .expect("a lowpass")["type_id"]
        .clone();

    // 3. Build the cue in one transaction.
    let created = ok(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Generated cue"
    }));
    let applied = ok(json!({
        "protocol_version": 1,
        "operation": "batch",
        "path": path,
        "progress": true,
        "requests": [
            {
                "operation": "run_generator",
                "type_id": generator,
                "seed": 7,
                "frame_count": 4800,
                "track_id": created["track_id"],
                "layer_id": created["layer_id"],
                "start_sample": 0
            },
            {
                "operation": "add_effect",
                "track": {"track_id": created["track_id"]},
                "type_id": effect
            }
        ]
    }));
    let steps = applied["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 2);
    let asset_id = steps[0]["result"]["asset_id"]
        .as_str()
        .expect("the generator names the asset it made");

    // 4. Render it, and read back what the render had to work around.
    let exported = ok(json!({
        "protocol_version": 1,
        "operation": "export_wav",
        "path": path,
        "output": output,
        "encoding": "float32"
    }));
    assert!(exported["frame_count"].as_u64().expect("frames") > 0);
    assert_eq!(
        exported["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .len(),
        0,
        "nothing degraded: {exported}"
    );

    // 5. Every ID the script needs came back as a field, not as prose.
    let inspected: Value = ok(json!({
        "protocol_version": 1,
        "operation": "inspect_project",
        "path": path
    }));
    assert!(
        inspected["project"]["assets"]
            .as_array()
            .expect("assets")
            .iter()
            .any(|asset| asset["id"] == asset_id)
    );
}
