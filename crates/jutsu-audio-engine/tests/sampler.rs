//! The sampler: which zone answers a note, what pitch it plays at, what a loop
//! does, and what happens when a sample is missing.

use std::sync::Arc;

use jutsu_audio_engine::{MixDiagnostic, SourceAudio, sampler};
use jutsu_audio_model::{AssetId, ClipNote, SampleLoopMode, SamplerZone};

const RATE: u32 = 48_000;
const FRAMES: usize = 480;

fn zone(asset_id: AssetId, root: f64, low: f64, high: f64) -> SamplerZone {
    SamplerZone {
        asset_id,
        root_pitch_hz: root,
        low_pitch_hz: low,
        high_pitch_hz: high,
        low_velocity: 0.0,
        high_velocity: 1.0,
        gain_db: 0.0,
        loop_mode: SampleLoopMode::OneShot,
    }
}

fn note(pitch: f64, velocity: f32) -> ClipNote {
    ClipNote {
        start_frame: 0,
        duration_frames: 240,
        pitch_hz: pitch,
        velocity,
    }
}

/// A short constant source: easy to see where it stops.
fn source(frames: usize, value: f32) -> SourceAudio {
    SourceAudio {
        sample_rate: RATE,
        channels: 1,
        samples: Arc::from(vec![value; frames]),
    }
}

#[allow(clippy::too_many_arguments)]
fn render(
    zones: &[SamplerZone],
    notes: &[ClipNote],
    mut load: impl FnMut(AssetId) -> Result<SourceAudio, String>,
) -> (Vec<f32>, Vec<MixDiagnostic>) {
    let mut diagnostics = Vec::new();
    let output = sampler::render(
        zones,
        notes,
        FRAMES,
        RATE,
        0.0,
        1.0,
        16,
        "clip",
        &mut load,
        &mut diagnostics,
    );
    (output, diagnostics)
}

#[test]
fn the_zone_that_covers_a_note_is_the_one_that_plays_it() {
    let low = AssetId::new();
    let high = AssetId::new();
    let zones = vec![
        zone(low, 220.0, 20.0, 400.0),
        zone(high, 880.0, 400.1, 2_000.0),
    ];

    let (played, _) = render(&zones, &[note(220.0, 1.0)], |asset_id| {
        Ok(source(FRAMES, if asset_id == low { 0.5 } else { -0.5 }))
    });
    assert!(played[0] > 0.0, "the low zone answered: {}", played[0]);

    let (played, _) = render(&zones, &[note(880.0, 1.0)], |asset_id| {
        Ok(source(FRAMES, if asset_id == low { 0.5 } else { -0.5 }))
    });
    assert!(played[0] < 0.0, "the high zone answered: {}", played[0]);
}

#[test]
fn a_velocity_layer_only_answers_inside_its_range() {
    let soft = AssetId::new();
    let hard = AssetId::new();
    let mut soft_zone = zone(soft, 440.0, 20.0, 2_000.0);
    soft_zone.high_velocity = 0.5;
    let mut hard_zone = zone(hard, 440.0, 20.0, 2_000.0);
    hard_zone.low_velocity = 0.5001;
    let zones = vec![soft_zone, hard_zone];

    let (quiet, _) = render(&zones, &[note(440.0, 0.4)], |asset_id| {
        Ok(source(FRAMES, if asset_id == soft { 0.5 } else { -0.5 }))
    });
    assert!(quiet[10] > 0.0, "a soft note took the soft layer");

    let (loud, _) = render(&zones, &[note(440.0, 1.0)], |asset_id| {
        Ok(source(FRAMES, if asset_id == soft { 0.5 } else { -0.5 }))
    });
    assert!(loud[10] < 0.0, "a hard note took the hard layer");
}

#[test]
fn a_note_above_the_root_plays_the_sample_faster() {
    let asset = AssetId::new();
    let zones = vec![zone(asset, 220.0, 20.0, 2_000.0)];
    // A ramp, so the read position is visible in the value.
    let ramp: Vec<f32> = (0..FRAMES)
        .map(|index| index as f32 / FRAMES as f32)
        .collect();
    let material = SourceAudio {
        sample_rate: RATE,
        channels: 1,
        samples: Arc::from(ramp),
    };

    let (root, _) = render(&zones, &[note(220.0, 1.0)], |_| Ok(material.clone()));
    let (octave, _) = render(&zones, &[note(440.0, 1.0)], |_| Ok(material.clone()));
    assert!(
        octave[100] > root[100] * 1.8,
        "an octave up reads twice as far in: {} against {}",
        octave[100],
        root[100]
    );
}

#[test]
fn a_one_shot_stops_at_the_end_of_its_sample_and_a_loop_keeps_going() {
    let asset = AssetId::new();
    let short = source(64, 0.5);

    let one_shot = vec![zone(asset, 440.0, 20.0, 2_000.0)];
    let (played, _) = render(&one_shot, &[note(440.0, 1.0)], |_| Ok(short.clone()));
    assert!(
        played[200].abs() < 1e-6,
        "a one-shot is silent past its sample"
    );

    let mut looping = zone(asset, 440.0, 20.0, 2_000.0);
    looping.loop_mode = SampleLoopMode::Loop {
        start_frame: 0,
        end_frame: 64,
    };
    let (played, _) = render(&[looping], &[note(440.0, 1.0)], |_| Ok(short.clone()));
    assert!(
        played[200].abs() > 0.1,
        "a loop is still sounding while the note is held: {}",
        played[200]
    );
}

#[test]
fn a_missing_sample_is_reported_and_the_rest_still_plays() {
    let missing = AssetId::new();
    let present = AssetId::new();
    let zones = vec![
        zone(missing, 220.0, 20.0, 400.0),
        zone(present, 880.0, 400.1, 2_000.0),
    ];

    let (played, diagnostics) = render(&zones, &[note(880.0, 1.0)], |asset_id| {
        if asset_id == missing {
            Err("no such file".into())
        } else {
            Ok(source(FRAMES, 0.5))
        }
    });
    assert!(played[10] > 0.0, "the zone that is there still plays");
    assert_eq!(diagnostics.len(), 1);
    assert!(
        diagnostics[0].message.contains("cannot be read"),
        "{}",
        diagnostics[0].message
    );
}

#[test]
fn the_same_notes_render_the_same_samples_every_time() {
    let asset = AssetId::new();
    let zones = vec![zone(asset, 440.0, 20.0, 2_000.0)];
    let notes = [
        note(440.0, 0.8),
        ClipNote {
            start_frame: 100,
            ..note(660.0, 0.6)
        },
    ];

    let (first, _) = render(&zones, &notes, |_| Ok(source(FRAMES, 0.4)));
    let (again, _) = render(&zones, &notes, |_| Ok(source(FRAMES, 0.4)));
    assert_eq!(first, again);
}

#[test]
fn the_voice_limit_holds_when_more_notes_start_than_it_allows() {
    let asset = AssetId::new();
    let zones = vec![zone(asset, 440.0, 20.0, 2_000.0)];
    let notes: Vec<ClipNote> = (0..64)
        .map(|index| ClipNote {
            start_frame: 0,
            duration_frames: 240,
            pitch_hz: 200.0 + f64::from(index),
            velocity: 1.0,
        })
        .collect();

    let mut diagnostics = Vec::new();
    let played = sampler::render(
        &zones,
        &notes,
        FRAMES,
        RATE,
        0.0,
        1.0,
        4,
        "clip",
        &mut |_| Ok(source(FRAMES, 0.25)),
        &mut diagnostics,
    );
    assert!(
        played.iter().all(|sample| sample.is_finite()),
        "voice stealing keeps the render finite"
    );
    assert!(
        played[10].abs() <= 4.0 * 0.25 + 1e-3,
        "no more than the voice limit is sounding: {}",
        played[10]
    );
}
