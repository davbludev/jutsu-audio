//! A stress harness for the mixdown, run by hand rather than asserted.
//!
//! Timing on a shared machine is not a pass/fail signal — the deterministic
//! guarantees live in the tests. What this gives is a number to compare against
//! the last one, on the same machine, when a change looks like it might cost
//! something.
//!
//! ```bash
//! cargo bench -p jutsu-audio-engine
//! ```

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use jutsu_audio_engine::{SourceAudio, mix_project_metered};
use jutsu_audio_extensions::{
    ExtensionRegistries, register_builtin, register_builtin_effects, register_sfx_generators,
};
use jutsu_audio_model::{
    Asset, AssetId, AudioAssetSource, BusId, CURRENT_PROJECT_SCHEMA_VERSION, Clip, ClipId,
    ClipNote, EffectId, EffectInsert, Layer, LayerId, MixerBus, ParameterValue, Project, ProjectId,
    ProjectMetadata, Track, TrackId,
};

const RATE: u32 = 48_000;
/// Ten seconds of timeline: long enough to dwarf any fixed cost.
const FRAMES: u64 = RATE as u64 * 10;

fn registries() -> ExtensionRegistries {
    let mut registries = ExtensionRegistries::default();
    register_builtin(&mut registries).expect("synths");
    register_builtin_effects(&mut registries).expect("effects");
    register_sfx_generators(&mut registries).expect("generators");
    registries
}

fn source() -> SourceAudio {
    SourceAudio {
        sample_rate: RATE,
        channels: 1,
        samples: Arc::from(
            (0..FRAMES)
                .map(|frame| ((frame % 480) as f32 / 240.0) - 1.0)
                .collect::<Vec<f32>>(),
        ),
    }
}

fn effect(type_id: &str) -> EffectInsert {
    EffectInsert {
        id: EffectId::new(),
        type_id: type_id.into(),
        state_version: 1,
        parameters: BTreeMap::new(),
        enabled: true,
        wet: 0.5,
        sidechain: None,
    }
}

/// `tracks` tracks of sample clips, `synth_tracks` of synth clips with
/// `notes_per_track` notes each, all through one bus with effects on it.
fn project(tracks: usize, synth_tracks: usize, notes_per_track: usize) -> Project {
    let master = BusId::new();
    let group = BusId::new();
    let sample = Asset {
        id: AssetId::new(),
        name: "Loop".into(),
        source: AudioAssetSource::File {
            path: "loop.wav".into(),
        },
    };
    let synth = Asset {
        id: AssetId::new(),
        name: "Tone".into(),
        source: AudioAssetSource::Synth {
            type_id: "builtin.oscillator".into(),
            state_version: 1,
            parameters: BTreeMap::new(),
        },
    };
    let sample_id = sample.id;
    let synth_id = synth.id;

    let mut all = Vec::new();
    for index in 0..tracks {
        all.push(Track {
            id: TrackId::new(),
            name: format!("Sample {index}"),
            output_bus_id: group,
            sends: Vec::new(),
            parameters: BTreeMap::from([("gain_db".into(), ParameterValue::Float(-6.0))]),
            layers: vec![Layer {
                id: LayerId::new(),
                name: "Layer".into(),
                clips: vec![Clip {
                    id: ClipId::new(),
                    asset_id: sample_id,
                    start_sample: 0,
                    source_start_sample: 0,
                    duration_samples: FRAMES,
                    parameters: BTreeMap::new(),
                    notes: Vec::new(),
                    pattern_id: None,
                }],
            }],
            effects: vec![effect("builtin.lowpass")],
        });
    }
    for index in 0..synth_tracks {
        let notes = (0..notes_per_track)
            .map(|note| ClipNote {
                start_frame: (FRAMES / notes_per_track.max(1) as u64) * note as u64,
                duration_frames: FRAMES / notes_per_track.max(1) as u64 / 2,
                pitch_hz: 110.0 + (note % 24) as f64 * 7.0,
                velocity: 0.8,
            })
            .collect();
        all.push(Track {
            id: TrackId::new(),
            name: format!("Synth {index}"),
            output_bus_id: group,
            sends: Vec::new(),
            parameters: BTreeMap::new(),
            layers: vec![Layer {
                id: LayerId::new(),
                name: "Layer".into(),
                clips: vec![Clip {
                    id: ClipId::new(),
                    asset_id: synth_id,
                    start_sample: 0,
                    source_start_sample: 0,
                    duration_samples: FRAMES,
                    parameters: BTreeMap::new(),
                    notes,
                    pattern_id: None,
                }],
            }],
            effects: Vec::new(),
        });
    }

    Project {
        schema_version: CURRENT_PROJECT_SCHEMA_VERSION,
        id: ProjectId::new(),
        metadata: ProjectMetadata {
            name: "Stress".into(),
            properties: BTreeMap::new(),
        },
        assets: vec![sample, synth],
        buses: vec![
            MixerBus {
                id: master,
                name: "Master".into(),
                output_bus_id: None,
                parameters: BTreeMap::new(),
                effects: vec![effect("builtin.compressor")],
            },
            MixerBus {
                id: group,
                name: "Group".into(),
                output_bus_id: Some(master),
                parameters: BTreeMap::new(),
                effects: vec![effect("builtin.reverb"), effect("builtin.delay")],
            },
        ],
        master_bus_id: master,
        tracks: all,
        markers: Vec::new(),
        loop_region: None,
        automation: Vec::new(),
        tempo: Vec::new(),
        patterns: Vec::new(),
    }
}

/// Mixes once and reports how long it took against how long it plays for.
fn run(label: &str, project: &Project, registries: &ExtensionRegistries) {
    let material = source();
    let started = Instant::now();
    let output = mix_project_metered(project, RATE, registries, |_| Ok(material.clone()))
        .expect("the stress project mixes");
    let elapsed = started.elapsed();

    let frames = output
        .snapshot
        .as_ref()
        .map_or(0, jutsu_audio_engine::PlaybackSnapshot::frame_count);
    let audio_seconds = frames as f64 / f64::from(RATE);
    let realtime = audio_seconds / elapsed.as_secs_f64().max(f64::EPSILON);
    println!(
        "{label}: {:.3}s to render {audio_seconds:.1}s of audio ({realtime:.0}x real time), \
         peak {:.3}",
        elapsed.as_secs_f64(),
        output.meters.master
    );
}

fn main() {
    let registries = registries();
    run("8 sample tracks", &project(8, 0, 0), &registries);
    run("32 sample tracks", &project(32, 0, 0), &registries);
    run("8 synth tracks, 64 notes", &project(0, 8, 64), &registries);
    run("16 tracks mixed", &project(8, 8, 64), &registries);
    run("64 tracks mixed", &project(32, 32, 64), &registries);
}
