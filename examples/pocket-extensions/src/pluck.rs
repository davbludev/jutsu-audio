//! `pocket.pluck` — a plucked tone.
//!
//! A sine per voice with an exponential decay. Small on purpose: what matters
//! is the shape of a synth, not the sound of this one.

use std::collections::BTreeMap;

use jutsu_audio_extensions::{
    ExtensionDescriptor, ExtensionError, ExtensionKind, ExtensionTypeId, NoteEvent, NoteEventKind,
    ParameterDescriptor, ParameterType, Synth, SynthFactory,
};
use jutsu_audio_model::ParameterValue;

pub const PLUCK_TYPE_ID: &str = "pocket.pluck";

/// How many notes sound at once. A fixed array, because the render path may not
/// allocate.
const VOICES: usize = 8;

pub struct PluckFactory {
    descriptor: ExtensionDescriptor,
}

impl Default for PluckFactory {
    fn default() -> Self {
        Self {
            descriptor: descriptor(),
        }
    }
}

fn descriptor() -> ExtensionDescriptor {
    ExtensionDescriptor {
        type_id: ExtensionTypeId::new(PLUCK_TYPE_ID).expect("a valid type ID"),
        kind: ExtensionKind::Synth,
        display_name: "Pocket Pluck".into(),
        // Version 1 is where every extension starts. Adding a parameter later
        // means version 2 and `introduced_in_state_version: 2` on the new one,
        // so a project written by the older build still loads.
        state_version: 1,
        parameters: vec![
            ParameterDescriptor {
                id: "decay_ms".into(),
                display_name: "Decay".into(),
                value_type: ParameterType::Float,
                default_value: ParameterValue::Float(400.0),
                introduced_in_state_version: 1,
                automatable: false,
                minimum: Some(10.0),
                maximum: Some(4_000.0),
                unit: Some("ms".into()),
            },
            ParameterDescriptor {
                id: "brightness".into(),
                display_name: "Brightness".into(),
                value_type: ParameterType::Float,
                default_value: ParameterValue::Float(0.3),
                introduced_in_state_version: 1,
                automatable: true,
                minimum: Some(0.0),
                maximum: Some(1.0),
                unit: Some("ratio".into()),
            },
        ],
    }
}

impl SynthFactory for PluckFactory {
    fn descriptor(&self) -> &ExtensionDescriptor {
        &self.descriptor
    }

    fn instantiate(
        &self,
        parameters: &BTreeMap<String, ParameterValue>,
    ) -> Result<Box<dyn Synth>, ExtensionError> {
        // The host has already checked these against the descriptor — types and
        // ranges both — so reading them is all that is left to do.
        Ok(Box::new(Pluck {
            decay_ms: crate::float(parameters, "decay_ms", 400.0),
            brightness: crate::float(parameters, "brightness", 0.3),
            sample_rate: 48_000.0,
            voices: [Voice::SILENT; VOICES],
        }))
    }
}

#[derive(Clone, Copy)]
struct Voice {
    /// Radians per frame. Zero means the voice is free.
    step: f64,
    phase: f64,
    level: f64,
    decay: f64,
}

impl Voice {
    const SILENT: Self = Self {
        step: 0.0,
        phase: 0.0,
        level: 0.0,
        decay: 0.0,
    };
}

struct Pluck {
    decay_ms: f64,
    brightness: f64,
    sample_rate: f64,
    voices: [Voice; VOICES],
}

impl Synth for Pluck {
    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = f64::from(sample_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.voices = [Voice::SILENT; VOICES];
    }

    fn render(&mut self, events: &[NoteEvent], output: &mut [f32]) {
        output.fill(0.0);
        let mut next_event = 0;
        for (frame, sample) in output.iter_mut().enumerate() {
            while next_event < events.len() && events[next_event].frame_offset == frame {
                self.handle(&events[next_event]);
                next_event += 1;
            }
            let mut mixed = 0.0;
            for voice in &mut self.voices {
                if voice.step == 0.0 {
                    continue;
                }
                // A touch of second harmonic is all "brightness" means here.
                let value = voice.phase.sin() + self.brightness * (voice.phase * 2.0).sin();
                mixed += value * voice.level / (1.0 + self.brightness);
                voice.phase += voice.step;
                voice.level *= voice.decay;
                if voice.level < 1e-4 {
                    *voice = Voice::SILENT;
                }
            }
            #[allow(clippy::cast_possible_truncation)]
            {
                // Divided by the voice count: eight notes at once must not add
                // up past full scale.
                *sample = (mixed / VOICES as f64).clamp(-1.0, 1.0) as f32;
            }
        }
    }
}

impl Pluck {
    fn handle(&mut self, event: &NoteEvent) {
        match event.kind {
            NoteEventKind::NoteOn {
                pitch_hz, velocity, ..
            } => {
                let frames = (self.decay_ms / 1_000.0) * self.sample_rate;
                let voice = Voice {
                    step: std::f64::consts::TAU * pitch_hz / self.sample_rate,
                    phase: 0.0,
                    level: f64::from(velocity),
                    // Reaching a thousandth of the level over the decay time.
                    decay: 0.001_f64.powf(1.0 / frames.max(1.0)),
                };
                // Steal the quietest voice when all of them are busy: dropping
                // the newest note is more noticeable than fading the oldest.
                let slot = self
                    .voices
                    .iter()
                    .position(|voice| voice.step == 0.0)
                    .unwrap_or_else(|| {
                        self.voices
                            .iter()
                            .enumerate()
                            .min_by(|(_, one), (_, two)| one.level.total_cmp(&two.level))
                            .map_or(0, |(index, _)| index)
                    });
                self.voices[slot] = voice;
            }
            // A pluck decays on its own; letting go of the key does nothing.
            NoteEventKind::NoteOff { .. } => {}
            // Panic, transport stop, a seek: everything stops now.
            NoteEventKind::AllNotesOff => self.voices = [Voice::SILENT; VOICES],
        }
    }
}
