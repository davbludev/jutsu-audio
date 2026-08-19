//! The waveform peak cache.
//!
//! Peaks are stored at several window sizes, coarsest last. Drawing picks the
//! level with about one peak per pixel, so a zoomed-out view of an hour-long
//! source folds tens of peaks per column instead of tens of thousands.
//!
//! The cache is keyed by the source's content fingerprint, so it can never
//! describe different audio than the file it was built from. What it *can* be
//! is an older format, which is what [`CACHE_FORMAT_VERSION`] catches.

use serde::{Deserialize, Serialize};

use crate::AudioMetadata;

/// Bumped whenever the cache gains data a reader needs. An older cache is
/// rejected on load and rebuilt rather than half-used.
pub const CACHE_FORMAT_VERSION: u32 = 1;

/// Frames per peak at the finest level. Fine enough to draw a short one-shot
/// zoomed right in.
pub const BASE_WINDOW_FRAMES: u64 = 1_024;

/// How much coarser each level is than the one before it.
const LEVEL_FACTOR: usize = 8;

/// How few peaks a level may have before there is no point going coarser.
const MINIMUM_LEVEL_PEAKS: usize = 32;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WaveformPeak {
    pub minimum: f32,
    pub maximum: f32,
}

/// Peaks at one window size.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PeakLevel {
    pub window_frames: u64,
    pub peaks: Vec<WaveformPeak>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CachedWaveform {
    pub metadata: AudioMetadata,
    /// Window of the finest level. Kept at the top level because it is what
    /// every reader needs first.
    pub window_frames: u64,
    pub peaks: Vec<WaveformPeak>,
    /// Coarser levels, each [`LEVEL_FACTOR`] times the window of the last.
    #[serde(default)]
    pub coarser: Vec<PeakLevel>,
    /// Absent in caches written before levels existed, which reads as `0` and
    /// so as "rebuild me".
    #[serde(default)]
    pub format_version: u32,
}

impl CachedWaveform {
    /// Builds every level from decoded interleaved samples.
    #[must_use]
    pub fn build(metadata: AudioMetadata, samples: &[f32]) -> Self {
        let channels = usize::from(metadata.channels).max(1);
        let window_samples = BASE_WINDOW_FRAMES as usize * channels;
        let peaks: Vec<WaveformPeak> = samples
            .chunks(window_samples)
            .map(|window| WaveformPeak {
                minimum: window.iter().copied().fold(f32::INFINITY, f32::min),
                maximum: window.iter().copied().fold(f32::NEG_INFINITY, f32::max),
            })
            .collect();

        // Each coarser level folds the one below it, so building them all costs
        // barely more than building the first.
        let mut coarser = Vec::new();
        let mut window = BASE_WINDOW_FRAMES;
        let mut current = peaks.as_slice();
        let mut owned;
        while current.len() > MINIMUM_LEVEL_PEAKS * LEVEL_FACTOR {
            owned = fold(current);
            window *= LEVEL_FACTOR as u64;
            coarser.push(PeakLevel {
                window_frames: window,
                peaks: owned,
            });
            current = &coarser.last().expect("just pushed").peaks;
        }

        Self {
            metadata,
            window_frames: BASE_WINDOW_FRAMES,
            peaks,
            coarser,
            format_version: CACHE_FORMAT_VERSION,
        }
    }

    /// True when this cache was written by an older build and should be rebuilt
    /// before it is drawn from.
    #[must_use]
    pub const fn is_current(&self) -> bool {
        self.format_version == CACHE_FORMAT_VERSION
    }

    /// The level to draw at this zoom: the coarsest one still giving at least
    /// one peak per pixel, so no column is drawn from a single fold.
    #[must_use]
    pub fn level_for(&self, source_frames_per_pixel: f64) -> (u64, &[WaveformPeak]) {
        let mut chosen = (self.window_frames, self.peaks.as_slice());
        for level in &self.coarser {
            if (level.window_frames as f64) <= source_frames_per_pixel && !level.peaks.is_empty() {
                chosen = (level.window_frames, level.peaks.as_slice());
            }
        }
        chosen
    }
}

/// Folds [`LEVEL_FACTOR`] peaks into one, keeping the extremes.
fn fold(peaks: &[WaveformPeak]) -> Vec<WaveformPeak> {
    peaks
        .chunks(LEVEL_FACTOR)
        .map(|group| WaveformPeak {
            minimum: group
                .iter()
                .map(|peak| peak.minimum)
                .fold(f32::INFINITY, f32::min),
            maximum: group
                .iter()
                .map(|peak| peak.maximum)
                .fold(f32::NEG_INFINITY, f32::max),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(frames: u64) -> AudioMetadata {
        AudioMetadata {
            sample_rate: 48_000,
            channels: 1,
            frame_count: frames,
            bits_per_sample: 32,
            sample_format: "float".into(),
        }
    }

    /// A ramp long enough to need several levels.
    fn long_source() -> Vec<f32> {
        (0..600_000)
            .map(|index| if index % 2 == 0 { 1.0 } else { -1.0 })
            .collect()
    }

    #[test]
    fn a_short_source_has_only_the_finest_level() {
        let samples = vec![0.5_f32; 4_096];
        let waveform = CachedWaveform::build(metadata(4_096), &samples);
        assert_eq!(waveform.peaks.len(), 4);
        assert!(
            waveform.coarser.is_empty(),
            "there is nothing to zoom out from"
        );
    }

    #[test]
    fn a_long_source_gains_coarser_levels_each_eight_times_the_last() {
        let waveform = CachedWaveform::build(metadata(600_000), &long_source());
        let windows: Vec<u64> = waveform
            .coarser
            .iter()
            .map(|level| level.window_frames)
            .collect();
        assert_eq!(
            windows,
            vec![8_192],
            "levels stop once one more would leave too few peaks to draw from"
        );
        assert!(
            waveform.coarser.iter().all(|level| !level.peaks.is_empty()),
            "an empty level would draw nothing"
        );
    }

    #[test]
    fn zooming_out_picks_a_coarser_level_and_zooming_in_returns_to_the_finest() {
        let waveform = CachedWaveform::build(metadata(600_000), &long_source());

        let (window, _) = waveform.level_for(64.0);
        assert_eq!(window, BASE_WINDOW_FRAMES, "zoomed in: draw the detail");

        let (window, _) = waveform.level_for(10_000.0);
        assert_eq!(window, 8_192);

        let (window, peaks) = waveform.level_for(1_000_000.0);
        assert_eq!(
            window, 8_192,
            "zooming further out than the coarsest level keeps using it"
        );
        assert!(!peaks.is_empty());
    }

    #[test]
    fn folding_keeps_the_extremes_so_a_zoomed_out_peak_is_never_understated() {
        let waveform = CachedWaveform::build(metadata(600_000), &long_source());
        let (_, coarse) = waveform.level_for(1_000_000.0);
        assert!(
            coarse
                .iter()
                .all(|peak| peak.maximum >= 1.0 && peak.minimum <= -1.0),
            "the full-scale ramp survives every fold"
        );
    }

    #[test]
    fn a_cache_written_before_levels_existed_reports_itself_as_stale() {
        let waveform = CachedWaveform {
            metadata: metadata(1_024),
            window_frames: BASE_WINDOW_FRAMES,
            peaks: Vec::new(),
            coarser: Vec::new(),
            format_version: 0,
        };
        assert!(!waveform.is_current());
        assert!(CachedWaveform::build(metadata(1_024), &[0.0; 1_024]).is_current());
    }
}
