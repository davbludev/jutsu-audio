//! What every built-in effect has to survive: silence, full scale, extreme
//! settings, and being run twice.

use std::collections::BTreeMap;

use jutsu_audio_extensions::effects::{delay, dynamics, filters, reverb};
use jutsu_audio_extensions::{
    Effect, ExtensionKind, ExtensionRegistries, ExtensionTypeId, register_builtin_effects,
};
use jutsu_audio_model::ParameterValue;

const RATE: u32 = 48_000;
const FRAMES: usize = 4_800;

const ALL: [&str; 5] = [
    filters::LOW_PASS_TYPE_ID,
    filters::HIGH_PASS_TYPE_ID,
    dynamics::TYPE_ID,
    delay::TYPE_ID,
    reverb::TYPE_ID,
];

fn registries() -> ExtensionRegistries {
    let mut registries = ExtensionRegistries::default();
    register_builtin_effects(&mut registries).expect("the effects register");
    registries
}

fn type_id(text: &str) -> ExtensionTypeId {
    ExtensionTypeId::new(text).expect("a valid type ID")
}

fn effect(
    registries: &ExtensionRegistries,
    effect_type: &str,
    parameters: &[(&str, f64)],
) -> Box<dyn Effect> {
    let parameters: BTreeMap<String, ParameterValue> = parameters
        .iter()
        .map(|(id, value)| ((*id).to_string(), ParameterValue::Float(*value)))
        .collect();
    let mut effect = registries
        .instantiate_effect(&type_id(effect_type), &parameters)
        .expect("instantiates");
    effect.prepare(RATE);
    effect.reset();
    effect
}

/// A full-scale square wave: broad in frequency and loud, so a filter has
/// something to remove and a compressor something to catch.
fn signal() -> Vec<f32> {
    (0..FRAMES)
        .map(|index| if (index / 24) % 2 == 0 { 1.0 } else { -1.0 })
        .collect()
}

fn peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
}

fn energy(samples: &[f32]) -> f64 {
    samples
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum()
}

#[test]
fn every_effect_is_registered_with_bounded_parameters_and_presets() {
    let registries = registries();
    for effect_type in ALL {
        let descriptor = registries
            .effect_descriptor(&type_id(effect_type))
            .unwrap_or_else(|| panic!("{effect_type} is registered"));
        assert_eq!(descriptor.kind, ExtensionKind::Effect);
        for parameter in &descriptor.parameters {
            assert!(
                parameter.minimum.is_some() && parameter.maximum.is_some(),
                "{effect_type}.{} declares its range",
                parameter.id
            );
        }
        let presets = registries
            .effect_presets(&type_id(effect_type))
            .expect("presets");
        assert!(
            !presets.is_empty(),
            "{effect_type} ships somewhere to start"
        );
        for preset in presets {
            registries
                .instantiate_effect(&type_id(effect_type), &preset.parameters)
                .unwrap_or_else(|error| {
                    panic!(
                        "preset '{}' of {effect_type}: {}",
                        preset.name, error.message
                    )
                });
        }
    }
}

#[test]
fn silence_in_is_silence_out() {
    let registries = registries();
    for effect_type in ALL {
        let mut effect = effect(&registries, effect_type, &[]);
        let mut samples = vec![0.0_f32; FRAMES];
        effect.process(&mut samples);
        assert!(
            samples.iter().all(|sample| sample.abs() < 1e-9),
            "{effect_type} invents signal from silence"
        );
    }
}

#[test]
fn nothing_leaves_full_scale_or_stops_being_a_number() {
    let registries = registries();
    for effect_type in ALL {
        let mut effect = effect(&registries, effect_type, &[]);
        let mut samples = signal();
        // Run it several times over: a feedback path that grows shows up over
        // repeats, not in one block.
        for _ in 0..8 {
            effect.process(&mut samples);
            assert!(
                samples.iter().all(|sample| sample.is_finite()),
                "{effect_type} produced a non-finite sample"
            );
            assert!(
                peak(&samples) <= 1.0,
                "{effect_type} left full scale: {}",
                peak(&samples)
            );
        }
    }
}

#[test]
fn the_most_extreme_settings_a_descriptor_allows_are_still_stable() {
    let registries = registries();
    for effect_type in ALL {
        let descriptor = registries
            .effect_descriptor(&type_id(effect_type))
            .expect("registered")
            .clone();
        for extreme in [true, false] {
            let parameters: BTreeMap<String, ParameterValue> = descriptor
                .parameters
                .iter()
                .filter_map(|parameter| {
                    let bound = if extreme {
                        parameter.maximum
                    } else {
                        parameter.minimum
                    }?;
                    Some((parameter.id.clone(), ParameterValue::Float(bound)))
                })
                .collect();
            let mut effect = registries
                .instantiate_effect(&type_id(effect_type), &parameters)
                .expect("the bounds are acceptable values");
            effect.prepare(RATE);
            effect.reset();

            let mut samples = signal();
            for _ in 0..8 {
                effect.process(&mut samples);
            }
            assert!(
                samples.iter().all(|sample| sample.is_finite()),
                "{effect_type} at its {} bounds is unstable",
                if extreme { "upper" } else { "lower" }
            );
            assert!(peak(&samples) <= 1.0, "{effect_type} left full scale");
        }
    }
}

#[test]
fn the_same_input_processes_to_the_same_output_every_run() {
    let registries = registries();
    for effect_type in ALL {
        let mut first = effect(&registries, effect_type, &[]);
        let mut second = effect(&registries, effect_type, &[]);
        let mut one = signal();
        let mut two = signal();
        first.process(&mut one);
        second.process(&mut two);
        assert_eq!(one, two, "{effect_type} is not deterministic");

        // And after a reset, an instance repeats itself.
        first.reset();
        let mut again = signal();
        first.process(&mut again);
        assert_eq!(one, again, "{effect_type} does not reset cleanly");
    }
}

#[test]
fn a_low_pass_removes_energy_and_a_high_pass_removes_the_offset() {
    let registries = registries();

    let mut low = effect(
        &registries,
        filters::LOW_PASS_TYPE_ID,
        &[("cutoff_hz", 200.0)],
    );
    let mut samples = signal();
    let before = energy(&samples);
    low.process(&mut samples);
    assert!(
        energy(&samples) < before * 0.5,
        "a low cutoff takes the top off a square wave"
    );

    let mut high = effect(
        &registries,
        filters::HIGH_PASS_TYPE_ID,
        &[("cutoff_hz", 2_000.0)],
    );
    let mut constant = vec![1.0_f32; FRAMES];
    high.process(&mut constant);
    assert!(
        constant[FRAMES - 1].abs() < 0.01,
        "a constant is entirely offset, and a high-pass removes it: {}",
        constant[FRAMES - 1]
    );
}

#[test]
fn a_compressor_reduces_what_is_over_its_threshold() {
    let registries = registries();
    let mut effect = effect(
        &registries,
        dynamics::TYPE_ID,
        &[
            ("threshold_db", -20.0),
            ("ratio", 8.0),
            ("attack_ms", 0.1),
            ("release_ms", 50.0),
            ("makeup_db", 0.0),
        ],
    );
    let mut samples = signal();
    effect.process(&mut samples);
    // Past the attack, the loud signal is held well below where it started.
    assert!(
        peak(&samples[FRAMES / 2..]) < 0.6,
        "the level is pulled down: {}",
        peak(&samples[FRAMES / 2..])
    );
}

#[test]
fn a_delay_repeats_after_its_time_and_reports_a_tail() {
    let registries = registries();
    let mut effect = effect(
        &registries,
        delay::TYPE_ID,
        &[("delay_ms", 10.0), ("feedback", 0.5), ("damping", 0.0)],
    );
    let delay_frames = (10.0 * f64::from(RATE) / 1_000.0) as usize;

    let mut samples = vec![0.0_f32; FRAMES];
    samples[0] = 1.0;
    effect.process(&mut samples);

    assert!(
        samples[delay_frames].abs() > 0.5,
        "the first repeat lands a delay later: {}",
        samples[delay_frames]
    );
    assert!(
        effect.tail_frames() > delay_frames as u32,
        "the tail outlasts one repeat"
    );
}

#[test]
fn a_reverb_keeps_ringing_after_its_input_stops() {
    let registries = registries();
    let mut effect = effect(
        &registries,
        reverb::TYPE_ID,
        &[("size", 0.8), ("damping", 0.2)],
    );

    let mut samples = vec![0.0_f32; FRAMES];
    samples[..64].fill(1.0);
    effect.process(&mut samples);

    let tail = &samples[FRAMES / 2..];
    assert!(
        peak(tail) > 1e-4,
        "the room is still audible long after the input: {}",
        peak(tail)
    );
    assert!(effect.tail_frames() > 0, "and it says how long it rings");
}
