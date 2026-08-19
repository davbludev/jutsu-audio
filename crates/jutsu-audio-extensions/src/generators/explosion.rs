//! `sfx.explosion` — a blast: broadband noise falling into a long rumble.

use jutsu_audio_model::ParameterValue;

use super::dsp::{LowPass, advance_phase, decay, lerp, normalise, seeded_noise, sine};
use super::{GeneratorPreset, Settings, SfxFactory, descriptor, preset, ranged};

pub const TYPE_ID: &str = "sfx.explosion";
pub const ALGORITHM_VERSION: u32 = 1;

#[must_use]
pub fn factory() -> SfxFactory {
    SfxFactory::new(
        descriptor(
            TYPE_ID,
            "Explosion",
            vec![
                ranged("size", "Size", 0.6, 0.0, 1.0),
                ranged("rumble", "Rumble", 0.5, 0.0, 1.0),
                ranged("decay_ms", "Decay", 1_400.0, 200.0, 8_000.0),
            ],
        ),
        presets(),
        render,
    )
}

fn presets() -> Vec<GeneratorPreset> {
    vec![
        preset(
            "Grenade",
            &[
                ("size", ParameterValue::Float(0.4)),
                ("rumble", ParameterValue::Float(0.3)),
                ("decay_ms", ParameterValue::Float(900.0)),
            ],
        ),
        preset(
            "Building collapse",
            &[
                ("size", ParameterValue::Float(0.9)),
                ("rumble", ParameterValue::Float(0.9)),
                ("decay_ms", ParameterValue::Float(3_500.0)),
            ],
        ),
    ]
}

fn render(settings: &Settings, seed: u64, frame_count: usize) -> Vec<f32> {
    let rate = settings.sample_rate;
    let size = settings.float("size");
    let rumble = settings.float("rumble");
    let decay_frames = settings.frames_from_ms("decay_ms");
    // The filter sweeps down over the tail: bright at the blast, muffled as it
    // rolls away. Bigger explosions start brighter and end deeper.
    let open_hz = 4_000.0 + 6_000.0 * size;
    let closed_hz = 120.0 - 60.0 * size;
    let rumble_hz = 55.0 - 25.0 * size;

    let mut noise = seeded_noise(seed, "explosion.blast");
    let mut filter = LowPass::new();
    let mut phase = 0.0;
    let mut samples = vec![0.0_f32; frame_count];

    for (frame, sample) in samples.iter_mut().enumerate() {
        let progress = if decay_frames > 0.0 {
            (frame as f64 / decay_frames).min(1.0)
        } else {
            1.0
        };
        let cutoff = lerp(open_hz, closed_hz, progress.powf(0.6));
        let blast =
            filter.process(noise.next_sample(), cutoff as f32, rate) * decay(frame, decay_frames);
        let low = sine(phase) * decay(frame, decay_frames * 1.6) * rumble as f32;
        phase = advance_phase(phase, rumble_hz, rate);
        *sample = blast + low;
    }
    normalise(&mut samples);
    samples
}
