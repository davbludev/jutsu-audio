//! Procedural generation from the machine surface: discovery, preview, and the
//! promise that a golden seed renders the same sound every run.

mod support;

use std::path::Path;

use jutsu_audio_project::ProjectStore;
use serde_json::{Value, json};
use support::{call, ok};

/// Seeds pinned here on purpose: if a generator's output changes, these change
/// with it, and that is exactly the review conversation worth having.
const GOLDEN_SEEDS: [(&str, u64); 5] = [
    ("sfx.impact", 1),
    ("sfx.explosion", 2),
    ("sfx.laser", 3),
    ("sfx.pickup", 4),
    ("sfx.ambience", 5),
];

const FRAMES: u64 = 8_000;

fn preview(generator_type: &str, seed: u64, parameters: Value) -> Value {
    ok(json!({
        "protocol_version": 1,
        "operation": "preview_generator",
        "type_id": generator_type,
        "seed": seed,
        "frame_count": FRAMES,
        "parameters": parameters
    }))
}

fn project_with_lane(directory: &Path) -> (std::path::PathBuf, Value, Value) {
    let path = directory.join("sfx.jutsu-audio.json");
    let created = ok(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Procedural"
    }));
    (
        path,
        created["track_id"].clone(),
        created["layer_id"].clone(),
    )
}

#[test]
fn a_caller_can_discover_every_generator_and_its_schema() {
    let listed = ok(json!({"protocol_version": 1, "operation": "list_extensions"}));
    let generators = listed["extensions"]["generators"].as_array().unwrap();
    assert_eq!(generators.len(), GOLDEN_SEEDS.len());

    for (generator_type, _) in GOLDEN_SEEDS {
        let described = ok(json!({
            "protocol_version": 1,
            "operation": "describe_generator",
            "type_id": generator_type
        }));
        let generator = &described["generator"];
        assert_eq!(generator["type_id"], generator_type);
        assert_eq!(generator["kind"], "generator");

        let parameters = generator["parameters"].as_array().unwrap();
        assert!(!parameters.is_empty(), "{generator_type} declares knobs");
        for parameter in parameters {
            assert!(
                parameter["default_value"].is_object(),
                "{generator_type}.{} has a default a caller can start from",
                parameter["id"]
            );
            assert!(
                parameter["minimum"].is_number() && parameter["maximum"].is_number(),
                "{generator_type}.{} publishes its bounds",
                parameter["id"]
            );
        }
        assert!(
            !generator["presets"].as_array().unwrap().is_empty(),
            "{generator_type} ships presets"
        );
    }
}

#[test]
fn a_golden_seed_previews_identically_every_run() {
    for (generator_type, seed) in GOLDEN_SEEDS {
        let first = preview(generator_type, seed, json!({}));
        let again = preview(generator_type, seed, json!({}));
        assert_eq!(
            first["fingerprint"], again["fingerprint"],
            "{generator_type} is not reproducible"
        );
        assert_eq!(first["frame_count"], FRAMES);
        assert!(
            first["peak"].as_f64().unwrap() > 0.1,
            "{generator_type} previews something audible"
        );
    }
}

#[test]
fn a_different_seed_or_parameter_previews_differently() {
    let base = preview("sfx.impact", 1, json!({}));
    let other_seed = preview("sfx.impact", 2, json!({}));
    assert_ne!(base["fingerprint"], other_seed["fingerprint"]);

    let other_parameter = preview(
        "sfx.impact",
        1,
        json!({"brightness": {"type": "float", "value": 1.0}}),
    );
    assert_ne!(base["fingerprint"], other_parameter["fingerprint"]);
}

#[test]
fn a_preview_can_be_written_to_a_wav_without_touching_a_project() {
    let directory = tempfile::tempdir().expect("temp dir");
    let output = directory.path().join("impact.wav");
    let result = ok(json!({
        "protocol_version": 1,
        "operation": "preview_generator",
        "type_id": "sfx.impact",
        "seed": 9,
        "frame_count": FRAMES,
        "output": output
    }));
    assert_eq!(result["output"], json!(output));

    let reader = hound::WavReader::open(&output).expect("the preview was written");
    assert_eq!(u64::from(reader.duration()), FRAMES);
}

#[test]
fn running_the_same_recipe_twice_produces_the_same_entities_and_replaces_in_place() {
    let directory = tempfile::tempdir().expect("temp dir");
    let (path, track_id, layer_id) = project_with_lane(directory.path());

    let request = json!({
        "protocol_version": 1,
        "operation": "run_generator",
        "path": path,
        "track_id": track_id,
        "layer_id": layer_id,
        "type_id": "sfx.laser",
        "seed": 11,
        "frame_count": FRAMES
    });

    let first = ok(request.clone());
    assert_eq!(first["replaced"], false);
    let second = ok(request);
    assert_eq!(
        second["replaced"], true,
        "a rerun replaces what the recipe produced before"
    );
    assert_eq!(
        first["asset_id"], second["asset_id"],
        "the same recipe names the same asset"
    );
    assert_eq!(first["clip_id"], second["clip_id"]);

    let project = ProjectStore::open(&path).expect("open").project;
    assert_eq!(project.assets.len(), 1, "replacing leaves one asset");
    let clips: usize = project
        .tracks
        .iter()
        .flat_map(|track| &track.layers)
        .map(|layer| layer.clips.len())
        .sum();
    assert_eq!(clips, 1);
}

#[test]
fn a_variant_run_leaves_the_original_alone() {
    let directory = tempfile::tempdir().expect("temp dir");
    let (path, track_id, layer_id) = project_with_lane(directory.path());

    let original = ok(json!({
        "protocol_version": 1,
        "operation": "run_generator",
        "path": path,
        "track_id": track_id,
        "layer_id": layer_id,
        "type_id": "sfx.pickup",
        "seed": 21,
        "frame_count": FRAMES
    }));
    let variant = ok(json!({
        "protocol_version": 1,
        "operation": "run_generator",
        "path": path,
        "track_id": track_id,
        "layer_id": layer_id,
        "type_id": "sfx.pickup",
        "seed": 21,
        "frame_count": FRAMES,
        "start_sample": FRAMES,
        "mode": "new",
        "variant": 1
    }));
    assert_ne!(original["asset_id"], variant["asset_id"]);

    let project = ProjectStore::open(&path).expect("open").project;
    assert_eq!(project.assets.len(), 2, "the original is still there");
}

#[test]
fn a_generated_clip_is_audible_in_an_export_and_stays_the_same_across_exports() {
    let directory = tempfile::tempdir().expect("temp dir");
    let (path, track_id, layer_id) = project_with_lane(directory.path());
    let first_export = directory.path().join("first.wav");
    let second_export = directory.path().join("second.wav");

    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "run_generator",
        "path": path,
        "track_id": track_id,
        "layer_id": layer_id,
        "type_id": "sfx.explosion",
        "seed": 31,
        "frame_count": FRAMES
    }));

    let samples = |output: &Path| {
        let _ = ok(json!({
            "protocol_version": 1,
            "operation": "export_wav",
            "path": path,
            "output": output,
            "encoding": "float32"
        }));
        let mut reader = hound::WavReader::open(output).expect("open export");
        reader
            .samples::<f32>()
            .map(|sample| sample.expect("sample"))
            .collect::<Vec<f32>>()
    };

    let first = samples(&first_export);
    let second = samples(&second_export);
    assert!(
        first.iter().any(|sample| sample.abs() > 0.1),
        "the generated clip is audible"
    );
    assert_eq!(
        first, second,
        "the same project exports the same audio every time"
    );
}

#[test]
fn an_unknown_generator_or_out_of_range_parameter_is_refused_before_anything_changes() {
    let directory = tempfile::tempdir().expect("temp dir");
    let (path, track_id, layer_id) = project_with_lane(directory.path());
    let before = ProjectStore::open(&path).expect("open").project;

    let (code, unknown) = call(json!({
        "protocol_version": 1,
        "operation": "run_generator",
        "path": path,
        "track_id": track_id,
        "layer_id": layer_id,
        "type_id": "sfx.kazoo",
        "seed": 1,
        "frame_count": FRAMES
    }));
    assert_eq!(code, 6, "{unknown}");
    assert_eq!(unknown["error"]["code"], "unknown_extension");
    assert!(
        unknown["error"]["message"]
            .as_str()
            .unwrap()
            .contains("sfx.impact"),
        "the error lists what this build has: {unknown}"
    );

    let (code, out_of_range) = call(json!({
        "protocol_version": 1,
        "operation": "run_generator",
        "path": path,
        "track_id": track_id,
        "layer_id": layer_id,
        "type_id": "sfx.impact",
        "seed": 1,
        "frame_count": FRAMES,
        "parameters": {"weight": {"type": "float", "value": 9.0}}
    }));
    assert_eq!(code, 6, "{out_of_range}");
    assert_eq!(out_of_range["error"]["code"], "invalid_parameter");

    assert_eq!(
        ProjectStore::open(&path).expect("open").project,
        before,
        "a refused run changes nothing"
    );
}
