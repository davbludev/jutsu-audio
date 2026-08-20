//! `sfx.impact` — something struck: a drum, a footfall, a body hitting a wall,
//! the near half of a gunshot.
//!
//! Three things make a struck sound read as struck rather than as a beep with
//! a click on it: the body **falls in pitch** as it decays, the partials above
//! the fundamental are at ratios that do not line up (a drum head is not a
//! guitar string), and the noise of the strike itself sits in a register rather
//! than across the whole spectrum.

use jutsu_audio_model::ParameterValue;

use super::dsp::{BandNoise, Partials, decay, envelope, glide, normalise, saturate};
use super::{GeneratorPreset, Settings, SfxFactory, descriptor, preset, ranged};
use crate::filter::Mode;

pub const TYPE_ID: &str = "sfx.impact";
pub const ALGORITHM_VERSION: u32 = 2;

/// Ratios of a struck membrane rather than of a plucked string. Whole-number
/// ratios would fuse into one pitched note; these stay heard as a hit.
const RATIOS: [f64; 3] = [1.0, 2.71, 5.43];

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
                ranged("attack_ms", "Attack", 1.5, 0.0, 60.0),
                ranged("pitch_drop", "Pitch drop", 1.4, 0.0, 4.0),
                ranged("tone", "Tone", 0.45, 0.0, 1.0),
                ranged("ring", "Ring", 0.25, 0.0, 1.0),
                ranged("strike_ms", "Strike", 40.0, 1.0, 800.0),
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
            "Kick drum",
            &[
                ("weight", ParameterValue::Float(0.85)),
                ("brightness", ParameterValue::Float(0.2)),
                ("decay_ms", ParameterValue::Float(320.0)),
                ("attack_ms", ParameterValue::Float(0.5)),
                ("pitch_drop", ParameterValue::Float(2.2)),
                ("tone", ParameterValue::Float(0.18)),
                ("strike_ms", ParameterValue::Float(12.0)),
                ("ring", ParameterValue::Float(0.1)),
                ("drive", ParameterValue::Float(9.0)),
            ],
        ),
        preset(
            "Snare",
            &[
                ("weight", ParameterValue::Float(0.35)),
                ("brightness", ParameterValue::Float(0.7)),
                ("decay_ms", ParameterValue::Float(190.0)),
                ("attack_ms", ParameterValue::Float(0.4)),
                ("pitch_drop", ParameterValue::Float(0.8)),
                ("tone", ParameterValue::Float(0.6)),
                ("strike_ms", ParameterValue::Float(120.0)),
                ("ring", ParameterValue::Float(0.45)),
                ("drive", ParameterValue::Float(8.0)),
            ],
        ),
        preset(
            "Gunshot body",
            &[
                ("weight", ParameterValue::Float(0.6)),
                ("brightness", ParameterValue::Float(0.85)),
                ("decay_ms", ParameterValue::Float(120.0)),
                ("attack_ms", ParameterValue::Float(0.0)),
                ("pitch_drop", ParameterValue::Float(3.2)),
                ("tone", ParameterValue::Float(0.88)),
                ("strike_ms", ParameterValue::Float(130.0)),
                ("ring", ParameterValue::Float(0.15)),
                ("drive", ParameterValue::Float(18.0)),
            ],
        ),
        preset(
            "Heavy slam",
            &[
                ("weight", ParameterValue::Float(1.0)),
                ("brightness", ParameterValue::Float(0.25)),
                ("decay_ms", ParameterValue::Float(700.0)),
                ("attack_ms", ParameterValue::Float(2.0)),
                ("pitch_drop", ParameterValue::Float(1.8)),
                ("tone", ParameterValue::Float(0.3)),
                ("strike_ms", ParameterValue::Float(60.0)),
                ("ring", ParameterValue::Float(0.2)),
                ("drive", ParameterValue::Float(10.0)),
            ],
        ),
        preset(
            "Metal clang",
            &[
                ("weight", ParameterValue::Float(0.25)),
                ("brightness", ParameterValue::Float(0.8)),
                ("decay_ms", ParameterValue::Float(1_400.0)),
                ("attack_ms", ParameterValue::Float(0.2)),
                ("pitch_drop", ParameterValue::Float(0.2)),
                ("tone", ParameterValue::Float(0.55)),
                ("strike_ms", ParameterValue::Float(500.0)),
                ("ring", ParameterValue::Float(0.95)),
                ("drive", ParameterValue::Float(4.0)),
            ],
        ),
    ]
}

fn render(settings: &Settings, seed: u64, frame_count: usize) -> Vec<f32> {
    let rate = settings.sample_rate;
    let sample_rate = f64::from(rate.max(1));
    let weight = settings.float("weight");
    let brightness = settings.float("brightness");
    let tone = settings.float("tone");
    let ring = settings.float("ring");
    let drive = settings.float("drive");
    let decay_frames = settings.frames_from_ms("decay_ms");
    let attack_frames = settings.frames_from_ms("attack_ms");
    let drop_octaves = settings.float("pitch_drop");

    // Where the body settles, and where it starts. Heavier means lower, and the
    // drop is what carries the force: a kick without one is a bass note.
    let settled_hz = 190.0 - 145.0 * weight;
    let struck_hz = settled_hz * 2.0_f64.powf(drop_octaves);
    let drop_frames = (decay_frames * 0.3).max(1.0);

    // The strike noise. Its length is its own parameter rather than a fraction
    // of `ring`: ring is about the body's upper partials — wood against metal —
    // and tying the two together made it impossible to ask for a shot, which
    // needs a long noise burst and no metallic ringing at all.
    // High-pass rather than band-pass: a band gives noise a pitch, and a pitch
    // is what makes a strike read as a struck cymbal instead of as a hit. Above
    // the corner everything is still there, which is what a snare's rattle and
    // a blast's front both are. `ring` resonates the corner, so the metallic
    // end of the range is still reachable — it is just no longer the only end.
    let noise_centre = 60.0 + 2_600.0 * brightness;
    let noise_resonance = (0.15 + 0.8 * ring).min(0.98);
    let noise_frames = settings.frames_from_ms("strike_ms").max(1.0);

    let mut noise = BandNoise::new(seed, "impact.strike", Mode::High);
    let mut body = Partials::new(RATIOS.len());
    let mut samples = vec![0.0_f32; frame_count];

    for (frame, sample) in samples.iter_mut().enumerate() {
        let progress = frame as f64 / drop_frames;
        let hz = glide(struck_hz, settled_hz, progress, 0.45);

        // Higher partials die first, and `ring` decides how much of them there
        // is at all — none is wood, plenty is metal. Every partial rides the
        // same front: a body whose upper partials ignored the attack would
        // click however slow the attack was set.
        let front = if attack_frames <= 1.0 {
            1.0
        } else {
            (frame as f64 / attack_frames).min(1.0) as f32
        };
        //
        // Ring lengthens them as well as raising them. Wood swallows its upper
        // partials before its fundamental; metal is the other way round, and a
        // hit whose partials always died first could never be metal whatever
        // the parameter said.
        let weights = [
            envelope(frame, attack_frames, decay_frames),
            front * decay(frame, decay_frames * (0.4 + 1.6 * ring)) * (0.15 + 0.75 * ring) as f32,
            front * decay(frame, decay_frames * (0.2 + 1.2 * ring)) * (0.05 + 0.45 * ring) as f32,
        ];
        let struck = body.sample(hz, &RATIOS, &weights, rate);

        let strike = noise.next(noise_centre, noise_resonance, sample_rate)
            * envelope(frame, attack_frames * 0.5, noise_frames);

        // Tone crosses between the two rather than adding one on top of the
        // other. Adding meant the body was always there — at `tone` 1.0 a hit
        // still had a tuned thump under it, so a shot could never be all
        // noise, which is exactly what a shot is.
        let mixed = struck * (1.0 - tone) as f32 * (0.45 + 0.55 * weight) as f32
            + strike * tone as f32 * (0.4 + 0.6 * brightness) as f32;
        *sample = saturate(mixed * 0.9, drive);
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
            .generate_mono(4, frames)
    }

    /// Zero crossings in a window: a crude pitch reading, and enough to see a
    /// body fall.
    fn crossings(samples: &[f32]) -> usize {
        samples
            .windows(2)
            .filter(|pair| (pair[0] > 0.0) != (pair[1] > 0.0))
            .count()
    }

    #[test]
    fn the_body_falls_in_pitch_while_it_decays() {
        let rendered = render_with(
            &[
                ("weight", 0.9),
                ("brightness", 0.0),
                ("decay_ms", 600.0),
                ("pitch_drop", 2.5),
                ("tone", 0.0),
                ("ring", 0.0),
                ("drive", 0.0),
            ],
            24_000,
        );
        let early = crossings(&rendered[200..2_400]);
        let late = crossings(&rendered[9_600..11_800]);
        assert!(
            early > late * 2,
            "the strike is far higher than the settled body: {early} against {late}"
        );
    }

    #[test]
    fn a_longer_attack_starts_from_less() {
        let sharp = render_with(&[("attack_ms", 0.0), ("decay_ms", 400.0)], 4_800);
        let soft = render_with(&[("attack_ms", 40.0), ("decay_ms", 400.0)], 4_800);
        let first = |samples: &[f32]| {
            samples[..48]
                .iter()
                .fold(0.0_f32, |peak, s| peak.max(s.abs()))
        };
        assert!(
            first(&soft) < first(&sharp) / 3.0,
            "a slow front does not arrive at once: {} against {}",
            first(&soft),
            first(&sharp)
        );
    }

    #[test]
    fn tone_crosses_between_body_and_noise_rather_than_adding() {
        // At tone 1.0 there must be no tuned body left at all: a shot is noise,
        // and a thump underneath it is what made every attempt read as a
        // hammer on wood.
        fn crossings(samples: &[f32]) -> usize {
            samples
                .windows(2)
                .filter(|pair| (pair[0] > 0.0) != (pair[1] > 0.0))
                .count()
        }
        let settings = |tone: f64| {
            vec![
                ("weight", 0.0),
                ("brightness", 1.0),
                ("decay_ms", 300.0),
                ("pitch_drop", 0.0),
                ("tone", tone),
                ("ring", 0.0),
                ("strike_ms", 300.0),
                ("drive", 0.0),
            ]
        };
        let body = render_with(&settings(0.0), 24_000);
        let noise = render_with(&settings(1.0), 24_000);
        // The body sits at 190 Hz — about 190 crossings in a tenth of a second.
        // The noise band sits at 8.3 kHz and moves far faster.
        assert!(
            crossings(&noise[..4_800]) > crossings(&body[..4_800]) * 10,
            "all-noise is nothing like all-body: {} against {}",
            crossings(&noise[..4_800]),
            crossings(&body[..4_800])
        );
    }

    #[test]
    fn the_strike_length_is_independent_of_ring() {
        // A shot needs a long noise burst and no metallic ringing at once.
        // While the two shared a parameter, that combination did not exist.
        fn noise_tail(samples: &[f32]) -> f32 {
            samples[4_800..9_600].iter().map(|s| s * s).sum()
        }
        let short = render_with(
            &[
                ("tone", 1.0),
                ("ring", 0.0),
                ("strike_ms", 5.0),
                ("decay_ms", 300.0),
            ],
            24_000,
        );
        let long = render_with(
            &[
                ("tone", 1.0),
                ("ring", 0.0),
                ("strike_ms", 300.0),
                ("decay_ms", 300.0),
            ],
            24_000,
        );
        assert!(
            noise_tail(&long) > noise_tail(&short) * 50.0,
            "a long strike is still there a tenth of a second in: {} against {}",
            noise_tail(&long),
            noise_tail(&short)
        );
    }

    #[test]
    fn ring_keeps_the_higher_partials_alive() {
        // Compared as a share of the whole, because every render is normalised:
        // a louder strike would otherwise look like a shorter tail.
        fn tail_share(samples: &[f32]) -> f32 {
            let energy = |slice: &[f32]| slice.iter().map(|s| s * s).sum::<f32>();
            energy(&samples[24_000..]) / energy(samples).max(f32::EPSILON)
        }
        let dead = render_with(&[("ring", 0.0), ("decay_ms", 800.0), ("tone", 0.0)], 48_000);
        let alive = render_with(&[("ring", 1.0), ("decay_ms", 800.0), ("tone", 0.0)], 48_000);
        assert!(
            tail_share(&alive) > tail_share(&dead) * 2.0,
            "ringing outlasts a dead hit: {} against {}",
            tail_share(&alive),
            tail_share(&dead)
        );
    }
}
