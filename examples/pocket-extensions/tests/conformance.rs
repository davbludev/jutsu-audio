//! The pack against the published conformance checks.
//!
//! This is the test a third party writes: register the pack into a fresh set of
//! registries, run `conformance::check_*` over each factory, and require no
//! findings. Nothing in here reaches into the host's internals — if this
//! passes, the pack is ready to hand to a build.

use std::collections::BTreeMap;

use jutsu_audio_extensions::{ExtensionRegistries, ExtensionTypeId, conformance};
use jutsu_audio_model::ParameterValue;
use pocket_extensions::{
    CLICK_TYPE_ID, ClickFactory, PLUCK_TYPE_ID, PluckFactory, TREMOLO_TYPE_ID, TremoloFactory,
    register,
};

#[test]
fn the_pack_registers_into_a_host_that_knows_nothing_about_it() {
    let mut registries = ExtensionRegistries::default();
    register(&mut registries).expect("the pack registers");

    for type_id in [PLUCK_TYPE_ID, TREMOLO_TYPE_ID, CLICK_TYPE_ID] {
        let type_id = ExtensionTypeId::new(type_id).expect("type ID");
        assert!(
            registries.synth_descriptor(&type_id).is_some()
                || registries.effect_descriptor(&type_id).is_some()
                || registries.generator_descriptor(&type_id).is_some(),
            "{type_id} is not discoverable after registering"
        );
    }

    // Registering the same pack twice is a refusal, not a silent overwrite: two
    // extensions answering to one type ID would make a project ambiguous.
    let error = register(&mut registries).expect_err("a second registration is refused");
    assert_eq!(
        error.code,
        jutsu_audio_extensions::ExtensionErrorCode::DuplicateTypeId
    );
}

#[test]
fn the_synth_passes_conformance() {
    let findings = conformance::check_synth(&PluckFactory::default());
    assert!(findings.is_empty(), "{findings:#?}");
}

#[test]
fn the_effect_passes_conformance() {
    let findings = conformance::check_effect(&TremoloFactory::default());
    assert!(findings.is_empty(), "{findings:#?}");
}

#[test]
fn the_generator_passes_conformance() {
    let findings = conformance::check_generator(&ClickFactory::default());
    assert!(findings.is_empty(), "{findings:#?}");
}

#[test]
fn the_host_refuses_parameters_the_descriptor_does_not_allow() {
    let mut registries = ExtensionRegistries::default();
    register(&mut registries).expect("register");
    let type_id = ExtensionTypeId::new(TREMOLO_TYPE_ID).expect("type ID");

    // Out of range, wrong type, and unknown: all three are the host's job, so
    // the extension body never has to check them.
    for (id, value) in [
        ("rate_hz", ParameterValue::Float(500.0)),
        ("depth", ParameterValue::Text("loud".into())),
        ("wobbliness", ParameterValue::Float(0.5)),
    ] {
        let parameters = BTreeMap::from([(id.to_owned(), value.clone())]);
        assert!(
            registries
                .instantiate_effect(&type_id, &parameters)
                .is_err(),
            "{id} = {value:?} should have been refused"
        );
    }
}

#[test]
fn the_generator_is_reproducible_across_instances() {
    let mut registries = ExtensionRegistries::default();
    register(&mut registries).expect("register");
    let type_id = ExtensionTypeId::new(CLICK_TYPE_ID).expect("type ID");

    let first = registries
        .instantiate_generator(&type_id, &BTreeMap::new())
        .expect("instantiate");
    let second = registries
        .instantiate_generator(&type_id, &BTreeMap::new())
        .expect("instantiate");
    assert_eq!(
        first.generate_mono(1_234, 4_800),
        second.generate_mono(1_234, 4_800),
        "a project stores the seed, not the audio: two runs must agree"
    );
}
