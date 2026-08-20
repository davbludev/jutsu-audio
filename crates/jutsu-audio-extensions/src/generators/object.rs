//! `sfx.object` — something real being struck, scraped or dropped.
//!
//! This exists because [`super::impact`] cannot make these, and no setting of
//! it ever could. An impact draws three sine partials and puts an envelope on
//! them, and three partials fuse into a single perceived pitch — so whatever
//! it is asked for, a listener names what they hear "a bell", "a marimba", "a
//! short musical thing". That is not a tuning failure. It is what three
//! partials *are*.
//!
//! What the ear uses to tell an object from a note is how many resonances
//! there are, how badly they disagree, and — just as much — **whether the hit
//! itself is audible**. A sound made only of resonances is a tuned bar
//! instrument however the modes are laid out, because everything reaching the
//! listener has been through a filter tuned to a pitch. Real objects radiate
//! the contact directly as well: the knuckle on the door, the tick of the
//! fingernail, the grit of the scrape. That part carries no pitch at all, and
//! it is most of what says *what happened*.
//!
//! So a sound here is three things: a burst of contact noise, the bank of
//! resonances that burst rings, and a direct path that lets the contact be
//! heard on its own. Friction sounds — creaks, drags, rolls — come from the
//! same machinery with the strike repeating instead of happening once.

use jutsu_audio_model::ParameterValue;

use super::dsp::{Modes, Resonance, decay, glide, normalise, saturate, seeded_noise};
use super::{GeneratorPreset, Settings, SfxFactory, descriptor, preset, ranged};
use crate::filter::{Svf, coefficients};

pub const TYPE_ID: &str = "sfx.object";
pub const ALGORITHM_VERSION: u32 = 2;

/// The most modes a dense material gets. Beyond this the ear gains nothing and
/// the render only gets slower.
const MAX_MODES: usize = 36;

/// The fewest, for the most damped material there is.
///
/// Not three or four: a bank that small beats audibly against itself, and a
/// listener reports that as "a pattern" rather than as an object. Density is
/// cheap and it is what stops a struck thing sounding tuned.
const MIN_MODES: usize = 12;

#[must_use]
pub fn factory() -> SfxFactory {
    SfxFactory::new(
        descriptor(
            TYPE_ID,
            "Object",
            vec![
                ranged("size", "Size", 0.45, 0.0, 1.0),
                ranged("material", "Material", 0.35, 0.0, 1.0),
                ranged("hardness", "Strike hardness", 0.6, 0.0, 1.0),
                ranged("contact", "Contact", 0.4, 0.0, 1.0),
                ranged("rough", "Roughness", 0.0, 0.0, 1.0),
                ranged("decay_ms", "Decay", 220.0, 8.0, 6_000.0),
                ranged("hollow", "Hollowness", 0.2, 0.0, 1.0),
                ranged("pitch_rise", "Pitch rise", 0.0, -2.0, 2.0),
                ranged("drive", "Drive", 2.0, 0.0, 24.0),
            ],
        ),
        presets(),
        render,
    )
}

fn presets() -> Vec<GeneratorPreset> {
    vec![
        preset(
            "Water drip",
            &[
                ("size", ParameterValue::Float(0.1)),
                ("material", ParameterValue::Float(0.02)),
                ("hardness", ParameterValue::Float(0.3)),
                ("contact", ParameterValue::Float(0.12)),
                ("rough", ParameterValue::Float(0.0)),
                ("decay_ms", ParameterValue::Float(55.0)),
                ("hollow", ParameterValue::Float(1.0)),
                ("pitch_rise", ParameterValue::Float(1.3)),
                ("drive", ParameterValue::Float(0.0)),
            ],
        ),
        preset(
            "Knuckle on a door",
            &[
                ("size", ParameterValue::Float(0.55)),
                ("material", ParameterValue::Float(0.2)),
                ("hardness", ParameterValue::Float(0.7)),
                ("contact", ParameterValue::Float(0.75)),
                ("rough", ParameterValue::Float(0.0)),
                ("decay_ms", ParameterValue::Float(60.0)),
                ("hollow", ParameterValue::Float(0.1)),
                ("pitch_rise", ParameterValue::Float(-0.15)),
                ("drive", ParameterValue::Float(4.0)),
            ],
        ),
        preset(
            "Axe into wood",
            &[
                ("size", ParameterValue::Float(0.72)),
                ("material", ParameterValue::Float(0.12)),
                ("hardness", ParameterValue::Float(0.85)),
                ("contact", ParameterValue::Float(0.8)),
                ("rough", ParameterValue::Float(0.12)),
                ("decay_ms", ParameterValue::Float(70.0)),
                ("hollow", ParameterValue::Float(0.0)),
                ("pitch_rise", ParameterValue::Float(-0.4)),
                ("drive", ParameterValue::Float(10.0)),
            ],
        ),
        preset(
            "Steel pipe",
            &[
                ("size", ParameterValue::Float(0.35)),
                ("material", ParameterValue::Float(1.0)),
                ("hardness", ParameterValue::Float(1.0)),
                ("contact", ParameterValue::Float(0.5)),
                ("rough", ParameterValue::Float(0.0)),
                ("decay_ms", ParameterValue::Float(700.0)),
                ("hollow", ParameterValue::Float(0.0)),
                ("pitch_rise", ParameterValue::Float(0.0)),
                ("drive", ParameterValue::Float(6.0)),
            ],
        ),
        preset(
            "Stone on stone",
            &[
                ("size", ParameterValue::Float(0.75)),
                ("material", ParameterValue::Float(0.45)),
                ("hardness", ParameterValue::Float(0.8)),
                ("contact", ParameterValue::Float(0.6)),
                ("rough", ParameterValue::Float(0.1)),
                ("decay_ms", ParameterValue::Float(45.0)),
                ("hollow", ParameterValue::Float(0.0)),
                ("pitch_rise", ParameterValue::Float(-0.25)),
                ("drive", ParameterValue::Float(8.0)),
            ],
        ),
        preset(
            "Cardboard",
            &[
                ("size", ParameterValue::Float(0.68)),
                ("material", ParameterValue::Float(0.0)),
                ("hardness", ParameterValue::Float(0.25)),
                ("contact", ParameterValue::Float(0.85)),
                ("rough", ParameterValue::Float(0.0)),
                ("decay_ms", ParameterValue::Float(28.0)),
                ("hollow", ParameterValue::Float(0.15)),
                ("pitch_rise", ParameterValue::Float(0.0)),
                ("drive", ParameterValue::Float(5.0)),
            ],
        ),
        preset(
            "Creaking timber",
            &[
                ("size", ParameterValue::Float(0.65)),
                ("material", ParameterValue::Float(0.3)),
                ("hardness", ParameterValue::Float(0.15)),
                ("contact", ParameterValue::Float(0.3)),
                ("rough", ParameterValue::Float(0.28)),
                ("decay_ms", ParameterValue::Float(1_200.0)),
                ("hollow", ParameterValue::Float(0.55)),
                ("pitch_rise", ParameterValue::Float(0.5)),
                ("drive", ParameterValue::Float(7.0)),
            ],
        ),
        preset(
            "Dragged across grit",
            &[
                ("size", ParameterValue::Float(0.4)),
                ("material", ParameterValue::Float(0.55)),
                ("hardness", ParameterValue::Float(0.75)),
                ("contact", ParameterValue::Float(0.9)),
                ("rough", ParameterValue::Float(0.9)),
                ("decay_ms", ParameterValue::Float(900.0)),
                ("hollow", ParameterValue::Float(0.0)),
                ("pitch_rise", ParameterValue::Float(0.0)),
                ("drive", ParameterValue::Float(3.0)),
            ],
        ),
    ]
}

/// Lays out the bank: where each mode sits, how long it rings, how loud it is.
///
/// `material` is two things at once, and they have to move together or nothing
/// separates one substance from another. A block of wood has plenty of
/// resonances but swallows almost all of them within a few tens of
/// milliseconds; a steel plate has more still and damps essentially nothing.
/// Density alone does not distinguish them — **how fast the top of the bank
/// dies** is what the ear is actually reading.
///
/// The scatter never goes to zero, even at the most damped end. Modes on tidy
/// ratios are a chord, and a chord is the one thing this generator exists to
/// avoid.
fn bank(seed: u64, material: f64, hollow: f64, decay_s: f64, ceiling: f64) -> Vec<Resonance> {
    let count = (MIN_MODES as f64 + (MAX_MODES - MIN_MODES) as f64 * material).round() as usize;
    let stretch = 1.52 - 0.2 * material;
    let scatter = 0.1 + 0.3 * material;
    let damping = 2.4 - 2.05 * material;
    let tilt = 1.15 - 0.85 * material;

    let mut jitter = seeded_noise(seed, "object.bank");
    (0..count)
        .map(|index| {
            let step = (index + 1) as f64;
            let ratio = step.powf(stretch) * (1.0 + scatter * f64::from(jitter.next_sample()));
            let ratio = ratio.max(1.0);
            // Hollowness holds the fundamental up while everything above it is
            // cut back: one resonance far louder than the rest is a cavity —
            // a bottle, a pipe, the pocket a water drop leaves behind.
            let hollowed = if index == 0 {
                1.0 + 1.6 * hollow
            } else {
                1.0 - 0.75 * hollow
            };
            Resonance {
                ratio,
                decay_s: (decay_s * ratio.powf(-damping)).max(0.001),
                gain: (ratio.powf(-tilt) * hollowed) as f32,
            }
        })
        // Modes that would land above Nyquist are dropped rather than clamped.
        // Clamping stacks them all on one frequency, which is a loud whistle
        // at the top of the range made of resonances that should not exist.
        .filter(|mode| mode.ratio <= ceiling)
        .collect()
}

fn render(settings: &Settings, seed: u64, frame_count: usize) -> Vec<f32> {
    let rate = settings.sample_rate;
    let sample_rate = f64::from(rate.max(1));
    let size = settings.float("size");
    let material = settings.float("material");
    let hardness = settings.float("hardness");
    let contact = settings.float("contact");
    let rough = settings.float("rough");
    let hollow = settings.float("hollow");
    let rise = settings.float("pitch_rise");
    let drive = settings.float("drive");
    let decay_frames = settings.frames_from_ms("decay_ms").max(1.0);
    let decay_s = decay_frames / sample_rate;

    // Small things ring high. The curve is steep because size is heard in
    // octaves: half as big is an octave up, not half the frequency.
    let fundamental = 60.0 + 1_500.0 * (1.0 - size).powf(2.2);
    let arrives_at = fundamental * 2.0_f64.powf(rise);

    let ceiling = sample_rate * 0.45 / fundamental.max(1.0);
    let modes = bank(seed, material, hollow, decay_s, ceiling);
    let mut bank_state = Modes::new(modes.len());

    // The strike itself. A hard strike is a very short bright contact — a
    // fingernail, a hammer; a soft one is longer and darker — a fist, a
    // mallet, water.
    let mut contact_noise = seeded_noise(seed, "object.contact");
    let mut gaps = seeded_noise(seed, "object.gaps");
    let mut shaping = Svf::new();
    let contact_frames = sample_rate * (0.0004 + 0.0075 * (1.0 - hardness));
    let (contact_g, contact_k) = coefficients(400.0 + 11_000.0 * hardness, 0.0, sample_rate);

    // Friction is not one event. A creak, a drag or a roll is stick-slip: the
    // surfaces catch and release dozens of times a second, each release
    // striking the object again. Nothing else in this crate can make that,
    // because everything else happens once.
    let repeating = rough > 0.001;
    let mean_gap = sample_rate / (12.0 + 160.0 * rough);
    let mut since_strike = 0_usize;
    let mut until_next = 0_usize;
    let mut strike_level = 1.0_f32;

    let mut samples = vec![0.0_f32; frame_count];
    for (frame, sample) in samples.iter_mut().enumerate() {
        if repeating && until_next == 0 {
            since_strike = 0;
            // Uneven in both timing and force. Evenly spaced catches are a
            // machine, or worse, a rhythm.
            strike_level = 0.3 + 0.7 * (0.5 + 0.5 * f64::from(gaps.next_sample())) as f32;
            let wobble = (1.0 + 0.85 * f64::from(gaps.next_sample())).clamp(0.2, 2.2);
            until_next = (mean_gap * wobble) as usize + 1;
        }
        until_next = until_next.saturating_sub(1);

        let burst = decay(since_strike, contact_frames) * strike_level;
        // A repeating excitation would otherwise run at full force to the end;
        // the sound has to go somewhere, so the whole stream falls away.
        let fade = if repeating {
            decay(frame, decay_frames)
        } else {
            1.0
        };
        let excitation =
            shaping.low_pass(contact_noise.next_sample(), contact_g, contact_k) * burst * fade;
        since_strike += 1;

        let progress = frame as f64 / decay_frames;
        // A water drop rises as the cavity it left closes; a stone dropped on
        // stone falls. Most objects do neither, which is what 0 means.
        let hz = if rise.abs() < 0.001 {
            fundamental
        } else {
            glide(fundamental, arrives_at, progress, 0.55)
        };

        // The direct path. Without it every sample the listener hears has been
        // through a resonator tuned to a pitch, which is the definition of a
        // tuned instrument — and it is why a knock, a coin and a box all came
        // back named as one.
        let ringing = bank_state.next(hz, &modes, excitation, sample_rate);
        let mixed = ringing.mul_add(
            1.0 - 0.45 * contact as f32,
            excitation * contact as f32 * 3.0,
        );
        *sample = saturate(mixed, drive);
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
            .generate_mono(11, 96_000)
    }

    /// The strongest repeat over any plausible pitch: high for a note, low for
    /// a sound the ear reads as a material.
    fn periodicity(samples: &[f32], rate: usize) -> f32 {
        let window = &samples[..rate / 4];
        let half = window.len() / 2;
        let energy: f32 = window[..half].iter().map(|s| s * s).sum();
        let mut best = 0.0_f32;
        for lag in rate / 4_000..half {
            let paired: f32 = (0..half).map(|n| window[n] * window[n + lag]).sum();
            best = best.max(paired / energy.max(f32::EPSILON));
        }
        best
    }

    #[test]
    fn a_dense_material_stops_reading_as_one_pitch() {
        // This is the whole reason the generator exists. Three partials are a
        // note however they are enveloped; a bank this dense is not.
        let sparse = render_with(&[
            ("material", 0.0),
            ("decay_ms", 600.0),
            ("hollow", 0.0),
            ("contact", 0.0),
        ]);
        let dense = render_with(&[
            ("material", 1.0),
            ("decay_ms", 600.0),
            ("hollow", 0.0),
            ("contact", 0.0),
        ]);
        let (sparse_pitch, dense_pitch) =
            (periodicity(&sparse, 48_000), periodicity(&dense, 48_000));
        assert!(
            dense_pitch < sparse_pitch * 0.8 && dense_pitch < 0.75,
            "a dense bank is markedly less like one note: {dense_pitch} against {sparse_pitch}"
        );
    }

    #[test]
    fn wood_loses_its_top_and_metal_keeps_it() {
        fn brightness_late(samples: &[f32]) -> f32 {
            // Energy in the second difference, which is what a high-pass does
            // to a signal: nearly nothing left for wood, plenty for metal.
            let late = &samples[24_000..48_000];
            late.windows(3)
                .map(|w| {
                    let d = w[0] - 2.0 * w[1] + w[2];
                    d * d
                })
                .sum::<f32>()
                / late.iter().map(|s| s * s).sum::<f32>().max(f32::EPSILON)
        }
        let wood = render_with(&[("material", 0.0), ("decay_ms", 900.0)]);
        let metal = render_with(&[("material", 1.0), ("decay_ms", 900.0)]);
        assert!(
            brightness_late(&metal) > brightness_late(&wood) * 2.0,
            "metal still has its top half a second in: {} against {}",
            brightness_late(&metal),
            brightness_late(&wood)
        );
    }

    #[test]
    fn contact_puts_unfiltered_noise_in_the_first_moment() {
        // The listener's report behind this: with no direct path, a knock, a
        // coin and a cardboard box all came back described as one tuned
        // instrument, because every sample had been through a pitched filter.
        fn roughness(samples: &[f32]) -> f32 {
            let front = &samples[..240];
            front
                .windows(3)
                .map(|w| {
                    let d = w[0] - 2.0 * w[1] + w[2];
                    d * d
                })
                .sum::<f32>()
                / front.iter().map(|s| s * s).sum::<f32>().max(f32::EPSILON)
        }
        let rung = render_with(&[("contact", 0.0), ("hardness", 0.9)]);
        let struck = render_with(&[("contact", 1.0), ("hardness", 0.9)]);
        assert!(
            roughness(&struck) > roughness(&rung) * 1.5,
            "the hit itself is audible: {} against {}",
            roughness(&struck),
            roughness(&rung)
        );
    }

    #[test]
    fn roughness_keeps_re_exciting_instead_of_decaying_smoothly() {
        /// How unevenly the level moves from one moment to the next.
        ///
        /// A single strike decays smoothly, so consecutive blocks differ by
        /// almost the same amount every time. Friction catches and releases at
        /// its own irregular rate, which shows up here and nowhere else.
        fn fluctuation(samples: &[f32]) -> f32 {
            let blocks: Vec<f32> = samples[4_800..48_000]
                .chunks(480)
                .map(|b| b.iter().map(|s| s * s).sum::<f32>().max(1e-12))
                .collect();
            let steps: Vec<f32> = blocks.windows(2).map(|p| (p[1] / p[0]).ln()).collect();
            let mean = steps.iter().sum::<f32>() / steps.len() as f32;
            steps.iter().map(|s| (s - mean).abs()).sum::<f32>() / steps.len() as f32
        }
        let struck = render_with(&[("rough", 0.0), ("decay_ms", 900.0)]);
        let dragged = render_with(&[("rough", 0.9), ("decay_ms", 900.0)]);
        assert!(
            fluctuation(&dragged) > fluctuation(&struck) * 2.0,
            "friction is a stream of contacts, not one: {} against {}",
            fluctuation(&dragged),
            fluctuation(&struck)
        );
    }

    #[test]
    fn the_strike_is_over_before_the_object_is() {
        // If the contact noise lasted, the sound would be a noise burst with a
        // ring on it rather than an object that was hit.
        let hit = render_with(&[("decay_ms", 800.0), ("hardness", 0.9)]);
        let first_ms: f32 = hit[..48].iter().map(|s| s.abs()).sum::<f32>() / 48.0;
        let later: f32 = hit[9_600..9_648].iter().map(|s| s.abs()).sum::<f32>() / 48.0;
        assert!(first_ms > 0.02, "the strike arrives at once: {first_ms}");
        assert!(
            later > 0.0,
            "and the object goes on ringing after it: {later}"
        );
    }

    #[test]
    fn size_moves_the_pitch_the_way_the_word_means() {
        fn crossings(samples: &[f32]) -> usize {
            samples[..24_000]
                .windows(2)
                .filter(|pair| (pair[0] > 0.0) != (pair[1] > 0.0))
                .count()
        }
        let small = render_with(&[("size", 0.0), ("material", 0.1), ("contact", 0.0)]);
        let large = render_with(&[("size", 1.0), ("material", 0.1), ("contact", 0.0)]);
        assert!(
            crossings(&small) > crossings(&large) * 3,
            "small rings high, large rings low: {} against {}",
            crossings(&small),
            crossings(&large)
        );
    }

    #[test]
    fn a_rise_takes_the_pitch_up_rather_than_down() {
        fn crossings(block: &[f32]) -> usize {
            block
                .windows(2)
                .filter(|pair| (pair[0] > 0.0) != (pair[1] > 0.0))
                .count()
        }
        // A water drop closes its cavity as it settles, so it rises. Nothing
        // else in this crate could do that: every other pitch envelope falls.
        let drip = render_with(&[
            ("pitch_rise", 1.5),
            ("decay_ms", 400.0),
            ("material", 0.0),
            ("hollow", 0.9),
            ("contact", 0.0),
        ]);
        let early = crossings(&drip[..4_800]);
        let late = crossings(&drip[9_600..14_400]);
        assert!(
            late > early,
            "it ends higher than it started: {late} against {early}"
        );
    }
}
