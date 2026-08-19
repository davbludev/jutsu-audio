//! Summing semantics. Playback, preview and export all read these rules, so a
//! change here changes what every surface hears.

use std::collections::BTreeMap;
use std::sync::Arc;

use jutsu_audio_engine::{MixErrorCode, SourceAudio, mix_project};
use jutsu_audio_model::{
    Asset, AssetId, AudioAssetSource, BusId, CURRENT_PROJECT_SCHEMA_VERSION, Clip, ClipId, Layer,
    LayerId, MixerBus, ParameterValue, Project, ProjectId, ProjectMetadata, Track, TrackId,
};

const RATE: u32 = 48_000;

struct Builder {
    project: Project,
    bus_id: BusId,
}

fn builder() -> Builder {
    let bus_id = BusId::new();
    Builder {
        project: Project {
            schema_version: CURRENT_PROJECT_SCHEMA_VERSION,
            id: ProjectId::new(),
            metadata: ProjectMetadata {
                name: "Mix".into(),
                properties: BTreeMap::new(),
            },
            assets: Vec::new(),
            buses: vec![MixerBus {
                id: bus_id,
                name: "Master".into(),
                output_bus_id: None,
                parameters: BTreeMap::new(),
            }],
            master_bus_id: bus_id,
            tracks: Vec::new(),
        },
        bus_id,
    }
}

impl Builder {
    /// Adds a track holding one clip of `asset`, and returns the track index so
    /// a test can set mute or solo on it.
    fn track(&mut self, asset_id: AssetId, clip: Clip) -> usize {
        self.project.tracks.push(Track {
            id: TrackId::new(),
            name: format!("Track {}", self.project.tracks.len() + 1),
            output_bus_id: self.bus_id,
            parameters: BTreeMap::new(),
            layers: vec![Layer {
                id: LayerId::new(),
                name: "Layer".into(),
                clips: vec![clip],
            }],
        });
        let _ = asset_id;
        self.project.tracks.len() - 1
    }

    fn asset(&mut self) -> AssetId {
        let asset = Asset {
            id: AssetId::new(),
            name: "Tone".into(),
            source: AudioAssetSource::File {
                path: "tone.wav".into(),
            },
        };
        let id = asset.id;
        self.project.assets.push(asset);
        id
    }

    fn flag(&mut self, track: usize, key: &str) {
        self.project.tracks[track]
            .parameters
            .insert(key.into(), ParameterValue::Bool(true));
    }
}

fn clip(asset_id: AssetId, start: u64, duration: u64, parameters: &[(&str, f64)]) -> Clip {
    Clip {
        id: ClipId::new(),
        asset_id,
        start_sample: start,
        source_start_sample: 0,
        duration_samples: duration,
        parameters: parameters
            .iter()
            .map(|(key, value)| ((*key).to_string(), ParameterValue::Float(*value)))
            .collect(),
    }
}

fn mono(sample_rate: u32, samples: &[f32]) -> SourceAudio {
    SourceAudio {
        sample_rate,
        channels: 1,
        samples: Arc::from(samples.to_vec()),
    }
}

/// Mixes with every clip reading the same source.
fn mix(project: &Project, source: SourceAudio) -> Vec<f32> {
    mix_project(project, RATE, |_| Ok(source.clone()))
        .expect("mix")
        .map(|snapshot| snapshot.samples().to_vec())
        .unwrap_or_default()
}

#[test]
fn a_clip_lands_at_its_start_frame_and_fans_a_mono_source_out_to_stereo() {
    let mut builder = builder();
    let asset = builder.asset();
    builder.track(asset, clip(asset, 2, 4, &[]));

    let mix = mix(&builder.project, mono(RATE, &[1.0, 0.5, -0.5, -1.0]));
    assert_eq!(
        mix,
        vec![
            0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.5, 0.5, -0.5, -0.5, -1.0, -1.0
        ]
    );
}

#[test]
fn overlapping_clips_sum_and_gain_applies_per_clip() {
    let mut builder = builder();
    let asset = builder.asset();
    // -6.0206 dB is exactly half amplitude.
    builder.track(asset, clip(asset, 0, 2, &[("gain_db", -6.020_6)]));
    builder.track(asset, clip(asset, 1, 2, &[("gain_db", -6.020_6)]));

    let mix = mix(&builder.project, mono(RATE, &[1.0, 1.0]));
    assert_eq!(mix.len(), 6);
    for (index, expected) in [0.5, 0.5, 1.0, 1.0, 0.5, 0.5].into_iter().enumerate() {
        assert!(
            (mix[index] - expected).abs() < 1e-4,
            "sample {index}: expected {expected}, got {}",
            mix[index]
        );
    }
}

#[test]
fn material_at_another_rate_is_resampled_onto_the_project_rate() {
    let mut builder = builder();
    let asset = builder.asset();
    builder.track(asset, clip(asset, 0, 3, &[]));

    // 24 kHz material in a 48 kHz project plays back over twice as many frames.
    let mix = mix(&builder.project, mono(24_000, &[0.0, 1.0]));
    let left: Vec<f32> = mix.iter().step_by(2).copied().collect();
    assert_eq!(left, vec![0.0, 0.5, 1.0]);
}

#[test]
fn panning_moves_a_clip_across_the_stereo_field_without_dropping_the_centre() {
    let mut panned = builder();
    let asset = panned.asset();
    panned.track(asset, clip(asset, 0, 1, &[("pan", -1.0)]));

    let hard_left = mix(&panned.project, mono(RATE, &[1.0]));
    assert!((hard_left[0] - std::f32::consts::SQRT_2).abs() < 1e-4);
    assert!(hard_left[1].abs() < 1e-6, "nothing leaks to the right");

    let mut centred = builder();
    let asset = centred.asset();
    centred.track(asset, clip(asset, 0, 1, &[]));
    let centred = mix(&centred.project, mono(RATE, &[1.0]));
    assert_eq!(
        centred,
        vec![1.0, 1.0],
        "no pan means unity in both channels"
    );
}

#[test]
fn a_muted_track_is_silent_and_the_rest_still_play() {
    let mut builder = builder();
    let asset = builder.asset();
    let muted = builder.track(asset, clip(asset, 0, 1, &[]));
    builder.track(asset, clip(asset, 1, 1, &[]));
    builder.flag(muted, "mute");

    let mix = mix(&builder.project, mono(RATE, &[1.0]));
    assert_eq!(mix, vec![0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn solo_wins_over_mute_and_silences_every_track_that_is_not_soloed() {
    let mut builder = builder();
    let asset = builder.asset();
    builder.track(asset, clip(asset, 0, 1, &[]));
    let soloed = builder.track(asset, clip(asset, 1, 1, &[]));
    builder.flag(soloed, "solo");
    // Muted *and* soloed: solo decides.
    builder.flag(soloed, "mute");

    let mix = mix(&builder.project, mono(RATE, &[1.0]));
    assert_eq!(mix, vec![0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn a_timeline_with_nothing_audible_is_not_an_error() {
    let empty = builder();
    assert!(
        mix_project(&empty.project, RATE, |_| Ok(mono(RATE, &[1.0])))
            .expect("mix")
            .is_none()
    );

    let mut all_muted = builder();
    let asset = all_muted.asset();
    let track = all_muted.track(asset, clip(asset, 0, 4, &[]));
    all_muted.flag(track, "mute");
    assert!(
        mix_project(&all_muted.project, RATE, |_| Ok(mono(RATE, &[1.0])))
            .expect("mix")
            .is_none(),
        "muting everything leaves nothing to play, which is not a failure"
    );
}

#[test]
fn a_source_the_loader_cannot_provide_is_a_structured_failure() {
    let mut builder = builder();
    let asset = builder.asset();
    builder.track(asset, clip(asset, 0, 4, &[]));

    let error = mix_project(&builder.project, RATE, |_| Err("no such file".into()))
        .expect_err("a missing source fails the mix");
    assert_eq!(error.code, MixErrorCode::SourceUnavailable);
    assert!(error.message.contains("no such file"), "{}", error.message);
}

#[test]
fn the_same_project_and_sources_always_mix_to_the_same_samples() {
    let mut builder = builder();
    let asset = builder.asset();
    builder.track(
        asset,
        clip(asset, 0, 4, &[("gain_db", -3.0), ("pan", 0.25)]),
    );
    builder.track(asset, clip(asset, 2, 4, &[("pan", -0.5)]));

    let source = mono(RATE, &[0.3, -0.7, 0.9, -0.1]);
    assert_eq!(
        mix(&builder.project, source.clone()),
        mix(&builder.project, source)
    );
}
