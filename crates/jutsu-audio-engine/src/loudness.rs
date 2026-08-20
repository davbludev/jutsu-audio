//! Loudness, measured the way a delivery specification means it.
//!
//! A peak meter says how close a file came to clipping. It says nothing about
//! how loud the file *is*, which is why a quiet mix and a crushed one can both
//! read −0.1 dBFS. Every game and broadcast pipeline that has a loudness target
//! states it in LUFS, so the exporter has to be able to answer in LUFS rather
//! than leaving it to whoever opens the file next.
//!
//! This is ITU-R BS.1770-4: K-weight the signal, take the mean square of
//! overlapping 400 ms blocks, throw away the blocks that are effectively
//! silence, then throw away the blocks more than 10 LU below what is left, and
//! average what survives. The two gates are the whole reason the number
//! corresponds to what a listener would call the loudness of a piece rather
//! than to its average level: without them, silence between hits would drag a
//! loud cue down.

/// What a render measures out at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loudness {
    /// Integrated loudness over the whole render, in LUFS.
    ///
    /// `None` when nothing in the render was loud enough to pass the absolute
    /// gate — silence has no loudness, and reporting a very negative number
    /// would invite someone to normalise against it.
    pub integrated_lufs: Option<f64>,
    /// The loudest sample, in dBFS.
    pub sample_peak_dbfs: f64,
    /// The loudest point of the waveform *between* samples, in dBFS.
    ///
    /// A signal can sit under full scale at every sample and still overshoot it
    /// once reconstructed, which is what clips a converter or a lossy encoder.
    /// Estimated by oversampling four times, as the specification requires.
    pub true_peak_dbfs: f64,
}

/// The block length loudness is measured over, and how far each block starts
/// after the last: 400 ms and 100 ms, as specified.
const BLOCK_MS: f64 = 400.0;
const STEP_MS: f64 = 100.0;

/// Blocks quieter than this are not part of the programme at all.
const ABSOLUTE_GATE_LUFS: f64 = -70.0;
/// And blocks more than this far below the ungated average are not either.
const RELATIVE_GATE_LU: f64 = -10.0;

/// The offset that turns weighted mean square into LUFS.
const LUFS_OFFSET: f64 = -0.691;

/// Measures interleaved audio.
///
/// # Panics
///
/// Never: an empty buffer, a zero channel count and a zero sample rate all
/// answer with silence rather than dividing by anything.
#[must_use]
pub fn measure(samples: &[f32], channels: u16, sample_rate: u32) -> Loudness {
    let channels = usize::from(channels.max(1));
    let sample_rate = sample_rate.max(1);
    let frames = samples.len() / channels;

    let sample_peak = samples
        .iter()
        .fold(0.0_f32, |loudest, sample| loudest.max(sample.abs()));

    if frames == 0 {
        return Loudness {
            integrated_lufs: None,
            sample_peak_dbfs: f64::NEG_INFINITY,
            true_peak_dbfs: f64::NEG_INFINITY,
        };
    }

    // K-weight each channel, keeping the squares: everything below works on
    // mean squares, never on the filtered samples themselves.
    let mut weighted = vec![0.0_f64; frames * channels];
    for channel in 0..channels {
        let mut shelf = Biquad::high_shelf(sample_rate);
        let mut high_pass = Biquad::high_pass(sample_rate);
        for frame in 0..frames {
            let sample = f64::from(samples[frame * channels + channel]);
            let filtered = high_pass.process(shelf.process(sample));
            weighted[frame * channels + channel] = filtered * filtered;
        }
    }

    let block_frames = ((BLOCK_MS / 1_000.0) * f64::from(sample_rate)).round() as usize;
    let step_frames = ((STEP_MS / 1_000.0) * f64::from(sample_rate)).round() as usize;
    let mut block_powers = Vec::new();
    if block_frames > 0 && step_frames > 0 && frames >= block_frames {
        let mut start = 0;
        while start + block_frames <= frames {
            // Channel weights are 1.0 for left and right; the surround weights
            // in the specification have no meaning for a stereo master.
            let mut power = 0.0_f64;
            for channel in 0..channels {
                let sum: f64 = (start..start + block_frames)
                    .map(|frame| weighted[frame * channels + channel])
                    .sum();
                power += sum / block_frames as f64;
            }
            block_powers.push(power);
            start += step_frames;
        }
    }

    let integrated = integrate(&block_powers);

    Loudness {
        integrated_lufs: integrated,
        sample_peak_dbfs: to_dbfs(f64::from(sample_peak)),
        true_peak_dbfs: to_dbfs(true_peak(samples, channels)),
    }
}

/// The two gates, applied in the order the specification gives them.
fn integrate(block_powers: &[f64]) -> Option<f64> {
    let loud_enough: Vec<f64> = block_powers
        .iter()
        .copied()
        .filter(|power| loudness_of(*power) > ABSOLUTE_GATE_LUFS)
        .collect();
    if loud_enough.is_empty() {
        return None;
    }

    let mean = loud_enough.iter().sum::<f64>() / loud_enough.len() as f64;
    let threshold = loudness_of(mean) + RELATIVE_GATE_LU;

    let kept: Vec<f64> = loud_enough
        .into_iter()
        .filter(|power| loudness_of(*power) > threshold)
        .collect();
    if kept.is_empty() {
        return None;
    }
    Some(loudness_of(kept.iter().sum::<f64>() / kept.len() as f64))
}

fn loudness_of(power: f64) -> f64 {
    if power <= 0.0 {
        f64::NEG_INFINITY
    } else {
        LUFS_OFFSET + 10.0 * power.log10()
    }
}

fn to_dbfs(amplitude: f64) -> f64 {
    if amplitude <= 0.0 {
        f64::NEG_INFINITY
    } else {
        20.0 * amplitude.log10()
    }
}

/// Four-times oversampled peak, per channel.
///
/// Catmull-Rom through the neighbouring samples rather than a windowed sinc:
/// the error against a true reconstruction is a small fraction of a decibel on
/// programme material, and the number exists to warn about an overshoot rather
/// than to certify one to three decimal places. What it must never do is report
/// *less* than the sample peak, and it cannot: the interpolation passes through
/// every sample it interpolates between.
fn true_peak(samples: &[f32], channels: usize) -> f64 {
    let frames = samples.len() / channels;
    let mut loudest = 0.0_f64;
    for channel in 0..channels {
        let at = |frame: isize| -> f64 {
            let frame = frame.clamp(0, frames as isize - 1) as usize;
            f64::from(samples[frame * channels + channel])
        };
        for frame in 0..frames as isize {
            let (p0, p1, p2, p3) = (at(frame - 1), at(frame), at(frame + 1), at(frame + 2));
            loudest = loudest.max(p1.abs());
            for step in 1..4 {
                let t = f64::from(step) / 4.0;
                let value = 0.5
                    * ((2.0 * p1)
                        + (-p0 + p2) * t
                        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t
                        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t * t * t);
                loudest = loudest.max(value.abs());
            }
        }
    }
    loudest
}

/// The two stages of the K-weighting filter, as a direct-form biquad.
#[derive(Clone, Copy, Debug)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    /// Stage one: a high shelf that models the acoustic effect of a head.
    /// The constants are the specification's, expressed so they hold at any
    /// sample rate rather than only at 48 kHz.
    fn high_shelf(sample_rate: u32) -> Self {
        let f0 = 1_681.974_450_955_533;
        let gain_db = 3.999_843_853_973_347;
        let q = 0.705_179_812_636_402_3;
        let k = (std::f64::consts::PI * f0 / f64::from(sample_rate)).tan();
        let vh = 10_f64.powf(gain_db / 20.0);
        let vb = vh.powf(0.499_666_774_154_460_2);
        let a0 = 1.0 + k / q + k * k;
        Self::new(
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
            2.0 * (k * k - 1.0) / a0,
            (1.0 - k / q + k * k) / a0,
        )
    }

    /// Stage two: a high-pass that takes out what a listener does not weigh.
    fn high_pass(sample_rate: u32) -> Self {
        let f0 = 38.135_470_876_024_44;
        let q = 0.500_327_037_323_877_3;
        let k = (std::f64::consts::PI * f0 / f64::from(sample_rate)).tan();
        let a0 = 1.0 + k / q + k * k;
        Self::new(
            1.0,
            -2.0,
            1.0,
            2.0 * (k * k - 1.0) / a0,
            (1.0 - k / q + k * k) / a0,
        )
    }

    const fn new(b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) -> Self {
        Self {
            b0,
            b1,
            b2,
            a1,
            a2,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn process(&mut self, input: f64) -> f64 {
        let output = self.b0 * input + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = if output.is_finite() { output } else { 0.0 };
        self.y1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// A 1 kHz sine in both channels at a known level, five seconds of it.
    fn tone(dbfs: f64) -> Vec<f32> {
        let amplitude = 10_f64.powf(dbfs / 20.0);
        let frames = RATE as usize * 5;
        let mut samples = Vec::with_capacity(frames * 2);
        for frame in 0..frames {
            let phase = frame as f64 / f64::from(RATE) * 1_000.0 * std::f64::consts::TAU;
            let value = (phase.sin() * amplitude) as f32;
            samples.push(value);
            samples.push(value);
        }
        samples
    }

    /// The published check: a 1 kHz sine at −23 dBFS in both channels measures
    /// −23 LUFS, within a tenth of a decibel. If the weighting, the gating or
    /// the offset were wrong, this number would not land.
    #[test]
    fn a_thousand_hertz_at_minus_twenty_three_reads_minus_twenty_three() {
        let measured = measure(&tone(-23.0), 2, RATE);
        let integrated = measured.integrated_lufs.expect("a loudness");
        assert!(
            (integrated - -23.0).abs() < 0.1,
            "measured {integrated} LUFS, expected -23"
        );
    }

    /// And it tracks: ten decibels quieter is ten LU quieter, which is what
    /// makes the number usable as a target rather than as a curiosity.
    #[test]
    fn ten_decibels_quieter_measures_ten_units_quieter() {
        let loud = measure(&tone(-13.0), 2, RATE)
            .integrated_lufs
            .expect("a loudness");
        let quiet = measure(&tone(-23.0), 2, RATE)
            .integrated_lufs
            .expect("a loudness");
        assert!(
            ((loud - quiet) - 10.0).abs() < 0.1,
            "{loud} against {quiet} is not ten units"
        );
    }

    #[test]
    fn silence_has_no_loudness_rather_than_a_very_small_one() {
        let measured = measure(&vec![0.0_f32; RATE as usize * 2], 2, RATE);
        assert_eq!(measured.integrated_lufs, None);
        assert_eq!(measured.sample_peak_dbfs, f64::NEG_INFINITY);
    }

    /// The case the true peak exists for: a signal that never reaches full
    /// scale at a sample, but does between them.
    #[test]
    fn the_true_peak_finds_what_the_sample_peak_misses() {
        // A half-rate square-ish alternation lands its real peak between
        // samples on every cycle.
        let samples: Vec<f32> = (0..4_800)
            .map(|frame| {
                let phase =
                    frame as f64 / f64::from(RATE) * 11_999.0 * std::f64::consts::TAU + 0.78;
                (phase.sin() * 0.9) as f32
            })
            .collect();
        let measured = measure(&samples, 1, RATE);
        assert!(
            measured.true_peak_dbfs > measured.sample_peak_dbfs,
            "true peak {} did not exceed sample peak {}",
            measured.true_peak_dbfs,
            measured.sample_peak_dbfs
        );
    }

    #[test]
    fn the_true_peak_is_never_below_the_sample_peak() {
        for dbfs in [-40.0, -12.0, -0.5] {
            let measured = measure(&tone(dbfs), 2, RATE);
            assert!(measured.true_peak_dbfs >= measured.sample_peak_dbfs - 1e-9);
        }
    }
}
