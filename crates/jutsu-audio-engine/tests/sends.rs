//! Parallel sends: a copy of a track's signal somewhere else, with the original
//! still going where it always went.
//!
//! Without this, putting a track in a reverb means putting the reverb *on* the
//! track, and the dry signal is gone. The tests below are the two halves of
//! that sentence: the copy arrives, and the original is untouched.

use std::collections::BTreeMap;
use std::sync::Arc;

use jutsu_audio_engine::{SourceAudio, mix_project};
use jutsu_audio_extensions::ExtensionRegistries;
use jutsu_audio_model::{
    Asset, AssetId, AudioAssetSource, BusId, CURRENT_PROJECT_SCHEMA_VERSION, Clip, ClipId, Layer,
    LayerId, MixerBus, ParameterValue, Project, ProjectId, ProjectMetadata, SendRoute, Track,
    TrackId,
};

const RATE: u32 = 48_000;
const FRAMES: u64 = 480;

/// A project with a master, one other bus, and one track playing a steady
/// signal into the master.
fn project(sends: Vec<SendRoute>, track_gain_db: f64) -> (Project, BusId) {
    let master = BusId::new();
    let aux = BusId::new();
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
                name: "Sends".into(),
                properties: BTreeMap::new(),
            },
            assets: vec![asset],
            buses: vec![
                MixerBus {
                    id: master,
                    name: "Master".into(),
                    output_bus_id: None,
                    parameters: BTreeMap::new(),
                    effects: Vec::new(),
                },
                MixerBus {
                    id: aux,
                    name: "Aux".into(),
                    output_bus_id: Some(master),
                    parameters: BTreeMap::new(),
                    effects: Vec::new(),
                },
            ],
            master_bus_id: master,
            tracks: vec![Track {
                id: TrackId::new(),
                name: "Track".into(),
                output_bus_id: master,
                sends,
                parameters: BTreeMap::from([(
                    "gain_db".into(),
                    ParameterValue::Float(track_gain_db),
                )]),
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
                effects: Vec::new(),
            }],
            markers: Vec::new(),
            loop_region: None,
            automation: Vec::new(),
            tempo: Vec::new(),
            patterns: Vec::new(),
        },
        aux,
    )
}

/// The level of the first channel, which is all a constant signal needs.
fn level(project: &Project) -> f32 {
    mix_project(project, RATE, &ExtensionRegistries::default(), |_| {
        Ok(SourceAudio {
            sample_rate: RATE,
            channels: 1,
            samples: Arc::from(vec![0.25_f32; FRAMES as usize]),
        })
    })
    .expect("mix")
    .expect("a snapshot")
    .samples()[100]
}

/// A project and its aux bus, with the sends already attached to it.
///
/// Every call builds fresh IDs, so a send has to be given the bus of the
/// project it lives in — pointing one at another project's bus is how the first
/// version of this test managed to prove nothing.
fn with_sends(gain_db: f64, pre_fader: bool, track_gain_db: f64) -> Project {
    let (mut built, aux) = project(Vec::new(), track_gain_db);
    built.tracks[0].sends = vec![SendRoute {
        bus_id: aux,
        gain_db,
        pre_fader,
    }];
    assert!(built.validate().is_empty(), "the send names a real bus");
    built
}

#[test]
fn a_send_adds_a_copy_without_taking_anything_from_the_output() {
    let dry = level(&project(Vec::new(), 0.0).0);
    let both = level(&with_sends(0.0, false, 0.0));

    // The send is at unity and the aux folds into the master, so the master
    // hears the track twice.
    assert!(
        (both - dry * 2.0).abs() < 1e-5,
        "a unity send should double what the master hears: {dry} became {both}"
    );
}

#[test]
fn a_send_carries_its_own_level() {
    let dry = level(&project(Vec::new(), 0.0).0);
    // -6.0206 dB is half the amplitude, so the master hears one and a half.
    let both = level(&with_sends(-6.0206, false, 0.0));
    assert!(
        (both - dry * 1.5).abs() < 1e-3,
        "the send ignored its own level: {dry} became {both}"
    );
}

/// Post-fader is the default because a reverb should follow the fader. The
/// pre-fader case exists for the times it should not, and the two have to be
/// distinguishable or the flag means nothing.
#[test]
fn pre_fader_ignores_the_fader_and_post_fader_follows_it() {
    let unity = level(&project(Vec::new(), 0.0).0);
    let post = level(&with_sends(0.0, false, -20.0));
    let pre = level(&with_sends(0.0, true, -20.0));

    // Both tracks are turned down 20 dB. The post-fader send is turned down
    // with it; the pre-fader one still arrives at full level.
    assert!(
        post < unity * 0.3,
        "post-fader send did not follow the fader: {post} against {unity}"
    );
    assert!(
        pre > unity * 0.9,
        "pre-fader send followed the fader anyway: {pre} against {unity}"
    );
}

/// A send naming a bus that is not there is a validation failure rather than a
/// silent nothing: it is the one routing mistake a project can carry that
/// changes nothing about how it sounds.
#[test]
fn a_send_to_a_bus_that_does_not_exist_is_refused_by_validation() {
    let (mut built, _) = project(Vec::new(), 0.0);
    built.tracks[0].sends = vec![SendRoute {
        bus_id: BusId::new(),
        gain_db: 0.0,
        pre_fader: false,
    }];
    let diagnostics = built.validate();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(diagnostics[0].path.contains("sends[0].bus_id"));
}
