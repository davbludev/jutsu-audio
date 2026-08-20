//! `sfx.ambience` — a bed: air moving somewhere, meant to loop.
//!
//! The failure this generator exists to avoid is hiss. White noise through a
//! gentle filter is still white noise, and a listener with no audio vocabulary
//! will call it exactly that. What makes a bed read as a *place* is that its
//! energy sits in a few registers rather than in all of them at once, and that
//! those registers drift independently — the low end swelling while the top
//! thins out is what wind does.

use jutsu_audio_model::ParameterValue;

use super::dsp::{BandNoise, advance_phase, normalise, sine};
use super::{GeneratorPreset, Settings, SfxFactory, descriptor, preset, ranged};
use crate::filter::Mode;

pub const TYPE_ID: &str = "sfx.ambience";
pub const ALGORITHM_VERSION: u32 = 2;

/// The three drifts run at different rates so the bed never lines up with
/// itself; a bed whose layers breathe together pulses instead of breathing.
const DRIFT_RATIOS: [f64; 3] = [1.0, 0.63, 1.37];

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
                ranged("depth", "Depth", 0.45, 0.0, 1.0),
                ranged("focus", "Focus", 0.4, 0.0, 1.0),
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
                ("depth", ParameterValue::Float(0.6)),
                ("focus", ParameterValue::Float(0.55)),
            ],
        ),
        preset(
            "Wind",
            &[
                ("brightness", ParameterValue::Float(0.5)),
                ("motion", ParameterValue::Float(0.8)),
                ("motion_hz", ParameterValue::Float(0.35)),
                ("depth", ParameterValue::Float(0.35)),
                ("focus", ParameterValue::Float(0.6)),
            ],
        ),
        preset(
            "Deep hum",
            &[
                ("brightness", ParameterValue::Float(0.05)),
                ("motion", ParameterValue::Float(0.3)),
                ("motion_hz", ParameterValue::Float(0.06)),
                ("depth", ParameterValue::Float(1.0)),
                ("focus", ParameterValue::Float(0.85)),
            ],
        ),
    ]
}

fn render(settings: &Settings, seed: u64, frame_count: usize) -> Vec<f32> {
    let rate = settings.sample_rate;
    let sample_rate = f64::from(rate.max(1));
    let brightness = settings.float("brightness");
    let motion = settings.float("motion");
    let motion_hz = settings.float("motion_hz");
    let depth = settings.float("depth");
    let focus = settings.float("focus");

    // Three registers rather than one spectrum. The middle one follows
    // brightness; the outer two frame it.
    let centres = [
        70.0 + 60.0 * depth,
        260.0 + 2_600.0 * brightness,
        1_600.0 + 7_000.0 * brightness,
    ];
    let levels = [
        (0.35 + 0.9 * depth) as f32,
        1.0_f32,
        (0.1 + 0.6 * brightness) as f32,
    ];
    // Focus is how narrow each band is. Wide is weather, narrow is a room with
    // a resonance in it.
    let resonance = (0.1 + 0.85 * focus).min(0.98);

    let labels = ["ambience.low", "ambience.mid", "ambience.high"];
    let mut bands: Vec<BandNoise> = labels
        .iter()
        .map(|label| BandNoise::new(seed, label, Mode::Band))
        .collect();
    let mut phases = [0.0_f64; 3];
    let mut samples = vec![0.0_f32; frame_count];

    for sample in &mut samples {
        let mut sum = 0.0_f32;
        for index in 0..3 {
            // Each band's drift both moves its level and slides its centre,
            // which is what makes a bed breathe rather than pulse.
            let sweep = f64::from(sine(phases[index])) * motion;
            phases[index] = advance_phase(phases[index], motion_hz * DRIFT_RATIOS[index], rate);
            let level = motion.mul_add(-0.4, 1.0) + sweep * 0.4;
            let centre = (centres[index] * (1.0 + sweep * 0.35)).max(30.0);
            sum += bands[index].next(centre, resonance, sample_rate)
                * levels[index]
                * level.clamp(0.0, 1.6) as f32;
        }
        *sample = sum * 0.5;
    }
    normalise(&mut samples);
    samples
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GeneratorFactory;
    use std::collections::BTreeMap;

    fn render_with(pairs: &[(&str, f64)]) -> Vec<f32> {
        let mut parameters = BTreeMap::new();
        for (id, value) in pairs {
            parameters.insert((*id).to_owned(), ParameterValue::Float(*value));
        }
        factory()
            .instantiate(&parameters)
            .expect("instantiate")
            .generate_mono(5, 96_000)
    }

    /// How often the signal changes sign — high for hiss, low for a rumble.
    fn crossings(samples: &[f32]) -> usize {
        samples
            .windows(2)
            .filter(|pair| (pair[0] > 0.0) != (pair[1] > 0.0))
            .count()
    }

    #[test]
    fn a_dark_bed_is_not_hiss() {
        // White noise at 48 kHz crosses zero about 24 000 times a second. A bed
        // that is meant to be low must be nowhere near that.
        let dark = render_with(&[
            ("brightness", 0.0),
            ("depth", 1.0),
            ("focus", 0.8),
            ("motion", 0.2),
        ]);
        let rate = crossings(&dark) / 2;
        assert!(
            rate < 3_000,
            "a dark bed sits low rather than everywhere: {rate} crossings a second"
        );
    }

    #[test]
    fn brightness_moves_where_the_energy_is() {
        let dark = render_with(&[("brightness", 0.0), ("motion", 0.0)]);
        let bright = render_with(&[("brightness", 1.0), ("motion", 0.0)]);
        assert!(
            crossings(&bright) > crossings(&dark) * 3,
            "bright sits far higher: {} against {}",
            crossings(&bright),
            crossings(&dark)
        );
    }

    #[test]
    fn motion_makes_the_level_move() {
        fn spread(samples: &[f32]) -> f32 {
            // Loudest tenth-second against quietest: a still bed has almost none.
            let blocks: Vec<f32> = samples
                .chunks(4_800)
                .map(|block| block.iter().map(|s| s * s).sum::<f32>() / block.len() as f32)
                .collect();
            let high = blocks.iter().fold(0.0_f32, |peak, b| peak.max(*b));
            let low = blocks.iter().fold(f32::INFINITY, |low, b| low.min(*b));
            high / low.max(f32::EPSILON)
        }
        // A still bed is not perfectly even — noise is random, so tenth-second
        // blocks differ by a fifth or so on their own. The bar is set above
        // that, not at one.
        let still = render_with(&[("motion", 0.0), ("motion_hz", 0.25)]);
        let moving = render_with(&[("motion", 1.0), ("motion_hz", 0.25)]);
        assert!(
            spread(&moving) > spread(&still) * 1.5,
            "a moving bed swells and falls: {} against {}",
            spread(&moving),
            spread(&still)
        );
    }
}
