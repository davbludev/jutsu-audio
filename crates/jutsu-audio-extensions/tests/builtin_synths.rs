//! The reference synths, driven the way both the audio callback and an offline
//! render drive them: through the registry, by note events, at a known rate.

use std::collections::BTreeMap;

use jutsu_audio_extensions::builtin::{noise_type_id, oscillator_type_id};
use jutsu_audio_extensions::{
    ExtensionErrorCode, ExtensionKind, ExtensionRegistries, NoteEvent, NoteEventKind, Synth,
    register_builtin,
};
use jutsu_audio_model::ParameterValue;

const RATE: u32 = 48_000;

fn registries() -> ExtensionRegistries {
    let mut registries = ExtensionRegistries::default();
    register_builtin(&mut registries).expect("the built-ins register");
    registries
}

fn synth(type_id_text: &str, parameters: &[(&str, ParameterValue)]) -> Box<dyn Synth> {
    let registries = registries();
    let type_id = if type_id_text == "oscillator" {
        oscillator_type_id()
    } else {
        noise_type_id()
    };
    let parameters: BTreeMap<String, ParameterValue> = parameters
        .iter()
        .map(|(id, value)| ((*id).to_string(), value.clone()))
        .collect();
    let mut synth = registries
        .instantiate_synth(&type_id, &parameters)
        .expect("instantiates");
    synth.prepare(RATE);
    synth
}

fn render(synth: &mut dyn Synth, events: &[NoteEvent], frames: usize) -> Vec<f32> {
    let mut output = vec![0.0; frames];
    synth.render(events, &mut output);
    output
}

#[test]
fn the_built_ins_are_discoverable_through_the_registry_with_their_parameters() {
    let registries = registries();
    let descriptor = registries
        .synth_descriptor(&oscillator_type_id())
        .expect("the oscillator is registered");
    assert_eq!(descriptor.kind, ExtensionKind::Synth);
    assert_eq!(descriptor.state_version, 1);
    let ids: Vec<&str> = descriptor
        .parameters
        .iter()
        .map(|parameter| parameter.id.as_str())
        .collect();
    assert_eq!(ids, ["waveform", "gain_db", "attack_ms", "release_ms"]);

    assert!(registries.synth_descriptor(&noise_type_id()).is_some());
}

#[test]
fn a_note_starts_on_the_frame_it_names_and_not_before() {
    let mut synth = synth(
        "oscillator",
        &[
            ("waveform", ParameterValue::Text("square".into())),
            ("attack_ms", ParameterValue::Float(0.0)),
        ],
    );

    let output = render(synth.as_mut(), &[NoteEvent::note_on(4, 1_000.0, 1.0)], 8);
    assert!(
        output[..4].iter().all(|sample| *sample == 0.0),
        "nothing sounds before the note-on frame: {output:?}"
    );
    assert!(
        output[4..].iter().any(|sample| *sample != 0.0),
        "the note sounds from its frame onward: {output:?}"
    );
}

#[test]
fn a_note_off_releases_the_voice_and_the_tail_falls_to_silence() {
    let mut synth = synth(
        "oscillator",
        &[
            ("waveform", ParameterValue::Text("square".into())),
            ("attack_ms", ParameterValue::Float(0.0)),
            // Ten frames of release at 48 kHz.
            ("release_ms", ParameterValue::Float(10.0 / 48.0)),
        ],
    );

    let output = render(
        synth.as_mut(),
        &[
            NoteEvent::note_on(0, 1_000.0, 1.0),
            NoteEvent::note_off(4, 1_000.0),
        ],
        32,
    );
    assert!(output[..4].iter().any(|sample| *sample != 0.0));
    assert!(
        output[20..].iter().all(|sample| sample.abs() < 1e-6),
        "the release finishes: {:?}",
        &output[16..]
    );
}

#[test]
fn overlapping_notes_sound_together_and_the_voice_limit_holds() {
    let mut synth = synth(
        "oscillator",
        &[
            ("waveform", ParameterValue::Text("square".into())),
            ("attack_ms", ParameterValue::Float(0.0)),
            ("release_ms", ParameterValue::Float(1_000.0)),
        ],
    );

    let one = render(synth.as_mut(), &[NoteEvent::note_on(0, 400.0, 1.0)], 4);
    synth.reset();
    let two = render(
        synth.as_mut(),
        &[
            NoteEvent::note_on(0, 400.0, 1.0),
            NoteEvent::note_on(0, 700.0, 1.0),
        ],
        4,
    );
    assert!(
        two[0].abs() > one[0].abs(),
        "two voices are louder than one: {two:?} against {one:?}"
    );

    // Far past the voice limit: still renders, still bounded.
    synth.reset();
    let events: Vec<NoteEvent> = (0..64)
        .map(|index| NoteEvent::note_on(0, 200.0 + f64::from(index) * 13.0, 1.0))
        .collect();
    let crowded = render(synth.as_mut(), &events, 16);
    assert!(
        crowded.iter().all(|sample| sample.is_finite()),
        "voice stealing keeps the mix finite"
    );
}

#[test]
fn all_notes_off_releases_everything_that_is_sounding() {
    let mut synth = synth(
        "oscillator",
        &[
            ("waveform", ParameterValue::Text("square".into())),
            ("attack_ms", ParameterValue::Float(0.0)),
            ("release_ms", ParameterValue::Float(10.0 / 48.0)),
        ],
    );

    let output = render(
        synth.as_mut(),
        &[
            NoteEvent::note_on(0, 400.0, 1.0),
            NoteEvent::note_on(0, 700.0, 1.0),
            NoteEvent {
                frame_offset: 2,
                kind: NoteEventKind::AllNotesOff,
            },
        ],
        32,
    );
    assert!(
        output[20..].iter().all(|sample| sample.abs() < 1e-6),
        "everything released: {:?}",
        &output[16..]
    );
}

#[test]
fn the_same_events_render_the_same_samples_after_a_reset() {
    for kind in ["oscillator", "noise"] {
        let mut synth = synth(kind, &[]);
        let events = [
            NoteEvent::note_on(0, 440.0, 0.8),
            NoteEvent::note_off(64, 440.0),
        ];
        let first = render(synth.as_mut(), &events, 256);
        synth.reset();
        let again = render(synth.as_mut(), &events, 256);
        assert_eq!(first, again, "{kind} is not deterministic across a reset");
    }
}

#[test]
fn the_same_note_at_two_rates_lasts_the_same_time_in_seconds() {
    // One millisecond of attack is one millisecond, whatever the device runs at.
    let mut fast = synth(
        "oscillator",
        &[
            ("waveform", ParameterValue::Text("saw".into())),
            ("attack_ms", ParameterValue::Float(1.0)),
        ],
    );
    fast.prepare(48_000);
    let mut slow = synth(
        "oscillator",
        &[
            ("waveform", ParameterValue::Text("saw".into())),
            ("attack_ms", ParameterValue::Float(1.0)),
        ],
    );
    slow.prepare(24_000);

    let fast_output = render(fast.as_mut(), &[NoteEvent::note_on(0, 480.0, 1.0)], 96);
    let slow_output = render(slow.as_mut(), &[NoteEvent::note_on(0, 480.0, 1.0)], 48);
    // Half the frames at half the rate is the same moment in time.
    assert!((fast_output[95].abs() - slow_output[47].abs()).abs() < 0.05);
}

#[test]
fn noise_with_the_same_seed_is_the_same_noise_and_another_seed_is_not() {
    let events = [NoteEvent::note_on(0, 440.0, 1.0)];
    let mut first = synth("noise", &[("seed", ParameterValue::Integer(42))]);
    let mut same = synth("noise", &[("seed", ParameterValue::Integer(42))]);
    let mut other = synth("noise", &[("seed", ParameterValue::Integer(43))]);

    let a = render(first.as_mut(), &events, 64);
    let b = render(same.as_mut(), &events, 64);
    let c = render(other.as_mut(), &events, 64);
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn an_undeclared_or_wrongly_typed_parameter_is_refused_by_the_registry() {
    let registries = registries();
    let Err(unknown) = registries.instantiate_synth(
        &oscillator_type_id(),
        &BTreeMap::from([("cutoff".into(), ParameterValue::Float(1.0))]),
    ) else {
        panic!("an undeclared parameter must be refused");
    };
    assert_eq!(unknown.code, ExtensionErrorCode::InvalidParameters);
    assert_eq!(unknown.parameter_id.as_deref(), Some("cutoff"));

    let Err(wrong_type) = registries.instantiate_synth(
        &oscillator_type_id(),
        &BTreeMap::from([("gain_db".into(), ParameterValue::Text("loud".into()))]),
    ) else {
        panic!("a wrongly typed parameter must be refused");
    };
    assert_eq!(wrong_type.code, ExtensionErrorCode::InvalidParameters);
    assert_eq!(wrong_type.parameter_id.as_deref(), Some("gain_db"));
}

#[test]
fn a_waveform_the_oscillator_does_not_have_names_itself_in_the_error() {
    let registries = registries();
    let Err(error) = registries.instantiate_synth(
        &oscillator_type_id(),
        &BTreeMap::from([("waveform".into(), ParameterValue::Text("bagpipe".into()))]),
    ) else {
        panic!("an unknown waveform must be refused");
    };
    assert_eq!(error.code, ExtensionErrorCode::InvalidParameters);
    assert_eq!(error.parameter_id.as_deref(), Some("waveform"));
    assert!(error.message.contains("bagpipe"), "{}", error.message);
}
