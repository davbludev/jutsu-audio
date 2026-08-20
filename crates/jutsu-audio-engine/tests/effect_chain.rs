//! The effect chain: order, bypass, wet/dry, timing, and what happens when an
//! extension a project needs is not there.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use jutsu_audio_engine::{MixDiagnosticCode, SourceAudio, mix_project_metered};
use jutsu_audio_extensions::{
    Effect, EffectFactory, ExtensionDescriptor, ExtensionError, ExtensionKind, ExtensionRegistries,
    ExtensionTypeId, ParameterDescriptor, ParameterType,
};
use jutsu_audio_model::{
    Asset, AssetId, AudioAssetSource, BusId, CURRENT_PROJECT_SCHEMA_VERSION, Clip, ClipId,
    EffectId, EffectInsert, Layer, LayerId, MixerBus, ParameterValue, Project, ProjectId,
    ProjectMetadata, Track, TrackId,
};

const RATE: u32 = 48_000;
const FRAMES: u64 = 4;

/// Adds a fixed offset to every sample, and records the order it ran in.
struct Adder {
    amount: f32,
    label: String,
    order: Arc<Mutex<Vec<String>>>,
    prepared_at: u32,
}

impl Effect for Adder {
    fn prepare(&mut self, sample_rate: u32) {
        self.prepared_at = sample_rate;
    }

    fn reset(&mut self) {}

    fn process(&mut self, samples: &mut [f32]) {
        assert_eq!(
            self.prepared_at, RATE,
            "an effect is prepared before it runs"
        );
        self.order.lock().expect("order").push(self.label.clone());
        for sample in samples {
            *sample += self.amount;
        }
    }

    fn latency_frames(&self) -> u32 {
        7
    }

    fn tail_frames(&self) -> u32 {
        11
    }
}

struct AdderFactory {
    descriptor: ExtensionDescriptor,
    label: String,
    order: Arc<Mutex<Vec<String>>>,
}

impl EffectFactory for AdderFactory {
    fn descriptor(&self) -> &ExtensionDescriptor {
        &self.descriptor
    }

    fn instantiate(
        &self,
        parameters: &BTreeMap<String, ParameterValue>,
    ) -> Result<Box<dyn Effect>, ExtensionError> {
        let amount = match parameters.get("amount") {
            Some(ParameterValue::Float(value)) => *value as f32,
            _ => 1.0,
        };
        Ok(Box::new(Adder {
            amount,
            label: self.label.clone(),
            order: Arc::clone(&self.order),
            prepared_at: 0,
        }))
    }
}

fn descriptor(type_id: &str, state_version: u32) -> ExtensionDescriptor {
    ExtensionDescriptor {
        type_id: ExtensionTypeId::new(type_id).expect("a valid ID"),
        kind: ExtensionKind::Effect,
        display_name: "Adder".into(),
        state_version,
        parameters: vec![ParameterDescriptor {
            id: "amount".into(),
            display_name: "Amount".into(),
            value_type: ParameterType::Float,
            default_value: ParameterValue::Float(1.0),
            introduced_in_state_version: 1,
            automatable: true,
            minimum: Some(-10.0),
            maximum: Some(10.0),
            unit: None,
        }],
    }
}

/// Registries holding two adders, and the log of what ran when.
fn registries() -> (ExtensionRegistries, Arc<Mutex<Vec<String>>>) {
    let order = Arc::new(Mutex::new(Vec::new()));
    let mut registries = ExtensionRegistries::default();
    for (type_id, label) in [("test.first", "first"), ("test.second", "second")] {
        registries
            .register_effect(Arc::new(AdderFactory {
                descriptor: descriptor(type_id, 1),
                label: label.into(),
                order: Arc::clone(&order),
            }))
            .expect("registers");
    }
    (registries, order)
}

fn insert(type_id: &str, amount: f64) -> EffectInsert {
    EffectInsert {
        id: EffectId::new(),
        type_id: type_id.into(),
        state_version: 1,
        parameters: BTreeMap::from([("amount".into(), ParameterValue::Float(amount))]),
        enabled: true,
        wet: 1.0,
    }
}

fn project(track_effects: Vec<EffectInsert>, bus_effects: Vec<EffectInsert>) -> Project {
    let master = BusId::new();
    let asset = Asset {
        id: AssetId::new(),
        name: "Tone".into(),
        source: AudioAssetSource::File {
            path: "tone.wav".into(),
        },
    };
    let asset_id = asset.id;
    Project {
        schema_version: CURRENT_PROJECT_SCHEMA_VERSION,
        id: ProjectId::new(),
        metadata: ProjectMetadata {
            name: "Chain".into(),
            properties: BTreeMap::new(),
        },
        assets: vec![asset],
        buses: vec![MixerBus {
            id: master,
            name: "Master".into(),
            output_bus_id: None,
            parameters: BTreeMap::new(),
            effects: bus_effects,
        }],
        master_bus_id: master,
        tracks: vec![Track {
            id: TrackId::new(),
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
                    pattern_id: None,
                }],
            }],
            effects: track_effects,
        }],
        markers: Vec::new(),
        loop_region: None,
        automation: Vec::new(),
        tempo: Vec::new(),
        patterns: Vec::new(),
    }
}

fn mix(project: &Project, registries: &ExtensionRegistries) -> jutsu_audio_engine::MixOutput {
    mix_project_metered(project, RATE, registries, |_| {
        Ok(SourceAudio {
            sample_rate: RATE,
            channels: 1,
            samples: Arc::from(vec![0.0_f32; FRAMES as usize]),
        })
    })
    .expect("mix")
}

fn left(output: &jutsu_audio_engine::MixOutput) -> Vec<f32> {
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
fn inserts_run_in_order_and_their_effects_accumulate() {
    let (registries, order) = registries();
    let project = project(
        vec![insert("test.first", 0.25), insert("test.second", 0.5)],
        Vec::new(),
    );

    let output = mix(&project, &registries);
    assert!(
        left(&output)
            .iter()
            .all(|sample| (*sample - 0.75).abs() < 1e-6),
        "both inserts applied: {:?}",
        left(&output)
    );
    let ran = order.lock().expect("order").clone();
    assert_eq!(
        ran.first().map(String::as_str),
        Some("first"),
        "chain order is project order: {ran:?}"
    );
}

#[test]
fn a_bypassed_insert_is_skipped_without_leaving_the_chain() {
    let (registries, order) = registries();
    let mut inserts = vec![insert("test.first", 0.25), insert("test.second", 0.5)];
    inserts[0].enabled = false;
    let project = project(inserts, Vec::new());

    let output = mix(&project, &registries);
    assert!(
        left(&output)
            .iter()
            .all(|sample| (*sample - 0.5).abs() < 1e-6)
    );
    assert!(
        !order
            .lock()
            .expect("order")
            .iter()
            .any(|label| label == "first"),
        "a bypassed insert does not run at all"
    );
}

#[test]
fn wet_blends_the_processed_signal_against_the_dry_one() {
    let (registries, _) = registries();
    let mut inserts = vec![insert("test.first", 1.0)];
    inserts[0].wet = 0.25;
    let project = project(inserts, Vec::new());

    let output = mix(&project, &registries);
    assert!(
        left(&output)
            .iter()
            .all(|sample| (*sample - 0.25).abs() < 1e-6),
        "a quarter wet is a quarter of the effect: {:?}",
        left(&output)
    );
}

#[test]
fn a_bus_chain_applies_to_everything_routed_through_it() {
    let (registries, _) = registries();
    let project = project(
        vec![insert("test.first", 0.25)],
        vec![insert("test.second", 0.5)],
    );

    let output = mix(&project, &registries);
    assert!(
        left(&output)
            .iter()
            .all(|sample| (*sample - 0.75).abs() < 1e-6)
    );
}

#[test]
fn a_chain_reports_the_latency_it_adds_and_the_tail_it_leaves() {
    let (registries, _) = registries();
    let project = project(
        vec![insert("test.first", 0.0), insert("test.second", 0.0)],
        vec![insert("test.first", 0.0)],
    );

    let output = mix(&project, &registries);
    assert_eq!(
        output.timing.latency_frames, 21,
        "latency adds up along the path"
    );
    assert_eq!(
        output.timing.tail_frames, 11,
        "a tail is the longest one, not the sum"
    );
}

#[test]
fn a_missing_effect_passes_its_audio_through_and_says_so() {
    let (registries, _) = registries();
    let project = project(vec![insert("test.absent", 1.0)], Vec::new());

    let output = mix(&project, &registries);
    assert!(
        left(&output).iter().all(|sample| sample.abs() < 1e-6),
        "the mix still renders, unprocessed"
    );
    let diagnostic = output
        .diagnostics
        .first()
        .expect("a diagnostic explaining why");
    assert_eq!(diagnostic.code, MixDiagnosticCode::EffectUnavailable);
    assert!(
        diagnostic.message.contains("test.absent") && diagnostic.message.contains("passes through"),
        "the message says what is missing and what happened instead: {}",
        diagnostic.message
    );
}

#[test]
fn an_effect_saved_at_another_state_version_still_plays_and_is_reported() {
    let (registries, _) = registries();
    let mut inserts = vec![insert("test.first", 0.5)];
    inserts[0].state_version = 7;
    let project = project(inserts, Vec::new());

    let output = mix(&project, &registries);
    assert!(
        left(&output)
            .iter()
            .all(|sample| (*sample - 0.5).abs() < 1e-6),
        "an older state is played rather than dropped"
    );
    let diagnostic = output.diagnostics.first().expect("a diagnostic");
    assert_eq!(diagnostic.code, MixDiagnosticCode::EffectVersionMismatch);
    assert!(diagnostic.message.contains('7'), "{}", diagnostic.message);
}

#[test]
fn parameters_the_extension_refuses_leave_the_audio_alone_and_name_the_problem() {
    let (registries, _) = registries();
    let mut inserts = vec![insert("test.first", 0.5)];
    inserts[0]
        .parameters
        .insert("amount".into(), ParameterValue::Float(500.0));
    let project = project(inserts, Vec::new());

    let output = mix(&project, &registries);
    assert!(left(&output).iter().all(|sample| sample.abs() < 1e-6));
    let diagnostic = output.diagnostics.first().expect("a diagnostic");
    assert_eq!(diagnostic.code, MixDiagnosticCode::EffectParametersRejected);
    assert!(
        diagnostic.message.contains("amount"),
        "{}",
        diagnostic.message
    );
}
