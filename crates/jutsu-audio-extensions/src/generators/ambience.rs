//! `sfx.ambience` — a bed: filtered noise breathing slowly, meant to loop.

use jutsu_audio_model::ParameterValue;

use super::dsp::{LowPass, advance_phase, normalise, seeded_noise, sine};
use super::{GeneratorPreset, Settings, SfxFactory, descriptor, preset, ranged};

pub const TYPE_ID: &str = "sfx.ambience";
pub const ALGORITHM_VERSION: u32 = 1;

#[must_use]
pub fn factory() -> SfxFactory {
    SfxFactory::new(
        descriptor(
            TYPE_ID,
            "Ambience",
            vec![
                ranged("brightness", "Brightness", 0.35, 0.0, 1.0),
                ranged("motion", "Motion", 0.4, 0.0, 1.0),
                ranged("motion_hz", "Motion rate", 0.25, 0.02, 4.0),
            ],
        ),
        presets(),
        render,
    )
}

fn presets() -> Vec<GeneratorPreset> {
    vec![
        preset(
            "Room tone",
            &[
                ("brightness", ParameterValue::Float(0.15)),
                ("motion", ParameterValue::Float(0.15)),
                ("motion_hz", ParameterValue::Float(0.1)),
            ],
        ),
        preset(
            "Wind",
            &[
                ("brightness", ParameterValue::Float(0.5)),
                ("motion", ParameterValue::Float(0.8)),
                ("motion_hz", ParameterValue::Float(0.35)),
            ],
        ),
    ]
}

fn render(settings: &Settings, seed: u64, frame_count: usize) -> Vec<f32> {
    let rate = settings.sample_rate;
    let brightness = settings.float("brightness");
    let motion = settings.float("motion");
    let motion_hz = settings.float("motion_hz");
    let cutoff = 300.0 + 6_000.0 * brightness;

    let mut noise = seeded_noise(seed, "ambience.bed");
    let mut filter = LowPass::new();
    let mut phase = 0.0;
    let mut samples = vec![0.0_f32; frame_count];

    for sample in &mut samples {
        // The slow oscillator both moves the level and opens the filter, which
        // is what makes a bed breathe rather than pulse.
        let sweep = f64::from(sine(phase)) * motion;
        phase = advance_phase(phase, motion_hz, rate);
        let level = motion.mul_add(-0.5, 1.0) + sweep * 0.5;
        let filtered = filter.process(
            noise.next_sample(),
            (cutoff * (1.0 + sweep * 0.6)).max(40.0) as f32,
            rate,
        );
        *sample = filtered * level.clamp(0.0, 1.5) as f32;
    }
    normalise(&mut samples);
    samples
}
