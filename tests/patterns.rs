//! Patterns and note transforms, from the machine surface: a pattern played by
//! a clip, quantised, transposed and humanised, and a humanisation that repeats
//! exactly when it is asked for again.

mod support;

use jutsu_audio_model::{ClipNote, Project};
use jutsu_audio_project::ProjectStore;
use serde_json::{Value, json};
use support::{call, ok};

const RATE: u64 = 48_000;

struct Fixture {
    _directory: tempfile::TempDir,
    path: std::path::PathBuf,
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
        "name": "Song"
    }));
    Fixture {
        _directory: directory,
        path,
        track_id: created["track_id"].clone(),
        layer_id: created["layer_id"].clone(),
    }
}

impl Fixture {
    /// A synth clip of `duration` frames with the notes given.
    fn synth_clip(&self, duration: u64, notes: Value) -> Value {
        ok(json!({
            "protocol_version": 1,
            "operation": "add_synth_clip",
            "path": self.path,
            "track_id": self.track_id,
            "layer_id": self.layer_id,
            "type_id": "builtin.oscillator",
            "start_sample": 0,
            "duration_samples": duration,
            "notes": notes
        }))
    }

    fn project(&self) -> Project {
        ProjectStore::open(&self.path).expect("open").project
    }
}

fn clip_notes(project: &Project) -> Vec<ClipNote> {
    project
        .tracks
        .iter()
        .flat_map(|track| &track.layers)
        .flat_map(|layer| &layer.clips)
        .flat_map(|clip| clip.resolved_notes(&project.patterns))
        .collect()
}

#[test]
fn a_pattern_repeats_for_the_length_of_the_clip_that_plays_it() {
    let fixture = fixture();
    let clip = fixture.synth_clip(RATE * 2, json!([]));

    let pattern = ok(json!({
        "protocol_version": 1,
        "operation": "add_pattern",
        "path": fixture.path,
        "name": "Pulse",
        "length_frames": RATE / 2,
        "notes": [{"start_frame": 0, "duration_frames": 4_800, "pitch_hz": 440.0}]
    }));
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "set_clip_pattern",
        "path": fixture.path,
        "clip_id": clip["clip_id"],
        "pattern_id": pattern["pattern_id"]
    }));

    let project = fixture.project();
    let notes = clip_notes(&project);
    assert_eq!(notes.len(), 4, "half-second pattern over two seconds");
    assert_eq!(notes[1].start_frame, RATE / 2);
    assert_eq!(notes[3].start_frame, RATE / 2 * 3);
}

#[test]
fn editing_a_pattern_changes_every_clip_playing_it() {
    let fixture = fixture();
    let first = fixture.synth_clip(RATE, json!([]));
    let second = fixture.synth_clip(RATE, json!([]));
    let pattern = ok(json!({
        "protocol_version": 1,
        "operation": "add_pattern",
        "path": fixture.path,
        "name": "Riff",
        "length_frames": RATE,
        "notes": [{"start_frame": 0, "duration_frames": 2_400, "pitch_hz": 220.0}]
    }));
    for clip in [&first, &second] {
        let _ = ok(json!({
            "protocol_version": 1,
            "operation": "set_clip_pattern",
            "path": fixture.path,
            "clip_id": clip["clip_id"],
            "pattern_id": pattern["pattern_id"]
        }));
    }

    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "set_pattern_notes",
        "path": fixture.path,
        "pattern_id": pattern["pattern_id"],
        "length_frames": RATE,
        "notes": [
            {"start_frame": 0, "duration_frames": 2_400, "pitch_hz": 330.0},
            {"start_frame": 12_000, "duration_frames": 2_400, "pitch_hz": 440.0}
        ]
    }));

    let notes = clip_notes(&fixture.project());
    assert_eq!(notes.len(), 4, "two notes in each of two clips");
    assert!(
        notes.iter().all(|note| note.pitch_hz > 300.0),
        "both clips followed the pattern: {notes:?}"
    );
}

#[test]
fn removing_a_pattern_unlinks_the_clips_that_played_it() {
    let fixture = fixture();
    let clip = fixture.synth_clip(RATE, json!([]));
    let pattern = ok(json!({
        "protocol_version": 1,
        "operation": "add_pattern",
        "path": fixture.path,
        "name": "Riff",
        "length_frames": RATE,
        "notes": [{"start_frame": 0, "duration_frames": 2_400, "pitch_hz": 220.0}]
    }));
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "set_clip_pattern",
        "path": fixture.path,
        "clip_id": clip["clip_id"],
        "pattern_id": pattern["pattern_id"]
    }));

    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "remove_pattern",
        "path": fixture.path,
        "pattern_id": pattern["pattern_id"]
    }));

    let project = fixture.project();
    assert!(project.patterns.is_empty());
    assert!(
        project.validate().is_empty(),
        "no clip is left pointing at a pattern that is gone: {:?}",
        project.validate()
    );
    assert!(clip_notes(&project).is_empty());
}

#[test]
fn quantising_snaps_notes_to_the_grid_the_tempo_defines() {
    let fixture = fixture();
    // At 120 BPM a sixteenth is 6 000 frames.
    let clip = fixture.synth_clip(
        RATE,
        json!([
            {"start_frame": 6_400, "duration_frames": 2_400, "pitch_hz": 440.0},
            {"start_frame": 11_500, "duration_frames": 2_400, "pitch_hz": 440.0}
        ]),
    );

    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "quantise_clip",
        "path": fixture.path,
        "clip_id": clip["clip_id"],
        "divisions_per_beat": 4
    }));

    let notes = clip_notes(&fixture.project());
    assert_eq!(notes[0].start_frame, 6_000);
    assert_eq!(notes[1].start_frame, 12_000);
}

#[test]
fn transposing_moves_every_pitch_and_leaves_the_timing_alone() {
    let fixture = fixture();
    let clip = fixture.synth_clip(
        RATE,
        json!([{"start_frame": 1_000, "duration_frames": 2_400, "pitch_hz": 440.0}]),
    );

    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "transpose_clip",
        "path": fixture.path,
        "clip_id": clip["clip_id"],
        "semitones": 12.0
    }));

    let notes = clip_notes(&fixture.project());
    assert!((notes[0].pitch_hz - 880.0).abs() < 1e-6, "an octave up");
    assert_eq!(notes[0].start_frame, 1_000);
}

#[test]
fn humanising_with_the_same_seed_gives_the_same_result_twice() {
    let run = || {
        let fixture = fixture();
        let clip = fixture.synth_clip(
            RATE,
            json!([
                {"start_frame": 0, "duration_frames": 2_400, "pitch_hz": 440.0},
                {"start_frame": 12_000, "duration_frames": 2_400, "pitch_hz": 550.0},
                {"start_frame": 24_000, "duration_frames": 2_400, "pitch_hz": 660.0}
            ]),
        );
        let _ = ok(json!({
            "protocol_version": 1,
            "operation": "humanise_clip",
            "path": fixture.path,
            "clip_id": clip["clip_id"],
            "seed": 42,
            "timing_frames": 400,
            "velocity_amount": 0.2
        }));
        clip_notes(&fixture.project())
    };

    let first = run();
    let second = run();
    assert_eq!(first, second, "the same seed is the same humanisation");
    assert_ne!(
        first[1].start_frame, 12_000,
        "and it actually moved something: {first:?}"
    );
    for (note, original) in first.iter().zip([0_u64, 12_000, 24_000]) {
        assert!(
            note.start_frame.abs_diff(original) <= 400,
            "each nudge stays inside the bound it was given: {note:?}"
        );
    }
}

#[test]
fn looping_a_clip_repeats_its_notes_at_a_fixed_period() {
    let fixture = fixture();
    let clip = fixture.synth_clip(
        RATE * 2,
        json!([{"start_frame": 0, "duration_frames": 2_400, "pitch_hz": 440.0}]),
    );

    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "loop_clip_notes",
        "path": fixture.path,
        "clip_id": clip["clip_id"],
        "period_frames": 12_000,
        "repeats": 3
    }));

    let notes = clip_notes(&fixture.project());
    let starts: Vec<u64> = notes.iter().map(|note| note.start_frame).collect();
    assert_eq!(starts, vec![0, 12_000, 24_000, 36_000]);
}

#[test]
fn a_transform_on_a_clip_that_is_gone_changes_nothing() {
    let fixture = fixture();
    let before = fixture.project();
    let (code, refused) = call(json!({
        "protocol_version": 1,
        "operation": "transpose_clip",
        "path": fixture.path,
        "clip_id": "00000000-0000-4000-8000-000000000000",
        "semitones": 2.0
    }));
    assert_eq!(code, 4, "{refused}");
    assert_eq!(refused["error"]["code"], "command_failed");
    assert_eq!(fixture.project(), before);
}
