//! `sfx.laser` — energy discharging: a fast pitch sweep with a body under it.
//!
//! A clean sine sweeping downwards reads as a whistle, not as a weapon, and no
//! amount of level fixes that. What separates the two is that a discharge has a
//! *front* — a burst of noise before the tone is established — harmonics above
//! the fundamental, and enough saturation that it sounds like it cost something
//! to fire.

use jutsu_audio_model::ParameterValue;

use super::dsp::{BandNoise, Partials, decay, envelope, glide, normalise, saturate};
use super::{GeneratorPreset, Settings, SfxFactory, descriptor, preset, ranged};
use crate::filter::Mode;

pub const TYPE_ID: &str = "sfx.laser";
pub const ALGORITHM_VERSION: u32 = 2;

/// Fundamental, octave, fifth-and-an-octave. Whole ratios here on purpose:
/// a discharge should read as one voice, not as a struck object.
const RATIOS: [f64; 3] = [1.0, 2.0, 3.0];

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
                ranged("harmonics", "Harmonics", 0.35, 0.0, 1.0),
                ranged("body", "Body", 0.3, 0.0, 1.0),
                ranged("drive", "Drive", 6.0, 0.0, 24.0),
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
                ("buzz", ParameterValue::Float(0.25)),
                ("harmonics", ParameterValue::Float(0.4)),
                ("body", ParameterValue::Float(0.25)),
                ("drive", ParameterValue::Float(8.0)),
            ],
        ),
        preset(
            "Charge up",
            &[
                ("start_hz", ParameterValue::Float(160.0)),
                ("end_hz", ParameterValue::Float(2_600.0)),
                ("decay_ms", ParameterValue::Float(700.0)),
                ("buzz", ParameterValue::Float(0.3)),
                ("harmonics", ParameterValue::Float(0.5)),
                ("body", ParameterValue::Float(0.15)),
                ("drive", ParameterValue::Float(4.0)),
            ],
        ),
        preset(
            "Heavy cannon",
            &[
                ("start_hz", ParameterValue::Float(900.0)),
                ("end_hz", ParameterValue::Float(70.0)),
                ("decay_ms", ParameterValue::Float(520.0)),
                ("buzz", ParameterValue::Float(0.6)),
                ("harmonics", ParameterValue::Float(0.8)),
                ("body", ParameterValue::Float(0.9)),
                ("drive", ParameterValue::Float(18.0)),
            ],
        ),
    ]
}

fn render(settings: &Settings, seed: u64, frame_count: usize) -> Vec<f32> {
    let rate = settings.sample_rate;
    let sample_rate = f64::from(rate.max(1));
    let start_hz = settings.float("start_hz");
    let end_hz = settings.float("end_hz");
    let buzz = settings.float("buzz");
    let harmonics = settings.float("harmonics");
    let body = settings.float("body");
    let drive = settings.float("drive");
    let decay_frames = settings.frames_from_ms("decay_ms").max(1.0);

    let mut grit = BandNoise::new(seed, "laser.buzz", Mode::Band);
    let mut front = BandNoise::new(seed, "laser.front", Mode::High);
    let mut tone = Partials::new(RATIOS.len());
    let mut sub = Partials::new(1);
    let mut samples = vec![0.0_f32; frame_count];

    for (frame, sample) in samples.iter_mut().enumerate() {
        let progress = (frame as f64 / decay_frames).min(1.0);
        // Front-loaded: the sweep covers most of its distance early, which is
        // what makes it read as fast rather than as a slide.
        let hz = glide(start_hz, end_hz, progress, 0.6);
        let level = decay(frame, decay_frames);

        let weights = [
            level,
            level * (0.2 + 0.7 * harmonics) as f32,
            level * (0.05 + 0.5 * harmonics) as f32 * 0.6,
        ];
        let voice = tone.sample(hz, &RATIOS, &weights, rate);

        // The sub sits an octave below and outlasts the sweep — this is the
        // difference between a whistle and something with a barrel.
        let under = sub.sample(
            (hz * 0.5).max(30.0),
            &[1.0],
            &[decay(frame, decay_frames * 1.6) * body as f32],
            rate,
        );

        // Buzz tracks the pitch, so it is part of the voice rather than a hiss
        // laid over it.
        let buzzing =
            grit.next((hz * 1.5).min(sample_rate * 0.45), 0.75, sample_rate) * level * buzz as f32;

        // A very short noise burst before the tone settles: the front.
        let opening = front.next(4_000.0, 0.3, sample_rate)
            * envelope(frame, 4.0, decay_frames * 0.04)
            * (0.25 + 0.5 * harmonics) as f32;

        *sample = saturate((voice + under + buzzing + opening) * 0.5, drive);
    }
    normalise(&mut samples);
    samples
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GeneratorFactory;
    use std::collections::BTreeMap;

    fn render_with(pairs: &[(&str, f64)], frames: usize) -> Vec<f32> {
        let mut parameters = BTreeMap::new();
        for (id, value) in pairs {
            parameters.insert((*id).to_owned(), ParameterValue::Float(*value));
        }
        factory()
            .instantiate(&parameters)
            .expect("instantiate")
            .generate_mono(2, frames)
    }

    #[test]
    fn the_sweep_goes_where_it_is_pointed() {
        fn crossings(samples: &[f32]) -> usize {
            samples
                .windows(2)
                .filter(|pair| (pair[0] > 0.0) != (pair[1] > 0.0))
                .count()
        }
        let rendered = render_with(
            &[
                ("start_hz", 4_000.0),
                ("end_hz", 120.0),
                ("decay_ms", 800.0),
                ("buzz", 0.0),
                ("harmonics", 0.0),
                ("body", 0.0),
                ("drive", 0.0),
            ],
            38_400,
        );
        let early = crossings(&rendered[..4_800]);
        let late = crossings(&rendered[19_200..24_000]);
        assert!(
            early > late * 4,
            "it starts far higher than it ends: {early} against {late}"
        );
    }

    #[test]
    fn body_puts_something_underneath() {
        fn low_energy(samples: &[f32]) -> f32 {
            // A crude low-pass: a running mean over 32 frames keeps only the
            // slow part of the signal.
            samples
                .chunks(32)
                .map(|chunk| {
                    let mean = chunk.iter().sum::<f32>() / chunk.len() as f32;
                    mean * mean
                })
                .sum()
        }
        let thin = render_with(&[("body", 0.0), ("decay_ms", 400.0)], 48_000);
        let full = render_with(&[("body", 1.0), ("decay_ms", 400.0)], 48_000);
        assert!(
            low_energy(&full) > low_energy(&thin) * 2.0,
            "there is more down there: {} against {}",
            low_energy(&full),
            low_energy(&thin)
        );
    }
}
