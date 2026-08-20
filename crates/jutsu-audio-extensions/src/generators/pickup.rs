//! `sfx.pickup` — a reward: a short rising phrase.
//!
//! Bare sine blips read as a handheld console from 1989. What moves this
//! towards an instrument is harmonic content above the fundamental, a layer
//! an octave up that decays faster, and a tail on the last step so the phrase
//! finishes rather than stopping.

use jutsu_audio_model::ParameterValue;

use super::dsp::{Partials, decay, normalise, saturate};
use super::{GeneratorPreset, Settings, SfxFactory, counted, descriptor, preset, ranged};

pub const TYPE_ID: &str = "sfx.pickup";
pub const ALGORITHM_VERSION: u32 = 2;

/// Fundamental, octave, and a twelfth: enough to have a timbre, few
/// enough to stay clean at the top of the range.
const RATIOS: [f64; 3] = [1.0, 2.0, 3.0];

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
                ranged("tone", "Tone", 0.4, 0.0, 1.0),
                ranged("sparkle", "Sparkle", 0.3, 0.0, 1.0),
                ranged("tail_ms", "Tail", 260.0, 20.0, 2_000.0),
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
    let tone = settings.float("tone");
    let sparkle = settings.float("sparkle");
    let tail_frames = settings.frames_from_ms("tail_ms").max(1.0);

    let mut samples = vec![0.0_f32; frame_count];
    let mut voice = Partials::new(RATIOS.len());
    let mut shine = Partials::new(1);
    // A pickup is a written phrase, not a roll of the dice: the seed changes
    // nothing here, and that is deliberate.
    for (frame, sample) in samples.iter_mut().enumerate() {
        let step = (frame as f64 / step_frames) as usize;
        if step >= steps {
            // The last step rings on rather than being cut off mid-phrase.
            let into_tail = frame as f64 - steps as f64 * step_frames;
            let last = STEPS[(steps - 1) % STEPS.len()] + 12.0 * ((steps - 1) / STEPS.len()) as f64;
            let frequency = base_hz * 2_f64.powf(last / 12.0);
            let level = decay(into_tail as usize, tail_frames);
            let weights = [level * 0.6, level * (0.2 * tone) as f32, 0.0];
            *sample = voice.sample(frequency, &RATIOS, &weights, rate);
            continue;
        }
        let octave = (step / STEPS.len()) as f64;
        let semitones = STEPS[step % STEPS.len()] + 12.0 * octave;
        let frequency = base_hz * 2_f64.powf(semitones / 12.0);
        let into_step = (frame as f64 - step as f64 * step_frames) as usize;
        let level = decay(into_step, step_frames * 0.8);

        // Tone decides how much sits above the fundamental — none is a sine,
        // plenty is closer to a plucked string.
        let weights = [
            level,
            level * (0.15 + 0.7 * tone) as f32,
            level * (0.4 * tone) as f32,
        ];
        let body = voice.sample(frequency, &RATIOS, &weights, rate);

        // Sparkle is an octave up and gone twice as fast: the glint on top.
        let glint = shine.sample(
            frequency * 2.0,
            &[1.0],
            &[decay(into_step, step_frames * 0.35) * sparkle as f32 * 0.5],
            rate,
        );

        *sample = saturate((body + glint) * 0.6, 4.0 * tone);
    }
    normalise(&mut samples);
    samples
}
