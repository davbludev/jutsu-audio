//! `sfx.ambience` — a bed: air moving somewhere, meant to loop.
//!
//! The failure this generator exists to avoid is hiss. Filtering noise is not
//! enough to avoid it: a listener with no audio vocabulary hears *any* amount
//! of purely aperiodic sound as "white noise", however its energy is shaped.
//! What separates a place from a hiss is that a place has a **pitch** in it —
//! a building hums, a cavern resonates, wind whistles across an edge — and
//! that pitch drifts and beats rather than sitting still. So a bed here is two
//! things stacked: registers of noise for the air, and a slow detuned drone
//! for the room the air is in.

use jutsu_audio_model::ParameterValue;

use super::dsp::{BandNoise, Partials, advance_phase, normalise, sine};
use super::{GeneratorPreset, Settings, SfxFactory, descriptor, preset, ranged};
use crate::filter::{Mode, Svf, coefficients};

pub const TYPE_ID: &str = "sfx.ambience";
pub const ALGORITHM_VERSION: u32 = 3;

/// The three drifts run at different rates so the bed never lines up with
/// itself; a bed whose layers breathe together pulses instead of breathing.
const DRIFT_RATIOS: [f64; 3] = [1.0, 0.63, 1.37];

/// The drone's partials: two pairs a few thousandths apart, then singles.
///
/// The near-duplicates are the whole point: two voices four thousandths apart
/// at 110 Hz drift in and out of each other every couple of seconds, which is
/// what stops a held tone sounding like a test signal. A single voice per
/// harmonic would be dead still.
///
/// The series runs to eight because a drone that is only a fundamental and a
/// second harmonic is inaudible when the fundamental is where beds want it —
/// 40 to 90 Hz. What the listener hears of a 58 Hz hum is its harmonics.
const TONE_RATIOS: [f64; 8] = [1.0, 1.004, 2.0, 2.006, 3.01, 4.02, 6.03, 8.05];

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
                ranged("tone", "Tone", 0.5, 0.0, 1.0),
                ranged("tone_hz", "Tone pitch", 110.0, 25.0, 800.0),
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
                ("tone", ParameterValue::Float(0.7)),
                ("tone_hz", ParameterValue::Float(58.0)),
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
                ("tone", ParameterValue::Float(0.12)),
                ("tone_hz", ParameterValue::Float(240.0)),
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
                ("tone", ParameterValue::Float(0.8)),
                ("tone_hz", ParameterValue::Float(41.0)),
            ],
        ),
        preset(
            "Cavern",
            &[
                ("brightness", ParameterValue::Float(0.2)),
                ("motion", ParameterValue::Float(0.45)),
                ("motion_hz", ParameterValue::Float(0.08)),
                ("depth", ParameterValue::Float(0.75)),
                ("focus", ParameterValue::Float(0.95)),
                ("tone", ParameterValue::Float(0.75)),
                ("tone_hz", ParameterValue::Float(87.0)),
            ],
        ),
        preset(
            "Open water",
            &[
                ("brightness", ParameterValue::Float(0.7)),
                ("motion", ParameterValue::Float(0.95)),
                ("motion_hz", ParameterValue::Float(0.18)),
                ("depth", ParameterValue::Float(0.5)),
                ("focus", ParameterValue::Float(0.15)),
                ("tone", ParameterValue::Float(0.0)),
                ("tone_hz", ParameterValue::Float(110.0)),
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
    let tone = settings.float("tone");
    let tone_hz = settings.float("tone_hz");

    // Three registers rather than one spectrum. The middle one follows
    // brightness; the outer two frame it.
    let centres = [
        70.0 + 60.0 * depth,
        260.0 + 2_600.0 * brightness,
        1_600.0 + 7_000.0 * brightness,
    ];
    // The middle band is the one the ear weighs most, so brightness has to
    // control how much of it there is and not only where it sits. Held at full
    // level it is the hiss that a dark bed was still being reported as.
    let levels = [
        (0.35 + 0.9 * depth) as f32,
        (0.25 + 0.75 * brightness) as f32,
        (0.1 + 0.6 * brightness) as f32,
    ];
    // Focus is how narrow each band is. Wide is weather, narrow is a room with
    // a resonance in it. The floor is well above zero because a band-pass at
    // low resonance passes nearly everything, which is the hiss this generator
    // is trying not to make.
    let resonance = (0.45 + 0.53 * focus).min(0.98);

    // How fast the partials fall away.
    //
    // Shallow on purpose. A steep series puts almost everything in the
    // fundamental, and since the mix below is balanced on what is *heard*, a
    // drone whose audible harmonics are weak has to be scaled enormously to
    // register — which buries the bed in sub-bass and still does not read as a
    // pitch. Keeping the harmonics within reach of the fundamental is what
    // makes a 58 Hz hum audible as a hum rather than as a rumble.
    let tilt = 1.0 - 0.55 * brightness;
    let tone_weights: Vec<f32> = TONE_RATIOS
        .iter()
        .map(|ratio| {
            // Partials below about 160 Hz are held back regardless of the
            // tilt. Hearing falls away steeply down there, so energy spent on
            // a 58 Hz fundamental buys rumble and no pitch: it sets the peak
            // the whole bed is normalised against, and every audible part of
            // the sound gets quieter to make room for something nobody hears.
            // The pitch of a low hum is carried by its third and fourth
            // partials, and this is what puts it there.
            let audibility = ((ratio * tone_hz) / 160.0).clamp(0.0, 1.0).powi(2);
            (ratio.powf(-tilt) * audibility) as f32
        })
        .collect();

    let labels = ["ambience.low", "ambience.mid", "ambience.high"];
    let mut bands: Vec<BandNoise> = labels
        .iter()
        .map(|label| BandNoise::new(seed, label, Mode::Band))
        .collect();
    let mut drone = Partials::new(TONE_RATIOS.len());
    let mut phases = [0.0_f64; 3];
    let mut air = vec![0.0_f32; frame_count];
    let mut hum = vec![0.0_f32; frame_count];

    for frame in 0..frame_count {
        let mut sum = 0.0_f32;
        let mut first_sweep = 0.0_f64;
        for index in 0..3 {
            // Each band's drift both moves its level and slides its centre,
            // which is what makes a bed breathe rather than pulse.
            let sweep = f64::from(sine(phases[index])) * motion;
            if index == 0 {
                first_sweep = sweep;
            }
            phases[index] = advance_phase(phases[index], motion_hz * DRIFT_RATIOS[index], rate);
            let level = motion.mul_add(-0.4, 1.0) + sweep * 0.4;
            let centre = (centres[index] * (1.0 + sweep * 0.35)).max(30.0);
            // The low band stays broad however narrow the others go. A razor
            // band down at 100 Hz is a boom, not a room, and because a narrow
            // band is compensated back up to full level it ends up several
            // times louder than everything audible above it.
            let width = if index == 0 { 0.55 } else { 1.0 };
            sum += bands[index].next(centre, resonance * width, sample_rate)
                * levels[index]
                * level.clamp(0.0, 1.6) as f32;
        }
        air[frame] = sum;
        // A real hum wanders by a fraction of a percent. Any more and it reads
        // as an instrument being played rather than a room being in.
        let wander = first_sweep.mul_add(0.004, 1.0);
        hum[frame] = drone.sample(tone_hz * wander, &TONE_RATIOS, &tone_weights, rate);
    }

    // The two halves are matched before being mixed, so `tone` means what it
    // says at every setting — and matched by what is *heard*, not by total
    // energy. That distinction is the whole fix: a 58 Hz hum can hold 90% of a
    // bed's energy and still be inaudible next to the 10% sitting up where the
    // ear is sharp, so a bed balanced on raw level is reported as hiss while
    // its numbers look tonal.
    let scale =
        (heard(&air, sample_rate) / heard(&hum, sample_rate).max(f32::EPSILON)).clamp(0.1, 40.0);
    // Air falls away steeply, not in proportion.
    //
    // Noise is far more noticeable than its level suggests: with the two
    // halves merely matched and the air taken down by half, a bed was still
    // reported as mostly noise. Broadband sound leads the ear, so it has to
    // drop by tens of decibels — not by a fraction — before the pitch is what
    // a listener hears first. At `tone` 0.5 this puts the air about 14 dB
    // down; at 1.0, nearly 30.
    let air_gain = 10.0_f32.powf(-1.4 * tone as f32);
    let hum_gain = tone as f32 * scale;
    let mut samples = air;
    for (sample, voiced) in samples.iter_mut().zip(&hum) {
        *sample = sample.mul_add(air_gain, voiced * hum_gain) * 0.5;
    }
    normalise(&mut samples);
    samples
}

/// Level as the ear weighs it: the band from 400 Hz to 5 kHz, where hearing is
/// most sensitive, rather than the whole spectrum.
///
/// Crude next to a real loudness curve, and enough for the job it does here —
/// deciding which of two layers a listener will actually notice.
fn heard(samples: &[f32], sample_rate: f64) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let (high_g, high_k) = coefficients(400.0, 0.0, sample_rate);
    let (low_g, low_k) = coefficients(5_000.0, 0.0, sample_rate);
    let mut high = Svf::new();
    let mut low = Svf::new();
    let mut sum = 0.0_f64;
    for sample in samples {
        let above = high.process(*sample, Mode::High, high_g, high_k);
        let inside = low.process(above, Mode::Low, low_g, low_k);
        sum += f64::from(inside) * f64::from(inside);
    }
    (sum / samples.len() as f64).sqrt() as f32
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

    /// How much the signal repeats itself one period later, `0.0..=1.0`.
    ///
    /// This is the measurement that matters for this generator: noise scores
    /// near zero however it is filtered, and anything with a pitch in it
    /// scores high. "It sounds like white noise" is a report about *this*
    /// number, not about brightness.
    fn periodicity(samples: &[f32], period: usize) -> f32 {
        let usable = samples.len() - period;
        let paired: f32 = (0..usable).map(|n| samples[n] * samples[n + period]).sum();
        let energy: f32 = samples[..usable].iter().map(|s| s * s).sum();
        paired / energy.max(f32::EPSILON)
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
            ("tone", 0.0),
        ]);
        let rate = crossings(&dark) / 2;
        assert!(
            rate < 3_000,
            "a dark bed sits low rather than everywhere: {rate} crossings a second"
        );
    }

    #[test]
    fn brightness_moves_where_the_energy_is() {
        let dark = render_with(&[("brightness", 0.0), ("motion", 0.0), ("tone", 0.0)]);
        let bright = render_with(&[("brightness", 1.0), ("motion", 0.0), ("tone", 0.0)]);
        assert!(
            crossings(&bright) > crossings(&dark) * 3,
            "bright sits far higher: {} against {}",
            crossings(&bright),
            crossings(&dark)
        );
    }

    #[test]
    fn tone_gives_the_bed_a_pitch_rather_than_a_colour() {
        // The reported failure was "this is just white noise" about a bed that
        // was already filtered dark. Dark is not the answer; periodic is.
        let air = render_with(&[("tone", 0.0), ("motion", 0.0), ("focus", 0.9)]);
        let hum = render_with(&[
            ("tone", 1.0),
            ("tone_hz", 100.0),
            ("motion", 0.0),
            ("focus", 0.9),
        ]);
        // 100 Hz at 48 kHz is 480 frames. Measured over half a second, which is
        // short next to the drone's own beat — over a longer window the beating
        // pulls the two detuned voices apart and hides the pitch that is
        // plainly there to the ear.
        let air_repeat = periodicity(&air[..24_000], 480);
        let hum_repeat = periodicity(&hum[..24_000], 480);
        // Air is not perfectly aperiodic — a narrow band of noise wanders
        // around a centre and repeats a little. The claim is about the gap.
        assert!(
            hum_repeat > air_repeat * 2.0 && hum_repeat > 0.6,
            "tone makes the bed repeat at its own pitch: {hum_repeat}              against {air_repeat} for air alone"
        );
    }

    #[test]
    fn the_drone_beats_instead_of_sitting_still() {
        // Two voices four thousandths apart at 100 Hz swap places every couple
        // of seconds. Without that a held tone reads as a test signal, which
        // is a different complaint but the same underlying deadness.
        let hum = render_with(&[
            ("tone", 1.0),
            ("tone_hz", 100.0),
            ("motion", 0.0),
            ("brightness", 0.0),
        ]);
        let blocks: Vec<f32> = hum
            .chunks(24_000)
            .map(|block| block.iter().map(|s| s * s).sum::<f32>() / block.len() as f32)
            .collect();
        let high = blocks.iter().fold(0.0_f32, |peak, b| peak.max(*b));
        let low = blocks.iter().fold(f32::INFINITY, |low, b| low.min(*b));
        assert!(
            high / low.max(f32::EPSILON) > 1.3,
            "the drone swells and thins on its own: {high} against {low}"
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
        let still = render_with(&[("motion", 0.0), ("motion_hz", 0.25), ("tone", 0.0)]);
        let moving = render_with(&[("motion", 1.0), ("motion_hz", 0.25), ("tone", 0.0)]);
        assert!(
            spread(&moving) > spread(&still) * 1.5,
            "a moving bed swells and falls: {} against {}",
            spread(&moving),
            spread(&still)
        );
    }
}
