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

#[test]
fn the_mixer_routing_effects_and_automation_are_all_reachable_from_the_cli() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("agent.jutsu-audio.json");
    let created = invoke(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Mixing"
    }))
    .1;
    let track_id = created["result"]["track_id"].clone();

    // The strip's own schema, discoverable like an extension's.
    let (code, strip) = invoke(json!({"protocol_version": 1, "operation": "describe_strip"}));
    assert_eq!(code, 0, "{strip}");
    let parameters = strip["result"]["strip"]["parameters"].as_array().unwrap();
    assert!(
        parameters
            .iter()
            .any(|parameter| parameter["id"] == "gain_db" && parameter["unit"] == "dB"),
        "a level is decibels, and says so: {strip}"
    );

    let (code, bus) = invoke(json!({
        "protocol_version": 1,
        "operation": "add_bus",
        "path": path,
        "name": "Reverb bus"
    }));
    assert_eq!(code, 0, "{bus}");
    let bus_id = bus["result"]["bus_id"].clone();

    let (code, routed) = invoke(json!({
        "protocol_version": 1,
        "operation": "set_track_output",
        "path": path,
        "track_id": track_id,
        "output_bus_id": bus_id
    }));
    assert_eq!(code, 0, "{routed}");

    let (code, level) = invoke(json!({
        "protocol_version": 1,
        "operation": "set_bus_parameter",
        "path": path,
        "bus_id": bus_id,
        "key": "gain_db",
        "value": {"type": "float", "value": -6.0}
    }));
    assert_eq!(code, 0, "{level}");

    let (code, refused) = invoke(json!({
        "protocol_version": 1,
        "operation": "set_bus_parameter",
        "path": path,
        "bus_id": bus_id,
        "key": "gain_db",
        "value": {"type": "float", "value": 400.0}
    }));
    assert_eq!(code, 6, "{refused}");
    assert_eq!(refused["error"]["code"], "invalid_parameter");

    let (code, described) = invoke(json!({
        "protocol_version": 1,
        "operation": "describe_effect",
        "type_id": "builtin.reverb"
    }));
    assert_eq!(code, 0, "{described}");
    assert!(
        !described["result"]["effect"]["presets"]
            .as_array()
            .unwrap()
            .is_empty(),
        "an effect publishes its presets: {described}"
    );

    let (code, added) = invoke(json!({
        "protocol_version": 1,
        "operation": "add_effect",
        "path": path,
        "bus": {"bus_id": bus_id},
        "type_id": "builtin.reverb",
        "parameters": {"size": {"type": "float", "value": 0.7}}
    }));
    assert_eq!(code, 0, "{added}");
    let effect_id = added["result"]["effect_id"].clone();

    let (code, bypassed) = invoke(json!({
        "protocol_version": 1,
        "operation": "set_effect_enabled",
        "path": path,
        "effect_id": effect_id,
        "enabled": false
    }));
    assert_eq!(code, 0, "{bypassed}");

    let (code, lane) = invoke(json!({
        "protocol_version": 1,
        "operation": "add_automation_lane",
        "path": path,
        "target": {"type": "track", "track_id": track_id},
        "parameter": "gain_db",
        "points": [
            {"frame": 4_800, "value": 0.0},
            {"frame": 0, "value": -24.0}
        ]
    }));
    assert_eq!(code, 0, "{lane}");
    let automation_id = lane["result"]["automation_id"].clone();

    let inspected = invoke(json!({
        "protocol_version": 1,
        "operation": "inspect_project",
        "path": path
    }))
    .1;
    let project = &inspected["result"]["project"];
    assert_eq!(project["buses"].as_array().unwrap().len(), 2);
    assert_eq!(project["tracks"][0]["output_bus_id"], bus_id);
    let lanes = project["automation"].as_array().unwrap();
    assert_eq!(
        lanes[0]["points"][0]["frame"], 0,
        "points are stored in order"
    );
    let bus = project["buses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|bus| bus["id"] == bus_id)
        .unwrap();
    assert_eq!(bus["effects"][0]["enabled"], false);

    let (code, cleared) = invoke(json!({
        "protocol_version": 1,
        "operation": "remove_automation_lane",
        "path": path,
        "automation_id": automation_id
    }));
    assert_eq!(code, 0, "{cleared}");
}

#[test]
fn an_effect_this_build_does_not_have_is_refused_with_what_it_does_have() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("agent.jutsu-audio.json");
    let created = invoke(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Mixing"
    }))
    .1;

    let (code, unknown) = invoke(json!({
        "protocol_version": 1,
        "operation": "add_effect",
        "path": path,
        "track": {"track_id": created["result"]["track_id"]},
        "type_id": "builtin.phaser"
    }));
    assert_eq!(code, 6, "{unknown}");
    assert_eq!(unknown["error"]["code"], "unknown_extension");
    assert!(
        unknown["error"]["message"]
            .as_str()
            .unwrap()
            .contains("builtin.reverb"),
        "{unknown}"
    );
}

#[test]
fn musical_time_converts_both_ways_and_follows_the_tempo_map() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("agent.jutsu-audio.json");
    invoke(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Song"
    }));

    // A project that never mentions tempo still has one: 120 BPM in 4/4.
    let (code, default) = invoke(json!({
        "protocol_version": 1,
        "operation": "convert_time",
        "path": path,
        "frame": 96_000
    }));
    assert_eq!(code, 0, "{default}");
    assert_eq!(default["result"]["beats_per_minute"], 120.0);
    assert_eq!(default["result"]["formatted"], "2.1.000");
    assert_eq!(default["result"]["position"]["bar"], 2);

    let (code, set) = invoke(json!({
        "protocol_version": 1,
        "operation": "set_tempo_map",
        "path": path,
        "changes": [
            {"frame": 96_000, "beats_per_minute": 90.0, "beats_per_bar": 3},
            {"frame": 0, "beats_per_minute": 140.0}
        ]
    }));
    assert_eq!(code, 0, "{set}");
    assert_eq!(set["result"]["change_count"], 2);

    let inspected = invoke(json!({
        "protocol_version": 1,
        "operation": "inspect_project",
        "path": path
    }))
    .1;
    let tempo = inspected["result"]["project"]["tempo"].as_array().unwrap();
    assert_eq!(tempo[0]["frame"], 0, "changes are stored in frame order");
    assert_eq!(tempo[0]["beats_per_minute"], 140.0);

    // A position converts to a frame and back to the same position.
    let (code, from_bar) = invoke(json!({
        "protocol_version": 1,
        "operation": "convert_time",
        "path": path,
        "position": {"bar": 5, "beat": 2, "tick": 240}
    }));
    assert_eq!(code, 0, "{from_bar}");
    let frame = from_bar["result"]["frame"].clone();
    let (code, round_trip) = invoke(json!({
        "protocol_version": 1,
        "operation": "convert_time",
        "path": path,
        "frame": frame
    }));
    assert_eq!(code, 0, "{round_trip}");
    assert_eq!(round_trip["result"]["formatted"], "5.2.240");

    let (code, refused) = invoke(json!({
        "protocol_version": 1,
        "operation": "set_tempo_map",
        "path": path,
        "changes": [{"frame": 0, "beats_per_minute": 0.0}]
    }));
    assert_eq!(code, 6, "{refused}");
    assert_eq!(refused["error"]["code"], "invalid_parameter");
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

/// The command engine has always been able to remove a track; for a while the
/// machine surface could not, which meant a script could build a project it
/// could not take apart.
#[test]
fn a_removed_track_takes_its_clips_with_it_and_leaves_the_rest() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("strip.jutsu-audio.json");
    let (_, created) = invoke(json!({"protocol_version": 1, "operation": "create_project",
                                     "path": path, "name": "Strip"}));
    let kept = created["result"]["track_id"].clone();

    let (_, added) = invoke(json!({"protocol_version": 1, "operation": "add_track",
                                   "path": path, "name": "Doomed"}));
    let doomed = added["result"]["track_id"].clone();
    invoke(
        json!({"protocol_version": 1, "operation": "add_synth_clip", "path": path,
                  "track_id": doomed, "layer_id": added["result"]["layer_id"],
                  "type_id": "builtin.oscillator", "start_sample": 0,
                  "duration_samples": 48_000}),
    );

    let (code, removed) = invoke(json!({"protocol_version": 1, "operation": "remove_track",
                                        "path": path, "track_id": doomed}));
    assert_eq!(code, 0, "{removed}");
    assert_eq!(removed["result"]["type"], "track_removed");

    let (_, inspected) = invoke(
        json!({"protocol_version": 1, "operation": "inspect_project",
                                       "path": path}),
    );
    let tracks = inspected["result"]["project"]["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 1, "{tracks:?}");
    assert_eq!(tracks[0]["id"], kept);

    // Naming a track that is gone is refused rather than silently accepted.
    let (code, refused) = invoke(json!({"protocol_version": 1, "operation": "remove_track",
                                        "path": path, "track_id": doomed}));
    assert_eq!(code, 4, "{refused}");
}

/// Stems: one file per track, from the same render the master comes out of.
/// The check that matters is that they add back up — a stem set that does not
/// sum to the mix is a set of files nobody can use.
#[test]
fn stems_are_written_per_track_and_sum_back_to_the_master() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("stems.jutsu-audio.json");
    let (_, created) = invoke(json!({"protocol_version": 1, "operation": "create_project",
                                     "path": path, "name": "Stems"}));
    let first = created["result"].clone();
    let (_, second) = invoke(json!({"protocol_version": 1, "operation": "add_track",
                                    "path": path, "name": "Second voice"}));

    for (track, layer, pitch) in [
        (first["track_id"].clone(), first["layer_id"].clone(), 220.0),
        (
            second["result"]["track_id"].clone(),
            second["result"]["layer_id"].clone(),
            330.0,
        ),
    ] {
        // Well under full scale each: the exported master is clamped at ±1, and
        // two loud voices would clip the sum, which would make this a test
        // about clipping rather than about stems.
        invoke(
            json!({"protocol_version": 1, "operation": "set_track_parameter", "path": path,
                      "track_id": track, "key": "gain_db",
                      "value": {"type": "float", "value": -12.0}}),
        );
        invoke(
            json!({"protocol_version": 1, "operation": "add_synth_clip", "path": path,
                      "track_id": track, "layer_id": layer, "type_id": "builtin.oscillator",
                      "start_sample": 0, "duration_samples": 24_000,
                      "notes": [{"start_frame": 0, "duration_frames": 24_000,
                                 "pitch_hz": pitch, "velocity": 0.8}]}),
        );
    }

    let stems_directory = directory.path().join("stems");
    let (code, exported) = invoke(json!({"protocol_version": 1, "operation": "export_stems",
                                         "path": path, "directory": stems_directory,
                                         "encoding": "float32"}));
    assert_eq!(code, 0, "{exported}");
    let stems = exported["result"]["stems"].as_array().unwrap();
    assert_eq!(stems.len(), 2, "{exported}");
    assert!(stems[1]["name"].as_str().unwrap().contains("Second"));
    // Named for the track, not for its index alone.
    assert!(
        stems[1]["output"]
            .as_str()
            .unwrap()
            .contains("second-voice"),
        "{}",
        stems[1]["output"]
    );

    let master = directory.path().join("master.wav");
    invoke(
        json!({"protocol_version": 1, "operation": "export_wav", "path": path,
                  "output": master, "encoding": "float32"}),
    );

    let read = |file: &std::path::Path| -> Vec<f32> {
        hound::WavReader::open(file)
            .unwrap()
            .into_samples::<f32>()
            .map(Result::unwrap)
            .collect()
    };
    let mixed = read(&master);
    let summed: Vec<f32> = stems
        .iter()
        .map(|stem| read(std::path::Path::new(stem["output"].as_str().unwrap())))
        .fold(vec![0.0_f32; mixed.len()], |mut total, stem| {
            for (slot, sample) in total.iter_mut().zip(stem) {
                *slot += sample;
            }
            total
        });

    assert_eq!(summed.len(), mixed.len());
    for (index, (stem_sum, master_sample)) in summed.iter().zip(&mixed).enumerate() {
        assert!(
            (stem_sum - master_sample).abs() < 1e-6,
            "stems and master part company at sample {index}: {stem_sum} against {master_sample}"
        );
    }
}

/// A loop lives in the project; the file it exports has to carry it too, or
/// the loop stops existing the moment the audio leaves the editor.
#[test]
fn an_exported_wav_carries_the_projects_loop_points() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("looped.jutsu-audio.json");
    let (_, created) = invoke(json!({"protocol_version": 1, "operation": "create_project",
                                     "path": path, "name": "Looped"}));
    invoke(
        json!({"protocol_version": 1, "operation": "add_synth_clip", "path": path,
                  "track_id": created["result"]["track_id"],
                  "layer_id": created["result"]["layer_id"],
                  "type_id": "builtin.oscillator", "start_sample": 0,
                  "duration_samples": 48_000,
                  "notes": [{"start_frame": 0, "duration_frames": 48_000,
                             "pitch_hz": 220.0, "velocity": 0.5}]}),
    );
    invoke(
        json!({"protocol_version": 1, "operation": "set_loop_region", "path": path,
                  "start_frame": 12_000, "end_frame": 36_000}),
    );

    let output = directory.path().join("looped.wav");
    let (code, exported) = invoke(json!({"protocol_version": 1, "operation": "export_wav",
                                         "path": path, "output": output,
                                         "encoding": "pcm16"}));
    assert_eq!(code, 0, "{exported}");
    assert_eq!(exported["result"]["loop_points"]["start_frame"], 12_000);
    assert_eq!(exported["result"]["loop_points"]["end_frame"], 36_000);

    // Read back out of the file itself, not out of the response: the response
    // could say anything.
    assert_eq!(
        jutsu_audio_engine::read_loop_points(&output),
        Some((12_000, 36_000))
    );

    // And the file is still an ordinary WAV that any reader opens.
    let reader = hound::WavReader::open(&output).unwrap();
    assert_eq!(reader.spec().sample_rate, 48_000);
    assert_eq!(reader.len() as u64 / 2, 48_000);
}

/// Exporting the loop itself makes the whole file the loop, which is what a
/// game engine wants from a file it is going to loop forever.
#[test]
fn exporting_the_loop_region_marks_the_whole_file_as_the_loop() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("region.jutsu-audio.json");
    let (_, created) = invoke(json!({"protocol_version": 1, "operation": "create_project",
                                     "path": path, "name": "Region"}));
    invoke(
        json!({"protocol_version": 1, "operation": "add_synth_clip", "path": path,
                  "track_id": created["result"]["track_id"],
                  "layer_id": created["result"]["layer_id"],
                  "type_id": "builtin.oscillator", "start_sample": 0,
                  "duration_samples": 48_000,
                  "notes": [{"start_frame": 0, "duration_frames": 48_000,
                             "pitch_hz": 220.0, "velocity": 0.5}]}),
    );
    invoke(
        json!({"protocol_version": 1, "operation": "set_loop_region", "path": path,
                  "start_frame": 6_000, "end_frame": 30_000}),
    );

    let output = directory.path().join("loop-only.wav");
    let (code, exported) = invoke(json!({"protocol_version": 1, "operation": "export_wav",
                                         "path": path, "output": output,
                                         "encoding": "pcm16", "use_loop_region": true}));
    assert_eq!(code, 0, "{exported}");
    assert_eq!(exported["result"]["frame_count"], 24_000);
    assert_eq!(
        jutsu_audio_engine::read_loop_points(&output),
        Some((0, 24_000))
    );
}

/// A repeated one-shot that is the same sound every time is the oldest tell in
/// game audio. A variation set is several seeds of one recipe, placed in turn,
/// and it has to stay as reproducible as a single generated sound is.
#[test]
fn a_variation_set_cycles_its_versions_and_repeats_exactly() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("variations.jutsu-audio.json");
    let (_, created) = invoke(json!({"protocol_version": 1, "operation": "create_project",
                                     "path": path, "name": "Variations"}));

    let request = json!({"protocol_version": 1, "operation": "run_generator_variations",
                         "path": path,
                         "track_id": created["result"]["track_id"],
                         "layer_id": created["result"]["layer_id"],
                         "type_id": "sfx.impact", "seed": 4_000, "frame_count": 12_000,
                         "variations": 3,
                         "placements": [0, 24_000, 48_000, 72_000, 96_000]});
    let (code, ran) = invoke(request.clone());
    assert_eq!(code, 0, "{ran}");

    let assets = ran["result"]["assets"].as_array().unwrap();
    assert_eq!(assets.len(), 3, "three seeds, three assets");
    let clips = ran["result"]["clips"].as_array().unwrap();
    assert_eq!(clips.len(), 5);
    // Five placements over three versions: the fourth is the first again.
    assert_eq!(clips[0]["asset_id"], clips[3]["asset_id"]);
    assert_eq!(clips[1]["asset_id"], clips[4]["asset_id"]);
    assert_ne!(clips[0]["asset_id"], clips[1]["asset_id"]);

    // The set is derived from the seed, so the same request names the same
    // assets — running it again adds clips, never a second set of sounds.
    let second = directory.path().join("again.jutsu-audio.json");
    invoke(json!({"protocol_version": 1, "operation": "create_project",
                  "path": second, "name": "Variations"}));
    let (_, inspected) = invoke(
        json!({"protocol_version": 1, "operation": "inspect_project",
                                       "path": second}),
    );
    let mut repeat = request;
    repeat["path"] = json!(second);
    repeat["track_id"] = inspected["result"]["project"]["tracks"][0]["id"].clone();
    repeat["layer_id"] = inspected["result"]["project"]["tracks"][0]["layers"][0]["id"].clone();
    let (_, again) = invoke(repeat);
    assert_eq!(again["result"]["assets"], ran["result"]["assets"]);

    // And each variation really is a different sound, not the same one three
    // times: the exported render of one is not the render of another.
    let (_, exported) = invoke(json!({"protocol_version": 1, "operation": "export_wav",
                                      "path": path,
                                      "output": directory.path().join("set.wav"),
                                      "encoding": "float32"}));
    assert_eq!(exported["ok"], true, "{exported}");
    let samples: Vec<f32> = hound::WavReader::open(directory.path().join("set.wav"))
        .unwrap()
        .into_samples::<f32>()
        .map(Result::unwrap)
        .collect();
    let first_hit = &samples[0..2_000];
    let second_hit = &samples[24_000 * 2..24_000 * 2 + 2_000];
    assert_ne!(
        first_hit, second_hit,
        "two variations rendered the same audio"
    );
}
