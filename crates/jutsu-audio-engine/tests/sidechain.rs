//! An insert that listens to a different strip than the one it sits on.
//!
//! A bass that drops out of the way every time the kick lands is most of what
//! a modern mix means by movement, and it cannot be built from a compressor
//! that only ever hears its own input. The claim here is measured: the same
//! steady tone, once with a key and once without, and the level under each kick
//! compared with the level between them.

use std::collections::BTreeMap;
use std::sync::Arc;

use jutsu_audio_engine::{SourceAudio, mix_project};
use jutsu_audio_extensions::{ExtensionRegistries, register_builtin_effects};
use jutsu_audio_model::{
    Asset, AssetId, AudioAssetSource, BusId, CURRENT_PROJECT_SCHEMA_VERSION, Clip, ClipId,
    EffectId, EffectInsert, Layer, LayerId, MixerBus, ParameterValue, Project, ProjectId,
    ProjectMetadata, Track, TrackId,
};

const RATE: u32 = 48_000;
const FRAMES: u64 = 48_000;
/// One kick every quarter of a second.
const PERIOD: usize = 12_000;
const BURST: usize = 2_000;

fn steady() -> Vec<f32> {
    (0..FRAMES)
        .map(|frame| {
            let phase = frame as f32 / RATE as f32 * 220.0;
            (phase * std::f32::consts::TAU).sin() * 0.5
        })
        .collect()
}

/// Short loud bursts with silence between: a kick, as far as a detector is
/// concerned.
///
/// Deliberately high — 8 kHz, nowhere near the 220 Hz tone. Both tracks are
/// heard in the sum, so the measurement has to be able to tell them apart, and
/// putting them octaves apart is cheaper than muting the key and then having no
/// key at all.
fn kicks() -> Vec<f32> {
    (0..FRAMES as usize)
        .map(|frame| {
            if frame % PERIOD < BURST {
                let decay = 1.0 - (frame % PERIOD) as f32 / BURST as f32;
                let phase = frame as f32 / RATE as f32 * 8_000.0;
                (phase * std::f32::consts::TAU).sin() * decay
            } else {
                0.0
            }
        })
        .collect()
}

struct Built {
    project: Project,
    tone: AssetId,
    kick: AssetId,
}

fn build(sidechain: Option<bool>) -> Built {
    let master = BusId::new();
    let tone = Asset {
        id: AssetId::new(),
        name: "Tone".into(),
        source: AudioAssetSource::File {
            path: "tone.wav".into(),
        },
    };
    let kick = Asset {
        id: AssetId::new(),
        name: "Kick".into(),
        source: AudioAssetSource::File {
            path: "kick.wav".into(),
        },
    };
    let (tone_id, kick_id) = (tone.id, kick.id);
    let kick_track = TrackId::new();

    let clip = |asset_id: AssetId| Clip {
        id: ClipId::new(),
        asset_id,
        start_sample: 0,
        source_start_sample: 0,
        duration_samples: FRAMES,
        parameters: BTreeMap::new(),
        notes: Vec::new(),
        pattern_id: None,
    };
    let layer = |asset_id: AssetId| Layer {
        id: LayerId::new(),
        name: "Layer".into(),
        clips: vec![clip(asset_id)],
    };

    let compressor = sidechain.map(|keyed| EffectInsert {
        id: EffectId::new(),
        type_id: "builtin.compressor".into(),
        state_version: 1,
        enabled: true,
        wet: 1.0,
        parameters: BTreeMap::from([
            ("threshold_db".into(), ParameterValue::Float(-30.0)),
            ("ratio".into(), ParameterValue::Float(12.0)),
            ("attack_ms".into(), ParameterValue::Float(1.0)),
            ("release_ms".into(), ParameterValue::Float(120.0)),
            ("makeup_db".into(), ParameterValue::Float(0.0)),
        ]),
        sidechain: keyed.then_some(kick_track),
    });

    Built {
        project: Project {
            schema_version: CURRENT_PROJECT_SCHEMA_VERSION,
            id: ProjectId::new(),
            metadata: ProjectMetadata {
                name: "Sidechain".into(),
                properties: BTreeMap::new(),
            },
            assets: vec![tone, kick],
            buses: vec![MixerBus {
                id: master,
                name: "Master".into(),
                output_bus_id: None,
                parameters: BTreeMap::new(),
                effects: Vec::new(),
            }],
            master_bus_id: master,
            tracks: vec![
                Track {
                    id: TrackId::new(),
                    name: "Bass".into(),
                    output_bus_id: master,
                    sends: Vec::new(),
                    parameters: BTreeMap::new(),
                    layers: vec![layer(tone_id)],
                    effects: compressor.into_iter().collect(),
                },
                Track {
                    id: kick_track,
                    name: "Kick".into(),
                    output_bus_id: master,
                    sends: Vec::new(),
                    // Not muted, and not turned down: the key is what the
                    // track sounds like after its own fader, so a silenced key
                    // would key nothing.
                    parameters: BTreeMap::new(),
                    layers: vec![layer(kick_id)],
                    effects: Vec::new(),
                },
            ],
            markers: Vec::new(),
            loop_region: None,
            automation: Vec::new(),
            tempo: Vec::new(),
            patterns: Vec::new(),
        },
        tone: tone_id,
        kick: kick_id,
    }
}

fn render(built: &Built) -> Vec<f32> {
    let (tone, kick) = (steady(), kicks());
    let (tone_id, kick_id) = (built.tone, built.kick);
    let mut registries = ExtensionRegistries::default();
    register_builtin_effects(&mut registries).expect("built-in effects");

    mix_project(&built.project, RATE, &registries, move |asset_id| {
        let samples = if asset_id == tone_id {
            tone.clone()
        } else if asset_id == kick_id {
            kick.clone()
        } else {
            Vec::new()
        };
        Ok(SourceAudio {
            sample_rate: RATE,
            channels: 1,
            samples: Arc::from(samples),
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

/// The tone's level, with the key's 8 kHz burst filtered out of the way.
/// Three poles at roughly 500 Hz: enough that what is left is the 220 Hz tone
/// and not the thing ducking it.
fn tone_level(window: &[f32]) -> f32 {
    let mut poles = [0.0_f32; 3];
    let mut loudest = 0.0_f32;
    for sample in window {
        let mut low = *sample;
        for pole in &mut poles {
            *pole += 0.065 * (low - *pole);
            low = *pole;
        }
        loudest = loudest.max(low.abs());
    }
    loudest
}

#[test]
fn a_key_track_ducks_the_strip_that_names_it() {
    let ducked = render(&build(Some(true)));

    // Just after a kick lands, against the quietest moment before the next one.
    let under = tone_level(&ducked[PERIOD + 200..PERIOD + 1_200]);
    let between = tone_level(&ducked[PERIOD + 9_000..PERIOD + 11_000]);
    assert!(
        between > under * 2.0,
        "the tone did not duck: {under} under the kick, {between} between them"
    );
}

#[test]
fn the_same_compressor_without_a_key_leaves_the_tone_alone() {
    let flat = render(&build(Some(false)));

    let under = tone_level(&flat[PERIOD + 200..PERIOD + 1_200]);
    let between = tone_level(&flat[PERIOD + 9_000..PERIOD + 11_000]);
    // A steady tone below the threshold is not compressed at all, so the two
    // windows are the same tone at the same level.
    let difference = (between - under).abs();
    assert!(
        difference < between * 0.1,
        "an unkeyed compressor moved with the kick anyway: {under} against {between}"
    );
}

/// A key naming a track that is not in the mix — muted, deleted, soloed away —
/// is silently no key at all. The mix carries on rather than refusing to
/// render, which is the same rule every other missing reference follows here.
#[test]
fn a_key_that_is_not_there_leaves_the_insert_listening_to_itself() {
    let mut built = build(Some(true));
    built.project.tracks.retain(|track| track.name != "Kick");
    let rendered = render(&built);
    assert!(rendered.iter().any(|sample| sample.abs() > 0.01));
}
