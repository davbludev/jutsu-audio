//! A topology-preserving state-variable filter, shared by everything in this
//! crate that needs a filter with a real slope.
//!
//! A one-pole low-pass rolls off at 6 dB per octave, which is gentle enough
//! that noise pushed through it is still audibly noise across the whole
//! spectrum. This one is 12 dB per octave and resonant, which is what lets a
//! noise layer occupy a register rather than all of them — the difference
//! between a sound with a pitch and a sound that is just hiss.
//!
//! The form is Zavalishin's TPT SVF: stable when the cutoff is swept fast,
//! which is exactly what a pitch envelope on an impact does.

/// Two integrator states. One instance filters one channel.
#[derive(Clone, Copy, Debug, Default)]
pub struct Svf {
    ic1: f32,
    ic2: f32,
}

/// What a filter does to what passes through it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    /// Keeps what is below the cutoff.
    Low,
    /// Keeps a band around the cutoff, which is how a noise layer is given a
    /// register and, with enough resonance, something close to a pitch.
    Band,
    /// Keeps what is above the cutoff.
    High,
}

impl Svf {
    #[must_use]
    pub const fn new() -> Self {
        Self { ic1: 0.0, ic2: 0.0 }
    }

    /// One sample. `g` and `k` come from [`coefficients`]; a caller sweeping a
    /// cutoff recomputes them per frame, a caller with a fixed cutoff once.
    pub fn process(&mut self, input: f32, mode: Mode, g: f32, k: f32) -> f32 {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = input - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        let output = match mode {
            Mode::Low => v2,
            Mode::Band => v1,
            Mode::High => input - k * v1 - v2,
        };
        // A resonant filter can ring past full scale on a transient, and a NaN
        // here would poison the whole mix.
        if output.is_finite() { output } else { 0.0 }
    }

    /// One low-pass sample, for callers that only ever want that.
    pub fn low_pass(&mut self, input: f32, g: f32, k: f32) -> f32 {
        self.process(input, Mode::Low, g, k)
    }
}

/// Pre-warped cutoff and damping for a cutoff in hertz and a resonance in
/// `0.0..=0.98`.
///
/// `k` is damping, not resonance: 2.0 is none at all and the lower it goes the
/// more the filter rings at its cutoff. Clamping the cutoff below Nyquist is
/// what lets a sweep run to the edges without the tangent blowing up.
#[must_use]
pub fn coefficients(cutoff_hz: f64, resonance: f64, sample_rate: f64) -> (f32, f32) {
    let nyquist = sample_rate * 0.5;
    let cutoff = cutoff_hz.clamp(20.0, nyquist * 0.98);
    let g = (std::f64::consts::PI * cutoff / sample_rate).tan() as f32;
    let k = (2.0 - 1.94 * resonance.clamp(0.0, 0.98)) as f32;
    (g, k)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Root mean square of a filtered sine at `hz`, after the filter settles.
    fn response(mode: Mode, cutoff: f64, resonance: f64, hz: f64) -> f32 {
        const RATE: f64 = 48_000.0;
        let (g, k) = coefficients(cutoff, resonance, RATE);
        let mut filter = Svf::new();
        let mut sum = 0.0_f32;
        let frames = 24_000;
        for frame in 0..frames {
            let input = (std::f64::consts::TAU * hz * frame as f64 / RATE).sin() as f32;
            let output = filter.process(input, mode, g, k);
            // Skip the first tenth: the integrators start at zero.
            if frame > frames / 10 {
                sum += output * output;
            }
        }
        (sum / (frames - frames / 10) as f32).sqrt()
    }

    #[test]
    fn a_low_pass_keeps_what_is_below_it_and_drops_what_is_above() {
        let passed = response(Mode::Low, 1_000.0, 0.0, 200.0);
        let stopped = response(Mode::Low, 1_000.0, 0.0, 8_000.0);
        assert!(
            passed > 0.6,
            "a tone well below the cutoff passes: {passed}"
        );
        assert!(
            stopped < passed / 20.0,
            "three octaves up is far quieter: {stopped} against {passed}"
        );
    }

    #[test]
    fn a_high_pass_is_the_other_way_round() {
        let passed = response(Mode::High, 1_000.0, 0.0, 8_000.0);
        let stopped = response(Mode::High, 1_000.0, 0.0, 125.0);
        assert!(passed > 0.6, "a tone above the cutoff passes: {passed}");
        assert!(
            stopped < passed / 20.0,
            "three octaves down is far quieter: {stopped} against {passed}"
        );
    }

    #[test]
    fn a_band_pass_keeps_only_what_is_near_the_cutoff() {
        let centre = response(Mode::Band, 1_000.0, 0.0, 1_000.0);
        let below = response(Mode::Band, 1_000.0, 0.0, 100.0);
        let above = response(Mode::Band, 1_000.0, 0.0, 10_000.0);
        assert!(
            below < centre / 5.0 && above < centre / 5.0,
            "the band is loudest at its centre: {below} / {centre} / {above}"
        );
    }

    #[test]
    fn resonance_lifts_the_cutoff_without_making_it_blow_up() {
        let flat = response(Mode::Low, 1_000.0, 0.0, 1_000.0);
        let ringing = response(Mode::Low, 1_000.0, 0.9, 1_000.0);
        assert!(
            ringing > flat * 1.5,
            "resonance emphasises the cutoff: {ringing} against {flat}"
        );
        assert!(ringing.is_finite() && ringing < 20.0, "and stays bounded");
    }

    #[test]
    fn a_swept_cutoff_stays_finite() {
        // A pitch envelope sweeps the cutoff a long way in a few milliseconds;
        // an unstable filter shows up here as a NaN rather than as a bad sound.
        let mut filter = Svf::new();
        let mut noise = crate::voice::Noise::new(9);
        for frame in 0..48_000 {
            let cutoff = 40.0 + 18_000.0 * (frame as f64 / 480.0).fract();
            let (g, k) = coefficients(cutoff, 0.95, 48_000.0);
            let output = filter.process(noise.next_sample(), Mode::Band, g, k);
            assert!(output.is_finite(), "frame {frame} produced {output}");
        }
    }
}
