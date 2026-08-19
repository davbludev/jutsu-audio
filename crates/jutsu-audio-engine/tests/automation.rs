//! Automation: a parameter that moves over time, evaluated per frame and
//! rendered the same way offline as in real time.

use std::collections::BTreeMap;
use std::sync::Arc;

use jutsu_audio_engine::{SourceAudio, mix_project};
use jutsu_audio_extensions::ExtensionRegistries;
use jutsu_audio_model::{
    Asset, AssetId, AudioAssetSource, AutomationId, AutomationLane, AutomationTarget, Breakpoint,
    BusId, CURRENT_PROJECT_SCHEMA_VERSION, Clip, ClipId, Curve, Layer, LayerId, MixerBus, Project,
    ProjectId, ProjectMetadata, Track, TrackId, ValidationCode,
};

const RATE: u32 = 48_000;
const FRAMES: u64 = 8;

fn project() -> (Project, TrackId, BusId) {
    let master = BusId::new();
    let track_id = TrackId::new();
    let asset = Asset {
        id: AssetId::new(),
        name: "Tone".into(),
        source: AudioAssetSource::File {
            path: "tone.wav".into(),
        },
    };
    let asset_id = asset.id;
    (
        Project {
            schema_version: CURRENT_PROJECT_SCHEMA_VERSION,
            id: ProjectId::new(),
            metadata: ProjectMetadata {
                name: "Automation".into(),
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
                id: track_id,
                name: "Track".into(),
                output_bus_id: master,
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
                    }],
                }],
                effects: Vec::new(),
            }],
            markers: Vec::new(),
            loop_region: None,
            automation: Vec::new(),
            tempo: Vec::new(),
        },
        track_id,
        master,
    )
}

fn lane(target: AutomationTarget, parameter: &str, points: Vec<Breakpoint>) -> AutomationLane {
    AutomationLane {
        id: AutomationId::new(),
        target,
        parameter: parameter.into(),
        points,
    }
}

fn point(frame: u64, value: f64, curve: Curve) -> Breakpoint {
    Breakpoint {
        frame,
        value,
        curve,
    }
}

fn left_channel(project: &Project) -> Vec<f32> {
    mix_project(project, RATE, &ExtensionRegistries::default(), |_| {
        Ok(SourceAudio {
            sample_rate: RATE,
            channels: 1,
            samples: Arc::from(vec![1.0_f32; FRAMES as usize]),
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

#[test]
fn a_linear_lane_ramps_between_its_breakpoints() {
    let lane = lane(
        AutomationTarget::Track {
            track_id: TrackId::new(),
        },
        "gain_db",
        vec![point(0, 0.0, Curve::Linear), point(10, 10.0, Curve::Linear)],
    );
    assert_eq!(lane.value_at(0), Some(0.0));
    assert_eq!(lane.value_at(5), Some(5.0));
    assert_eq!(lane.value_at(10), Some(10.0));
}

#[test]
fn a_lane_holds_its_value_before_the_first_point_and_after_the_last() {
    let lane = lane(
        AutomationTarget::Track {
            track_id: TrackId::new(),
        },
        "gain_db",
        vec![
            point(100, -6.0, Curve::Linear),
            point(200, 0.0, Curve::Linear),
        ],
    );
    assert_eq!(lane.value_at(0), Some(-6.0));
    assert_eq!(lane.value_at(1_000), Some(0.0));
}

#[test]
fn a_stepped_lane_holds_until_the_next_point_and_then_jumps() {
    let lane = lane(
        AutomationTarget::Track {
            track_id: TrackId::new(),
        },
        "gain_db",
        vec![point(0, -6.0, Curve::Step), point(4, 0.0, Curve::Step)],
    );
    assert_eq!(lane.value_at(3), Some(-6.0));
    assert_eq!(lane.value_at(4), Some(0.0));
}

#[test]
fn an_empty_lane_says_nothing_rather_than_zero() {
    let lane = lane(
        AutomationTarget::Track {
            track_id: TrackId::new(),
        },
        "gain_db",
        Vec::new(),
    );
    assert_eq!(lane.value_at(0), None);
}

#[test]
fn an_automated_track_level_moves_the_audio_frame_by_frame() {
    let (mut project, track_id, _) = project();
    // Silence to unity across the clip: -60 dB is the floor the strip declares.
    project.automation.push(lane(
        AutomationTarget::Track { track_id },
        "gain_db",
        vec![
            point(0, -60.0, Curve::Linear),
            point(FRAMES - 1, 0.0, Curve::Linear),
        ],
    ));

    let left = left_channel(&project);
    assert!(left[0] < 0.01, "it starts near silence: {left:?}");
    assert!(
        (left[FRAMES as usize - 1] - 1.0).abs() < 1e-3,
        "and arrives at unity: {left:?}"
    );
    assert!(
        left.windows(2).all(|pair| pair[1] >= pair[0]),
        "rising the whole way, with no step: {left:?}"
    );
}

#[test]
fn an_automated_bus_level_applies_to_everything_through_it() {
    let (mut project, _, master) = project();
    project.automation.push(lane(
        AutomationTarget::Bus { bus_id: master },
        "gain_db",
        vec![point(0, -6.020_6, Curve::Step)],
    ));

    let left = left_channel(&project);
    assert!(
        left.iter().all(|sample| (*sample - 0.5).abs() < 1e-3),
        "the whole mix is halved: {left:?}"
    );
}

#[test]
fn a_lane_overrides_the_stored_value_it_automates() {
    let (mut project, track_id, _) = project();
    if let Some(track) = project.tracks.first_mut() {
        track.parameters.insert(
            "gain_db".into(),
            jutsu_audio_model::ParameterValue::Float(-60.0),
        );
    }
    project.automation.push(lane(
        AutomationTarget::Track { track_id },
        "gain_db",
        vec![point(0, 0.0, Curve::Step)],
    ));

    let left = left_channel(&project);
    assert!(
        left.iter().all(|sample| (*sample - 1.0).abs() < 1e-3),
        "the lane wins over the stored level: {left:?}"
    );
}

#[test]
fn the_same_project_automates_to_the_same_samples_every_render() {
    let (mut project, track_id, _) = project();
    project.automation.push(lane(
        AutomationTarget::Track { track_id },
        "pan",
        vec![
            point(0, -1.0, Curve::Linear),
            point(FRAMES - 1, 1.0, Curve::Linear),
        ],
    ));
    assert_eq!(left_channel(&project), left_channel(&project));
}

#[test]
fn a_lane_with_no_target_or_out_of_order_points_is_refused_by_validation() {
    let (mut orphaned, _, _) = project();
    orphaned.automation.push(lane(
        AutomationTarget::Track {
            track_id: TrackId::new(),
        },
        "gain_db",
        vec![point(0, 0.0, Curve::Linear)],
    ));
    assert!(
        orphaned
            .validate()
            .iter()
            .any(|diagnostic| diagnostic.code == ValidationCode::MissingAutomationTarget)
    );

    let (mut unordered, track_id, _) = project();
    unordered.automation.push(lane(
        AutomationTarget::Track { track_id },
        "gain_db",
        vec![point(10, 0.0, Curve::Linear), point(2, 0.0, Curve::Linear)],
    ));
    assert!(
        unordered
            .validate()
            .iter()
            .any(|diagnostic| diagnostic.code == ValidationCode::UnorderedAutomation)
    );
}
