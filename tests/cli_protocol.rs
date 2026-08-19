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

#[test]
fn tracks_layers_mute_and_solo_are_editable_from_the_machine_surface() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("agent.jutsu-audio.json");
    invoke(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Agent SFX"
    }));

    let (code, added) = invoke(json!({
        "protocol_version": 1,
        "operation": "add_track",
        "path": path,
        "name": "Layers"
    }));
    assert_eq!(code, 0, "{added}");
    assert_eq!(added["result"]["type"], "track_added");
    let track_id = added["result"]["track_id"].clone();

    let (code, layered) = invoke(json!({
        "protocol_version": 1,
        "operation": "add_layer",
        "path": path,
        "track_id": track_id,
        "name": "Tail"
    }));
    assert_eq!(code, 0, "{layered}");
    assert_eq!(layered["result"]["type"], "layer_added");

    let (code, muted) = invoke(json!({
        "protocol_version": 1,
        "operation": "set_track_mute",
        "path": path,
        "track_id": track_id,
        "muted": true
    }));
    assert_eq!(code, 0, "{muted}");
    assert_eq!(muted["result"]["muted"], true);

    let (code, inspected) = invoke(json!({
        "protocol_version": 1,
        "operation": "inspect_project",
        "path": path
    }));
    assert_eq!(code, 0);
    let tracks = inspected["result"]["project"]["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 2, "the project started with one track");
    let added = tracks.iter().find(|track| track["id"] == track_id).unwrap();
    assert_eq!(added["layers"].as_array().unwrap().len(), 2);
    assert_eq!(
        added["parameters"]["mute"],
        json!({"type": "bool", "value": true})
    );
}

#[test]
fn muting_a_track_silences_it_in_an_exported_wav() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("agent.jutsu-audio.json");
    let source = directory.path().join("blip.wav");
    let output = directory.path().join("mix.wav");
    write_test_wav(&source);

    let created = invoke(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Agent SFX"
    }))
    .1;
    let imported = invoke(json!({
        "protocol_version": 1,
        "operation": "import_sample",
        "path": path,
        "source": source
    }))
    .1;
    let track_id = created["result"]["track_id"].clone();
    invoke(json!({
        "protocol_version": 1,
        "operation": "add_clip",
        "path": path,
        "asset_id": imported["result"]["asset_id"],
        "track_id": track_id,
        "layer_id": created["result"]["layer_id"],
        "start_sample": 0,
        "source_start_sample": 0,
        "duration_samples": 480
    }));

    assert!(peak_of_export(&path, &output) > 0.0, "the clip is audible");

    let (code, muted) = invoke(json!({
        "protocol_version": 1,
        "operation": "set_track_mute",
        "path": path,
        "track_id": track_id,
        "muted": true
    }));
    assert_eq!(code, 0, "{muted}");
    assert_eq!(
        peak_of_export(&path, &output),
        0.0,
        "a muted track must be silent in the export, as it is in playback"
    );
}

#[test]
fn split_fade_and_ripple_delete_are_reachable_from_the_machine_surface() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("agent.jutsu-audio.json");
    let source = directory.path().join("blip.wav");
    write_test_wav(&source);

    let created = invoke(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Agent SFX"
    }))
    .1;
    let imported = invoke(json!({
        "protocol_version": 1,
        "operation": "import_sample",
        "path": path,
        "source": source
    }))
    .1;
    let lane = json!({
        "track_id": created["result"]["track_id"],
        "layer_id": created["result"]["layer_id"]
    });
    let added = invoke(json!({
        "protocol_version": 1,
        "operation": "add_clip",
        "path": path,
        "asset_id": imported["result"]["asset_id"],
        "track_id": lane["track_id"],
        "layer_id": lane["layer_id"],
        "start_sample": 0,
        "source_start_sample": 0,
        "duration_samples": 480
    }))
    .1;
    let clip_id = added["result"]["clip_id"].clone();

    let (code, split) = invoke(json!({
        "protocol_version": 1,
        "operation": "split_clip",
        "path": path,
        "clip_id": clip_id,
        "at_frame": 240
    }));
    assert_eq!(code, 0, "{split}");
    assert_eq!(split["result"]["type"], "clip_split");
    assert_eq!(clips_in(&path).len(), 2, "a split makes two clips");

    let (code, refused) = invoke(json!({
        "protocol_version": 1,
        "operation": "split_clip",
        "path": path,
        "clip_id": clip_id,
        "at_frame": 0
    }));
    assert_eq!(code, 4, "{refused}");
    assert_eq!(refused["error"]["code"], "command_failed");

    let (code, faded) = invoke(json!({
        "protocol_version": 1,
        "operation": "set_clip_fades",
        "path": path,
        "clip_id": clip_id,
        "fade_in_samples": 60,
        "fade_out_samples": 9_000
    }));
    assert_eq!(code, 0, "{faded}");
    let head = clips_in(&path)
        .into_iter()
        .find(|clip| clip["id"] == clip_id)
        .unwrap();
    assert_eq!(head["parameters"]["fade_in_samples"]["value"], 60);
    assert_eq!(
        head["parameters"]["fade_out_samples"]["value"], 180,
        "a fade longer than the clip is trimmed to what is left of it"
    );

    let (code, deleted) = invoke(json!({
        "protocol_version": 1,
        "operation": "delete_clip",
        "path": path,
        "clip_id": clip_id,
        "ripple": true
    }));
    assert_eq!(code, 0, "{deleted}");
    assert_eq!(deleted["result"]["ripple"], true);
    let remaining = clips_in(&path);
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0]["start_sample"], 0,
        "the tail moved up into the gap the head left"
    );
}

/// Every clip in the project, read back from disk.
fn clips_in(path: &std::path::Path) -> Vec<Value> {
    let inspected = invoke(json!({
        "protocol_version": 1,
        "operation": "inspect_project",
        "path": path
    }))
    .1;
    inspected["result"]["project"]["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|track| track["layers"].as_array().unwrap())
        .flat_map(|layer| layer["clips"].as_array().unwrap())
        .cloned()
        .collect()
}

#[test]
fn markers_and_the_loop_region_survive_a_round_trip_and_bound_an_export() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("agent.jutsu-audio.json");
    let source = directory.path().join("blip.wav");
    let output = directory.path().join("loop.wav");
    write_test_wav(&source);

    let created = invoke(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Agent SFX"
    }))
    .1;
    let imported = invoke(json!({
        "protocol_version": 1,
        "operation": "import_sample",
        "path": path,
        "source": source
    }))
    .1;
    invoke(json!({
        "protocol_version": 1,
        "operation": "add_clip",
        "path": path,
        "asset_id": imported["result"]["asset_id"],
        "track_id": created["result"]["track_id"],
        "layer_id": created["result"]["layer_id"],
        "start_sample": 0,
        "source_start_sample": 0,
        "duration_samples": 480
    }));

    let (code, marker) = invoke(json!({
        "protocol_version": 1,
        "operation": "add_marker",
        "path": path,
        "name": "Impact",
        "frame": 120
    }));
    assert_eq!(code, 0, "{marker}");
    let marker_id = marker["result"]["marker_id"].clone();

    let (code, moved) = invoke(json!({
        "protocol_version": 1,
        "operation": "move_marker",
        "path": path,
        "marker_id": marker_id,
        "frame": 240
    }));
    assert_eq!(code, 0, "{moved}");

    let (code, looped) = invoke(json!({
        "protocol_version": 1,
        "operation": "set_loop_region",
        "path": path,
        "start_frame": 100,
        "end_frame": 340
    }));
    assert_eq!(code, 0, "{looped}");
    assert_eq!(looped["result"]["enabled"], true);

    let inspected = invoke(json!({
        "protocol_version": 1,
        "operation": "inspect_project",
        "path": path
    }))
    .1;
    let project = &inspected["result"]["project"];
    assert_eq!(project["markers"][0]["frame"], 240);
    assert_eq!(project["loop_region"]["start_frame"], 100);

    let (code, exported) = invoke(json!({
        "protocol_version": 1,
        "operation": "export_wav",
        "path": path,
        "output": output,
        "encoding": "float32",
        "use_loop_region": true
    }));
    assert_eq!(code, 0, "{exported}");
    assert_eq!(
        exported["result"]["frame_count"], 240,
        "an exported loop is exactly the loop that plays"
    );

    let (code, cleared) = invoke(json!({
        "protocol_version": 1,
        "operation": "clear_loop_region",
        "path": path
    }));
    assert_eq!(code, 0, "{cleared}");
    let (code, refused) = invoke(json!({
        "protocol_version": 1,
        "operation": "export_wav",
        "path": path,
        "output": output,
        "encoding": "float32",
        "use_loop_region": true
    }));
    assert_eq!(code, 3, "{refused}");
    assert_eq!(refused["error"]["code"], "export_failed");
}

#[test]
fn extensions_are_discoverable_and_a_synth_clip_renders_into_an_export() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("agent.jutsu-audio.json");
    let output = directory.path().join("tone.wav");

    let (code, listed) = invoke(json!({
        "protocol_version": 1,
        "operation": "list_extensions"
    }));
    assert_eq!(code, 0, "{listed}");
    let synths = listed["result"]["extensions"]["synths"].as_array().unwrap();
    let oscillator = synths
        .iter()
        .find(|synth| synth["type_id"] == "builtin.oscillator")
        .expect("the oscillator is discoverable");
    assert_eq!(oscillator["kind"], "synth");
    assert!(
        oscillator["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|parameter| parameter["id"] == "waveform"),
        "a caller can see the parameters without reading prose"
    );

    let created = invoke(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Tones"
    }))
    .1;
    let (code, added) = invoke(json!({
        "protocol_version": 1,
        "operation": "add_synth_clip",
        "path": path,
        "track_id": created["result"]["track_id"],
        "layer_id": created["result"]["layer_id"],
        "type_id": "builtin.oscillator",
        "start_sample": 0,
        "duration_samples": 4_800,
        "parameters": {"waveform": {"type": "text", "value": "square"}},
        "notes": [{"start_frame": 0, "duration_frames": 2_400, "pitch_hz": 440.0}]
    }));
    assert_eq!(code, 0, "{added}");
    let asset_id = added["result"]["asset_id"].clone();
    let clip_id = added["result"]["clip_id"].clone();

    let (code, exported) = invoke(json!({
        "protocol_version": 1,
        "operation": "export_wav",
        "path": path,
        "output": output,
        "encoding": "float32"
    }));
    assert_eq!(code, 0, "{exported}");
    let mut reader = hound::WavReader::open(&output).unwrap();
    let peak = reader
        .samples::<f32>()
        .map(|sample| sample.unwrap().abs())
        .fold(0.0_f32, f32::max);
    assert!(peak > 0.0, "the synth clip is audible in the export");

    let (code, retuned) = invoke(json!({
        "protocol_version": 1,
        "operation": "set_clip_notes",
        "path": path,
        "clip_id": clip_id,
        "notes": [
            {"start_frame": 0, "duration_frames": 1_200, "pitch_hz": 220.0, "velocity": 0.5},
            {"start_frame": 1_200, "duration_frames": 1_200, "pitch_hz": 330.0}
        ]
    }));
    assert_eq!(code, 0, "{retuned}");
    assert_eq!(retuned["result"]["note_count"], 2);

    let (code, adjusted) = invoke(json!({
        "protocol_version": 1,
        "operation": "set_synth_parameters",
        "path": path,
        "asset_id": asset_id,
        "parameters": {"waveform": {"type": "text", "value": "saw"}}
    }));
    assert_eq!(code, 0, "{adjusted}");
}

#[test]
fn an_unknown_synth_or_parameter_answers_with_what_the_registry_knows() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("agent.jutsu-audio.json");
    let created = invoke(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Tones"
    }))
    .1;
    let lane = json!({
        "track_id": created["result"]["track_id"],
        "layer_id": created["result"]["layer_id"]
    });

    let (code, unknown_type) = invoke(json!({
        "protocol_version": 1,
        "operation": "add_synth_clip",
        "path": path,
        "track_id": lane["track_id"],
        "layer_id": lane["layer_id"],
        "type_id": "builtin.theremin",
        "start_sample": 0,
        "duration_samples": 480
    }));
    assert_eq!(code, 6, "{unknown_type}");
    assert_eq!(unknown_type["error"]["code"], "unknown_extension");
    assert!(
        unknown_type["error"]["message"]
            .as_str()
            .unwrap()
            .contains("builtin.oscillator"),
        "the error lists what this build does have: {unknown_type}"
    );

    let (code, unknown_parameter) = invoke(json!({
        "protocol_version": 1,
        "operation": "add_synth_clip",
        "path": path,
        "track_id": lane["track_id"],
        "layer_id": lane["layer_id"],
        "type_id": "builtin.oscillator",
        "start_sample": 0,
        "duration_samples": 480,
        "parameters": {"cutoff_hz": {"type": "float", "value": 800.0}}
    }));
    assert_eq!(code, 6, "{unknown_parameter}");
    assert_eq!(unknown_parameter["error"]["code"], "unknown_parameter");
    assert!(
        unknown_parameter["error"]["message"]
            .as_str()
            .unwrap()
            .contains("waveform"),
        "the error names the parameters it does take: {unknown_parameter}"
    );

    let (code, wrong_value) = invoke(json!({
        "protocol_version": 1,
        "operation": "add_synth_clip",
        "path": path,
        "track_id": lane["track_id"],
        "layer_id": lane["layer_id"],
        "type_id": "builtin.oscillator",
        "start_sample": 0,
        "duration_samples": 480,
        "parameters": {"waveform": {"type": "text", "value": "bagpipe"}}
    }));
    assert_eq!(code, 6, "{wrong_value}");
    assert_eq!(wrong_value["error"]["code"], "invalid_parameter");
    assert!(
        wrong_value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("bagpipe"),
        "{wrong_value}"
    );
}

/// Exports the project and returns the loudest absolute sample in the file.
fn peak_of_export(path: &std::path::Path, output: &std::path::Path) -> f32 {
    let (code, exported) = invoke(json!({
        "protocol_version": 1,
        "operation": "export_wav",
        "path": path,
        "output": output,
        "encoding": "float32"
    }));
    assert_eq!(code, 0, "{exported}");
    let mut reader = hound::WavReader::open(output).unwrap();
    reader
        .samples::<f32>()
        .map(|sample| sample.unwrap().abs())
        .fold(0.0_f32, f32::max)
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
