//! Mixer routing: tracks into buses, buses into the master, and the levels
//! each of them contributed.

use std::collections::BTreeMap;
use std::sync::Arc;

use jutsu_audio_engine::{SourceAudio, mix_project_metered};
use jutsu_audio_extensions::ExtensionRegistries;
use jutsu_audio_model::{
    Asset, AssetId, AudioAssetSource, BusId, CURRENT_PROJECT_SCHEMA_VERSION, Clip, ClipId, Layer,
    LayerId, MixerBus, ParameterValue, Project, ProjectId, ProjectMetadata, Track, TrackId,
};

const RATE: u32 = 48_000;
const FRAMES: u64 = 4;

struct Fixture {
    project: Project,
    asset_id: AssetId,
    master: BusId,
}

fn fixture() -> Fixture {
    let master = BusId::new();
    let asset = Asset {
        id: AssetId::new(),
        name: "Tone".into(),
        source: AudioAssetSource::File {
            path: "tone.wav".into(),
        },
    };
    Fixture {
        asset_id: asset.id,
        master,
        project: Project {
            schema_version: CURRENT_PROJECT_SCHEMA_VERSION,
            id: ProjectId::new(),
            metadata: ProjectMetadata {
                name: "Routing".into(),
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
            tracks: Vec::new(),
            markers: Vec::new(),
            loop_region: None,
            automation: Vec::new(),
            tempo: Vec::new(),
            patterns: Vec::new(),
        },
    }
}

impl Fixture {
    /// Adds a bus feeding `output`, and returns its ID.
    fn bus(&mut self, name: &str, output: Option<BusId>) -> BusId {
        let id = BusId::new();
        self.project.buses.push(MixerBus {
            id,
            name: name.into(),
            output_bus_id: output,
            parameters: BTreeMap::new(),
            effects: Vec::new(),
        });
        id
    }

    /// Adds a track holding one full-scale clip, routed to `output`.
    fn track(&mut self, output: BusId) -> TrackId {
        let id = TrackId::new();
        self.project.tracks.push(Track {
            id,
            name: format!("Track {}", self.project.tracks.len() + 1),
            output_bus_id: output,
            parameters: BTreeMap::new(),
            layers: vec![Layer {
                id: LayerId::new(),
                name: "Layer".into(),
                clips: vec![Clip {
                    id: ClipId::new(),
                    asset_id: self.asset_id,
                    start_sample: 0,
                    source_start_sample: 0,
                    duration_samples: FRAMES,
                    parameters: BTreeMap::new(),
                    notes: Vec::new(),
                    pattern_id: None,
                }],
            }],
            effects: Vec::new(),
        });
        id
    }

    fn set_track(&mut self, track_id: TrackId, key: &str, value: ParameterValue) {
        if let Some(track) = self
            .project
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
        {
            track.parameters.insert(key.into(), value);
        }
    }

    fn set_bus(&mut self, bus_id: BusId, key: &str, value: ParameterValue) {
        if let Some(bus) = self.project.buses.iter_mut().find(|bus| bus.id == bus_id) {
            bus.parameters.insert(key.into(), value);
        }
    }
}

fn source() -> SourceAudio {
    SourceAudio {
        sample_rate: RATE,
        channels: 1,
        samples: Arc::from(vec![1.0_f32; FRAMES as usize]),
    }
}

fn mix(project: &Project) -> jutsu_audio_engine::MixOutput {
    mix_project_metered(project, RATE, &ExtensionRegistries::default(), |_| {
        Ok(source())
    })
    .expect("mix")
}

fn left_channel(output: &jutsu_audio_engine::MixOutput) -> Vec<f32> {
    output
        .snapshot
        .as_ref()
        .expect("a snapshot")
        .samples()
        .iter()
        .step_by(2)
        .copied()
        .collect()
}

#[test]
fn tracks_sum_through_their_bus_into_the_master() {
    let mut fixture = fixture();
    let group = fixture.bus("Group", Some(fixture.master));
    fixture.track(group);
    fixture.track(group);

    let output = mix(&fixture.project);
    assert!(
        left_channel(&output).iter().all(|sample| *sample == 2.0),
        "two full-scale tracks sum on the way through"
    );
    assert_eq!(output.meters.buses.get(&group), Some(&2.0));
    assert_eq!(output.meters.master, 2.0);
}

#[test]
fn a_bus_level_applies_to_everything_routed_through_it() {
    let mut fixture = fixture();
    let group = fixture.bus("Group", Some(fixture.master));
    fixture.track(group);
    fixture.track(fixture.master);
    // -6.0206 dB is exactly half.
    fixture.set_bus(group, "gain_db", ParameterValue::Float(-6.020_6));

    let output = mix(&fixture.project);
    let left = left_channel(&output);
    assert!(
        (left[0] - 1.5).abs() < 1e-3,
        "the bussed track is halved, the direct one is not: {left:?}"
    );
}

#[test]
fn a_track_level_and_pan_apply_before_its_bus() {
    let mut fixture = fixture();
    let track = fixture.track(fixture.master);
    fixture.set_track(track, "gain_db", ParameterValue::Float(-6.020_6));
    fixture.set_track(track, "pan", ParameterValue::Float(-1.0));

    let output = mix(&fixture.project);
    let samples = output.snapshot.as_ref().expect("a snapshot").samples();
    assert!(
        (samples[0] - 0.5 * std::f32::consts::SQRT_2).abs() < 1e-3,
        "hard left keeps the level in one channel: {}",
        samples[0]
    );
    assert!(samples[1].abs() < 1e-6, "nothing leaks to the right");
}

#[test]
fn a_chain_of_buses_folds_from_the_leaves_inward() {
    let mut fixture = fixture();
    let outer = fixture.bus("Outer", Some(fixture.master));
    let inner = fixture.bus("Inner", Some(outer));
    fixture.track(inner);
    fixture.set_bus(inner, "gain_db", ParameterValue::Float(-6.020_6));
    fixture.set_bus(outer, "gain_db", ParameterValue::Float(-6.020_6));

    let output = mix(&fixture.project);
    let left = left_channel(&output);
    assert!(
        (left[0] - 0.25).abs() < 1e-3,
        "both bus levels apply, innermost first: {left:?}"
    );
    assert!(
        output
            .meters
            .buses
            .get(&inner)
            .is_some_and(|peak| (peak - 0.5).abs() < 1e-3)
    );
}

#[test]
fn a_track_routed_to_a_bus_that_goes_nowhere_is_not_in_the_master() {
    let mut fixture = fixture();
    let orphan = fixture.bus("Orphan", None);
    fixture.track(orphan);
    fixture.track(fixture.master);

    let output = mix(&fixture.project);
    let left = left_channel(&output);
    assert!(
        left.iter().all(|sample| (*sample - 1.0).abs() < 1e-6),
        "only the track that reaches the master is heard: {left:?}"
    );
    assert_eq!(
        output.meters.buses.get(&orphan),
        Some(&1.0),
        "and the orphan bus still reports what it held"
    );
}

#[test]
fn a_muted_track_contributes_nothing_anywhere() {
    let mut fixture = fixture();
    let group = fixture.bus("Group", Some(fixture.master));
    let muted = fixture.track(group);
    fixture.track(group);
    fixture.set_track(muted, "mute", ParameterValue::Bool(true));

    let output = mix(&fixture.project);
    assert!(left_channel(&output).iter().all(|sample| *sample == 1.0));
    assert!(
        !output.meters.tracks.contains_key(&muted),
        "a muted track is not in the mix, so it has no level"
    );
}

#[test]
fn routing_that_loops_is_rejected_by_validation() {
    let mut fixture = fixture();
    let first = fixture.bus("First", None);
    let second = fixture.bus("Second", Some(first));
    if let Some(bus) = fixture.project.buses.iter_mut().find(|bus| bus.id == first) {
        bus.output_bus_id = Some(second);
    }

    let diagnostics = fixture.project.validate();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == jutsu_audio_model::ValidationCode::BusCycle),
        "a loop is refused before it can be rendered: {diagnostics:?}"
    );
}
