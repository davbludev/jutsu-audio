//! `sfx.laser` — a shot: a fast pitch sweep with an optional buzz.

use jutsu_audio_model::ParameterValue;

use super::dsp::{advance_phase, decay, lerp, normalise, seeded_noise, sine};
use super::{GeneratorPreset, Settings, SfxFactory, descriptor, preset, ranged};

pub const TYPE_ID: &str = "sfx.laser";
pub const ALGORITHM_VERSION: u32 = 1;

#[must_use]
pub fn factory() -> SfxFactory {
    SfxFactory::new(
        descriptor(
            TYPE_ID,
            "Laser",
            vec![
                ranged("start_hz", "Start pitch", 1_800.0, 40.0, 12_000.0),
                ranged("end_hz", "End pitch", 220.0, 40.0, 12_000.0),
                ranged("decay_ms", "Decay", 260.0, 20.0, 2_000.0),
                ranged("buzz", "Buzz", 0.25, 0.0, 1.0),
            ],
        ),
        presets(),
        render,
    )
}

fn presets() -> Vec<GeneratorPreset> {
    vec![
        preset(
            "Pew",
            &[
                ("start_hz", ParameterValue::Float(2_400.0)),
                ("end_hz", ParameterValue::Float(300.0)),
                ("decay_ms", ParameterValue::Float(180.0)),
                ("buzz", ParameterValue::Float(0.1)),
            ],
        ),
        preset(
            "Charge up",
            &[
                ("start_hz", ParameterValue::Float(160.0)),
                ("end_hz", ParameterValue::Float(2_600.0)),
                ("decay_ms", ParameterValue::Float(700.0)),
                ("buzz", ParameterValue::Float(0.3)),
            ],
        ),
    ]
}

fn render(settings: &Settings, seed: u64, frame_count: usize) -> Vec<f32> {
    let rate = settings.sample_rate;
    let start_hz = settings.float("start_hz");
    let end_hz = settings.float("end_hz");
    let decay_frames = settings.frames_from_ms("decay_ms");
    let buzz = settings.float("buzz");

    let mut noise = seeded_noise(seed, "laser.buzz");
    let mut phase = 0.0;
    let mut samples = vec![0.0_f32; frame_count];

    for (frame, sample) in samples.iter_mut().enumerate() {
        let progress = if decay_frames > 0.0 {
            (frame as f64 / decay_frames).min(1.0)
        } else {
            1.0
        };
        // Sweeping in the exponent keeps the glide even to the ear, which
        // hears pitch logarithmically.
        let frequency = lerp(start_hz.ln(), end_hz.ln(), progress).exp();
        let tone = sine(phase);
        phase = advance_phase(phase, frequency, rate);
        let grit = noise.next_sample() * buzz as f32;
        *sample = (tone + grit) * decay(frame, decay_frames);
    }
    normalise(&mut samples);
    samples
}
