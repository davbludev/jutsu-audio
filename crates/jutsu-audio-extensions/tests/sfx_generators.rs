//! What every SFX generator has to be: reproducible from its seed, bounded in
//! its parameters, audible, and sane at the edges of its ranges.

use std::collections::BTreeMap;

use jutsu_audio_extensions::generators::{
    ambience, explosion, impact, laser, pickup, register_sfx_generators,
};
use jutsu_audio_extensions::{
    ExtensionErrorCode, ExtensionKind, ExtensionRegistries, ExtensionTypeId, GeneratorFactory,
};
use jutsu_audio_model::ParameterValue;

/// Every generator this build ships, by type ID.
const ALL: [&str; 5] = [
    impact::TYPE_ID,
    explosion::TYPE_ID,
    laser::TYPE_ID,
    pickup::TYPE_ID,
    ambience::TYPE_ID,
];

const FRAMES: usize = 12_000;

fn registries() -> ExtensionRegistries {
    let mut registries = ExtensionRegistries::default();
    register_sfx_generators(&mut registries).expect("the generators register");
    registries
}

fn type_id(text: &str) -> ExtensionTypeId {
    ExtensionTypeId::new(text).expect("a valid type ID")
}

fn render(
    registries: &ExtensionRegistries,
    generator_type: &str,
    parameters: &BTreeMap<String, ParameterValue>,
    seed: u64,
) -> Vec<f32> {
    registries
        .instantiate_generator(&type_id(generator_type), parameters)
        .expect("instantiates")
        .generate_mono(seed, FRAMES)
}

fn peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
}

#[test]
fn every_generator_is_registered_with_bounded_parameters_and_presets() {
    let registries = registries();
    for generator_type in ALL {
        let descriptor = registries
            .generator_descriptor(&type_id(generator_type))
            .unwrap_or_else(|| panic!("{generator_type} is registered"));
        assert_eq!(descriptor.kind, ExtensionKind::Generator);
        assert!(
            !descriptor.parameters.is_empty(),
            "{generator_type} has something to tune"
        );
        for parameter in &descriptor.parameters {
            assert!(
                parameter.minimum.is_some() && parameter.maximum.is_some(),
                "{generator_type}.{} declares its range",
                parameter.id
            );
            assert!(
                parameter.minimum <= parameter.maximum,
                "{generator_type}.{} has a usable range",
                parameter.id
            );
        }
    }
}

#[test]
fn every_generator_makes_a_sound_inside_full_scale() {
    let registries = registries();
    for generator_type in ALL {
        let samples = render(&registries, generator_type, &BTreeMap::new(), 1);
        assert_eq!(samples.len(), FRAMES);
        assert!(peak(&samples) > 0.1, "{generator_type} is audible");
        assert!(
            samples.iter().all(|sample| sample.abs() <= 1.0),
            "{generator_type} stays inside full scale"
        );
        assert!(
            samples.iter().all(|sample| sample.is_finite()),
            "{generator_type} produces finite audio"
        );
    }
}

#[test]
fn the_same_seed_renders_the_same_samples_and_another_seed_does_not() {
    let registries = registries();
    for generator_type in ALL {
        let first = render(&registries, generator_type, &BTreeMap::new(), 7);
        let again = render(&registries, generator_type, &BTreeMap::new(), 7);
        assert_eq!(first, again, "{generator_type} is not reproducible");

        // A pickup is a written phrase, so its seed deliberately changes
        // nothing; everything with noise in it must vary.
        if generator_type != pickup::TYPE_ID {
            let other = render(&registries, generator_type, &BTreeMap::new(), 8);
            assert_ne!(first, other, "{generator_type} ignores its seed");
        }
    }
}

#[test]
fn a_parameter_outside_its_declared_range_is_refused() {
    let registries = registries();
    let Err(error) = registries.instantiate_generator(
        &type_id(impact::TYPE_ID),
        &BTreeMap::from([("weight".into(), ParameterValue::Float(4.0))]),
    ) else {
        panic!("a weight above the maximum must be refused");
    };
    assert_eq!(error.code, ExtensionErrorCode::InvalidParameters);
    assert_eq!(error.parameter_id.as_deref(), Some("weight"));
    assert!(error.message.contains("maximum"), "{}", error.message);

    let Err(below) = registries.instantiate_generator(
        &type_id(laser::TYPE_ID),
        &BTreeMap::from([("start_hz".into(), ParameterValue::Float(1.0))]),
    ) else {
        panic!("a pitch below the minimum must be refused");
    };
    assert_eq!(below.parameter_id.as_deref(), Some("start_hz"));
}

#[test]
fn every_preset_is_something_the_generator_accepts() {
    for factory in [
        impact::factory(),
        explosion::factory(),
        laser::factory(),
        pickup::factory(),
        ambience::factory(),
    ] {
        let mut registries = ExtensionRegistries::default();
        let type_id = factory.descriptor().type_id.clone();
        let presets = factory.presets().to_vec();
        assert!(!presets.is_empty(), "{type_id} ships somewhere to start");

        registries
            .register_generator(std::sync::Arc::new(factory))
            .expect("registers");
        for preset in presets {
            let generator = registries
                .instantiate_generator(&type_id, &preset.parameters)
                .unwrap_or_else(|error| {
                    panic!("preset '{}' of {type_id}: {}", preset.name, error.message)
                });
            let samples = generator.generate_mono(3, FRAMES);
            assert!(
                peak(&samples) > 0.1,
                "preset '{}' of {type_id} makes a sound",
                preset.name
            );
        }
    }
}

#[test]
fn parameters_change_the_sound_they_are_named_for() {
    let registries = registries();

    let dull = render(
        &registries,
        impact::TYPE_ID,
        &BTreeMap::from([("brightness".into(), ParameterValue::Float(0.0))]),
        5,
    );
    let bright = render(
        &registries,
        impact::TYPE_ID,
        &BTreeMap::from([("brightness".into(), ParameterValue::Float(1.0))]),
        5,
    );
    assert_ne!(dull, bright, "brightness does something");

    let short = render(
        &registries,
        explosion::TYPE_ID,
        &BTreeMap::from([("decay_ms".into(), ParameterValue::Float(200.0))]),
        5,
    );
    let long = render(
        &registries,
        explosion::TYPE_ID,
        &BTreeMap::from([("decay_ms".into(), ParameterValue::Float(8_000.0))]),
        5,
    );
    // A longer decay is still going where a short one has faded out.
    let tail = FRAMES - FRAMES / 8;
    assert!(
        peak(&long[tail..]) > peak(&short[tail..]),
        "a longer decay rings on"
    );
}

#[test]
fn a_generator_asked_for_no_frames_returns_no_samples() {
    let registries = registries();
    for generator_type in ALL {
        let samples = registries
            .instantiate_generator(&type_id(generator_type), &BTreeMap::new())
            .expect("instantiates")
            .generate_mono(1, 0);
        assert!(samples.is_empty(), "{generator_type} handles a zero length");
    }
}
