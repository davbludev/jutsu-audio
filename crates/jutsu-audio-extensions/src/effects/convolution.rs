//! `builtin.convolution` — reverb that is a recording of a place rather than a
//! model of one.
//!
//! The algorithmic reverb next door builds a room out of delays: cheap, tunable,
//! and always recognisably a machine. Convolution instead multiplies the signal
//! by a recording of how a real space answers a single click, so a hall sounds
//! like that hall and a corridor like that corridor. Nothing else gets that.
//!
//! The cost is arithmetic: a two-second impulse at 48 kHz is ninety-six thousand
//! multiply-accumulates per sample, which no amount of care makes fast enough
//! directly. So it is done in the frequency domain, in partitions — the impulse
//! is cut into blocks, each block's spectrum is precomputed once, and every
//! input block is multiplied against all of them and summed. That is uniform
//! partitioned convolution, and it turns a quadratic problem into a linear one.
//!
//! The impulse itself never comes from here. Extensions do not read files; the
//! host loads the asset and hands the samples over through `set_impulse`.

use std::collections::VecDeque;
use std::sync::Arc;

use jutsu_audio_model::ParameterValue;
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use super::{BuiltinEffectFactory, descriptor, from_decibels, ranged, safe};
use crate::parameters::{Preset, UNIT_DECIBELS, UNIT_NORMALISED};
use crate::{Effect, ExtensionDescriptor};

pub const TYPE_ID: &str = "builtin.convolution";

/// The parameter naming the project asset to convolve with.
pub const IMPULSE_PARAMETER: &str = "impulse_asset";

/// Frames per partition. The same size the host walks chains in, so a block of
/// audio arrives whole rather than split across two partition boundaries.
const PARTITION: usize = 1_024;

/// The longest impulse taken, in seconds. A response longer than this is
/// truncated rather than refused: eight seconds is already a cathedral, and
/// there is no sensible reading of a minute-long reverb tail.
const MAXIMUM_SECONDS: f64 = 8.0;

#[must_use]
pub fn factory() -> BuiltinEffectFactory {
    BuiltinEffectFactory::new(
        convolution_descriptor(),
        vec![
            Preset::new(
                "Full wet",
                &[
                    ("output_db", ParameterValue::Float(0.0)),
                    ("pre_delay_ms", ParameterValue::Float(0.0)),
                ],
            ),
            Preset::new(
                "Distant",
                &[
                    ("output_db", ParameterValue::Float(-6.0)),
                    ("pre_delay_ms", ParameterValue::Float(40.0)),
                ],
            ),
        ],
        |settings| {
            Box::new(Convolution {
                output_gain: from_decibels(settings.float("output_db")),
                pre_delay_ms: settings.float("pre_delay_ms"),
                sample_rate: 48_000,
                engine: None,
                impulse: Vec::new(),
                impulse_rate: 0,
            })
        },
    )
}

fn convolution_descriptor() -> ExtensionDescriptor {
    let mut parameters = vec![
        ranged("output_db", "Output", 0.0, -24.0, 12.0, UNIT_DECIBELS),
        ranged(
            "pre_delay_ms",
            "Pre-delay",
            0.0,
            0.0,
            200.0,
            UNIT_NORMALISED,
        ),
    ];
    // Text rather than a number: it names an asset in the project, and the host
    // is what turns that name into audio.
    parameters.push(crate::ParameterDescriptor {
        id: IMPULSE_PARAMETER.into(),
        display_name: "Impulse".into(),
        value_type: crate::ParameterType::Text,
        default_value: ParameterValue::Text(String::new()),
        introduced_in_state_version: 1,
        automatable: false,
        minimum: None,
        maximum: None,
        unit: None,
    });
    descriptor(TYPE_ID, "Convolution", parameters)
}

struct Convolution {
    output_gain: f32,
    pre_delay_ms: f64,
    sample_rate: u32,
    /// Built when an impulse arrives, and rebuilt if the rate changes. `None`
    /// means there is nothing to convolve with, and the effect passes audio
    /// through — an insert naming an impulse the project has lost should not
    /// silence the track it is on.
    engine: Option<PartitionedConvolver>,
    impulse: Vec<f32>,
    impulse_rate: u32,
}

impl Convolution {
    fn rebuild(&mut self) {
        if self.impulse.is_empty() {
            self.engine = None;
            return;
        }
        let resampled = resample(&self.impulse, self.impulse_rate, self.sample_rate);
        let longest = (MAXIMUM_SECONDS * f64::from(self.sample_rate)) as usize;
        let pre_delay =
            (self.pre_delay_ms.max(0.0) / 1_000.0 * f64::from(self.sample_rate)) as usize;

        let mut response = vec![0.0_f32; pre_delay];
        response.extend_from_slice(&resampled);
        response.truncate(longest.max(1));
        self.engine = Some(PartitionedConvolver::new(&response));
    }
}

impl Effect for Convolution {
    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate.max(1);
        self.rebuild();
    }

    fn reset(&mut self) {
        if let Some(engine) = &mut self.engine {
            engine.reset();
        }
    }

    fn set_impulse(&mut self, samples: &[f32], sample_rate: u32) {
        self.impulse = samples.to_vec();
        self.impulse_rate = sample_rate.max(1);
        self.rebuild();
    }

    fn set_parameter(&mut self, id: &str, value: f64) {
        match id {
            "output_db" => self.output_gain = from_decibels(value),
            // Pre-delay is part of the impulse rather than a separate line, so
            // changing it rebuilds. Between blocks, never inside one, and only
            // when it really moved: rebuilding costs an FFT per partition.
            "pre_delay_ms" if (value - self.pre_delay_ms).abs() > f64::EPSILON => {
                self.pre_delay_ms = value;
                self.rebuild();
            }
            _ => {}
        }
    }

    fn latency_frames(&self) -> u32 {
        // None. Overlap-save produces the output for a partition as soon as
        // that partition has arrived, and the host renders in partitions of
        // exactly this size — so what comes out lines up with what went in.
        //
        // The one exception is a final block shorter than a partition: its
        // output waits for a partition that never arrives, so the last few
        // hundred frames of a render come back silent. The reverb tail past the
        // end of the timeline is not rendered at all either, and both are
        // written down in `tail_frames`.
        0
    }

    fn tail_frames(&self) -> u32 {
        self.engine
            .as_ref()
            .map_or(0, |engine| u32::try_from(engine.length()).unwrap_or(0))
    }

    fn process(&mut self, samples: &mut [f32]) {
        let Some(engine) = &mut self.engine else {
            return;
        };
        let gain = self.output_gain;
        engine.process(samples);
        for sample in samples {
            *sample = safe(*sample * gain);
        }
    }
}

/// Uniform partitioned overlap-save convolution.
struct PartitionedConvolver {
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    /// One spectrum per partition of the impulse.
    impulse: Vec<Vec<Complex<f32>>>,
    /// The last N input spectra, newest first once rotated.
    history: VecDeque<Vec<Complex<f32>>>,
    /// Input that has arrived but does not yet fill a partition.
    pending: Vec<f32>,
    /// Output that is ready to be handed back.
    ready: VecDeque<f32>,
    /// The previous partition of input, kept because overlap-save needs the
    /// transform to cover twice the partition length.
    previous: Vec<f32>,
    scratch: Vec<Complex<f32>>,
    length: usize,
}

impl PartitionedConvolver {
    fn new(response: &[f32]) -> Self {
        let size = PARTITION * 2;
        let mut planner = FftPlanner::new();
        let forward = planner.plan_fft_forward(size);
        let inverse = planner.plan_fft_inverse(size);

        let mut impulse = Vec::new();
        for partition in response.chunks(PARTITION) {
            let mut buffer = vec![Complex::new(0.0, 0.0); size];
            for (slot, sample) in buffer.iter_mut().zip(partition) {
                slot.re = *sample;
            }
            forward.process(&mut buffer);
            impulse.push(buffer);
        }

        let history = (0..impulse.len())
            .map(|_| vec![Complex::new(0.0, 0.0); size])
            .collect();

        Self {
            forward,
            inverse,
            impulse,
            history,
            pending: Vec::with_capacity(PARTITION),
            ready: VecDeque::with_capacity(PARTITION * 4),
            previous: vec![0.0; PARTITION],
            scratch: vec![Complex::new(0.0, 0.0); size],
            length: response.len(),
        }
    }

    const fn length(&self) -> usize {
        self.length
    }

    fn reset(&mut self) {
        for spectrum in &mut self.history {
            spectrum.fill(Complex::new(0.0, 0.0));
        }
        self.pending.clear();
        self.ready.clear();
        self.previous.fill(0.0);
    }

    /// One partition in, one partition of output out.
    fn advance(&mut self, block: &[f32]) {
        let size = PARTITION * 2;
        // Overlap-save: the transform covers the previous partition and this
        // one, and only the second half of the result is valid.
        let mut input = vec![Complex::new(0.0, 0.0); size];
        for (index, sample) in self.previous.iter().chain(block).enumerate() {
            input[index].re = *sample;
        }
        self.forward.process(&mut input);

        self.history.pop_back();
        self.history.push_front(input);

        self.scratch.fill(Complex::new(0.0, 0.0));
        for (spectrum, partition) in self.history.iter().zip(&self.impulse) {
            for (slot, (left, right)) in self.scratch.iter_mut().zip(spectrum.iter().zip(partition))
            {
                *slot += left * right;
            }
        }
        self.inverse.process(&mut self.scratch);

        // rustfft does not normalise, so the round trip is scaled by its size.
        let scale = 1.0 / size as f32;
        for index in PARTITION..size {
            self.ready.push_back(self.scratch[index].re * scale);
        }
        self.previous.copy_from_slice(block);
    }

    fn process(&mut self, samples: &mut [f32]) {
        self.pending.extend_from_slice(samples);
        while self.pending.len() >= PARTITION {
            let block: Vec<f32> = self.pending.drain(..PARTITION).collect();
            self.advance(&block);
        }
        for sample in samples {
            // Nothing ready yet is the latency this effect declares, and
            // silence is what that latency sounds like.
            *sample = self.ready.pop_front().unwrap_or(0.0);
        }
    }
}

/// Linear resampling, for an impulse recorded at a different rate.
///
/// Linear is not good enough for programme material and is more than good
/// enough here: an impulse response is noise-like, and the error is far below
/// the reverb tail it lands in.
fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == 0 || from == to || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = f64::from(to) / f64::from(from);
    let length = (samples.len() as f64 * ratio) as usize;
    (0..length)
        .map(|index| {
            let position = index as f64 / ratio;
            let left = position.floor() as usize;
            let fraction = (position - position.floor()) as f32;
            let a = samples.get(left).copied().unwrap_or(0.0);
            let b = samples.get(left + 1).copied().unwrap_or(a);
            a + (b - a) * fraction
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EffectFactory;
    use std::collections::BTreeMap;

    fn convolver(impulse: &[f32]) -> Box<dyn Effect> {
        let mut effect = factory()
            .instantiate(&BTreeMap::new())
            .expect("instantiate");
        effect.prepare(48_000);
        effect.set_impulse(impulse, 48_000);
        effect
    }

    /// The definition, checked directly: convolving with a single spike at
    /// offset zero returns the input, delayed by the declared latency and by
    /// nothing else.
    #[test]
    fn an_impulse_of_one_spike_returns_the_signal_it_was_given() {
        let mut effect = convolver(&[1.0]);

        let mut input: Vec<f32> = (0..4_096)
            .map(|index| ((index % 97) as f32 / 97.0) - 0.5)
            .collect();
        let original = input.clone();
        effect.process(&mut input);

        for (index, expected) in original.iter().enumerate() {
            assert!(
                (input[index] - expected).abs() < 1e-4,
                "sample {index}: {} against {expected}",
                input[index]
            );
        }
    }

    /// A spike further into the impulse is a delay, which is the simplest
    /// audible thing a convolution can be.
    #[test]
    fn a_spike_further_along_the_impulse_delays_by_that_much() {
        let mut impulse = vec![0.0_f32; 500];
        impulse[400] = 1.0;
        let mut effect = convolver(&impulse);

        let mut input = vec![0.0_f32; 4_096];
        input[0] = 1.0;
        effect.process(&mut input);

        let peak = input
            .iter()
            .enumerate()
            .max_by(|(_, left), (_, right)| left.abs().total_cmp(&right.abs()))
            .map(|(index, _)| index)
            .expect("a peak");
        assert_eq!(peak, 400, "the spike moved somewhere other than its offset");
    }

    /// An impulse with a tail keeps sounding after the input stops. That is the
    /// whole point of a reverb, and the thing a passthrough would not do.
    #[test]
    fn a_decaying_impulse_leaves_a_tail_behind_the_signal() {
        let impulse: Vec<f32> = (0..8_000)
            .map(|index| {
                let decay = 1.0 - index as f32 / 8_000.0;
                let noise = ((index * 2_654_435_761_usize) % 2_000) as f32 / 1_000.0 - 1.0;
                noise * decay * 0.2
            })
            .collect();
        let mut effect = convolver(&impulse);

        let mut buffer = vec![0.0_f32; 16_384];
        buffer[0..64].fill(0.5);
        effect.process(&mut buffer);

        let tail: f32 = buffer[6_000..10_000]
            .iter()
            .fold(0.0_f32, |loudest, sample| loudest.max(sample.abs()));
        assert!(tail > 0.001, "the tail died immediately: {tail}");
    }

    /// No impulse means no reverb, not silence: an insert whose asset has gone
    /// must not take the track with it.
    #[test]
    fn without_an_impulse_the_audio_passes_through_untouched() {
        let mut effect = factory()
            .instantiate(&BTreeMap::new())
            .expect("instantiate");
        effect.prepare(48_000);

        let mut buffer: Vec<f32> = (0..512).map(|index| index as f32 / 512.0).collect();
        let original = buffer.clone();
        effect.process(&mut buffer);
        assert_eq!(buffer, original);
        assert_eq!(effect.latency_frames(), 0);
    }
}
