//! What an extension has to get right, as a function anyone can run.
//!
//! The registries check a descriptor when it is registered. That catches a
//! malformed declaration, not a misbehaving implementation: a synth that keeps
//! playing after `reset`, an effect that returns NaN at the edge of its own
//! declared range, a generator that is not reproducible from its seed. Those
//! break a project quietly — an export that differs from playback, a mix full
//! of silence — so they are worth catching before a build ships.
//!
//! Third-party extensions run the same checks the built-in ones do:
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use jutsu_audio_extensions::{SynthFactory, conformance};
//! # fn example(factory: Arc<dyn SynthFactory>) {
//! let findings = conformance::check_synth(factory.as_ref());
//! assert!(findings.is_empty(), "{findings:?}");
//! # }
//! ```
//!
//! Every check is deterministic: fixed rate, fixed block, fixed events, fixed
//! seeds. A finding is a defect, never a flake.

use std::collections::BTreeMap;

use jutsu_audio_model::ParameterValue;

use crate::{
    EffectFactory, ExtensionDescriptor, GeneratorFactory, NoteEvent, NoteEventKind, ParameterType,
    SynthFactory,
};

/// The rate and block every check uses.
const RATE: u32 = 48_000;
const BLOCK: usize = 512;
/// Anything past this is not audio, whatever the extension meant by it.
const FULL_SCALE: f32 = 1.0;

/// Instantiating an extension with a given parameter set, for the checks that
/// only care whether it was accepted.
type Instantiate<'a> = &'a dyn Fn(&BTreeMap<String, ParameterValue>) -> Result<(), ()>;

/// One thing an extension gets wrong, in the words a fix needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Finding {
    /// Which rule was broken, stable enough to match on.
    pub rule: &'static str,
    pub detail: String,
}

impl Finding {
    fn new(rule: &'static str, detail: impl Into<String>) -> Self {
        Self {
            rule,
            detail: detail.into(),
        }
    }
}

/// Checks a synth: descriptor, defaults, determinism, reset, and output range.
#[must_use]
pub fn check_synth(factory: &dyn SynthFactory) -> Vec<Finding> {
    let descriptor = factory.descriptor();
    let mut findings = check_descriptor(descriptor);

    let defaults = defaults(descriptor);
    let Ok(mut first) = factory.instantiate(&defaults) else {
        findings.push(Finding::new(
            "defaults_instantiate",
            "the descriptor's own default parameters are refused",
        ));
        return findings;
    };
    let Ok(mut second) = factory.instantiate(&defaults) else {
        return findings;
    };

    let events = [
        NoteEvent {
            frame_offset: 0,
            kind: NoteEventKind::NoteOn {
                pitch_hz: 220.0,
                velocity: 0.8,
            },
        },
        NoteEvent {
            frame_offset: 300,
            kind: NoteEventKind::NoteOff { pitch_hz: 220.0 },
        },
    ];

    let one = render_synth(&mut *first, &events);
    let two = render_synth(&mut *second, &events);
    if one != two {
        findings.push(Finding::new(
            "deterministic",
            "two instances with the same parameters rendered different audio",
        ));
    }
    findings.extend(check_samples("synth", &one));

    // Reset has to undo everything the first render left behind, or an export
    // will not match what playback just produced.
    first.reset();
    first.prepare(RATE);
    let after_reset = render_synth(&mut *first, &events);
    if after_reset != one {
        findings.push(Finding::new(
            "reset_restores_initial_state",
            "rendering after reset did not reproduce the first render",
        ));
    }

    // Silence in, silence out: no events must mean no sound, or a project gets
    // audio nobody put there.
    let mut idle = vec![0.0_f32; BLOCK];
    let Ok(mut fresh) = factory.instantiate(&defaults) else {
        return findings;
    };
    fresh.prepare(RATE);
    fresh.render(&[], &mut idle);
    if idle.iter().any(|sample| *sample != 0.0) {
        findings.push(Finding::new(
            "silent_without_notes",
            "the synth produced sound with no note events",
        ));
    }

    findings.extend(check_parameter_bounds(descriptor, &|parameters| {
        factory.instantiate(parameters).map(|_| ()).map_err(drop)
    }));
    findings
}

/// Checks an effect: descriptor, defaults, determinism, reset, range, and that
/// silence stays silent once any declared tail has passed.
#[must_use]
pub fn check_effect(factory: &dyn EffectFactory) -> Vec<Finding> {
    let descriptor = factory.descriptor();
    let mut findings = check_descriptor(descriptor);

    let defaults = defaults(descriptor);
    let Ok(mut first) = factory.instantiate(&defaults) else {
        findings.push(Finding::new(
            "defaults_instantiate",
            "the descriptor's own default parameters are refused",
        ));
        return findings;
    };
    let Ok(mut second) = factory.instantiate(&defaults) else {
        return findings;
    };

    let one = process(&mut *first, &input());
    let two = process(&mut *second, &input());
    if one != two {
        findings.push(Finding::new(
            "deterministic",
            "two instances with the same parameters processed the same input differently",
        ));
    }
    findings.extend(check_samples("effect", &one));

    first.reset();
    first.prepare(RATE);
    let after_reset = process(&mut *first, &input());
    if after_reset != one {
        findings.push(Finding::new(
            "reset_restores_initial_state",
            "processing after reset did not reproduce the first result",
        ));
    }

    // A fresh instance handed silence has nothing to decay from, so anything
    // it produces is invented.
    let Ok(mut fresh) = factory.instantiate(&defaults) else {
        return findings;
    };
    fresh.prepare(RATE);
    let mut silence = vec![0.0_f32; BLOCK];
    fresh.process(&mut silence);
    if silence.iter().any(|sample| sample.abs() > 1e-6) {
        findings.push(Finding::new(
            "silence_in_silence_out",
            "a fresh instance turned silence into sound",
        ));
    }

    // Latency and tail are what a chain uses to line audio up. An implausible
    // number is worse than none: it delays or extends every render.
    if first.latency_frames() > RATE {
        findings.push(Finding::new(
            "plausible_latency",
            format!(
                "latency of {} frames is over a second",
                first.latency_frames()
            ),
        ));
    }
    if first.tail_frames() > RATE * 60 {
        findings.push(Finding::new(
            "plausible_tail",
            format!("tail of {} frames is over a minute", first.tail_frames()),
        ));
    }

    findings.extend(check_parameter_bounds(descriptor, &|parameters| {
        factory.instantiate(parameters).map(|_| ()).map_err(drop)
    }));
    findings
}

/// Checks a generator: descriptor, defaults, range, and reproducibility from
/// the seed — the property the whole regenerate flow rests on.
#[must_use]
pub fn check_generator(factory: &dyn GeneratorFactory) -> Vec<Finding> {
    let descriptor = factory.descriptor();
    let mut findings = check_descriptor(descriptor);

    let defaults = defaults(descriptor);
    let Ok(generator) = factory.instantiate(&defaults) else {
        findings.push(Finding::new(
            "defaults_instantiate",
            "the descriptor's own default parameters are refused",
        ));
        return findings;
    };

    let frames = RATE as usize / 4;
    let one = generator.generate_mono(7, frames);
    let two = generator.generate_mono(7, frames);
    if one != two {
        findings.push(Finding::new(
            "same_seed_same_audio",
            "the same seed produced different audio on the second call",
        ));
    }
    if one.len() != frames {
        findings.push(Finding::new(
            "honours_frame_count",
            format!("asked for {frames} frames, got {}", one.len()),
        ));
    }
    findings.extend(check_samples("generator", &one));

    // A second instance must agree with the first, or a project regenerated on
    // another machine is a different sound.
    if let Ok(again) = factory.instantiate(&defaults)
        && again.generate_mono(7, frames) != one
    {
        findings.push(Finding::new(
            "same_seed_same_audio",
            "a second instance produced different audio for the same seed",
        ));
    }
    // Note what is *not* checked: that a different seed gives different audio.
    // A generator is allowed to ignore its seed — `sfx.pickup` plays a written
    // phrase — and demanding variation would make that a defect.
    if !generator.generate_mono(7, 0).is_empty() {
        findings.push(Finding::new(
            "honours_frame_count",
            "asked for no frames, got samples",
        ));
    }

    findings.extend(check_parameter_bounds(descriptor, &|parameters| {
        factory.instantiate(parameters).map(|_| ()).map_err(drop)
    }));
    findings
}

/// Rules about the declaration itself, beyond what registration enforces.
fn check_descriptor(descriptor: &ExtensionDescriptor) -> Vec<Finding> {
    let mut findings = Vec::new();
    if descriptor.display_name.trim().is_empty() {
        findings.push(Finding::new(
            "descriptor_has_display_name",
            "an extension with no display name cannot be offered in a menu",
        ));
    }
    for parameter in &descriptor.parameters {
        if parameter.display_name.trim().is_empty() {
            findings.push(Finding::new(
                "parameter_has_display_name",
                format!("parameter '{}' has no display name", parameter.id),
            ));
        }
        // A parameter introduced after this version could never be set: the
        // state version is what a project stores to know what it may write.
        if parameter.introduced_in_state_version > descriptor.state_version {
            findings.push(Finding::new(
                "parameter_version_within_state_version",
                format!(
                    "parameter '{}' claims state version {} on an extension at {}",
                    parameter.id, parameter.introduced_in_state_version, descriptor.state_version
                ),
            ));
        }
        if let (Some(minimum), Some(maximum)) = (parameter.minimum, parameter.maximum)
            && minimum > maximum
        {
            findings.push(Finding::new(
                "parameter_range_is_ordered",
                format!(
                    "parameter '{}' has a minimum above its maximum",
                    parameter.id
                ),
            ));
        }
        if !matches!(parameter.value_type, ParameterType::Text)
            && let Some(number) = numeric(&parameter.default_value)
            && (parameter.minimum.is_some_and(|minimum| number < minimum)
                || parameter.maximum.is_some_and(|maximum| number > maximum))
        {
            findings.push(Finding::new(
                "default_within_range",
                format!(
                    "parameter '{}' defaults outside its own range",
                    parameter.id
                ),
            ));
        }
    }
    findings
}

/// A value outside a declared range must be refused. The host does that check
/// from the descriptor, so this is really a check that the descriptor declares
/// the range it means: an automation lane writes anywhere inside it.
fn check_parameter_bounds(
    descriptor: &ExtensionDescriptor,
    instantiate: Instantiate<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for parameter in &descriptor.parameters {
        let Some(maximum) = parameter.maximum else {
            continue;
        };
        let over = match parameter.value_type {
            ParameterType::Float => ParameterValue::Float(maximum + 1.0),
            ParameterType::Integer => ParameterValue::Integer(maximum as i64 + 1),
            ParameterType::Bool | ParameterType::Text => continue,
        };
        let mut parameters = defaults(descriptor);
        parameters.insert(parameter.id.clone(), over.clone());
        // Through the same validation a host runs before instantiating: an
        // extension body is entitled to assume its parameters are in range.
        let refused =
            crate::parameters::validate_named(&descriptor.parameters, &parameter.id, &over)
                .is_err();
        if !refused && instantiate(&parameters).is_ok() {
            findings.push(Finding::new(
                "out_of_range_is_refused",
                format!(
                    "parameter '{}' accepted a value above its declared maximum",
                    parameter.id
                ),
            ));
        }

        // The other half of the same rule: the extremes it does declare have to
        // work, or the range promises more than the extension delivers.
        for edge in [parameter.minimum, Some(maximum)].into_iter().flatten() {
            let value = match parameter.value_type {
                ParameterType::Float => ParameterValue::Float(edge),
                #[allow(clippy::cast_possible_truncation)]
                ParameterType::Integer => ParameterValue::Integer(edge as i64),
                ParameterType::Bool | ParameterType::Text => continue,
            };
            let mut parameters = defaults(descriptor);
            parameters.insert(parameter.id.clone(), value);
            if instantiate(&parameters).is_err() {
                findings.push(Finding::new(
                    "declared_range_is_usable",
                    format!(
                        "parameter '{}' refused {edge}, which its own range allows",
                        parameter.id
                    ),
                ));
            }
        }
    }
    findings
}

fn check_samples(what: &'static str, samples: &[f32]) -> Vec<Finding> {
    let mut findings = Vec::new();
    if samples.iter().any(|sample| !sample.is_finite()) {
        findings.push(Finding::new(
            "finite_output",
            format!("the {what} produced NaN or infinity"),
        ));
    }
    if let Some(peak) = samples
        .iter()
        .filter(|sample| sample.is_finite())
        .map(|sample| sample.abs())
        .reduce(f32::max)
        && peak > FULL_SCALE
    {
        findings.push(Finding::new(
            "within_full_scale",
            format!("the {what} peaked at {peak:.3}, past full scale"),
        ));
    }
    findings
}

fn defaults(descriptor: &ExtensionDescriptor) -> BTreeMap<String, ParameterValue> {
    descriptor
        .parameters
        .iter()
        .map(|parameter| (parameter.id.clone(), parameter.default_value.clone()))
        .collect()
}

fn numeric(value: &ParameterValue) -> Option<f64> {
    match value {
        ParameterValue::Float(number) => Some(*number),
        #[allow(clippy::cast_precision_loss)]
        ParameterValue::Integer(number) => Some(*number as f64),
        ParameterValue::Bool(_) | ParameterValue::Text(_) => None,
    }
}

fn render_synth(synth: &mut dyn crate::Synth, events: &[NoteEvent]) -> Vec<f32> {
    synth.prepare(RATE);
    let mut output = vec![0.0_f32; BLOCK];
    synth.render(events, &mut output);
    let mut tail = vec![0.0_f32; BLOCK];
    synth.render(&[], &mut tail);
    output.extend(tail);
    output
}

/// A fixed signal with a step, a ramp and a burst: enough to move a filter, a
/// compressor and a delay off their initial state.
fn input() -> Vec<f32> {
    (0..BLOCK * 2)
        .map(|index| {
            let phase = index as f32 / 32.0;
            0.5 * phase.sin() + if index % 128 == 0 { 0.4 } else { 0.0 }
        })
        .collect()
}

fn process(effect: &mut dyn crate::Effect, signal: &[f32]) -> Vec<f32> {
    effect.prepare(RATE);
    let mut buffer = signal.to_vec();
    for block in buffer.chunks_mut(BLOCK) {
        effect.process(block);
    }
    buffer
}
