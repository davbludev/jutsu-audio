//! Presets from the machine surface: saving what a project holds, applying it
//! back, moving one between libraries, and being told when one does not fit.

mod support;

use std::path::PathBuf;

use jutsu_audio_model::{AudioAssetSource, Project};
use jutsu_audio_project::ProjectStore;
use serde_json::{Value, json};
use support::{call, ok};

struct Fixture {
    _directory: tempfile::TempDir,
    path: PathBuf,
    track_id: Value,
    layer_id: Value,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("song.jutsu-audio.json");
    let created = ok(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Presets"
    }));
    Fixture {
        _directory: directory,
        path,
        track_id: created["track_id"].clone(),
        layer_id: created["layer_id"].clone(),
    }
}

impl Fixture {
    fn project(&self) -> Project {
        ProjectStore::open(&self.path).expect("open").project
    }

    /// A synth clip, and the asset behind it.
    fn synth(&self, waveform: &str) -> Value {
        ok(json!({
            "protocol_version": 1,
            "operation": "add_synth_clip",
            "path": self.path,
            "track_id": self.track_id,
            "layer_id": self.layer_id,
            "type_id": "builtin.oscillator",
            "start_sample": 0,
            "duration_samples": 4_800,
            "parameters": {"waveform": {"type": "text", "value": waveform}}
        }))
    }

    fn add_effect(&self, type_id: &str) -> Value {
        ok(json!({
            "protocol_version": 1,
            "operation": "add_effect",
            "path": self.path,
            "track": {"track_id": self.track_id},
            "type_id": type_id
        }))
    }
}

fn synth_parameters(project: &Project) -> Value {
    let asset = project
        .assets
        .iter()
        .find(|asset| matches!(asset.source, AudioAssetSource::Synth { .. }))
        .expect("a synth");
    let AudioAssetSource::Synth { parameters, .. } = &asset.source else {
        unreachable!()
    };
    serde_json::to_value(parameters).expect("encode")
}

#[test]
fn the_built_in_presets_are_listed_alongside_the_user_library() {
    let fixture = fixture();
    let listed = ok(json!({
        "protocol_version": 1,
        "operation": "list_presets",
        "path": fixture.path
    }));

    let builtin = listed["builtin"].as_array().unwrap();
    assert!(
        builtin
            .iter()
            .any(|preset| preset["type_id"] == "builtin.reverb"),
        "the effects ship presets: {listed}"
    );
    assert!(
        builtin
            .iter()
            .any(|preset| preset["type_id"] == "sfx.impact"),
        "so do the generators"
    );
    assert!(listed["user"].as_array().unwrap().is_empty());
}

#[test]
fn a_synth_preset_saves_what_it_is_set_to_and_applies_it_back() {
    let fixture = fixture();
    let square = fixture.synth("square");

    let saved = ok(json!({
        "protocol_version": 1,
        "operation": "save_preset",
        "path": fixture.path,
        "name": "Square Lead",
        "tags": ["lead", "bright"],
        "asset": {"asset_id": square["asset_id"]}
    }));
    assert_eq!(saved["preset_id"], "square-lead");
    assert_eq!(saved["kind"], "synth");

    // Change it, then put the preset back.
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "set_synth_parameters",
        "path": fixture.path,
        "asset_id": square["asset_id"],
        "parameters": {"waveform": {"type": "text", "value": "sine"}}
    }));
    assert!(
        synth_parameters(&fixture.project())
            .to_string()
            .contains("sine"),
        "the change landed"
    );

    let applied = ok(json!({
        "protocol_version": 1,
        "operation": "apply_preset",
        "path": fixture.path,
        "preset_id": "square-lead",
        "asset": {"asset_id": square["asset_id"]}
    }));
    assert!(applied["incompatibilities"].as_array().unwrap().is_empty());
    assert!(
        synth_parameters(&fixture.project())
            .to_string()
            .contains("square"),
        "the preset put it back"
    );

    let listed = ok(json!({
        "protocol_version": 1,
        "operation": "list_presets",
        "path": fixture.path
    }));
    let user = listed["user"].as_array().unwrap();
    assert_eq!(user.len(), 1);
    assert_eq!(user[0]["tags"], json!(["bright", "lead"]));
}

#[test]
fn a_chain_preset_replaces_a_whole_rack() {
    let fixture = fixture();
    fixture.add_effect("builtin.lowpass");
    fixture.add_effect("builtin.delay");

    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "save_preset",
        "path": fixture.path,
        "name": "Space",
        "track_chain": {"track_id": fixture.track_id}
    }));

    // Strip it back to one different effect, then apply the preset.
    let project = fixture.project();
    for insert in &project.tracks[0].effects {
        let _ = ok(json!({
            "protocol_version": 1,
            "operation": "remove_effect",
            "path": fixture.path,
            "effect_id": insert.id
        }));
    }
    fixture.add_effect("builtin.compressor");

    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "apply_preset",
        "path": fixture.path,
        "preset_id": "space",
        "track_chain": {"track_id": fixture.track_id}
    }));

    let effects = &fixture.project().tracks[0].effects;
    let types: Vec<&str> = effects
        .iter()
        .map(|insert| insert.type_id.as_str())
        .collect();
    assert_eq!(
        types,
        vec!["builtin.lowpass", "builtin.delay"],
        "the preset replaced the rack, in order"
    );
}

#[test]
fn an_instrument_preset_carries_its_zones() {
    let fixture = fixture();
    let source = fixture.path.parent().unwrap().join("hit.wav");
    support::write_test_wav(&source);
    let imported = ok(json!({
        "protocol_version": 1,
        "operation": "import_sample",
        "path": fixture.path,
        "source": source
    }));
    let sampler = ok(json!({
        "protocol_version": 1,
        "operation": "add_sampler",
        "path": fixture.path,
        "name": "Kit",
        "zones": [{"asset_id": imported["asset_id"], "root_pitch_hz": 440.0}]
    }));

    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "save_preset",
        "path": fixture.path,
        "name": "Kit One",
        "asset": {"asset_id": sampler["asset_id"]}
    }));

    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "set_sampler_zones",
        "path": fixture.path,
        "asset_id": sampler["asset_id"],
        "zones": []
    }));
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "apply_preset",
        "path": fixture.path,
        "preset_id": "kit-one",
        "asset": {"asset_id": sampler["asset_id"]}
    }));

    let project = fixture.project();
    let asset = project
        .assets
        .iter()
        .find(|asset| asset.id.to_string() == sampler["asset_id"].as_str().unwrap())
        .expect("the sampler");
    let AudioAssetSource::Sampler { zones, .. } = &asset.source else {
        panic!("still a sampler");
    };
    assert_eq!(zones.len(), 1, "the preset restored the mapping");
}

#[test]
fn a_preset_moves_between_libraries_by_export_and_import() {
    let fixture = fixture();
    let square = fixture.synth("saw");
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "save_preset",
        "path": fixture.path,
        "name": "Saw Bass",
        "asset": {"asset_id": square["asset_id"]}
    }));

    let file = fixture.path.parent().unwrap().join("saw-bass.json");
    let elsewhere = fixture.path.parent().unwrap().join("other-library");
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "export_preset",
        "path": fixture.path,
        "preset_id": "saw-bass",
        "kind": "synth",
        "to": file
    }));
    let imported = ok(json!({
        "protocol_version": 1,
        "operation": "import_preset",
        "path": fixture.path,
        "from": file,
        "library": elsewhere
    }));
    assert_eq!(imported["preset_id"], "saw-bass");

    let listed = ok(json!({
        "protocol_version": 1,
        "operation": "list_presets",
        "path": fixture.path,
        "library": elsewhere
    }));
    assert_eq!(listed["user"].as_array().unwrap().len(), 1);
}

#[test]
fn a_preset_from_a_newer_format_is_refused_with_its_reason() {
    let fixture = fixture();
    let square = fixture.synth("square");
    let saved = ok(json!({
        "protocol_version": 1,
        "operation": "save_preset",
        "path": fixture.path,
        "name": "From The Future",
        "asset": {"asset_id": square["asset_id"]}
    }));

    // Rewrite the file as if a newer build had written it.
    let file = PathBuf::from(saved["file"].as_str().unwrap());
    let mut stored: Value =
        serde_json::from_slice(&std::fs::read(&file).expect("read")).expect("parse");
    stored["schema_version"] = json!(99);
    std::fs::write(&file, serde_json::to_vec_pretty(&stored).unwrap()).expect("write");

    let (code, refused) = call(json!({
        "protocol_version": 1,
        "operation": "apply_preset",
        "path": fixture.path,
        "preset_id": "from-the-future",
        "asset": {"asset_id": square["asset_id"]}
    }));
    assert_eq!(code, 6, "{refused}");
    assert_eq!(refused["error"]["code"], "incompatible_preset");
    assert!(
        refused["error"]["message"].as_str().unwrap().contains("99"),
        "{refused}"
    );
}

#[test]
fn a_file_asset_is_not_something_a_preset_can_describe() {
    let fixture = fixture();
    let source = fixture.path.parent().unwrap().join("hit.wav");
    support::write_test_wav(&source);
    let imported = ok(json!({
        "protocol_version": 1,
        "operation": "import_sample",
        "path": fixture.path,
        "source": source
    }));

    let (code, refused) = call(json!({
        "protocol_version": 1,
        "operation": "save_preset",
        "path": fixture.path,
        "name": "Not A Preset",
        "asset": {"asset_id": imported["asset_id"]}
    }));
    assert_eq!(code, 6, "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not a preset"),
        "{refused}"
    );
}
