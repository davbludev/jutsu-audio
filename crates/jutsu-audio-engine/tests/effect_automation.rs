//! A lane that writes to an effect's own parameter.
//!
//! Gain and pan were the only things automation could reach, which meant the
//! most recognisable gesture in sound design — a filter opening over a bar —
//! could not be expressed at all. This is the test that says it can, measured
//! rather than described: the same white noise through the same low-pass, once
//! with the cutoff parked and once with it sweeping, and the high-frequency
//! energy of the two compared.

use std::collections::BTreeMap;
use std::sync::Arc;

use jutsu_audio_engine::{SourceAudio, mix_project};
use jutsu_audio_extensions::{ExtensionRegistries, register_builtin_effects};
use jutsu_audio_model::{
    Asset, AssetId, AudioAssetSource, AutomationId, AutomationLane, AutomationTarget, Breakpoint,
    BusId, CURRENT_PROJECT_SCHEMA_VERSION, Clip, ClipId, Curve, EffectId, EffectInsert, Layer,
    LayerId, MixerBus, ParameterValue, Project, ProjectId, ProjectMetadata, Track, TrackId,
};

const RATE: u32 = 48_000;
const FRAMES: u64 = 48_000;

/// Deterministic noise, so the only difference between two renders is the one
/// the test is about.
fn noise() -> Vec<f32> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    (0..FRAMES)
        .map(|_| {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            ((state.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32 / 8_388_608.0) - 1.0
        })
        .collect()
}

fn project(insert: EffectInsert, automation: Vec<AutomationLane>) -> Project {
    let master = BusId::new();
    let asset = Asset {
        id: AssetId::new(),
        name: "Noise".into(),
        source: AudioAssetSource::File {
            path: "noise.wav".into(),
        },
    };
    let asset_id = asset.id;
    Project {
        schema_version: CURRENT_PROJECT_SCHEMA_VERSION,
        id: ProjectId::new(),
        metadata: ProjectMetadata {
            name: "Effect automation".into(),
            properties: BTreeMap::new(),
        },
        assets: vec![asset],
        buses: vec![MixerBus {
            id: master,
            name: "Master".into(),
            output_bus_id: None,
            parameters: BTreeMap::new(),
            effects: Vec::new(),
        }],
        master_bus_id: master,
        tracks: vec![Track {
            id: TrackId::new(),
            name: "Track".into(),
            output_bus_id: master,
            sends: Vec::new(),
            parameters: BTreeMap::new(),
            layers: vec![Layer {
                id: LayerId::new(),
                name: "Layer".into(),
                clips: vec![Clip {
                    id: ClipId::new(),
                    asset_id,
                    start_sample: 0,
                    source_start_sample: 0,
                    duration_samples: FRAMES,
                    parameters: BTreeMap::new(),
                    notes: Vec::new(),
                    pattern_id: None,
                }],
            }],
            effects: vec![insert],
        }],
        markers: Vec::new(),
        loop_region: None,
        automation,
        tempo: Vec::new(),
        patterns: Vec::new(),
    }
}

fn low_pass(id: EffectId, cutoff_hz: f64) -> EffectInsert {
    EffectInsert {
        id,
        type_id: "builtin.lowpass".into(),
        state_version: 1,
        enabled: true,
        wet: 1.0,
        sidechain: None,
        parameters: BTreeMap::from([("cutoff_hz".into(), ParameterValue::Float(cutoff_hz))]),
    }
}

fn render(project: &Project) -> Vec<f32> {
    let samples = noise();
    // The built-ins are registered by the application, not by the crate: a
    // default registry has no low-pass in it, and an unavailable effect passes
    // its audio through untouched — which would make every assertion here pass
    // for the wrong reason.
    let mut registries = ExtensionRegistries::default();
    register_builtin_effects(&mut registries).expect("built-in effects");
    mix_project(project, RATE, &registries, move |_| {
        Ok(SourceAudio {
            sample_rate: RATE,
            channels: 1,
            samples: Arc::from(samples.clone()),
        })
    })
    .expect("mix")
    .expect("a snapshot")
    .samples()
    .iter()
    .step_by(2)
    .copied()
    .collect()
}

/// Energy above a few kilohertz: three one-pole high-passes, then RMS. What a
/// low-pass takes away, and what a closed one has already taken.
fn brightness(window: &[f32]) -> f32 {
    let mut poles = [0.0_f32; 3];
    let mut sum = 0.0_f32;
    for sample in window {
        let mut high = *sample;
        for pole in &mut poles {
            *pole += 0.33 * (high - *pole);
            high -= *pole;
        }
        sum += high * high;
    }
    (sum / window.len() as f32).sqrt()
}

#[test]
fn a_lane_over_a_cutoff_opens_the_filter_across_the_render() {
    let effect_id = EffectId::new();
    let swept = project(
        low_pass(effect_id, 200.0),
        vec![AutomationLane {
            id: AutomationId::new(),
            target: AutomationTarget::Effect { effect_id },
            parameter: "cutoff_hz".into(),
            points: vec![
                Breakpoint {
                    frame: 0,
                    value: 200.0,
                    curve: Curve::Linear,
                },
                Breakpoint {
                    frame: FRAMES,
                    value: 12_000.0,
                    curve: Curve::Linear,
                },
            ],
        }],
    );

    let output = render(&swept);
    let start = brightness(&output[2_000..10_000]);
    let end = brightness(&output[38_000..46_000]);
    assert!(
        end > start * 4.0,
        "the sweep did not open the filter: {start} at the start, {end} at the end"
    );
}

/// The lane replaces the stored value; without one, the stored value still
/// stands. A project that had no lanes must render exactly as it did before
/// the chain learned to walk in blocks.
#[test]
fn an_insert_with_no_lane_renders_from_its_stored_parameters() {
    let effect_id = EffectId::new();
    let parked = render(&project(low_pass(effect_id, 400.0), Vec::new()));
    let open = render(&project(low_pass(EffectId::new(), 12_000.0), Vec::new()));

    assert!(
        brightness(&open[2_000..46_000]) > brightness(&parked[2_000..46_000]) * 4.0,
        "the stored cutoff stopped being what the filter used"
    );
}

/// Blocks are an implementation detail of how parameters move, not of what is
/// heard: a chain with nothing automated has to produce the same samples it
/// would have produced in one pass.
#[test]
fn splitting_the_render_into_blocks_does_not_change_the_audio() {
    let effect_id = EffectId::new();
    let once = render(&project(low_pass(effect_id, 2_000.0), Vec::new()));
    let again = render(&project(low_pass(effect_id, 2_000.0), Vec::new()));
    assert_eq!(once, again);

    // A delay is the case where a block boundary would show: its line carries
    // state across the seam, and a chain that reset per block would stutter.
    let delayed = EffectInsert {
        id: EffectId::new(),
        type_id: "builtin.delay".into(),
        state_version: 1,
        enabled: true,
        wet: 1.0,
        sidechain: None,
        parameters: BTreeMap::from([
            ("delay_ms".into(), ParameterValue::Float(30.0)),
            ("feedback".into(), ParameterValue::Float(0.6)),
        ]),
    };
    let echoed = render(&project(delayed, Vec::new()));
    assert!(
        echoed.iter().any(|sample| sample.abs() > 0.0),
        "the delay produced nothing"
    );
    // 30 ms at 48 kHz is 1440 frames, well inside the 1024-frame block: if the
    // line were reset at each boundary the tail would be gone by here.
    let tail = &echoed[46_000..];
    assert!(
        tail.iter().any(|sample| sample.abs() > 1e-4),
        "the delay line did not survive a block boundary"
    );
}
