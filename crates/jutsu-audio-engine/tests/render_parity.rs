//! Real-time and offline renders of the same project, compared sample for
//! sample.
//!
//! The reference project spans everything that can move audio: two tracks
//! routed through a group bus, a sample clip, a synth clip, a generated clip,
//! insert effects on both a track and a bus, and automation on a level. If any
//! of those paths differ between playback and export, this fails.
//!
//! Tolerances are written up in `docs/design/render-parity-and-tolerances.md`.

use std::collections::BTreeMap;
use std::sync::Arc;

use hound::WavReader;
use jutsu_audio_engine::{
    ExportEncoding, ExportRange, MIX_CHANNELS, OfflineExporter, PlaybackRenderer, SnapshotExchange,
    SourceAudio, TransportController, mix_project_metered,
};
use jutsu_audio_extensions::{
    ExtensionRegistries, register_builtin, register_builtin_effects, register_sfx_generators,
};
use jutsu_audio_model::{
    Asset, AssetId, AudioAssetSource, AutomationId, AutomationLane, AutomationTarget, Breakpoint,
    BusId, CURRENT_PROJECT_SCHEMA_VERSION, Clip, ClipId, ClipNote, Curve, EffectId, EffectInsert,
    Layer, LayerId, MixerBus, ParameterValue, Project, ProjectId, ProjectMetadata, Track, TrackId,
};
use tempfile::tempdir;

const RATE: u32 = 48_000;
const FRAMES: u64 = 4_800;

fn registries() -> ExtensionRegistries {
    let mut registries = ExtensionRegistries::default();
    register_builtin(&mut registries).expect("synths");
    register_builtin_effects(&mut registries).expect("effects");
    register_sfx_generators(&mut registries).expect("generators");
    registries
}

/// A deterministic sample source: a slow ramp, so a level change is visible.
fn source() -> SourceAudio {
    SourceAudio {
        sample_rate: RATE,
        channels: 1,
        samples: Arc::from(
            (0..FRAMES)
                .map(|frame| (frame as f32 / FRAMES as f32).mul_add(2.0, -1.0) * 0.5)
                .collect::<Vec<f32>>(),
        ),
    }
}

fn insert(type_id: &str, parameters: &[(&str, f64)]) -> EffectInsert {
    EffectInsert {
        id: EffectId::new(),
        type_id: type_id.into(),
        state_version: 1,
        parameters: parameters
            .iter()
            .map(|(id, value)| ((*id).to_string(), ParameterValue::Float(*value)))
            .collect(),
        enabled: true,
        wet: 0.8,
    }
}

fn clip(asset_id: AssetId, start: u64, notes: Vec<ClipNote>) -> Clip {
    Clip {
        id: ClipId::new(),
        asset_id,
        start_sample: start,
        source_start_sample: 0,
        duration_samples: FRAMES,
        parameters: BTreeMap::from([("gain_db".into(), ParameterValue::Float(-3.0))]),
        notes,
    }
}

fn track(name: &str, output: BusId, clips: Vec<Clip>, effects: Vec<EffectInsert>) -> Track {
    Track {
        id: TrackId::new(),
        name: name.into(),
        output_bus_id: output,
        parameters: BTreeMap::from([("pan".into(), ParameterValue::Float(-0.3))]),
        layers: vec![Layer {
            id: LayerId::new(),
            name: "Layer".into(),
            clips,
        }],
        effects,
    }
}

/// Everything at once: routing, automation, a sample, a synth, a generator and
/// effects on a track and a bus.
fn reference_project() -> Project {
    let master = BusId::new();
    let group = BusId::new();

    let sample = Asset {
        id: AssetId::new(),
        name: "Ramp".into(),
        source: AudioAssetSource::File {
            path: "ramp.wav".into(),
        },
    };
    let synth = Asset {
        id: AssetId::new(),
        name: "Tone".into(),
        source: AudioAssetSource::Synth {
            type_id: "builtin.oscillator".into(),
            state_version: 1,
            parameters: BTreeMap::from([(
                "waveform".into(),
                ParameterValue::Text("triangle".into()),
            )]),
        },
    };
    let generated = Asset {
        id: AssetId::new(),
        name: "Impact".into(),
        source: AudioAssetSource::Generated {
            generator_type: "sfx.impact".into(),
            algorithm_version: 1,
            seed: 5,
            parameters: BTreeMap::from([("weight".into(), ParameterValue::Float(0.7))]),
        },
    };

    let sample_track = track(
        "Samples",
        group,
        vec![clip(sample.id, 0, Vec::new())],
        vec![insert("builtin.lowpass", &[("cutoff_hz", 3_000.0)])],
    );
    let synth_track = track(
        "Synth",
        group,
        vec![
            clip(
                synth.id,
                0,
                vec![ClipNote {
                    start_frame: 0,
                    duration_frames: FRAMES / 2,
                    pitch_hz: 220.0,
                    velocity: 0.8,
                }],
            ),
            clip(generated.id, FRAMES / 2, Vec::new()),
        ],
        Vec::new(),
    );
    let automation = AutomationLane {
        id: AutomationId::new(),
        target: AutomationTarget::Track {
            track_id: sample_track.id,
        },
        parameter: "gain_db".into(),
        points: vec![
            Breakpoint {
                frame: 0,
                value: -24.0,
                curve: Curve::Linear,
            },
            Breakpoint {
                frame: FRAMES,
                value: 0.0,
                curve: Curve::Linear,
            },
        ],
    };

    Project {
        schema_version: CURRENT_PROJECT_SCHEMA_VERSION,
        id: ProjectId::new(),
        metadata: ProjectMetadata {
            name: "Reference".into(),
            properties: BTreeMap::new(),
        },
        assets: vec![sample, synth, generated],
        buses: vec![
            MixerBus {
                id: master,
                name: "Master".into(),
                output_bus_id: None,
                parameters: BTreeMap::from([("gain_db".into(), ParameterValue::Float(-2.0))]),
                effects: Vec::new(),
            },
            MixerBus {
                id: group,
                name: "Group".into(),
                output_bus_id: Some(master),
                parameters: BTreeMap::new(),
                effects: vec![insert(
                    "builtin.delay",
                    &[("delay_ms", 40.0), ("feedback", 0.3), ("damping", 0.4)],
                )],
            },
        ],
        master_bus_id: master,
        tracks: vec![sample_track, synth_track],
        markers: Vec::new(),
        loop_region: None,
        automation: vec![automation],
        tempo: Vec::new(),
    }
}

fn mix(project: &Project) -> jutsu_audio_engine::MixOutput {
    mix_project_metered(project, RATE, &registries(), |_| Ok(source())).expect("mix")
}

/// What the device path produces for the whole snapshot, one block at a time.
fn played(snapshot: Arc<jutsu_audio_engine::PlaybackSnapshot>, block: usize) -> Vec<f32> {
    let exchange = SnapshotExchange::new(Some(Arc::clone(&snapshot)));
    let transport = TransportController::new();
    let mut renderer = PlaybackRenderer::new(
        exchange.reader(),
        transport.reader(),
        snapshot.sample_rate(),
        snapshot.channel_count(),
    );
    transport.play();

    let mut rendered = Vec::with_capacity(snapshot.samples().len());
    let mut buffer = vec![0.0_f32; block * usize::from(snapshot.channel_count())];
    while rendered.len() < snapshot.samples().len() {
        renderer.render(&mut buffer);
        rendered.extend_from_slice(&buffer);
    }
    rendered.truncate(snapshot.samples().len());
    rendered
}

#[test]
fn the_reference_project_renders_identically_in_real_time_and_offline() {
    let project = reference_project();
    let output = mix(&project);
    let snapshot = Arc::new(output.snapshot.expect("the reference project is audible"));

    let directory = tempdir().expect("temp dir");
    let path = directory.path().join("reference.wav");
    let report = OfflineExporter::export_wav(
        Arc::clone(&snapshot),
        &path,
        ExportRange::full(),
        ExportEncoding::Float32,
    )
    .expect("export");
    assert_eq!(report.sample_rate, RATE);
    assert_eq!(report.channel_count, MIX_CHANNELS);

    let mut reader = WavReader::open(&path).expect("open export");
    let exported: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
    let live = played(Arc::clone(&snapshot), 256);

    assert_eq!(exported.len(), live.len());
    assert_eq!(
        exported, live,
        "the device path and the export path are bit-identical when their formats match"
    );
}

#[test]
fn block_size_does_not_change_what_is_played() {
    let project = reference_project();
    let snapshot = Arc::new(mix(&project).snapshot.expect("audible"));

    let small = played(Arc::clone(&snapshot), 64);
    let large = played(Arc::clone(&snapshot), 1_024);
    assert_eq!(
        small, large,
        "a device with a different buffer size hears the same audio"
    );
}

#[test]
fn mixing_the_same_project_twice_produces_the_same_audio() {
    let project = reference_project();
    let first = mix(&project).snapshot.expect("audible");
    let second = mix(&project).snapshot.expect("audible");
    assert_eq!(
        first.samples(),
        second.samples(),
        "synths, generators and effects are all deterministic together"
    );
}

#[test]
fn the_reference_project_reports_the_latency_and_tail_of_its_chains() {
    let project = reference_project();
    let output = mix(&project);

    // The built-ins used here declare no latency; the delay declares a tail.
    assert_eq!(
        output.timing.latency_frames, 0,
        "nothing in this chain delays the signal"
    );
    assert!(
        output.timing.tail_frames > 0,
        "the delay's repeats outlive their input, and the mix says so"
    );
    assert!(
        output.diagnostics.is_empty(),
        "every extension the project names is available: {:?}",
        output.diagnostics
    );
}

#[test]
fn an_export_covers_the_timeline_and_not_the_tail_beyond_it() {
    let project = reference_project();
    let output = mix(&project);
    let snapshot = output.snapshot.expect("audible");

    // The timeline is two clips of FRAMES each, the second starting halfway.
    let expected = FRAMES + FRAMES / 2;
    assert_eq!(
        snapshot.frame_count(),
        expected,
        "the mix is exactly as long as the timeline"
    );
    assert!(
        output.timing.tail_frames > 0,
        "there is a tail that the export does not include; a caller who wants \
         it extends the timeline or the loop region"
    );
}
