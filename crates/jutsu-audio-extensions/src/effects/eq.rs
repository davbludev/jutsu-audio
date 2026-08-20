//! `builtin.eq` — three bands: a low shelf, a peak, a high shelf.
//!
//! `builtin.lowpass` and `builtin.highpass` take something away. An EQ decides
//! how much of what, and where — the difference between removing the top of a
//! sound and making room for another one under it.
//!
//! Three bands rather than a general parametric with any number: three is what
//! a mix decision usually is (weight, presence, air), and a fixed set means
//! every band is a named parameter a caller can discover rather than an array
//! it has to construct.

use jutsu_audio_model::ParameterValue;

use super::{BuiltinEffectFactory, descriptor, ranged, safe};
use crate::parameters::{Preset, UNIT_DECIBELS, UNIT_HERTZ, UNIT_NORMALISED};
use crate::{Effect, ExtensionDescriptor};

pub const TYPE_ID: &str = "builtin.eq";

#[must_use]
pub fn factory() -> BuiltinEffectFactory {
    BuiltinEffectFactory::new(
        eq_descriptor(),
        vec![
            Preset::new(
                "Make room for the voice",
                &[
                    ("mid_hz", ParameterValue::Float(2_200.0)),
                    ("mid_db", ParameterValue::Float(-4.5)),
                    ("mid_q", ParameterValue::Float(1.2)),
                ],
            ),
            Preset::new(
                "Weight and air",
                &[
                    ("low_hz", ParameterValue::Float(90.0)),
                    ("low_db", ParameterValue::Float(3.0)),
                    ("high_hz", ParameterValue::Float(9_000.0)),
                    ("high_db", ParameterValue::Float(2.5)),
                ],
            ),
            Preset::new(
                "Telephone",
                &[
                    ("low_hz", ParameterValue::Float(400.0)),
                    ("low_db", ParameterValue::Float(-18.0)),
                    ("mid_hz", ParameterValue::Float(1_800.0)),
                    ("mid_db", ParameterValue::Float(8.0)),
                    ("mid_q", ParameterValue::Float(1.6)),
                    ("high_hz", ParameterValue::Float(4_000.0)),
                    ("high_db", ParameterValue::Float(-18.0)),
                ],
            ),
        ],
        |settings| {
            Box::new(Equaliser {
                low_hz: settings.float("low_hz"),
                low_db: settings.float("low_db"),
                mid_hz: settings.float("mid_hz"),
                mid_db: settings.float("mid_db"),
                mid_q: settings.float("mid_q"),
                high_hz: settings.float("high_hz"),
                high_db: settings.float("high_db"),
                sample_rate: 48_000,
                low: Biquad::default(),
                mid: Biquad::default(),
                high: Biquad::default(),
                stale: true,
            })
        },
    )
}

fn eq_descriptor() -> ExtensionDescriptor {
    descriptor(
        TYPE_ID,
        "EQ",
        vec![
            ranged("low_hz", "Low frequency", 120.0, 20.0, 800.0, UNIT_HERTZ),
            ranged("low_db", "Low gain", 0.0, -24.0, 24.0, UNIT_DECIBELS),
            ranged(
                "mid_hz",
                "Mid frequency",
                1_200.0,
                80.0,
                12_000.0,
                UNIT_HERTZ,
            ),
            ranged("mid_db", "Mid gain", 0.0, -24.0, 24.0, UNIT_DECIBELS),
            // Q is bandwidth: low is a broad tilt, high is a surgical notch.
            ranged("mid_q", "Mid Q", 1.0, 0.2, 8.0, UNIT_NORMALISED),
            ranged(
                "high_hz",
                "High frequency",
                8_000.0,
                1_000.0,
                18_000.0,
                UNIT_HERTZ,
            ),
            ranged("high_db", "High gain", 0.0, -24.0, 24.0, UNIT_DECIBELS),
        ],
    )
}

/// A direct-form biquad. Coefficients follow the standard cookbook shapes, so
/// a shelf here matches a shelf anywhere else with the same numbers.
#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    fn process(&mut self, input: f32) -> f32 {
        let output = self.b0 * input + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = if output.is_finite() { output } else { 0.0 };
        self.y1
    }

    fn set(&mut self, b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) {
        // Normalising by a0 is what turns the cookbook's six numbers into the
        // five a direct form actually uses.
        let inverse = if a0.abs() < f64::EPSILON {
            1.0
        } else {
            1.0 / a0
        };
        self.b0 = (b0 * inverse) as f32;
        self.b1 = (b1 * inverse) as f32;
        self.b2 = (b2 * inverse) as f32;
        self.a1 = (a1 * inverse) as f32;
        self.a2 = (a2 * inverse) as f32;
    }

    fn low_shelf(&mut self, frequency: f64, gain_db: f64, sample_rate: u32) {
        let (a, omega, alpha) = shelf_terms(frequency, gain_db, sample_rate);
        let (cos, sqrt_a) = (omega.cos(), a.sqrt());
        let two_sqrt_a_alpha = 2.0 * sqrt_a * alpha;
        self.set(
            a * ((a + 1.0) - (a - 1.0) * cos + two_sqrt_a_alpha),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
            a * ((a + 1.0) - (a - 1.0) * cos - two_sqrt_a_alpha),
            (a + 1.0) + (a - 1.0) * cos + two_sqrt_a_alpha,
            -2.0 * ((a - 1.0) + (a + 1.0) * cos),
            (a + 1.0) + (a - 1.0) * cos - two_sqrt_a_alpha,
        );
    }

    fn high_shelf(&mut self, frequency: f64, gain_db: f64, sample_rate: u32) {
        let (a, omega, alpha) = shelf_terms(frequency, gain_db, sample_rate);
        let (cos, sqrt_a) = (omega.cos(), a.sqrt());
        let two_sqrt_a_alpha = 2.0 * sqrt_a * alpha;
        self.set(
            a * ((a + 1.0) + (a - 1.0) * cos + two_sqrt_a_alpha),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
            a * ((a + 1.0) + (a - 1.0) * cos - two_sqrt_a_alpha),
            (a + 1.0) - (a - 1.0) * cos + two_sqrt_a_alpha,
            2.0 * ((a - 1.0) - (a + 1.0) * cos),
            (a + 1.0) - (a - 1.0) * cos - two_sqrt_a_alpha,
        );
    }

    fn peak(&mut self, frequency: f64, gain_db: f64, q: f64, sample_rate: u32) {
        let a = 10_f64.powf(gain_db / 40.0);
        let omega = angular(frequency, sample_rate);
        let alpha = omega.sin() / (2.0 * q.max(0.05));
        let cos = omega.cos();
        self.set(
            1.0 + alpha * a,
            -2.0 * cos,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos,
            1.0 - alpha / a,
        );
    }
}

fn angular(frequency: f64, sample_rate: u32) -> f64 {
    let nyquist = f64::from(sample_rate.max(1)) * 0.5;
    let frequency = frequency.clamp(10.0, nyquist * 0.95);
    std::f64::consts::TAU * frequency / f64::from(sample_rate.max(1))
}

/// The terms both shelves share: linear gain, angular frequency, and the
/// bandwidth term at a fixed slope of one.
fn shelf_terms(frequency: f64, gain_db: f64, sample_rate: u32) -> (f64, f64, f64) {
    let a = 10_f64.powf(gain_db / 40.0);
    let omega = angular(frequency, sample_rate);
    // The cookbook's shelf term at a slope of one, where the general
    // expression collapses to a constant.
    let alpha = omega.sin() / 2.0 * std::f64::consts::SQRT_2;
    (a, omega, alpha)
}

struct Equaliser {
    low_hz: f64,
    low_db: f64,
    mid_hz: f64,
    mid_db: f64,
    mid_q: f64,
    high_hz: f64,
    high_db: f64,
    sample_rate: u32,
    low: Biquad,
    mid: Biquad,
    high: Biquad,
    /// Set whenever a parameter changes, so coefficients are recomputed once
    /// before the next block rather than once per sample.
    stale: bool,
}

impl Equaliser {
    fn refresh(&mut self) {
        self.low
            .low_shelf(self.low_hz, self.low_db, self.sample_rate);
        self.mid
            .peak(self.mid_hz, self.mid_db, self.mid_q, self.sample_rate);
        self.high
            .high_shelf(self.high_hz, self.high_db, self.sample_rate);
        self.stale = false;
    }
}

impl Effect for Equaliser {
    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate.max(1);
        self.refresh();
        self.reset();
    }

    fn reset(&mut self) {
        self.low.reset();
        self.mid.reset();
        self.high.reset();
    }

    fn set_parameter(&mut self, id: &str, value: f64) {
        match id {
            "low_hz" => self.low_hz = value,
            "low_db" => self.low_db = value,
            "mid_hz" => self.mid_hz = value,
            "mid_db" => self.mid_db = value,
            "mid_q" => self.mid_q = value,
            "high_hz" => self.high_hz = value,
            "high_db" => self.high_db = value,
            _ => return,
        }
        self.stale = true;
    }

    fn process(&mut self, samples: &mut [f32]) {
        if self.stale {
            self.refresh();
        }
        for sample in samples {
            let value = self
                .high
                .process(self.mid.process(self.low.process(*sample)));
            *sample = safe(value);
        }
    }
}
