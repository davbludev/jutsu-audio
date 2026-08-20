//! The built-in extensions held to the same rules third-party ones are.
//!
//! The checks live in the library rather than here, so an extension shipped by
//! anyone can run exactly what the built-ins run. If a rule is worth enforcing
//! on a stranger's synth it is worth enforcing on ours.

use std::collections::BTreeMap;

use jutsu_audio_extensions::{
    ExtensionDescriptor, ExtensionError, ExtensionKind, ExtensionRegistries, ExtensionTypeId,
    Generator, GeneratorFactory, ParameterDescriptor, ParameterType, conformance, register_builtin,
    register_builtin_effects, register_sfx_generators,
};
use jutsu_audio_model::ParameterValue;

fn registries() -> ExtensionRegistries {
    let mut registries = ExtensionRegistries::default();
    register_builtin(&mut registries).expect("synths");
    register_builtin_effects(&mut registries).expect("effects");
    register_sfx_generators(&mut registries).expect("generators");
    registries
}

#[test]
fn every_builtin_synth_conforms() {
    let registries = registries();
    for type_id in registries.synth_type_ids() {
        let factory = registries.synth_factory(type_id).expect("factory");
        let findings = conformance::check_synth(factory.as_ref());
        assert!(findings.is_empty(), "{type_id}: {findings:#?}");
    }
}

#[test]
fn every_builtin_effect_conforms() {
    let registries = registries();
    for type_id in registries.effect_type_ids() {
        let factory = registries.effect_factory(type_id).expect("factory");
        let findings = conformance::check_effect(factory.as_ref());
        assert!(findings.is_empty(), "{type_id}: {findings:#?}");
    }
}

#[test]
fn every_builtin_generator_conforms() {
    let registries = registries();
    for type_id in registries.generator_type_ids() {
        let factory = registries.generator_factory(type_id).expect("factory");
        let findings = conformance::check_generator(factory.as_ref());
        assert!(findings.is_empty(), "{type_id}: {findings:#?}");
    }
}

/// A generator that gets everything wrong, to prove the checks catch it rather
/// than passing anything handed to them.
struct BadGenerator {
    descriptor: ExtensionDescriptor,
}

impl Default for BadGenerator {
    fn default() -> Self {
        Self {
            descriptor: ExtensionDescriptor {
                type_id: ExtensionTypeId::new("bad.generator").expect("type ID"),
                kind: ExtensionKind::Generator,
                display_name: "Bad".into(),
                state_version: 1,
                parameters: vec![ParameterDescriptor {
                    id: "amount".into(),
                    display_name: "Amount".into(),
                    value_type: ParameterType::Float,
                    // Outside its own declared range.
                    default_value: ParameterValue::Float(9.0),
                    introduced_in_state_version: 1,
                    automatable: false,
                    minimum: Some(0.0),
                    maximum: Some(1.0),
                    unit: None,
                }],
            },
        }
    }
}

impl GeneratorFactory for BadGenerator {
    fn descriptor(&self) -> &ExtensionDescriptor {
        &self.descriptor
    }

    fn instantiate(
        &self,
        _parameters: &BTreeMap<String, ParameterValue>,
    ) -> Result<Box<dyn Generator>, ExtensionError> {
        Ok(Box::new(Unreproducible {
            counter: std::cell::Cell::new(0),
        }))
    }
}

/// Different audio every call, and past full scale while it is at it.
struct Unreproducible {
    counter: std::cell::Cell<u32>,
}

// Safe here because the checks call it from one thread; a real extension would
// not be written this way, which is the point.
unsafe impl Send for Unreproducible {}

impl Generator for Unreproducible {
    fn generate_mono(&self, _seed: u64, frame_count: usize) -> Vec<f32> {
        let run = self.counter.get();
        self.counter.set(run + 1);
        (0..frame_count).map(|_| 2.0 + run as f32).collect()
    }
}

#[test]
fn the_checks_actually_catch_a_bad_extension() {
    let findings = conformance::check_generator(&BadGenerator::default());
    let rules: Vec<&str> = findings.iter().map(|finding| finding.rule).collect();
    for expected in [
        "default_within_range",
        "same_seed_same_audio",
        "within_full_scale",
    ] {
        assert!(
            rules.contains(&expected),
            "{expected} not caught: {rules:?}"
        );
    }
}
