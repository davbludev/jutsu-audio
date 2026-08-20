//! A complete music cue, start to finish, in one scenario.
//!
//! Write a pattern, arrange it into a song, play it with a synth and a sampler,
//! mix it through a bus with an effect and an automated fade, adjust it from the
//! CLI while the editor holds the project, save, reopen, and export. Every step
//! goes through the shipped surfaces.
//!
//! Written up in `docs/workflows/first-music-cue.md`.

mod support;

use std::path::{Path, PathBuf};

use jutsu_audio_model::Project;
use jutsu_audio_project::ProjectStore;
use serde_json::{Value, json};
use support::{Editor, ok, write_test_wav};

/// 100 BPM in 4/4 at 48 kHz: a beat is 28 800 frames, a bar 115 200.
const RATE: u64 = 48_000;
const BEAT: u64 = 28_800;
const BAR: u64 = BEAT * 4;

fn project_at(path: &Path) -> Project {
    ProjectStore::open(path).expect("open").project
}

fn peak_of(path: &Path) -> f32 {
    let mut reader = hound::WavReader::open(path).expect("open export");
    reader
        .samples::<f32>()
        .map(|sample| sample.expect("sample").abs())
        .fold(0.0_f32, f32::max)
}

fn samples_of(path: &Path) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).expect("open export");
    reader
        .samples::<f32>()
        .map(|sample| sample.expect("sample"))
        .collect()
}

#[test]
fn a_pattern_becomes_an_arranged_mixed_and_exported_cue() {
    let directory = tempfile::tempdir().expect("temp dir");
    let root = directory.path();
    let path = root.join("cue.jutsu-audio.json");
    let source = root.join("kick.wav");
    let export = root.join("cue.wav");
    write_test_wav(&source);

    // ─── 1. a project at a tempo ───
    let created = ok(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Cue"
    }));
    let bass_track = created["track_id"].clone();
    let bass_layer = created["layer_id"].clone();
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "set_tempo_map",
        "path": path,
        "changes": [{"frame": 0, "beats_per_minute": 100.0, "beats_per_bar": 4}]
    }));

    // ─── 2. a pattern, one bar long ───
    let pattern = ok(json!({
        "protocol_version": 1,
        "operation": "add_pattern",
        "path": path,
        "name": "Bass riff",
        "length_frames": BAR,
        "notes": [
            {"start_frame": 0, "duration_frames": BEAT, "pitch_hz": 110.0, "velocity": 0.9},
            {"start_frame": BEAT * 2, "duration_frames": BEAT, "pitch_hz": 146.83, "velocity": 0.8},
            {"start_frame": BEAT * 3, "duration_frames": BEAT / 2, "pitch_hz": 164.81, "velocity": 0.7}
        ]
    }));

    // ─── 3. a synth playing it across four bars ───
    let bass_clip = ok(json!({
        "protocol_version": 1,
        "operation": "add_synth_clip",
        "path": path,
        "track_id": bass_track,
        "layer_id": bass_layer,
        "type_id": "builtin.oscillator",
        "start_sample": 0,
        "duration_samples": BAR * 4,
        "parameters": {"waveform": {"type": "text", "value": "saw"}}
    }));
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "set_clip_pattern",
        "path": path,
        "clip_id": bass_clip["clip_id"],
        "pattern_id": pattern["pattern_id"]
    }));

    let arranged = project_at(&path);
    let notes: usize = arranged
        .tracks
        .iter()
        .flat_map(|track| &track.layers)
        .flat_map(|layer| &layer.clips)
        .map(|clip| clip.resolved_notes(&arranged.patterns).len())
        .sum();
    assert_eq!(notes, 12, "one bar of three notes, four times over");

    // ─── 4. a sampler on its own track ───
    let drum_track = ok(json!({
        "protocol_version": 1,
        "operation": "add_track",
        "path": path,
        "name": "Drums"
    }));
    let imported = ok(json!({
        "protocol_version": 1,
        "operation": "import_sample",
        "path": path,
        "source": source
    }));
    let sampler = ok(json!({
        "protocol_version": 1,
        "operation": "add_sampler",
        "path": path,
        "name": "Kit",
        "zones": [{"asset_id": imported["asset_id"], "root_pitch_hz": 220.0}]
    }));
    let drum_clip = ok(json!({
        "protocol_version": 1,
        "operation": "add_clip",
        "path": path,
        "asset_id": sampler["asset_id"],
        "track_id": drum_track["track_id"],
        "layer_id": drum_track["layer_id"],
        "start_sample": 0,
        "source_start_sample": 0,
        "duration_samples": BAR * 4
    }));
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "set_clip_notes",
        "path": path,
        "clip_id": drum_clip["clip_id"],
        "notes": (0..16)
            .map(|beat| json!({
                "start_frame": BEAT * beat,
                "duration_frames": BEAT / 4,
                "pitch_hz": 220.0,
                "velocity": if beat % 4 == 0 { 1.0 } else { 0.6 }
            }))
            .collect::<Vec<Value>>()
    }));

    // ─── 5. quantise and humanise, reproducibly ───
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "quantise_clip",
        "path": path,
        "clip_id": drum_clip["clip_id"],
        "divisions_per_beat": 4
    }));
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "humanise_clip",
        "path": path,
        "clip_id": drum_clip["clip_id"],
        "seed": 7,
        "timing_frames": 300,
        "velocity_amount": 0.15
    }));

    // ─── 6. a mix: a bus, an effect, levels, and an automated fade ───
    let bus = ok(json!({
        "protocol_version": 1,
        "operation": "add_bus",
        "path": path,
        "name": "Music"
    }));
    for track in [&bass_track, &drum_track["track_id"]] {
        let _ = ok(json!({
            "protocol_version": 1,
            "operation": "set_track_output",
            "path": path,
            "track_id": track,
            "output_bus_id": bus["bus_id"]
        }));
    }
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "set_track_parameter",
        "path": path,
        "track_id": bass_track,
        "key": "gain_db",
        "value": {"type": "float", "value": -4.0}
    }));
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "add_effect",
        "path": path,
        "bus": {"bus_id": bus["bus_id"]},
        "type_id": "builtin.reverb",
        "parameters": {"size": {"type": "float", "value": 0.4}}
    }));
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "add_automation_lane",
        "path": path,
        "target": {"type": "bus", "bus_id": bus["bus_id"]},
        "parameter": "gain_db",
        "points": [
            {"frame": BAR * 3, "value": 0.0},
            {"frame": BAR * 4, "value": -60.0}
        ]
    }));

    // ─── 7. the editor opens it, and the CLI keeps working ───
    let editor = Editor::open(&path);
    let adjusted = ok(json!({
        "protocol_version": 1,
        "operation": "transpose_clip",
        "path": path,
        "clip_id": bass_clip["clip_id"],
        "semitones": -12.0
    }));
    assert_eq!(adjusted["delivery"], "session");

    let live = editor.project();
    let live_pitch = live
        .tracks
        .iter()
        .flat_map(|track| &track.layers)
        .flat_map(|layer| &layer.clips)
        .find(|clip| clip.id.to_string() == bass_clip["clip_id"].as_str().unwrap())
        .map(|clip| clip.resolved_notes(&live.patterns)[0].pitch_hz)
        .expect("the bass clip");
    assert!(
        (live_pitch - 55.0).abs() < 0.5,
        "the editor sees the octave drop: {live_pitch}"
    );
    assert!(
        project_at(&path)
            .tracks
            .iter()
            .flat_map(|track| &track.layers)
            .flat_map(|layer| &layer.clips)
            .all(|clip| clip.notes.is_empty() || clip.notes[0].pitch_hz > 100.0),
        "and the file still holds what was last saved"
    );

    // ─── 8. save and close ───
    editor.save();
    drop(editor);

    // ─── 9. reopen and export ───
    let reopened = project_at(&path);
    assert_eq!(reopened.tracks.len(), 2);
    assert_eq!(reopened.patterns.len(), 1);
    assert_eq!(reopened.automation.len(), 1);
    assert_eq!(reopened.tempo[0].beats_per_minute, 100.0);

    let exported = ok(json!({
        "protocol_version": 1,
        "operation": "export_wav",
        "path": path,
        "output": export,
        "encoding": "float32"
    }));
    assert_eq!(exported["frame_count"], BAR * 4);
    let peak = peak_of(&export);
    assert!(peak > 0.05, "the cue is audible: {peak}");
    assert!(peak <= 1.0, "and inside full scale: {peak}");

    // ─── 10. and it is the same cue every time ───
    let again = root.join("cue-again.wav");
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "export_wav",
        "path": path,
        "output": again,
        "encoding": "float32"
    }));
    assert_eq!(
        samples_of(&export),
        samples_of(&again),
        "pattern, sampler, effects and automation are deterministic together"
    );

    // The automated fade means the last bar ends far quieter than the first.
    let samples = samples_of(&export);
    let first_bar = &samples[..(BAR * 2) as usize];
    let last_frames = &samples[samples.len() - (BEAT / 2) as usize..];
    let loudest = |window: &[f32]| window.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
    assert!(
        loudest(last_frames) < loudest(first_bar) * 0.2,
        "the fade landed: {} against {}",
        loudest(last_frames),
        loudest(first_bar)
    );
}

#[test]
fn the_cue_survives_being_bundled_and_opened_somewhere_else() {
    let directory = tempfile::tempdir().expect("temp dir");
    let root = directory.path();
    let path = root.join("cue.jutsu-audio.json");
    let source = root.join("kick.wav");
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
        "duration_samples": RATE
    }));

    let bundle = root.join("bundle");
    let report = ok(json!({
        "protocol_version": 1,
        "operation": "bundle_project",
        "path": path,
        "destination": bundle
    }));
    let bundled = PathBuf::from(report["project"].as_str().unwrap());

    let output = bundle.join("mix.wav");
    let exported = ok(json!({
        "protocol_version": 1,
        "operation": "export_wav",
        "path": bundled,
        "output": output,
        "encoding": "float32"
    }));
    assert_eq!(exported["frame_count"], RATE);
    assert!(peak_of(&output) > 0.0, "the bundled cue still plays");
}
