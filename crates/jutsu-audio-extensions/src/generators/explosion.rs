//! `sfx.explosion` — a blast: the crack, the roar, and the low end rolling
//! away underneath it.
//!
//! The part people actually recognise is the *order*. A crack that is over in
//! twenty milliseconds, a body of noise sweeping downwards behind it, and a low
//! partial that outlasts both. Take away the crack and it is wind; take away
//! the low end and it is a hiss; take away the sweep and it is a burst of
//! static.

use jutsu_audio_model::ParameterValue;

use super::dsp::{BandNoise, Partials, decay, envelope, glide, normalise, saturate};
use super::{GeneratorPreset, Settings, SfxFactory, descriptor, preset, ranged};
use crate::filter::{Mode, Svf, coefficients};

pub const TYPE_ID: &str = "sfx.explosion";
pub const ALGORITHM_VERSION: u32 = 2;

/// The low end of a blast is not one tone; a second partial just off a whole
/// ratio keeps it from reading as a bass note.
const RATIOS: [f64; 2] = [1.0, 1.61];

#[must_use]
pub fn factory() -> SfxFactory {
    SfxFactory::new(
        descriptor(
            TYPE_ID,
            "Explosion",
            vec![
                ranged("size", "Size", 0.6, 0.0, 1.0),
                ranged("roar", "Roar", 0.8, 0.0, 1.0),
                ranged("rumble", "Rumble", 0.5, 0.0, 1.0),
                // Down to 30 ms: a rifle shot is over in about fifty, and while
                // the floor was 200 the low end always outlasted the crack by
                // a factor of ten, which reads as a drum rather than a shot.
                ranged("decay_ms", "Decay", 1_400.0, 30.0, 8_000.0),
                ranged("crack", "Crack", 0.5, 0.0, 1.0),
                ranged("crack_ms", "Crack length", 6.0, 0.5, 120.0),
                ranged("distance", "Distance", 0.15, 0.0, 1.0),
                ranged("drive", "Drive", 8.0, 0.0, 24.0),
            ],
        ),
        presets(),
        render,
    )
}

fn presets() -> Vec<GeneratorPreset> {
    vec![
        preset(
            "Rifle shot",
            &[
                ("size", ParameterValue::Float(0.15)),
                ("roar", ParameterValue::Float(0.12)),
                ("rumble", ParameterValue::Float(0.3)),
                ("decay_ms", ParameterValue::Float(60.0)),
                ("crack", ParameterValue::Float(1.0)),
                ("crack_ms", ParameterValue::Float(3.0)),
                ("distance", ParameterValue::Float(0.0)),
                ("drive", ParameterValue::Float(16.0)),
            ],
        ),
        preset(
            "Grenade",
            &[
                ("size", ParameterValue::Float(0.5)),
                ("roar", ParameterValue::Float(0.7)),
                ("rumble", ParameterValue::Float(0.55)),
                ("decay_ms", ParameterValue::Float(900.0)),
                ("crack", ParameterValue::Float(0.85)),
                ("crack_ms", ParameterValue::Float(12.0)),
                ("distance", ParameterValue::Float(0.05)),
                ("drive", ParameterValue::Float(12.0)),
            ],
        ),
        preset(
            "Distant artillery",
            &[
                ("size", ParameterValue::Float(0.9)),
                ("roar", ParameterValue::Float(0.9)),
                ("rumble", ParameterValue::Float(0.9)),
                ("decay_ms", ParameterValue::Float(3_200.0)),
                ("crack", ParameterValue::Float(0.2)),
                ("crack_ms", ParameterValue::Float(40.0)),
                ("distance", ParameterValue::Float(0.85)),
                ("drive", ParameterValue::Float(5.0)),
            ],
        ),
        preset(
            "Building collapse",
            &[
                ("size", ParameterValue::Float(1.0)),
                ("roar", ParameterValue::Float(1.0)),
                ("rumble", ParameterValue::Float(1.0)),
                ("decay_ms", ParameterValue::Float(5_000.0)),
                ("crack", ParameterValue::Float(0.35)),
                ("crack_ms", ParameterValue::Float(25.0)),
                ("distance", ParameterValue::Float(0.3)),
                ("drive", ParameterValue::Float(9.0)),
            ],
        ),
    ]
}

fn render(settings: &Settings, seed: u64, frame_count: usize) -> Vec<f32> {
    let rate = settings.sample_rate;
    let sample_rate = f64::from(rate.max(1));
    let size = settings.float("size");
    let roar_level = settings.float("roar") as f32;
    let rumble = settings.float("rumble");
    let crack = settings.float("crack");
    let distance = settings.float("distance");
    let drive = settings.float("drive");
    let decay_frames = settings.frames_from_ms("decay_ms");

    // The three parts of a blast each carry their own level: the crack at the
    // front, the roar behind it, the low end rolling away under both. A rifle
    // is almost all crack; a collapse is almost all roar and rumble. Without a
    // level on the middle one the first of those could not be asked for.
    //
    // The roar starts wide open and closes as it rolls away. Bigger blasts
    // start brighter and end deeper, which is what makes them read as bigger.
    let open_hz = 3_000.0 + 6_000.0 * size;
    let closed_hz = 110.0 - 55.0 * size;
    let low_start = (75.0 - 30.0 * size) * 2.0;
    let low_end = 38.0 - 16.0 * size;

    // Distance takes the top off everything and takes the crack away first: a
    // blast heard from far off has lost its front to the air between. The air
    // filter has a real slope, because a gentle one leaves it sounding near.
    let (air_g, air_k) = coefficients(19_000.0 - 18_200.0 * distance, 0.0, sample_rate);
    // Air kills the front first and almost completely: a blast on the
    // horizon has no crack left at all, only the roll.
    let crack_level = (crack * (1.0 - 0.98 * distance)) as f32;

    // The crack is broadband, not a band. A resonant band-pass gives noise a
    // pitch, and a pitch is exactly what turns a blast into a struck cymbal —
    // which is what a listener called it. A high-pass keeps everything above
    // its cutoff, which is what an expanding pressure front actually is.
    let crack_frames = settings.frames_from_ms("crack_ms").max(1.0);
    let crack_hz = 120.0 + 900.0 * (1.0 - size);

    let mut roar = BandNoise::new(seed, "explosion.roar", Mode::Low);
    let mut snap = BandNoise::new(seed, "explosion.crack", Mode::High);
    let mut low = Partials::new(RATIOS.len());
    let mut air = Svf::new();
    let mut samples = vec![0.0_f32; frame_count];

    for (frame, sample) in samples.iter_mut().enumerate() {
        let progress = (frame as f64 / decay_frames).min(1.0);

        let cutoff = glide(open_hz, closed_hz, progress, 0.55);
        let roaring = roar.next(cutoff, 0.2, sample_rate) * decay(frame, decay_frames);

        // Its own length, not a fraction of the tail: a crack is a physical
        // event of a fixed duration. Two or three milliseconds is a rifle,
        // fifteen is a grenade, and tying it to a five-second collapse would
        // give that a hundred-millisecond "crack" — a burst of static.
        // The front builds over about a millisecond and then holds. Measured
        // against a real gunshot recording: its loudest sample is at 0.8 ms and
        // the level is still at full scale three milliseconds in. A front that
        // spikes on its first sample and falls away immediately is a tick — the
        // ear needs the plateau to read it as a bang.
        let snapping = snap.next(crack_hz, 0.0, sample_rate)
            * envelope(frame, sample_rate * 0.0008, crack_frames)
            * crack_level;

        let hz = glide(low_start, low_end, progress, 0.4);
        let weights = [
            decay(frame, decay_frames * 1.7) * rumble as f32,
            decay(frame, decay_frames * 0.8) * (rumble * 0.4) as f32,
        ];
        let rolling = low.sample(hz, &RATIOS, &weights, rate);

        // The crack is added after the drive, not before it. Saturation is
        // what makes the blast dense, and a transient pushed through it comes
        // out the same height as everything else — which is exactly the front
        // the sound needs to keep.
        let driven = saturate((roaring * roar_level + rolling) * 0.7, drive);
        // A blast is mostly its front. The roar is what is left over
        // after it, not the other way round.
        *sample = air.low_pass(driven + snapping * 4.0, air_g, air_k);
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
            .generate_mono(11, frames)
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()))
    }

    #[test]
    fn the_crack_is_at_the_front_and_only_at_the_front() {
        let with = render_with(&[("crack", 1.0), ("decay_ms", 2_000.0)], 96_000);
        let without = render_with(&[("crack", 0.0), ("decay_ms", 2_000.0)], 96_000);
        // Measured in peaks, not in energy: six milliseconds cannot carry much
        // energy next to a two-second roar however hard it hits, and a crack is
        // a peak event by definition. Compared against a tenth of a second
        // later, because both renders are normalised and a loud crack pulls the
        // rest of its own blast down with it.
        let share = |samples: &[f32]| peak(&samples[..480]) / peak(&samples[4_800..9_600]);
        assert!(
            share(&with) > share(&without) * 1.4,
            "a cracking blast starts harder: {} against {}",
            share(&with),
            share(&without)
        );
    }

    #[test]
    fn the_front_holds_rather_than_spiking() {
        // A real shot is at full level for two or three milliseconds. One
        // sample at full scale followed by a fall is a tick, and no amount of
        // level makes it a bang.
        let rendered = render_with(
            &[
                ("crack", 1.0),
                ("crack_ms", 25.0),
                ("roar", 0.0),
                ("rumble", 0.0),
                ("decay_ms", 200.0),
                ("drive", 0.0),
            ],
            48_000,
        );
        let window = |from: usize, to: usize| {
            rendered[from..to]
                .iter()
                .fold(0.0_f32, |peak, s| peak.max(s.abs()))
        };
        let first = window(0, 24);
        let held = window(48, 144);
        assert!(
            held >= first * 0.8,
            "three milliseconds in it is still there: {held} against {first}"
        );
    }

    #[test]
    fn distance_takes_the_top_off() {
        fn brightness(samples: &[f32]) -> f32 {
            // The second difference — a two-pole high-pass. One difference is
            // only 6 dB per octave, gentle enough that a signal filtered down
            // to 800 Hz still scores highly on it.
            samples
                .windows(3)
                .map(|w| (w[2] - 2.0 * w[1] + w[0]).abs())
                .sum::<f32>()
                / samples.len() as f32
        }
        let near = render_with(&[("distance", 0.0), ("decay_ms", 1_200.0)], 48_000);
        let far = render_with(&[("distance", 1.0), ("decay_ms", 1_200.0)], 48_000);
        assert!(
            brightness(&far) < brightness(&near) / 3.0,
            "far is duller: {} against {}",
            brightness(&far),
            brightness(&near)
        );
    }

    #[test]
    fn rumble_outlasts_the_blast() {
        let quiet = render_with(&[("rumble", 0.0), ("decay_ms", 800.0)], 96_000);
        let heavy = render_with(&[("rumble", 1.0), ("decay_ms", 800.0)], 96_000);
        let tail = |samples: &[f32]| samples[48_000..].iter().map(|s| s * s).sum::<f32>();
        assert!(
            tail(&heavy) > tail(&quiet) * 3.0,
            "the low end is still there after the noise has gone: {} against {}",
            tail(&heavy),
            tail(&quiet)
        );
    }
}
