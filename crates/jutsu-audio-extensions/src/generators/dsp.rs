//! The small pieces of DSP the SFX generators share.
//!
//! Everything here is deterministic and allocation-light: given the same seed
//! and the same parameters it produces the same samples, which is the whole
//! promise a recipe makes.
//!
//! The pieces are chosen around what makes a synthesised sound read as a real
//! event rather than as noise with a shape: a front that arrives faster than
//! the ear can follow, a body whose **pitch falls** as it decays, noise that
//! sits in a **register** instead of across the whole spectrum, and enough
//! saturation that loud sounds loud. A one-pole filter and a fixed sine cannot
//! do any of those, which is why they are no longer the whole toolbox.

use crate::filter::{Mode, Svf, coefficients};
use crate::voice::Noise;

/// A one-pole low-pass. Cheap and gentle — 6 dB per octave. Useful for taking
/// the very top off something, not for giving noise a register: for that use
/// [`BandNoise`], which is twice as steep and can resonate.
#[derive(Clone, Copy, Debug, Default)]
pub struct LowPass {
    state: f32,
}

impl LowPass {
    #[must_use]
    pub const fn new() -> Self {
        Self { state: 0.0 }
    }

    /// `cutoff_hz` is clamped into the audible range and to below Nyquist, so a
    /// parameter sweep can run to the edges without blowing up.
    pub fn process(&mut self, sample: f32, cutoff_hz: f32, sample_rate: u32) -> f32 {
        let nyquist = sample_rate as f32 * 0.5;
        let cutoff = cutoff_hz.clamp(20.0, nyquist * 0.98);
        let coefficient =
            (1.0 - (-std::f32::consts::TAU * cutoff / sample_rate as f32).exp()).clamp(0.0, 1.0);
        self.state += coefficient * (sample - self.state);
        self.state
    }
}

/// An exponential decay from 1.0, reaching about -60 dB after `decay_frames`.
/// Exponential rather than linear because that is what a real impact does.
#[must_use]
pub fn decay(frame: usize, decay_frames: f64) -> f32 {
    if decay_frames <= 0.0 {
        return 0.0;
    }
    let progress = frame as f64 / decay_frames;
    (-6.907 * progress).exp() as f32
}

/// A decay with a front on it.
///
/// A sound that starts at full scale on its first sample clicks; a sound that
/// takes 50 ms to arrive has no impact. The interesting range is one to ten
/// milliseconds, and the difference between the two ends of it is most of what
/// separates a punch from a thump.
#[must_use]
pub fn envelope(frame: usize, attack_frames: f64, decay_frames: f64) -> f32 {
    let attack = if attack_frames <= 1.0 {
        1.0
    } else {
        (frame as f64 / attack_frames).min(1.0)
    };
    // The decay starts when the attack finishes, so a longer front does not
    // eat into the tail.
    let decayed = decay(frame.saturating_sub(attack_frames as usize), decay_frames);
    (attack as f32) * decayed
}

/// A sine at `phase` turns, where one turn is a full cycle.
#[must_use]
pub fn sine(phase: f64) -> f32 {
    (phase * std::f64::consts::TAU).sin() as f32
}

/// Advances a phase by `frequency_hz` for one frame, wrapping at one turn.
#[must_use]
pub fn advance_phase(phase: f64, frequency_hz: f64, sample_rate: u32) -> f64 {
    (phase + frequency_hz / f64::from(sample_rate.max(1))).fract()
}

/// Interpolates between two values over a `0.0..1.0` progress.
#[must_use]
pub fn lerp(from: f64, to: f64, progress: f64) -> f64 {
    to.mul_add(progress, from * (1.0 - progress))
}

/// Interpolates between two frequencies the way the ear hears the distance —
/// in octaves rather than in hertz.
///
/// A linear sweep from 400 Hz to 40 Hz spends most of its time in the top
/// octave and arrives at the bottom almost instantly, which sounds like a
/// glitch. The same sweep in the log domain falls evenly, which sounds like a
/// drum. `curve` above 1.0 makes the fall front-loaded, which is what a struck
/// object actually does.
#[must_use]
pub fn glide(from_hz: f64, to_hz: f64, progress: f64, curve: f64) -> f64 {
    let shaped = progress.clamp(0.0, 1.0).powf(curve.max(0.01));
    let from = from_hz.max(1.0).log2();
    let to = to_hz.max(1.0).log2();
    2.0_f64.powf(lerp(from, to, shaped))
}

/// Noise with a register.
///
/// White noise is every frequency at once, which is why it reads as hiss no
/// matter how it is shaped in level. Pushing it through a resonant band-pass
/// puts it *somewhere* — low and wide for a rumble, high and narrow for a
/// cymbal, narrow enough and it acquires something close to a pitch.
#[derive(Clone, Debug)]
pub struct BandNoise {
    noise: Noise,
    filter: Svf,
    mode: Mode,
}

impl BandNoise {
    #[must_use]
    pub fn new(seed: u64, label: &str, mode: Mode) -> Self {
        Self {
            noise: seeded_noise(seed, label),
            filter: Svf::new(),
            mode,
        }
    }

    /// One sample at the given centre. `resonance` is `0.0..=0.98`; above about
    /// 0.8 the band starts to ring, which reads as metal.
    ///
    /// The output is scaled by how much of the spectrum the filter kept, so a
    /// narrow band at 200 Hz comes out at about the same level as a wide one at
    /// 8 kHz. Without that, a layer's level would depend on where it sits, and
    /// a quiet low band would be mistaken for a missing one.
    pub fn next(&mut self, centre_hz: f64, resonance: f64, sample_rate: f64) -> f32 {
        let (g, k) = coefficients(centre_hz, resonance, sample_rate);
        let nyquist = sample_rate * 0.5;
        let centre = centre_hz.clamp(20.0, nyquist * 0.98);
        let kept = match self.mode {
            // A band-pass keeps roughly `centre * k` of the spectrum, a
            // low-pass everything below its cutoff, a high-pass everything
            // above it.
            Mode::Band => centre * f64::from(k).max(0.05),
            Mode::Low => centre,
            Mode::High => (nyquist - centre).max(nyquist * 0.02),
        };
        let compensation = (nyquist / kept).sqrt().clamp(0.5, 30.0) as f32;
        self.filter
            .process(self.noise.next_sample(), self.mode, g, k)
            * compensation
    }
}

/// A body made of several partials, each with its own decay.
///
/// One sine is a tuning fork. What makes a drum a drum, or a gunshot a gunshot,
/// is several partials at ratios that are **not** whole numbers, the higher
/// ones dying first. `ratios` are multiples of the fundamental; 2.7 and 5.4 are
/// close to a drum head, 2.0 and 3.0 would be a musical instrument.
#[derive(Clone, Debug, Default)]
pub struct Partials {
    phases: Vec<f64>,
}

impl Partials {
    #[must_use]
    pub fn new(count: usize) -> Self {
        Self {
            phases: vec![0.0; count],
        }
    }

    /// One sample of the whole body at a fundamental of `hz`.
    ///
    /// `weights` scales each partial; pass a decayed weight per partial to make
    /// the higher ones fade first.
    pub fn sample(&mut self, hz: f64, ratios: &[f64], weights: &[f32], sample_rate: u32) -> f32 {
        let mut sum = 0.0;
        for (index, phase) in self.phases.iter_mut().enumerate() {
            let ratio = ratios.get(index).copied().unwrap_or(1.0);
            let weight = weights.get(index).copied().unwrap_or(0.0);
            sum += sine(*phase) * weight;
            *phase = advance_phase(*phase, hz * ratio, sample_rate);
        }
        sum
    }
}

/// One resonance of a struck object: where it sits, how long it rings, how
/// loud it is.
#[derive(Clone, Copy, Debug)]
pub struct Resonance {
    /// Multiple of the fundamental. Whole numbers make a musical note; the
    /// awkward numbers in between are what make an object.
    pub ratio: f64,
    /// Seconds to fall 60 dB. Per mode, because that is the difference between
    /// materials: wood loses its top instantly, metal keeps everything.
    pub decay_s: f64,
    pub gain: f32,
}

/// A bank of ringing resonances, struck by an excitation signal.
///
/// This exists because three sine partials is a musical note, whatever
/// envelope is put on it. A listener asked to name one says "a bell", "a
/// marimba", "a short musical thing" — never "a knock" or "a drip". The ear
/// separates *an object* from *a note* by counting: a handful of partials
/// fuse into one pitch, while twenty or thirty at unrelated ratios, each dying
/// at its own rate, stop being a pitch and start being a material.
///
/// So a sound here is not drawn — it is struck. A short burst of contact noise
/// goes into a bank of narrow resonators, and what comes out is the bank's own
/// ringing. That is how real objects work, and it is why the same bank makes a
/// knock, a tick, a coin and a drip depending only on how the modes are laid
/// out.
#[derive(Clone, Debug, Default)]
pub struct Modes {
    filters: Vec<Svf>,
}

impl Modes {
    #[must_use]
    pub fn new(count: usize) -> Self {
        Self {
            filters: vec![Svf::new(); count],
        }
    }

    /// One sample: `excitation` struck into every mode, summed.
    ///
    /// The coefficients are recomputed each frame so the whole bank can glide,
    /// which is what a water drop does as its cavity closes.
    pub fn next(
        &mut self,
        fundamental_hz: f64,
        modes: &[Resonance],
        excitation: f32,
        sample_rate: f64,
    ) -> f32 {
        let nyquist = sample_rate * 0.5;
        let mut sum = 0.0_f32;
        for (filter, mode) in self.filters.iter_mut().zip(modes) {
            let hz = (fundamental_hz * mode.ratio).clamp(20.0, nyquist * 0.95);
            let g = (std::f64::consts::PI * hz / sample_rate).tan() as f32;
            // Ring time sets the damping: a band-pass decays as
            // exp(-pi * f * t / Q), so a mode asked to last `decay_s` needs
            // exactly this much Q, and k is its reciprocal.
            let quality = (std::f64::consts::PI * hz * mode.decay_s / 6.907).max(0.5);
            let k = (1.0 / quality) as f32;
            // No correction for Q here, deliberately. A band-pass struck by an
            // impulse rings at roughly the same peak however narrow it is —
            // narrower only means longer. Scaling by k instead makes every
            // long-ringing mode quiet, which leaves the fundamental alone in
            // front and turns the whole bank back into a single pitch.
            sum += filter.process(excitation, Mode::Band, g, k) * mode.gain;
        }
        sum
    }
}

/// Soft saturation. `drive_db` above zero pushes the signal into the curve.
///
/// This is most of what makes a loud sound *sound* loud: the peaks round off
/// instead of growing, so the sound gets denser rather than just bigger, and
/// the harmonics that adds are what the ear reads as force. The output is
/// scaled back so that driving something does not simply raise its level.
#[must_use]
pub fn saturate(sample: f32, drive_db: f64) -> f32 {
    if drive_db <= 0.0 {
        return sample;
    }
    let drive = 10.0_f32.powf(drive_db as f32 / 20.0);
    let normalise = 1.0 / drive.tanh().max(f32::EPSILON);
    (sample * drive).tanh() * normalise
}

/// Normalises a buffer to just under full scale. A generator's parameters are
/// about character, not level; peak-matching keeps one recipe from being ten
/// times louder than the next.
pub fn normalise(samples: &mut [f32]) {
    let peak = samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    if peak <= f32::EPSILON {
        return;
    }
    let scale = 0.98 / peak;
    for sample in samples {
        *sample *= scale;
    }
}

/// A noise source seeded from a recipe seed and a label, so two parts of one
/// generator never share a stream.
#[must_use]
pub fn seeded_noise(seed: u64, label: &str) -> Noise {
    Noise::new(crate::recipe::GeneratorRecipe::new("", 0, seed).derive_seed(label))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_glide_falls_evenly_in_octaves() {
        // Half way through a two-octave fall, the pitch is one octave down —
        // which a linear interpolation would put far lower.
        let middle = glide(400.0, 100.0, 0.5, 1.0);
        assert!(
            (middle - 200.0).abs() < 1.0,
            "half of two octaves is one octave: {middle}"
        );
    }

    #[test]
    fn an_envelope_has_a_front_and_a_tail() {
        let attack = 48.0;
        let start = envelope(0, attack, 4_800.0);
        let peak = envelope(48, attack, 4_800.0);
        let later = envelope(4_800, attack, 4_800.0);
        assert!(start < 0.05, "it starts from nothing: {start}");
        assert!(peak > 0.9, "and arrives at the top of the attack: {peak}");
        assert!(later < peak / 100.0, "then decays away: {later}");
    }

    #[test]
    fn band_noise_puts_its_energy_where_it_is_asked_to() {
        // Measured as how often the signal crosses zero: a low band wanders,
        // a high band flickers. It is a crude pitch estimate and enough here.
        fn crossings(centre: f64) -> usize {
            let mut band = BandNoise::new(3, "test", Mode::Band);
            let mut previous = 0.0_f32;
            let mut count = 0;
            for _ in 0..48_000 {
                let sample = band.next(centre, 0.7, 48_000.0);
                if (sample > 0.0) != (previous > 0.0) {
                    count += 1;
                }
                previous = sample;
            }
            count
        }
        let low = crossings(120.0);
        let high = crossings(6_000.0);
        assert!(
            high > low * 8,
            "a high band moves far faster than a low one: {low} against {high}"
        );
    }

    #[test]
    fn saturation_rounds_peaks_off_rather_than_growing_them() {
        let quiet = saturate(0.1, 12.0);
        let loud = saturate(1.0, 12.0);
        assert!(quiet > 0.1, "quiet parts come up: {quiet}");
        assert!(loud <= 1.0, "loud parts do not run away: {loud}");
        assert!(
            quiet / 0.1 > loud / 1.0,
            "which is what compression of the peaks means"
        );
    }

    #[test]
    fn partials_at_inharmonic_ratios_do_not_repeat_like_one_tone() {
        let mut body = Partials::new(3);
        let ratios = [1.0, 2.7, 5.4];
        let weights = [1.0, 0.6, 0.3];
        let rendered: Vec<f32> = (0..4_800)
            .map(|_| body.sample(100.0, &ratios, &weights, 48_000))
            .collect();
        // One cycle of the fundamental is 480 frames. A single sine would repeat
        // exactly; inharmonic partials will not.
        let difference: f32 = (0..480)
            .map(|frame| (rendered[frame] - rendered[frame + 480]).abs())
            .sum();
        assert!(
            difference > 10.0,
            "the second cycle differs from the first: {difference}"
        );
    }
}
