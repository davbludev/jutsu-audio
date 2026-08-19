//! The first complete SFX workflow, start to finish, in one scenario.
//!
//! Import a sample, layer it, trim it, fade it, loop it, adjust it from the CLI
//! while the editor holds the project, save, and export. Every step goes
//! through the shipped surfaces — the CLI request handler and the session host
//! — so what passes here is what a user gets.
//!
//! Written up in `docs/workflows/first-sfx-edit.md`.

mod support;

use std::path::Path;

use jutsu_audio_engine::{SnapshotExchange, SystemAudioOutput, TransportController};
use jutsu_audio_model::Project;
use jutsu_audio_project::ProjectStore;
use serde_json::json;
use support::{Editor, call, ok, write_test_wav};

/// Frames of the imported sample used by each clip.
const CLIP_FRAMES: u64 = 480;
/// Where the second layer's clip starts, overlapping the first by half.
const TAIL_START: u64 = 240;

fn clips(project: &Project) -> Vec<&jutsu_audio_model::Clip> {
    project
        .tracks
        .iter()
        .flat_map(|track| &track.layers)
        .flat_map(|layer| &layer.clips)
        .collect()
}

/// The loudest absolute sample in a float WAV, and how many frames it holds.
fn peak_and_frames(path: &Path) -> (f32, u64) {
    let mut reader = hound::WavReader::open(path).expect("open export");
    let frames = u64::from(reader.duration());
    let peak = reader
        .samples::<f32>()
        .map(|sample| sample.expect("sample").abs())
        .fold(0.0_f32, f32::max);
    (peak, frames)
}

#[test]
fn a_sample_becomes_a_layered_trimmed_faded_looped_and_exported_sound() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("hit.jutsu-audio.json");
    let source = directory.path().join("blip.wav");
    let export = directory.path().join("hit.wav");
    write_test_wav(&source);

    // ─── 1. create and import ───
    let created = ok(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Impact"
    }));
    let imported = ok(json!({
        "protocol_version": 1,
        "operation": "import_sample",
        "path": path,
        "source": source
    }));
    assert_eq!(imported["status"], "added");
    let asset_id = imported["asset_id"].clone();
    let track_id = created["track_id"].clone();
    let layer_id = created["layer_id"].clone();

    // ─── 2. layer: a second lane on the same track ───
    let tail_lane = ok(json!({
        "protocol_version": 1,
        "operation": "add_layer",
        "path": path,
        "track_id": track_id,
        "name": "Tail"
    }));

    let body = ok(json!({
        "protocol_version": 1,
        "operation": "add_clip",
        "path": path,
        "asset_id": asset_id,
        "track_id": track_id,
        "layer_id": layer_id,
        "start_sample": 0,
        "source_start_sample": 0,
        "duration_samples": CLIP_FRAMES
    }));
    let tail = ok(json!({
        "protocol_version": 1,
        "operation": "add_clip",
        "path": path,
        "asset_id": asset_id,
        "track_id": track_id,
        "layer_id": tail_lane["layer_id"],
        "start_sample": TAIL_START,
        "source_start_sample": 0,
        "duration_samples": CLIP_FRAMES
    }));

    // ─── 3. trim and fade ───
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "update_clip",
        "path": path,
        "clip_id": body["clip_id"],
        "start_sample": 0,
        "source_start_sample": 0,
        "duration_samples": 360,
        "gain_db": 0.0
    }));
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "crossfade_clips",
        "path": path,
        "first_clip_id": body["clip_id"],
        "second_clip_id": tail["clip_id"]
    }));

    // ─── 4. loop the whole sound ───
    let loop_end = TAIL_START + CLIP_FRAMES;
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "set_loop_region",
        "path": path,
        "start_frame": 0,
        "end_frame": loop_end
    }));

    // ─── 5. the editor opens it, and the CLI keeps working ───
    let editor = Editor::open(&path);
    let status = ok(json!({
        "protocol_version": 1,
        "operation": "session_status",
        "path": path
    }));
    assert_eq!(status["attached"], true);

    let adjusted = ok(json!({
        "protocol_version": 1,
        "operation": "set_clip_pan",
        "path": path,
        "clip_id": tail["clip_id"],
        "pan": 0.5
    }));
    assert_eq!(
        adjusted["delivery"], "session",
        "with the editor open, an edit goes through it"
    );

    let live = editor.project();
    assert_eq!(clips(&live).len(), 2, "the editor sees both layers");
    assert!(
        clips(&live)
            .iter()
            .any(|clip| clip.parameters.contains_key("pan")),
        "the editor sees the pan the CLI just set"
    );
    assert_eq!(
        ProjectStore::open(&path)
            .expect("open")
            .project
            .tracks
            .iter()
            .flat_map(|track| &track.layers)
            .flat_map(|layer| &layer.clips)
            .filter(|clip| clip.parameters.contains_key("pan"))
            .count(),
        0,
        "and the file still holds what was last saved, not the live state"
    );

    // ─── 6. the editor saves, and the session ends ───
    editor.save();
    drop(editor);

    // ─── 7. export the loop ───
    let exported = ok(json!({
        "protocol_version": 1,
        "operation": "export_wav",
        "path": path,
        "output": export,
        "encoding": "float32",
        "use_loop_region": true
    }));
    assert_eq!(exported["frame_count"], loop_end);

    let (peak, frames) = peak_and_frames(&export);
    assert_eq!(frames, loop_end, "the file is exactly the looped span");
    assert!(peak > 0.0, "the export is not silence");
    assert!(
        peak <= 1.0,
        "the layered mix stays inside full scale, got {peak}"
    );

    // ─── 8. the saved project is what was exported from ───
    let saved = ProjectStore::open(&path).expect("reopen").project;
    assert_eq!(clips(&saved).len(), 2);
    assert!(saved.loop_region.is_some_and(|region| region.is_active()));
    assert!(
        clips(&saved)
            .iter()
            .any(|clip| clip.parameters.contains_key("fade_out_samples")),
        "the cross-fade survived the round trip"
    );
}

#[test]
fn a_refused_edit_in_the_middle_of_the_workflow_leaves_the_project_alone() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("hit.jutsu-audio.json");
    let _ = ok(json!({
        "protocol_version": 1,
        "operation": "create_project",
        "path": path,
        "name": "Impact"
    }));

    let before = ProjectStore::open(&path).expect("open").project;
    let (code, refused) = call(json!({
        "protocol_version": 1,
        "operation": "set_loop_region",
        "path": path,
        "start_frame": 500,
        "end_frame": 100
    }));
    assert_eq!(code, 4, "{refused}");
    assert_eq!(refused["error"]["code"], "command_failed");
    assert_eq!(
        ProjectStore::open(&path).expect("open").project,
        before,
        "a rejected batch changes nothing on disk"
    );
}

/// The one part of the workflow that needs hardware. Reported, never failed:
/// a build machine without an output device still runs everything above.
#[test]
fn preview_playback_reports_whether_this_machine_has_an_output_device() {
    let transport = TransportController::new();
    let snapshots = SnapshotExchange::new(None);
    match SystemAudioOutput::open_default(snapshots.reader(), transport.reader()) {
        Ok(_output) => println!("audio device available: preview can be heard"),
        Err(error) => println!("no audio device ({error:?}); preview is unavailable here"),
    }
}
