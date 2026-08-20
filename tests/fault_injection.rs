//! What happens when things go wrong on disk.
//!
//! Every scenario here breaks something a user cannot control — the machine
//! loses power mid-edit, a sample gets deleted or truncated by a sync client, a
//! file arrives from a build that knows extensions this one does not — and
//! asserts the same property each time: **nothing is lost silently**. Either the
//! work is still there, or the tool says out loud what it could not do.
//!
//! Recovery and compatibility rules these pin down live in
//! `docs/design/crash-recovery-and-compatibility.md`.

mod support;

use std::fs;
use std::path::Path;

use jutsu_audio_project::{ProjectStore, autosave, report};
use serde_json::{Value, json};
use support::{call, ok, write_test_wav};

/// A second sample with different content, so importing it is a second asset
/// rather than a fingerprint match onto the first.
fn write_square_wav(path: &Path) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("wav");
    for frame in 0..480_i32 {
        writer
            .write_sample(if frame % 48 < 24 { 12_000 } else { -12_000 })
            .expect("write sample");
    }
    writer.finalize().expect("finalize");
}

/// A project with one imported sample and one clip on it, ready to break.
fn project_with_a_sample(directory: &Path) -> (std::path::PathBuf, std::path::PathBuf, Value) {
    let path = directory.join("cue.jutsu-audio.json");
    let source = directory.join("blip.wav");
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
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "add_clip",
        "path": path,
        "asset_id": imported["asset_id"],
        "track_id": created["track_id"],
        "layer_id": created["layer_id"],
        "start_sample": 0,
        "source_start_sample": 0,
        "duration_samples": 480
    }));

    // The import copies the sample into the project's own folder; that copy is
    // what the scenarios damage, not the file the user handed over.
    let project = ProjectStore::open(&path).expect("open").project;
    let jutsu_audio_model::AudioAssetSource::ManagedFile { path: relative, .. } =
        &project.assets[0].source
    else {
        panic!("an imported sample is a managed file");
    };
    let managed = directory.join(relative);
    (path, managed, imported["asset_id"].clone())
}

#[test]
fn power_loss_between_autosave_and_save_keeps_both_the_saved_and_the_unsaved_state() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("cue.jutsu-audio.json");
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Saved"
    }));
    let saved = ProjectStore::open(&path).expect("open").project;

    // Edited past the last save, then the machine goes down: no save, no
    // discard, nothing tidied up.
    let mut edited = saved.clone();
    edited.metadata.name = "Edited past the last save".into();
    autosave::write(&path, &edited).expect("autosave");

    // What comes back up.
    let on_disk = ProjectStore::open(&path).expect("reopen").project;
    assert_eq!(
        on_disk.metadata.name, "Saved",
        "the saved file is exactly what was saved — recovery never rewrites it"
    );
    let recovered = autosave::recover(&path)
        .expect("recover")
        .expect("unsaved work is offered back");
    assert_eq!(recovered.project.metadata.name, "Edited past the last save");

    // Declining recovery keeps the saved file and clears the parked state, so
    // the next launch is not asked again.
    autosave::discard(&path).expect("discard");
    assert!(autosave::recover(&path).expect("recover").is_none());
    assert_eq!(
        ProjectStore::open(&path)
            .expect("reopen")
            .project
            .metadata
            .name,
        "Saved"
    );
}

#[test]
fn a_torn_write_cannot_replace_a_good_project_with_a_broken_one() {
    let directory = tempfile::tempdir().expect("temp dir");
    let (path, _, _) = project_with_a_sample(directory.path());
    let before = fs::read(&path).expect("read");

    // A save that fails validation is the closest thing to a write that dies
    // partway: the store has decided to write, and then does not.
    let mut broken = ProjectStore::open(&path).expect("open").project;
    broken.tracks[0].output_bus_id = jutsu_audio_model::BusId::new(); // no such bus
    let error = ProjectStore::save(&path, &broken).expect_err("an invalid project is refused");
    assert!(!error.diagnostics.is_empty(), "and says what is wrong");

    assert_eq!(
        fs::read(&path).expect("read"),
        before,
        "the project on disk is byte for byte what it was"
    );
    assert!(
        !directory
            .path()
            .read_dir()
            .expect("read dir")
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().ends_with(".tmp")),
        "and no half-written temporary file is left behind"
    );
}

#[test]
fn a_project_from_a_newer_build_is_refused_without_touching_it() {
    let directory = tempfile::tempdir().expect("temp dir");
    let (path, _, _) = project_with_a_sample(directory.path());

    let mut raw: Value = serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
    raw["schema_version"] = json!(9_999);
    let from_the_future = serde_json::to_vec_pretty(&raw).expect("encode");
    fs::write(&path, &from_the_future).expect("write");

    let (code, response) = call(json!({
        "protocol_version": 1,
        "operation": "inspect_project",
        "path": path
    }));
    assert_ne!(code, 0, "opening it fails: {response}");
    assert!(
        response["error"]["message"]
            .as_str()
            .expect("message")
            .contains("newer than supported"),
        "and says why: {response}"
    );
    assert_eq!(
        fs::read(&path).expect("read"),
        from_the_future,
        "a project this build cannot read is left exactly as it was"
    );
}

#[test]
fn a_migration_keeps_the_file_it_migrated_from() {
    let directory = tempfile::tempdir().expect("temp dir");
    let (path, _, _) = project_with_a_sample(directory.path());
    let current: Value = serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");

    let mut old = current;
    old["schema_version"] = json!(0);
    let old_bytes = serde_json::to_vec_pretty(&old).expect("encode");
    fs::write(&path, &old_bytes).expect("write");
    let opened = ProjectStore::open(&path).expect("migrate");
    assert_eq!(opened.migrated_from, Some(0));
    let backup = opened.backup_path.expect("a backup beside the project");
    assert_eq!(
        fs::read(&backup).expect("read backup"),
        old_bytes,
        "the backup is the file as it arrived, not the migrated one"
    );
}

#[test]
fn an_extension_this_build_does_not_know_survives_a_round_trip_untouched() {
    let directory = tempfile::tempdir().expect("temp dir");
    let (path, _, _) = project_with_a_sample(directory.path());

    // A project saved by a build with a third-party synth and effect installed.
    let mut raw: Value = serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
    raw["assets"].as_array_mut().expect("assets").push(json!({
        "id": "8f7bd0a4-1f6d-4a7e-9d5e-2a4f6c8b0d13",
        "name": "Vendor Lead",
        "source": {
            "type": "synth",
            "type_id": "vendor.super_lead",
            "state_version": 4,
            "parameters": {"warmth": {"type": "float", "value": 0.75}}
        }
    }));
    raw["tracks"][0]["effects"] = json!([{
        "id": "3c1e5b90-77a2-4c1b-8f0e-91d3a5b7c204",
        "type_id": "vendor.tape",
        "state_version": 2,
        "parameters": {"drive": {"type": "float", "value": 3.5}},
        "enabled": true,
        "wet": 0.4
    }]);
    fs::write(&path, serde_json::to_vec_pretty(&raw).expect("encode")).expect("write");

    // An ordinary edit through the CLI: opens, mutates, saves.
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "set_track_parameter",
        "path": path,
        "track_id": raw["tracks"][0]["id"],
        "key": "gain_db",
        "value": {"type": "float", "value": -3.0}
    }));

    let after: Value = serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
    assert_eq!(
        after["assets"].as_array().expect("assets").last(),
        raw["assets"].as_array().expect("assets").last(),
        "the unknown synth is written back exactly as it arrived"
    );
    assert_eq!(
        after["tracks"][0]["effects"], raw["tracks"][0]["effects"],
        "and so is the unknown effect, parameters and all"
    );

    // It is also visible to a report, so a user can find out what they need.
    let diagnosed = report::collect(&path);
    assert!(
        diagnosed
            .extension_type_ids
            .iter()
            .any(|type_id| type_id == "vendor.super_lead"),
        "{:?}",
        diagnosed.extension_type_ids
    );
}

#[test]
fn a_damaged_sample_silences_its_own_clip_and_nothing_else() {
    let directory = tempfile::tempdir().expect("temp dir");
    let (path, managed, _) = project_with_a_sample(directory.path());

    // A second, healthy clip, so there is something left to hear.
    let second_source = directory.path().join("tone.wav");
    write_square_wav(&second_source);
    let second = ok(json!({
        "protocol_version": 1,
        "operation": "import_sample",
        "path": path,
        "source": second_source
    }));
    let project = ProjectStore::open(&path).expect("open").project;
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "add_clip",
        "path": path,
        "asset_id": second["asset_id"],
        "track_id": project.tracks[0].id,
        "layer_id": project.tracks[0].layers[0].id,
        "start_sample": 0,
        "source_start_sample": 0,
        "duration_samples": 480
    }));

    // Something truncated the first sample: header intact, audio gone.
    fs::write(&managed, b"RIFF").expect("truncate");

    let output = directory.path().join("cue.wav");
    let exported = ok(json!({
        "protocol_version": 1,
        "operation": "export_wav",
        "path": path,
        "output": output,
        "encoding": "float32"
    }));
    assert!(
        exported["frame_count"].as_u64().expect("frames") > 0,
        "the export still runs: {exported}"
    );
    let diagnostics = exported["diagnostics"]
        .as_array()
        .expect("diagnostics are reported");
    assert!(
        diagnostics.iter().any(|diagnostic| diagnostic["message"]
            .as_str()
            .is_some_and(|message| message.contains("plays silence"))),
        "and name the clip that went silent: {exported}"
    );

    let mut reader = hound::WavReader::open(&output).expect("open export");
    let peak = reader
        .samples::<f32>()
        .map(|sample| sample.expect("sample").abs())
        .fold(0.0_f32, f32::max);
    assert!(peak > 0.0, "the healthy clip is still audible");

    // The project itself is untouched by any of this.
    let checked = ok(json!({
        "protocol_version": 1,
        "operation": "check_assets",
        "path": path
    }));
    assert_eq!(
        checked["unresolved"].as_array().expect("unresolved").len(),
        1,
        "and the damaged source is listed as needing attention: {checked}"
    );
}

#[test]
fn a_project_that_will_not_open_can_still_be_reported_on() {
    let directory = tempfile::tempdir().expect("temp dir");
    let (path, _, _) = project_with_a_sample(directory.path());
    let good = ProjectStore::open(&path).expect("open").project;
    autosave::write(&path, &good).expect("autosave");

    fs::write(&path, b"{\"schema_version\": 0, \"tracks\": [").expect("corrupt");

    let bundle = directory.path().join("report");
    let result = ok(json!({
        "protocol_version": 1,
        "operation": "diagnose",
        "path": path,
        "destination": bundle
    }));
    assert_eq!(result["open_status"]["state"], "failed");
    assert_eq!(
        result["recovery"]["autosave_present"], true,
        "the report points at the recovery file that still holds the work: {result}"
    );
    assert_eq!(result["recovery"]["autosave_readable"], true);

    // The bundle is self-contained: the report, and a copy of the file as found.
    let written: Value = serde_json::from_slice(
        &fs::read(bundle.join(report::REPORT_FILE_NAME)).expect("read report"),
    )
    .expect("json");
    assert_eq!(written["project_path"], result["project_path"]);
    assert_eq!(
        fs::read(bundle.join("cue.jutsu-audio.json")).expect("copy"),
        b"{\"schema_version\": 0, \"tracks\": [",
        "including the broken file exactly as it was found"
    );
}

#[test]
fn a_project_needing_an_extension_this_build_lacks_still_edits_and_exports() {
    let directory = tempfile::tempdir().expect("temp dir");
    let (path, _, _) = project_with_a_sample(directory.path());

    // A synth from an extension pack this build does not have installed, with
    // a clip playing it. `examples/pocket-extensions` is where it comes from.
    let mut raw: Value = serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
    raw["assets"].as_array_mut().expect("assets").push(json!({
        "id": "2b5e2b28-6d3a-4a1c-9f2f-7c1f0f4a55b1",
        "name": "Pocket Pluck",
        "source": {
            "type": "synth",
            "type_id": "pocket.pluck",
            "state_version": 1,
            "parameters": {"decay_ms": {"type": "float", "value": 400.0}}
        }
    }));
    raw["tracks"][0]["layers"][0]["clips"]
        .as_array_mut()
        .expect("clips")
        .push(json!({
            "id": "6f1b9d02-1c4e-49f8-8f3a-52d1f2b7a903",
            "asset_id": "2b5e2b28-6d3a-4a1c-9f2f-7c1f0f4a55b1",
            "start_sample": 0,
            "source_start_sample": 0,
            "duration_samples": 480,
            "notes": [{
                "start_frame": 0,
                "duration_frames": 240,
                "pitch_hz": 220.0,
                "velocity": 0.8
            }]
        }));
    fs::write(&path, serde_json::to_vec_pretty(&raw).expect("encode")).expect("write");

    // Editing something else entirely still works.
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "add_track",
        "path": path,
        "name": "Beside it"
    }));

    let output = directory.path().join("cue.wav");
    let exported = ok(json!({
        "protocol_version": 1,
        "operation": "export_wav",
        "path": path,
        "output": output,
        "encoding": "float32"
    }));
    assert!(
        exported["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("pocket.pluck"))),
        "the missing extension is named: {exported}"
    );

    let after: Value = serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
    let assets = after["assets"].as_array().expect("assets");
    assert!(
        assets
            .iter()
            .any(|asset| asset["source"]["type_id"] == "pocket.pluck"
                && asset["source"]["parameters"]["decay_ms"]["value"] == 400.0),
        "and the asset it could not play is written back untouched"
    );
}
