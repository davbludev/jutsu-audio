//! `builtin.subtractive` — an oscillator stack through a resonant filter.
//!
//! What `builtin.oscillator` is missing, and why a cue built on it reads as a
//! console rather than an instrument: its tone is identical on the first frame
//! of a note and the last. Nothing moves while a note is held, so every note is
//! the same note played louder or quieter.
//!
//! This one moves. Two envelopes rather than one — level and filter, with their
//! own times — a state-variable low-pass the filter envelope opens and closes,
//! and a stack of detuned oscillators per voice so a single note is already a
//! chorus of itself.
//!
//! Mono, like every synth here: the mix places a clip in the stereo field, so a
//! synth that panned its own voices would be fighting the strip that owns that
//! decision.

use std::collections::BTreeMap;

use jutsu_audio_model::ParameterValue;

use crate::filter::{Svf, coefficients};

use crate::builtin::{
    DEFAULT_SAMPLE_RATE, Waveform, decibels_to_gain, float_value, text, text_value,
};
use crate::effects::ranged;
use crate::parameters::{UNIT_HERTZ, UNIT_MILLISECONDS, UNIT_NORMALISED};
use crate::voice::{Adsr, MAX_POLYPHONY, NoteEvent, NoteEventKind};
use crate::{
    ExtensionDescriptor, ExtensionError, ExtensionErrorCode, ExtensionKind, ExtensionTypeId,
    ParameterDescriptor, ParameterType, Synth, SynthFactory,
};

pub const TYPE_ID: &str = "builtin.subtractive";

/// The most oscillators one voice stacks. Seven is already more than the ear
/// resolves as separate; the cap exists so a project cannot ask for a thousand.
const MAX_UNISON: usize = 7;

/// `builtin.subtractive`
#[must_use]
pub fn type_id() -> ExtensionTypeId {
    ExtensionTypeId::new(TYPE_ID).expect("a valid built-in ID")
}

#[must_use]
pub fn factory() -> SubtractiveFactory {
    SubtractiveFactory
}

pub struct SubtractiveFactory;

impl SynthFactory for SubtractiveFactory {
    fn descriptor(&self) -> &ExtensionDescriptor {
        static DESCRIPTOR: std::sync::OnceLock<ExtensionDescriptor> = std::sync::OnceLock::new();
        DESCRIPTOR.get_or_init(|| ExtensionDescriptor {
            type_id: type_id(),
            kind: ExtensionKind::Synth,
            display_name: "Subtractive".into(),
            state_version: 1,
            parameters: vec![
                text("waveform", "Waveform", "saw"),
                ranged("gain_db", "Gain", -6.0, -60.0, 12.0, "dB"),
                // Level.
                ranged("attack_ms", "Attack", 6.0, 0.0, 4_000.0, UNIT_MILLISECONDS),
                ranged("decay_ms", "Decay", 260.0, 1.0, 8_000.0, UNIT_MILLISECONDS),
                ranged("sustain", "Sustain", 0.7, 0.0, 1.0, UNIT_NORMALISED),
                ranged(
                    "release_ms",
                    "Release",
                    220.0,
                    1.0,
                    8_000.0,
                    UNIT_MILLISECONDS,
                ),
                // Filter.
                ranged("cutoff_hz", "Cutoff", 1_200.0, 20.0, 18_000.0, UNIT_HERTZ),
                ranged("resonance", "Resonance", 0.35, 0.0, 0.98, UNIT_NORMALISED),
                ranged("env_octaves", "Env amount", 2.5, -6.0, 6.0, UNIT_NORMALISED),
                ranged(
                    "filter_attack_ms",
                    "Filter attack",
                    3.0,
                    0.0,
                    4_000.0,
                    UNIT_MILLISECONDS,
                ),
                ranged(
                    "filter_decay_ms",
                    "Filter decay",
                    320.0,
                    1.0,
                    8_000.0,
                    UNIT_MILLISECONDS,
                ),
                ranged(
                    "filter_sustain",
                    "Filter sustain",
                    0.25,
                    0.0,
                    1.0,
                    UNIT_NORMALISED,
                ),
                ranged(
                    "velocity_to_cutoff",
                    "Velocity → cutoff",
                    0.5,
                    0.0,
                    1.0,
                    UNIT_NORMALISED,
                ),
                // Unison.
                ParameterDescriptor {
                    id: "unison".into(),
                    display_name: "Unison".into(),
                    value_type: ParameterType::Integer,
                    default_value: ParameterValue::Integer(3),
                    introduced_in_state_version: 1,
                    automatable: false,
                    minimum: Some(1.0),
                    maximum: Some(MAX_UNISON as f64),
                    unit: None,
                },
                ranged("detune_cents", "Detune", 14.0, 0.0, 60.0, UNIT_NORMALISED),
            ],
        })
    }

    fn instantiate(
        &self,
        parameters: &BTreeMap<String, ParameterValue>,
    ) -> Result<Box<dyn Synth>, ExtensionError> {
        let waveform = text_value(parameters, "waveform", "saw");
        let waveform = Waveform::parse(&waveform).ok_or_else(|| ExtensionError {
            code: ExtensionErrorCode::InvalidParameters,
            message: format!("waveform '{waveform}' is not one of sine, triangle, saw, square"),
            kind: Some(ExtensionKind::Synth),
            type_id: Some(type_id()),
            parameter_id: Some("waveform".into()),
        })?;

        let unison = match parameters.get("unison") {
            Some(ParameterValue::Integer(value)) => *value,
            _ => 3,
        };
        let unison = unison.clamp(1, MAX_UNISON as i64) as usize;

        Ok(Box::new(Subtractive {
            settings: Settings {
                waveform,
                gain: decibels_to_gain(float_value(parameters, "gain_db", -6.0)),
                attack_ms: float_value(parameters, "attack_ms", 6.0),
                decay_ms: float_value(parameters, "decay_ms", 260.0),
                sustain: float_value(parameters, "sustain", 0.7),
                release_ms: float_value(parameters, "release_ms", 220.0),
                cutoff_hz: float_value(parameters, "cutoff_hz", 1_200.0),
                resonance: float_value(parameters, "resonance", 0.35),
                env_octaves: float_value(parameters, "env_octaves", 2.5),
                filter_attack_ms: float_value(parameters, "filter_attack_ms", 3.0),
                filter_decay_ms: float_value(parameters, "filter_decay_ms", 320.0),
                filter_sustain: float_value(parameters, "filter_sustain", 0.25),
                velocity_to_cutoff: float_value(parameters, "velocity_to_cutoff", 0.5),
                unison,
                detune_cents: float_value(parameters, "detune_cents", 14.0),
            },
            sample_rate: DEFAULT_SAMPLE_RATE,
            voices: Vec::with_capacity(MAX_POLYPHONY),
        }))
    }
}

/// Everything resolved once, at instantiation.
#[derive(Clone, Copy, Debug)]
struct Settings {
    waveform: Waveform,
    gain: f32,
    attack_ms: f64,
    decay_ms: f64,
    sustain: f64,
    release_ms: f64,
    cutoff_hz: f64,
    resonance: f64,
    env_octaves: f64,
    filter_attack_ms: f64,
    filter_decay_ms: f64,
    filter_sustain: f64,
    velocity_to_cutoff: f64,
    unison: usize,
    detune_cents: f64,
}

/// A polynomial band-limited step, for rounding off the discontinuity in a saw
/// or a square.
///
/// A jump from +1 to -1 in one sample is infinitely wide in frequency, and
/// everything above half the sample rate folds back down as inharmonic
/// aliasing — the metallic edge that makes cheap digital synths sound cheap.
/// `builtin.oscillator` keeps its naive shapes on purpose: they are what a
/// chiptune wants, and its tests pin them. This one is an instrument.
fn poly_blep(phase: f64, step: f64) -> f64 {
    if step <= 0.0 {
        return 0.0;
    }
    if phase < step {
        let t = phase / step;
        t + t - t * t - 1.0
    } else if phase > 1.0 - step {
        let t = (phase - 1.0) / step;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

/// One oscillator sample, band-limited where the shape has an edge.
///
/// `step` is the phase advanced per frame, which is how wide the correction
/// has to be: a high note needs a wider one than a low note.
fn band_limited(waveform: Waveform, phase: f64, step: f64) -> f32 {
    match waveform {
        // Continuous shapes have nothing to correct.
        Waveform::Sine | Waveform::Triangle => waveform.sample(phase),
        Waveform::Saw => (phase.mul_add(2.0, -1.0) - poly_blep(phase, step)) as f32,
        Waveform::Square => {
            let raw = if phase < 0.5 { 1.0 } else { -1.0 };
            // Two edges per cycle: one at the start, one at the halfway point.
            let corrected = raw + poly_blep(phase, step) - poly_blep((phase + 0.5).fract(), step);
            corrected as f32
        }
    }
}

/// One sounding note: a stack of detuned oscillators, two envelopes, a filter.
#[derive(Clone, Copy, Debug)]
struct Voice {
    pitch_hz: f64,
    velocity: f32,
    phases: [f64; MAX_UNISON],
    ratios: [f64; MAX_UNISON],
    amp: Adsr,
    filter_env: Adsr,
    filter: Svf,
}

pub struct Subtractive {
    settings: Settings,
    sample_rate: u32,
    voices: Vec<Voice>,
}

impl Subtractive {
    fn note_on(&mut self, pitch_hz: f64, velocity: f32) {
        let settings = self.settings;
        let mut phases = [0.0_f64; MAX_UNISON];
        let mut ratios = [1.0_f64; MAX_UNISON];
        for index in 0..settings.unison {
            // Detune spreads symmetrically around the played pitch, so the note
            // itself never drifts however wide the stack is.
            let offset = if settings.unison == 1 {
                0.0
            } else {
                let position = index as f64 / (settings.unison - 1) as f64;
                (position - 0.5) * 2.0 * settings.detune_cents
            };
            ratios[index] = 2.0_f64.powf(offset / 1_200.0);
            // Phases spread too: starting every oscillator at zero makes the
            // first frames of a note a single loud transient.
            phases[index] = index as f64 / settings.unison as f64;
        }

        let mut amp = Adsr::new(
            self.sample_rate,
            settings.attack_ms,
            settings.decay_ms,
            settings.sustain,
            settings.release_ms,
        );
        amp.trigger();
        let mut filter_env = Adsr::new(
            self.sample_rate,
            settings.filter_attack_ms,
            settings.filter_decay_ms,
            settings.filter_sustain,
            settings.release_ms,
        );
        filter_env.trigger();

        let voice = Voice {
            pitch_hz,
            velocity: velocity.clamp(0.0, 1.0),
            phases,
            ratios,
            amp,
            filter_env,
            filter: Svf::default(),
        };
        if self.voices.len() < MAX_POLYPHONY {
            self.voices.push(voice);
            return;
        }
        let quietest = self
            .voices
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                left.amp
                    .level
                    .partial_cmp(&right.amp.level)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map_or(0, |(index, _)| index);
        self.voices[quietest] = voice;
    }

    fn apply(&mut self, kind: NoteEventKind) {
        match kind {
            NoteEventKind::NoteOn { pitch_hz, velocity } => self.note_on(pitch_hz, velocity),
            NoteEventKind::NoteOff { pitch_hz } => {
                for voice in &mut self.voices {
                    if voice.pitch_hz == pitch_hz {
                        voice.amp.release();
                        voice.filter_env.release();
                    }
                }
            }
            NoteEventKind::AllNotesOff => {
                for voice in &mut self.voices {
                    voice.amp.release();
                    voice.filter_env.release();
                }
            }
        }
    }
}

impl Synth for Subtractive {
    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate.max(1);
        self.reset();
    }

    fn reset(&mut self) {
        self.voices.clear();
    }

    fn render(&mut self, events: &[NoteEvent], output: &mut [f32]) {
        output.fill(0.0);
        let settings = self.settings;
        let sample_rate = f64::from(self.sample_rate.max(1));
        let nyquist = sample_rate * 0.5;
        // Damping from resonance: 2.0 is no resonance at all, and the lower it
        // goes the more the filter rings at its cutoff.
        let resonance = settings.resonance;
        // One oscillator's share of the stack, so unison is a thicker note
        // rather than a louder one.
        let voice_gain = settings.gain / settings.unison as f32;

        let mut next_event = 0;
        for (frame, slot) in output.iter_mut().enumerate() {
            while next_event < events.len() && events[next_event].frame_offset <= frame {
                self.apply(events[next_event].kind);
                next_event += 1;
            }

            let mut sum = 0.0_f32;
            for voice in &mut self.voices {
                let amp = voice.amp.advance();
                let filter_level = voice.filter_env.advance();
                if !voice.amp.is_active() {
                    continue;
                }

                // Cutoff in octaves: the envelope and the velocity both move it
                // multiplicatively, which is how a filter is heard.
                let octaves = settings.env_octaves * f64::from(filter_level)
                    + settings.velocity_to_cutoff * f64::from(voice.velocity) * 2.0;
                let cutoff =
                    (settings.cutoff_hz * 2.0_f64.powf(octaves)).clamp(20.0, nyquist * 0.98);
                let (g, k) = coefficients(cutoff, resonance, sample_rate);

                let mut stack = 0.0_f32;
                for index in 0..settings.unison {
                    let step = voice.pitch_hz * voice.ratios[index] / sample_rate;
                    stack += band_limited(settings.waveform, voice.phases[index], step);
                    voice.phases[index] = (voice.phases[index] + step).fract();
                }
                let filtered = voice.filter.low_pass(stack, g, k);
                sum += filtered * amp * voice.velocity * voice_gain;
            }
            *slot = if sum.is_finite() {
                sum.clamp(-4.0, 4.0)
            } else {
                0.0
            };
        }

        while next_event < events.len() {
            self.apply(events[next_event].kind);
            next_event += 1;
        }
        self.voices.retain(|voice| voice.amp.is_active());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(parameters: &[(&str, ParameterValue)], frames: usize) -> Vec<f32> {
        let settings: BTreeMap<String, ParameterValue> = parameters
            .iter()
            .map(|(id, value)| ((*id).to_owned(), value.clone()))
            .collect();
        let mut synth = factory().instantiate(&settings).expect("instantiate");
        synth.prepare(48_000);
        let events = [NoteEvent::note_on(0, 110.0, 1.0)];
        let mut output = vec![0.0_f32; frames];
        synth.render(&events, &mut output);
        output
    }

    /// High-frequency energy: three one-pole high-passes at roughly 3 kHz,
    /// then the RMS of what survives.
    ///
    /// One pole is not enough. A 110 Hz saw carries so much more energy at its
    /// fundamental than above 3 kHz that a single-pole measurement is mostly
    /// leakage, and a filter sweep barely moves it. Three poles is 18 dB per
    /// octave, which puts the fundamental far enough down to be irrelevant.
    fn brightness(window: &[f32]) -> f32 {
        let coefficient = 0.33_f32;
        let mut poles = [0.0_f32; 3];
        let mut sum = 0.0_f32;
        for sample in window {
            let mut high = *sample;
            for pole in &mut poles {
                *pole += coefficient * (high - *pole);
                high -= *pole;
            }
            sum += high * high;
        }
        (sum / window.len() as f32).sqrt()
    }

    /// The reason this synth exists: the tone at the end of a held note is not
    /// the tone at its start.
    #[test]
    fn a_held_note_gets_darker_as_its_filter_envelope_closes() {
        let output = render(
            &[
                ("waveform", ParameterValue::Text("saw".into())),
                ("attack_ms", ParameterValue::Float(1.0)),
                ("decay_ms", ParameterValue::Float(4_000.0)),
                ("sustain", ParameterValue::Float(1.0)),
                ("cutoff_hz", ParameterValue::Float(400.0)),
                ("env_octaves", ParameterValue::Float(5.0)),
                ("filter_attack_ms", ParameterValue::Float(1.0)),
                ("filter_decay_ms", ParameterValue::Float(400.0)),
                ("filter_sustain", ParameterValue::Float(0.0)),
                ("velocity_to_cutoff", ParameterValue::Float(0.0)),
                // One oscillator: a detuned stack beats, and a beat moves the
                // measurement around for reasons that have nothing to do with
                // the filter this test is about.
                ("unison", ParameterValue::Integer(1)),
            ],
            48_000,
        );

        // Three windows rather than two: a single pair could be a coincidence
        // of where the measurement landed, a falling sequence could not.
        let opening = brightness(&output[2_000..6_000]);
        let middle = brightness(&output[12_000..16_000]);
        let closed = brightness(&output[40_000..44_000]);
        assert!(
            opening > middle && middle > closed,
            "the filter did not close steadily: {opening}, {middle}, {closed}"
        );
        // Measured, not aspirational: the run this was written against drops by
        // a factor of four and a half, and a filter that stopped moving would
        // sit at one.
        assert!(
            opening > closed * 3.0,
            "the filter barely moved: {opening} then {closed}"
        );
        assert!(
            closed > 0.0,
            "the note fell silent instead of getting darker"
        );
    }

    /// A filter that only ever sits still is the old oscillator with extra
    /// parameters, so the envelope's depth has to change what is heard.
    #[test]
    fn the_envelope_depth_is_what_opens_the_filter() {
        let settings = |octaves: f64| {
            vec![
                ("waveform", ParameterValue::Text("saw".into())),
                ("cutoff_hz", ParameterValue::Float(400.0)),
                ("filter_decay_ms", ParameterValue::Float(2_000.0)),
                ("filter_sustain", ParameterValue::Float(1.0)),
                ("velocity_to_cutoff", ParameterValue::Float(0.0)),
                ("unison", ParameterValue::Integer(1)),
                ("env_octaves", ParameterValue::Float(octaves)),
            ]
        };
        let shut = render(&settings(0.0), 12_000);
        let open = render(&settings(5.0), 12_000);
        let (open, shut) = (
            brightness(&open[4_000..12_000]),
            brightness(&shut[4_000..12_000]),
        );
        assert!(
            open > shut * 3.0,
            "opening the filter did not brighten the note: {open} against {shut}"
        );
    }

    #[test]
    fn unison_detunes_around_the_played_pitch_rather_than_off_it() {
        let voices = |unison: i64, cents: f64| {
            render(
                &[
                    // A sine has one zero crossing per cycle and no harmonics to
                    // confuse the count, which is what makes the pitch legible.
                    ("waveform", ParameterValue::Text("sine".into())),
                    ("cutoff_hz", ParameterValue::Float(18_000.0)),
                    ("env_octaves", ParameterValue::Float(0.0)),
                    ("velocity_to_cutoff", ParameterValue::Float(0.0)),
                    ("unison", ParameterValue::Integer(unison)),
                    ("detune_cents", ParameterValue::Float(cents)),
                ],
                4_800,
            )
        };
        let one = voices(1, 0.0);
        let seven = voices(7, 30.0);
        assert_ne!(one, seven, "unison changed nothing");

        let crossings = |samples: &[f32]| {
            samples
                .windows(2)
                .filter(|pair| (pair[0] <= 0.0) != (pair[1] <= 0.0))
                .count()
        };
        // 110 Hz over 4800 frames at 48 kHz is eleven cycles: twenty-two
        // crossings, and a stack detuned around that pitch keeps them.
        let single = crossings(&one[480..]);
        let stacked = crossings(&seven[480..]);
        let drift = (single as i64 - stacked as i64).abs();
        assert!(
            drift <= 2,
            "the stack shifted the pitch: {single} crossings alone, {stacked} stacked"
        );
    }

    #[test]
    fn every_declared_waveform_is_accepted_and_anything_else_is_refused() {
        for waveform in ["sine", "triangle", "saw", "square"] {
            let settings = BTreeMap::from([(
                "waveform".to_owned(),
                ParameterValue::Text(waveform.to_owned()),
            )]);
            assert!(factory().instantiate(&settings).is_ok(), "{waveform}");
        }
        let settings = BTreeMap::from([(
            "waveform".to_owned(),
            ParameterValue::Text("sawtooth".to_owned()),
        )]);
        let error = match factory().instantiate(&settings) {
            Ok(_) => panic!("an unknown waveform was accepted"),
            Err(error) => error,
        };
        assert_eq!(error.parameter_id.as_deref(), Some("waveform"));
    }
}
