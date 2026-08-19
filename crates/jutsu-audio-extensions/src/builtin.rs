//! The reference synths: one oscillator, one noise source.
//!
//! They exist to prove the contract end to end — registry, parameters, note
//! lifecycle, polyphony, reset — and because a game SFX toolkit needs exactly
//! these two building blocks before anything else.

use std::collections::BTreeMap;
use std::sync::Arc;

use jutsu_audio_model::ParameterValue;

use crate::voice::{Envelope, MAX_POLYPHONY, Noise, NoteEvent, NoteEventKind};
use crate::{
    ExtensionDescriptor, ExtensionError, ExtensionKind, ExtensionRegistries, ExtensionTypeId,
    ParameterDescriptor, ParameterType, Synth, SynthFactory,
};

/// Registers everything this crate ships. Applications call it once at start-up
/// and may register their own factories after it.
pub fn register_builtin(registries: &mut ExtensionRegistries) -> Result<(), ExtensionError> {
    registries.register_synth(Arc::new(OscillatorFactory))?;
    registries.register_synth(Arc::new(NoiseFactory))?;
    Ok(())
}

/// `builtin.oscillator`
#[must_use]
pub fn oscillator_type_id() -> ExtensionTypeId {
    ExtensionTypeId::new("builtin.oscillator").expect("a valid built-in ID")
}

/// `builtin.noise`
#[must_use]
pub fn noise_type_id() -> ExtensionTypeId {
    ExtensionTypeId::new("builtin.noise").expect("a valid built-in ID")
}

/// The shape an oscillator voice traces. Text rather than an integer so a
/// project file says `"square"`, not `2`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Waveform {
    Sine,
    Triangle,
    Saw,
    Square,
}

impl Waveform {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "sine" => Some(Self::Sine),
            "triangle" => Some(Self::Triangle),
            "saw" => Some(Self::Saw),
            "square" => Some(Self::Square),
            _ => None,
        }
    }

    /// One cycle, phase in `0.0..1.0`.
    fn sample(self, phase: f64) -> f32 {
        let value = match self {
            Self::Sine => (phase * std::f64::consts::TAU).sin(),
            Self::Triangle => 4.0 * (phase - (phase + 0.5).floor()).abs() - 1.0,
            Self::Saw => phase.mul_add(2.0, -1.0),
            Self::Square => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        };
        value as f32
    }
}

/// One sounding note.
#[derive(Clone, Copy, Debug)]
struct Voice {
    pitch_hz: f64,
    phase: f64,
    velocity: f32,
    envelope: Envelope,
}

/// The voice bookkeeping both built-in synths share: allocation, stealing,
/// release by pitch, and reset.
struct VoicePool {
    voices: Vec<Voice>,
    sample_rate: u32,
    attack_ms: f64,
    release_ms: f64,
}

impl VoicePool {
    fn new(sample_rate: u32, attack_ms: f64, release_ms: f64) -> Self {
        Self {
            voices: Vec::with_capacity(MAX_POLYPHONY),
            sample_rate,
            attack_ms,
            release_ms,
        }
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
        self.reset();
    }

    fn reset(&mut self) {
        self.voices.clear();
    }

    fn note_on(&mut self, pitch_hz: f64, velocity: f32) {
        let mut envelope = Envelope::new(self.sample_rate, self.attack_ms, self.release_ms);
        envelope.trigger();
        let voice = Voice {
            pitch_hz,
            phase: 0.0,
            velocity: velocity.clamp(0.0, 1.0),
            envelope,
        };
        if self.voices.len() < MAX_POLYPHONY {
            self.voices.push(voice);
            return;
        }
        // Steal the quietest voice: the least likely to be missed.
        let quietest = self
            .voices
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                left.envelope
                    .level
                    .partial_cmp(&right.envelope.level)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map_or(0, |(index, _)| index);
        self.voices[quietest] = voice;
    }

    fn note_off(&mut self, pitch_hz: f64) {
        for voice in &mut self.voices {
            // Pitches come from the same stored value on both sides, so an
            // exact comparison is the right one here.
            if voice.pitch_hz == pitch_hz {
                voice.envelope.release();
            }
        }
    }

    fn all_notes_off(&mut self) {
        for voice in &mut self.voices {
            voice.envelope.release();
        }
    }

    fn apply(&mut self, kind: NoteEventKind) {
        match kind {
            NoteEventKind::NoteOn { pitch_hz, velocity } => self.note_on(pitch_hz, velocity),
            NoteEventKind::NoteOff { pitch_hz } => self.note_off(pitch_hz),
            NoteEventKind::AllNotesOff => self.all_notes_off(),
        }
    }

    /// Drops voices that have finished releasing, so a long render does not
    /// keep stepping silent envelopes.
    fn retire(&mut self) {
        self.voices.retain(|voice| voice.envelope.is_active());
    }
}

/// Renders `output` frame by frame, applying events on the frames they name.
fn render_with(
    pool: &mut VoicePool,
    events: &[NoteEvent],
    output: &mut [f32],
    mut voice_sample: impl FnMut(&mut Voice, u32) -> f32,
) {
    output.fill(0.0);
    let mut next_event = 0;
    for (frame, slot) in output.iter_mut().enumerate() {
        while next_event < events.len() && events[next_event].frame_offset <= frame {
            pool.apply(events[next_event].kind);
            next_event += 1;
        }
        let sample_rate = pool.sample_rate;
        let mut sum = 0.0;
        for voice in &mut pool.voices {
            sum += voice_sample(voice, sample_rate);
        }
        *slot = sum;
    }
    // Events past the end of the block still take effect, so a note-off on the
    // last frame boundary is not lost between blocks.
    while next_event < events.len() {
        pool.apply(events[next_event].kind);
        next_event += 1;
    }
    pool.retire();
}

/// A tone generator with one waveform and a linear attack/release.
pub struct OscillatorSynth {
    waveform: Waveform,
    gain: f32,
    pool: VoicePool,
}

impl Synth for OscillatorSynth {
    fn prepare(&mut self, sample_rate: u32) {
        self.pool.prepare(sample_rate);
    }

    fn reset(&mut self) {
        self.pool.reset();
    }

    fn render(&mut self, events: &[NoteEvent], output: &mut [f32]) {
        let waveform = self.waveform;
        let gain = self.gain;
        render_with(&mut self.pool, events, output, move |voice, sample_rate| {
            let sample = waveform.sample(voice.phase);
            voice.phase = (voice.phase + voice.pitch_hz / f64::from(sample_rate.max(1))).fract();
            sample * voice.envelope.advance() * voice.velocity * gain
        });
    }
}

/// Filtered-nothing white noise, gated by the same envelope. The basis of
/// impacts, explosions and wind.
pub struct NoiseSynth {
    gain: f32,
    noise: Noise,
    pool: VoicePool,
}

impl Synth for NoiseSynth {
    fn prepare(&mut self, sample_rate: u32) {
        self.pool.prepare(sample_rate);
        self.noise.reset();
    }

    fn reset(&mut self) {
        self.pool.reset();
        self.noise.reset();
    }

    fn render(&mut self, events: &[NoteEvent], output: &mut [f32]) {
        let gain = self.gain;
        let noise = &mut self.noise;
        render_with(&mut self.pool, events, output, move |voice, _| {
            noise.next_sample() * voice.envelope.advance() * voice.velocity * gain
        });
    }
}

struct OscillatorFactory;
struct NoiseFactory;

impl SynthFactory for OscillatorFactory {
    fn descriptor(&self) -> &ExtensionDescriptor {
        static DESCRIPTOR: std::sync::OnceLock<ExtensionDescriptor> = std::sync::OnceLock::new();
        DESCRIPTOR.get_or_init(|| ExtensionDescriptor {
            type_id: oscillator_type_id(),
            kind: ExtensionKind::Synth,
            display_name: "Oscillator".into(),
            state_version: 1,
            parameters: vec![
                text("waveform", "Waveform", "sine"),
                gain_db(),
                envelope_time("attack_ms", "Attack", 1.0),
                envelope_time("release_ms", "Release", 60.0),
            ],
        })
    }

    fn instantiate(
        &self,
        parameters: &BTreeMap<String, ParameterValue>,
    ) -> Result<Box<dyn Synth>, ExtensionError> {
        let waveform = text_value(parameters, "waveform", "sine");
        let waveform = Waveform::parse(&waveform).ok_or_else(|| ExtensionError {
            code: crate::ExtensionErrorCode::InvalidParameters,
            message: format!("waveform '{waveform}' is not one of sine, triangle, saw, square"),
            kind: Some(ExtensionKind::Synth),
            type_id: Some(oscillator_type_id()),
            parameter_id: Some("waveform".into()),
        })?;
        let attack_ms = float_value(parameters, "attack_ms", 1.0);
        let release_ms = float_value(parameters, "release_ms", 60.0);
        Ok(Box::new(OscillatorSynth {
            waveform,
            gain: decibels_to_gain(float_value(parameters, "gain_db", 0.0)),
            pool: VoicePool::new(DEFAULT_SAMPLE_RATE, attack_ms, release_ms),
        }))
    }
}

impl SynthFactory for NoiseFactory {
    fn descriptor(&self) -> &ExtensionDescriptor {
        static DESCRIPTOR: std::sync::OnceLock<ExtensionDescriptor> = std::sync::OnceLock::new();
        DESCRIPTOR.get_or_init(|| ExtensionDescriptor {
            type_id: noise_type_id(),
            kind: ExtensionKind::Synth,
            display_name: "Noise".into(),
            state_version: 1,
            parameters: vec![
                ParameterDescriptor {
                    id: "seed".into(),
                    display_name: "Seed".into(),
                    value_type: ParameterType::Integer,
                    default_value: ParameterValue::Integer(1),
                    introduced_in_state_version: 1,
                    automatable: false,
                    minimum: None,
                    maximum: None,
                    unit: None,
                },
                gain_db(),
                envelope_time("attack_ms", "Attack", 0.0),
                envelope_time("release_ms", "Release", 120.0),
            ],
        })
    }

    fn instantiate(
        &self,
        parameters: &BTreeMap<String, ParameterValue>,
    ) -> Result<Box<dyn Synth>, ExtensionError> {
        let seed = match parameters.get("seed") {
            Some(ParameterValue::Integer(value)) => *value as u64,
            _ => 1,
        };
        let attack_ms = float_value(parameters, "attack_ms", 0.0);
        let release_ms = float_value(parameters, "release_ms", 120.0);
        Ok(Box::new(NoiseSynth {
            gain: decibels_to_gain(float_value(parameters, "gain_db", 0.0)),
            noise: Noise::new(seed),
            pool: VoicePool::new(DEFAULT_SAMPLE_RATE, attack_ms, release_ms),
        }))
    }
}

/// What a synth renders at until `prepare` says otherwise.
const DEFAULT_SAMPLE_RATE: u32 = 48_000;

fn gain_db() -> ParameterDescriptor {
    ParameterDescriptor {
        id: "gain_db".into(),
        display_name: "Gain".into(),
        value_type: ParameterType::Float,
        default_value: ParameterValue::Float(0.0),
        introduced_in_state_version: 1,
        automatable: true,
        minimum: None,
        maximum: None,
        unit: None,
    }
}

fn envelope_time(id: &str, display_name: &str, default_ms: f64) -> ParameterDescriptor {
    ParameterDescriptor {
        id: id.into(),
        display_name: display_name.into(),
        value_type: ParameterType::Float,
        default_value: ParameterValue::Float(default_ms),
        introduced_in_state_version: 1,
        automatable: false,
        minimum: None,
        maximum: None,
        unit: None,
    }
}

fn text(id: &str, display_name: &str, default_value: &str) -> ParameterDescriptor {
    ParameterDescriptor {
        id: id.into(),
        display_name: display_name.into(),
        value_type: ParameterType::Text,
        default_value: ParameterValue::Text(default_value.into()),
        introduced_in_state_version: 1,
        automatable: false,
        minimum: None,
        maximum: None,
        unit: None,
    }
}

fn float_value(parameters: &BTreeMap<String, ParameterValue>, id: &str, fallback: f64) -> f64 {
    match parameters.get(id) {
        Some(ParameterValue::Float(value)) => *value,
        _ => fallback,
    }
}

fn text_value(parameters: &BTreeMap<String, ParameterValue>, id: &str, fallback: &str) -> String {
    match parameters.get(id) {
        Some(ParameterValue::Text(value)) => value.clone(),
        _ => fallback.to_owned(),
    }
}

fn decibels_to_gain(decibels: f64) -> f32 {
    10_f32.powf(decibels as f32 / 20.0)
}
