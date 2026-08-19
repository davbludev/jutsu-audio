use std::collections::BTreeMap;
use std::sync::Arc;

use jutsu_audio_extensions::{
    Effect, EffectFactory, ExtensionDescriptor, ExtensionError, ExtensionErrorCode, ExtensionKind,
    ExtensionRegistries, ExtensionTypeId, Generator, GeneratorFactory, NoteEvent,
    ParameterDescriptor, ParameterType, Synth, SynthFactory,
};
use jutsu_audio_model::ParameterValue;

fn descriptor(kind: ExtensionKind, type_id: &str) -> ExtensionDescriptor {
    ExtensionDescriptor {
        type_id: ExtensionTypeId::new(type_id).unwrap(),
        kind,
        display_name: "Mock".into(),
        state_version: 2,
        parameters: vec![ParameterDescriptor {
            id: "amount".into(),
            display_name: "Amount".into(),
            value_type: ParameterType::Float,
            default_value: ParameterValue::Float(0.5),
            introduced_in_state_version: 1,
            automatable: true,
            minimum: None,
            maximum: None,
            unit: None,
        }],
    }
}

struct MockSynthFactory(ExtensionDescriptor);
struct MockSynth(f32);

impl Synth for MockSynth {
    fn prepare(&mut self, _sample_rate: u32) {}

    fn reset(&mut self) {}

    fn render(&mut self, _events: &[NoteEvent], output: &mut [f32]) {
        output.fill(self.0);
    }
}

impl SynthFactory for MockSynthFactory {
    fn descriptor(&self) -> &ExtensionDescriptor {
        &self.0
    }

    fn instantiate(
        &self,
        parameters: &BTreeMap<String, ParameterValue>,
    ) -> Result<Box<dyn Synth>, ExtensionError> {
        let amount = match parameters.get("amount") {
            Some(ParameterValue::Float(value)) => *value as f32,
            _ => 0.5,
        };
        Ok(Box::new(MockSynth(amount)))
    }
}

struct MockEffectFactory(ExtensionDescriptor);
struct MockEffect;

impl Effect for MockEffect {
    fn process_mono(&mut self, samples: &mut [f32]) {
        for sample in samples {
            *sample *= 2.0;
        }
    }
}

impl EffectFactory for MockEffectFactory {
    fn descriptor(&self) -> &ExtensionDescriptor {
        &self.0
    }

    fn instantiate(
        &self,
        _parameters: &BTreeMap<String, ParameterValue>,
    ) -> Result<Box<dyn Effect>, ExtensionError> {
        Ok(Box::new(MockEffect))
    }
}

struct MockGeneratorFactory(ExtensionDescriptor);
struct MockGenerator;

impl Generator for MockGenerator {
    fn generate_mono(&self, seed: u64, frame_count: usize) -> Vec<f32> {
        vec![(seed % 10) as f32; frame_count]
    }
}

impl GeneratorFactory for MockGeneratorFactory {
    fn descriptor(&self) -> &ExtensionDescriptor {
        &self.0
    }

    fn instantiate(
        &self,
        _parameters: &BTreeMap<String, ParameterValue>,
    ) -> Result<Box<dyn Generator>, ExtensionError> {
        Ok(Box::new(MockGenerator))
    }
}

#[test]
fn typed_registries_register_describe_and_instantiate_all_extension_kinds() {
    let synth_id = ExtensionTypeId::new("builtin.mock_synth").unwrap();
    let effect_id = ExtensionTypeId::new("builtin.mock_effect").unwrap();
    let generator_id = ExtensionTypeId::new("builtin.mock_generator").unwrap();
    let mut registries = ExtensionRegistries::default();

    registries
        .register_synth(Arc::new(MockSynthFactory(descriptor(
            ExtensionKind::Synth,
            synth_id.as_str(),
        ))))
        .unwrap();
    registries
        .register_effect(Arc::new(MockEffectFactory(descriptor(
            ExtensionKind::Effect,
            effect_id.as_str(),
        ))))
        .unwrap();
    registries
        .register_generator(Arc::new(MockGeneratorFactory(descriptor(
            ExtensionKind::Generator,
            generator_id.as_str(),
        ))))
        .unwrap();

    assert_eq!(
        registries
            .synth_descriptor(&synth_id)
            .unwrap()
            .state_version,
        2
    );
    assert_eq!(
        registries.synth_descriptor(&synth_id).unwrap().parameters[0].introduced_in_state_version,
        1
    );

    let mut synth = registries
        .instantiate_synth(
            &synth_id,
            &BTreeMap::from([("amount".into(), ParameterValue::Float(0.25))]),
        )
        .unwrap();
    let mut synth_output = [0.0; 2];
    synth.render(&[], &mut synth_output);
    assert_eq!(synth_output, [0.25, 0.25]);

    let mut effect = registries
        .instantiate_effect(&effect_id, &BTreeMap::new())
        .unwrap();
    let mut effect_output = [1.0, -1.0];
    effect.process_mono(&mut effect_output);
    assert_eq!(effect_output, [2.0, -2.0]);

    let generator = registries
        .instantiate_generator(&generator_id, &BTreeMap::new())
        .unwrap();
    assert_eq!(generator.generate_mono(7, 2), vec![7.0, 7.0]);
}

#[test]
fn unavailable_extension_returns_structured_kind_and_type_id() {
    let registries = ExtensionRegistries::default();
    let type_id = ExtensionTypeId::new("missing.synth").unwrap();

    let error = match registries.instantiate_synth(&type_id, &BTreeMap::new()) {
        Ok(_) => panic!("missing synth unexpectedly instantiated"),
        Err(error) => error,
    };

    assert_eq!(error.code, ExtensionErrorCode::UnavailableType);
    assert_eq!(error.kind, Some(ExtensionKind::Synth));
    assert_eq!(error.type_id.as_ref(), Some(&type_id));
}

#[test]
fn registry_rejects_duplicate_type_id_without_replacing_factory() {
    let type_id = ExtensionTypeId::new("builtin.mock_synth").unwrap();
    let mut registries = ExtensionRegistries::default();
    let first = Arc::new(MockSynthFactory(descriptor(
        ExtensionKind::Synth,
        type_id.as_str(),
    )));
    registries.register_synth(first).unwrap();

    let error = registries
        .register_synth(Arc::new(MockSynthFactory(descriptor(
            ExtensionKind::Synth,
            type_id.as_str(),
        ))))
        .unwrap_err();

    assert_eq!(error.code, ExtensionErrorCode::DuplicateTypeId);
    assert!(registries.synth_descriptor(&type_id).is_some());
}

#[test]
fn registry_rejects_descriptor_kind_or_parameter_type_mismatch() {
    let mut registries = ExtensionRegistries::default();
    let wrong_kind = registries
        .register_synth(Arc::new(MockSynthFactory(descriptor(
            ExtensionKind::Effect,
            "broken.kind",
        ))))
        .unwrap_err();
    assert_eq!(wrong_kind.code, ExtensionErrorCode::InvalidDescriptor);

    let mut invalid = descriptor(ExtensionKind::Synth, "broken.parameter");
    invalid.parameters[0].default_value = ParameterValue::Bool(false);
    let wrong_parameter = registries
        .register_synth(Arc::new(MockSynthFactory(invalid)))
        .unwrap_err();
    assert_eq!(wrong_parameter.code, ExtensionErrorCode::InvalidDescriptor);
}

#[test]
fn instantiation_rejects_wrong_parameter_type_before_factory_call() {
    let type_id = ExtensionTypeId::new("builtin.mock_synth").unwrap();
    let mut registries = ExtensionRegistries::default();
    registries
        .register_synth(Arc::new(MockSynthFactory(descriptor(
            ExtensionKind::Synth,
            type_id.as_str(),
        ))))
        .unwrap();

    let error = match registries.instantiate_synth(
        &type_id,
        &BTreeMap::from([("amount".into(), ParameterValue::Bool(false))]),
    ) {
        Ok(_) => panic!("invalid parameter unexpectedly instantiated"),
        Err(error) => error,
    };

    assert_eq!(error.code, ExtensionErrorCode::InvalidParameters);
    assert_eq!(error.parameter_id.as_deref(), Some("amount"));
}

#[test]
fn deserialization_rejects_invalid_extension_type_id() {
    let error = serde_json::from_str::<ExtensionTypeId>("\"NOT VALID\"").unwrap_err();

    assert!(error.to_string().contains("lowercase dotted identifier"));
}
