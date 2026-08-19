//! `sfx.pickup` — a reward: a short rising arpeggio of clean blips.

use jutsu_audio_model::ParameterValue;

use super::dsp::{advance_phase, decay, normalise, sine};
use super::{GeneratorPreset, Settings, SfxFactory, counted, descriptor, preset, ranged};

pub const TYPE_ID: &str = "sfx.pickup";
pub const ALGORITHM_VERSION: u32 = 1;

/// Semitone steps of a major triad, which is what a pickup almost always is.
const STEPS: [f64; 4] = [0.0, 4.0, 7.0, 12.0];

#[must_use]
pub fn factory() -> SfxFactory {
    SfxFactory::new(
        descriptor(
            TYPE_ID,
            "Pickup",
            vec![
                ranged("base_hz", "Base pitch", 660.0, 80.0, 4_000.0),
                counted("steps", "Steps", 3, 1, 8),
                ranged("step_ms", "Step length", 70.0, 20.0, 400.0),
            ],
        ),
        presets(),
        render,
    )
}

fn presets() -> Vec<GeneratorPreset> {
    vec![
        preset(
            "Coin",
            &[
                ("base_hz", ParameterValue::Float(990.0)),
                ("steps", ParameterValue::Integer(2)),
                ("step_ms", ParameterValue::Float(55.0)),
            ],
        ),
        preset(
            "Level up",
            &[
                ("base_hz", ParameterValue::Float(520.0)),
                ("steps", ParameterValue::Integer(4)),
                ("step_ms", ParameterValue::Float(110.0)),
            ],
        ),
    ]
}

fn render(settings: &Settings, _seed: u64, frame_count: usize) -> Vec<f32> {
    let rate = settings.sample_rate;
    let base_hz = settings.float("base_hz");
    let steps = usize::try_from(settings.integer("steps").max(1)).unwrap_or(1);
    let step_frames = settings.frames_from_ms("step_ms").max(1.0);

    let mut samples = vec![0.0_f32; frame_count];
    let mut phase = 0.0;
    // A pickup is a written phrase, not a roll of the dice: the seed changes
    // nothing here, and that is deliberate.
    for (frame, sample) in samples.iter_mut().enumerate() {
        let step = (frame as f64 / step_frames) as usize;
        if step >= steps {
            break;
        }
        let octave = (step / STEPS.len()) as f64;
        let semitones = STEPS[step % STEPS.len()] + 12.0 * octave;
        let frequency = base_hz * 2_f64.powf(semitones / 12.0);
        let into_step = frame as f64 - step as f64 * step_frames;
        *sample = sine(phase) * decay(into_step as usize, step_frames * 0.8);
        phase = advance_phase(phase, frequency, rate);
    }
    normalise(&mut samples);
    samples
}
