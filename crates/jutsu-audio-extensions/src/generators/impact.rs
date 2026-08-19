//! `sfx.impact` — a hit: a short noise transient over a low body thump.

use jutsu_audio_model::ParameterValue;

use super::dsp::{LowPass, advance_phase, decay, normalise, seeded_noise, sine};
use super::{GeneratorPreset, Settings, SfxFactory, descriptor, preset, ranged};

pub const TYPE_ID: &str = "sfx.impact";
/// Bumped when the rendered sound changes, so an old recipe keeps its sound.
pub const ALGORITHM_VERSION: u32 = 1;

#[must_use]
pub fn factory() -> SfxFactory {
    SfxFactory::new(
        descriptor(
            TYPE_ID,
            "Impact",
            vec![
                ranged("weight", "Weight", 0.5, 0.0, 1.0),
                ranged("brightness", "Brightness", 0.5, 0.0, 1.0),
                ranged("decay_ms", "Decay", 220.0, 20.0, 3_000.0),
            ],
        ),
        presets(),
        render,
    )
}

fn presets() -> Vec<GeneratorPreset> {
    vec![
        preset(
            "Wood knock",
            &[
                ("weight", ParameterValue::Float(0.2)),
                ("brightness", ParameterValue::Float(0.8)),
                ("decay_ms", ParameterValue::Float(90.0)),
            ],
        ),
        preset(
            "Body hit",
            &[
                ("weight", ParameterValue::Float(0.6)),
                ("brightness", ParameterValue::Float(0.4)),
                ("decay_ms", ParameterValue::Float(260.0)),
            ],
        ),
        preset(
            "Heavy slam",
            &[
                ("weight", ParameterValue::Float(1.0)),
                ("brightness", ParameterValue::Float(0.25)),
                ("decay_ms", ParameterValue::Float(700.0)),
            ],
        ),
    ]
}

fn render(settings: &Settings, seed: u64, frame_count: usize) -> Vec<f32> {
    let rate = settings.sample_rate;
    let weight = settings.float("weight");
    let brightness = settings.float("brightness");
    let decay_frames = settings.frames_from_ms("decay_ms");
    // Heavier hits are lower and ring longer; brighter ones let more of the
    // noise transient through.
    let body_hz = 180.0 - 120.0 * weight;
    let transient_frames = decay_frames * 0.12;
    let cutoff = 800.0 + 9_000.0 * brightness;

    let mut noise = seeded_noise(seed, "impact.transient");
    let mut filter = LowPass::new();
    let mut phase = 0.0;
    let mut samples = vec![0.0_f32; frame_count];

    for (frame, sample) in samples.iter_mut().enumerate() {
        let transient = filter.process(noise.next_sample(), cutoff as f32, rate)
            * decay(frame, transient_frames);
        let body = sine(phase) * decay(frame, decay_frames);
        phase = advance_phase(phase, body_hz, rate);
        *sample = transient * (0.4 + 0.6 * brightness as f32) + body * (0.5 + 0.5 * weight as f32);
    }
    normalise(&mut samples);
    samples
}
