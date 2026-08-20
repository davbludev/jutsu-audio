//! Bundling and relinking from the machine surface: a project packed up,
//! opened somewhere else, and put back together after its audio moved.

mod support;

use std::path::{Path, PathBuf};

use jutsu_audio_model::{AudioAssetSource, Project};
use jutsu_audio_project::ProjectStore;
use serde_json::{Value, json};
use support::{ok, write_test_wav};

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    path: PathBuf,
    asset_id: Value,
}

/// A saved project with one imported sample and one saved preset.
fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("temp dir");
    let root = directory.path().to_path_buf();
    let path = root.join("song.jutsu-audio.json");
    let source = root.join("hit.wav");
    write_test_wav(&source);

    let created = ok(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Portable"
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
    let synth = ok(json!({
        "protocol_version": 1,
        "operation": "add_synth_clip",
        "path": path,
        "track_id": created["track_id"],
        "layer_id": created["layer_id"],
        "type_id": "builtin.oscillator",
        "start_sample": 480,
        "duration_samples": 480
    }));
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "save_preset",
        "path": path,
        "name": "Travelling Lead",
        "asset": {"asset_id": synth["asset_id"]}
    }));

    Fixture {
        _directory: directory,
        root,
        path,
        asset_id: imported["asset_id"].clone(),
    }
}

fn project_at(path: &Path) -> Project {
    ProjectStore::open(path).expect("open").project
}

fn managed_path(project: &Project) -> String {
    project
        .assets
        .iter()
        .find_map(|asset| match &asset.source {
            AudioAssetSource::ManagedFile { path, .. } => Some(path.clone()),
            _ => None,
        })
        .expect("a managed asset")
}

#[test]
fn a_bundle_carries_the_project_its_audio_and_its_presets() {
    let fixture = fixture();
    let destination = fixture.root.join("bundle");

    let report = ok(json!({
        "protocol_version": 1,
        "operation": "bundle_project",
        "path": fixture.path,
        "destination": destination
    }));
    assert_eq!(report["assets_copied"], 1);
    assert_eq!(report["presets_copied"], 1);
    assert!(report["unresolved"].as_array().unwrap().is_empty());

    // What a bundle is for: it opens, and everything it names is there.
    let bundled_path = PathBuf::from(report["project"].as_str().unwrap());
    let checked = ok(json!({
        "protocol_version": 1,
        "operation": "check_assets",
        "path": bundled_path
    }));
    assert!(checked["unresolved"].as_array().unwrap().is_empty());
    assert!(
        checked["absolute_paths"].as_array().unwrap().is_empty(),
        "nothing in a bundle names a place outside it: {checked}"
    );
    assert!(
        destination
            .join("presets/synths/travelling-lead.json")
            .exists()
    );
}

#[test]
fn a_bundle_still_renders_after_being_moved_somewhere_else() {
    let fixture = fixture();
    let destination = fixture.root.join("bundle");
    let report = ok(json!({
        "protocol_version": 1,
        "operation": "bundle_project",
        "path": fixture.path,
        "destination": destination
    }));

    // Move the whole bundle: the point is that nothing inside cared where it was.
    let moved = fixture.root.join("moved-bundle");
    std::fs::rename(&destination, &moved).expect("move the bundle");
    let moved_project = moved.join(
        PathBuf::from(report["project"].as_str().unwrap())
            .file_name()
            .unwrap(),
    );

    let output = moved.join("mix.wav");
    let exported = ok(json!({
        "protocol_version": 1,
        "operation": "export_wav",
        "path": moved_project,
        "output": output,
        "encoding": "float32"
    }));
    assert!(exported["frame_count"].as_u64().unwrap() > 0);

    let mut reader = hound::WavReader::open(&output).expect("open export");
    let peak = reader
        .samples::<f32>()
        .map(|sample| sample.expect("sample").abs())
        .fold(0.0_f32, f32::max);
    assert!(peak > 0.0, "the moved bundle still makes a sound");
}

#[test]
fn moved_audio_is_found_again_by_fingerprint_and_the_project_follows() {
    let fixture = fixture();
    let project = project_at(&fixture.path);
    let relative = managed_path(&project);

    // Move the audio out of the project directory, under a new name.
    let elsewhere = fixture.root.join("archive");
    std::fs::create_dir_all(&elsewhere).expect("create");
    std::fs::rename(fixture.root.join(&relative), elsewhere.join("renamed.wav"))
        .expect("move the audio");

    let before = ok(json!({
        "protocol_version": 1,
        "operation": "check_assets",
        "path": fixture.path
    }));
    assert_eq!(before["unresolved"].as_array().unwrap().len(), 1);

    let relinked = ok(json!({
        "protocol_version": 1,
        "operation": "relink_assets",
        "path": fixture.path,
        "search_paths": [elsewhere]
    }));
    assert_eq!(relinked["relinked"].as_array().unwrap().len(), 1);
    assert_eq!(relinked["relinked"][0]["asset_id"], fixture.asset_id);
    assert!(relinked["unresolved"].as_array().unwrap().is_empty());

    let after = ok(json!({
        "protocol_version": 1,
        "operation": "check_assets",
        "path": fixture.path
    }));
    assert!(
        after["unresolved"].as_array().unwrap().is_empty(),
        "the project now points at where the audio actually is: {after}"
    );
    assert!(managed_path(&project_at(&fixture.path)).contains("renamed"));
}

#[test]
fn audio_that_is_nowhere_is_reported_rather_than_quietly_dropped() {
    let fixture = fixture();
    let project = project_at(&fixture.path);
    std::fs::remove_file(fixture.root.join(managed_path(&project))).expect("remove");

    let empty = fixture.root.join("empty");
    std::fs::create_dir_all(&empty).expect("create");
    let relinked = ok(json!({
        "protocol_version": 1,
        "operation": "relink_assets",
        "path": fixture.path,
        "search_paths": [empty]
    }));
    assert!(relinked["relinked"].as_array().unwrap().is_empty());
    assert_eq!(relinked["unresolved"].as_array().unwrap().len(), 1);
    assert!(
        relinked["unresolved"][0]["message"]
            .as_str()
            .unwrap()
            .contains("missing"),
        "{relinked}"
    );
}

#[test]
fn a_bundle_of_a_project_with_missing_audio_is_written_and_says_what_is_missing() {
    let fixture = fixture();
    let project = project_at(&fixture.path);
    std::fs::remove_file(fixture.root.join(managed_path(&project))).expect("remove");

    let destination = fixture.root.join("bundle");
    let report = ok(json!({
        "protocol_version": 1,
        "operation": "bundle_project",
        "path": fixture.path,
        "destination": destination
    }));
    assert_eq!(report["assets_copied"], 0);
    assert_eq!(report["unresolved"].as_array().unwrap().len(), 1);
    assert!(
        PathBuf::from(report["project"].as_str().unwrap()).exists(),
        "a project missing one sound still bundles"
    );
}
