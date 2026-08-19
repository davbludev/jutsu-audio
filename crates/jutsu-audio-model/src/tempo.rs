//! Musical time: tempo, time signature, and converting between frames and
//! bars.
//!
//! One conversion, used by everything. A bar number in the editor's status bar,
//! in a CLI response and in a quantised note all come from here, so they cannot
//! disagree about where bar 9 starts.

use serde::{Deserialize, Serialize};

/// Ticks per beat. 960 divides cleanly by 2, 3, 4, 5, 6 and 8, so triplets and
/// sixteenths land on whole ticks.
pub const TICKS_PER_BEAT: u32 = 960;

/// What a project runs at before anyone says otherwise.
pub const DEFAULT_BEATS_PER_MINUTE: f64 = 120.0;
/// Four beats to a bar, the beat being a quarter note.
pub const DEFAULT_BEATS_PER_BAR: u32 = 4;
pub const DEFAULT_BEAT_UNIT: u32 = 4;

/// A tempo and time signature, from one frame until the next change.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct TempoChange {
    /// Where this takes effect, in project frames.
    pub frame: u64,
    pub beats_per_minute: f64,
    /// The top of the time signature.
    pub beats_per_bar: u32,
    /// The bottom of the time signature: 4 for a quarter, 8 for an eighth.
    pub beat_unit: u32,
}

impl Default for TempoChange {
    fn default() -> Self {
        Self {
            frame: 0,
            beats_per_minute: DEFAULT_BEATS_PER_MINUTE,
            beats_per_bar: DEFAULT_BEATS_PER_BAR,
            beat_unit: DEFAULT_BEAT_UNIT,
        }
    }
}

impl TempoChange {
    /// Frames one beat lasts at this tempo.
    #[must_use]
    pub fn frames_per_beat(&self, sample_rate: u32) -> f64 {
        let bpm = self.beats_per_minute.max(1.0);
        f64::from(sample_rate) * 60.0 / bpm
    }

    #[must_use]
    pub fn beats_per_bar(&self) -> f64 {
        f64::from(self.beats_per_bar.max(1))
    }
}

/// Where a frame falls in musical time. Bars and beats are 1-based, the way a
/// musician counts; ticks are 0-based within the beat.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MusicalPosition {
    pub bar: u64,
    pub beat: u64,
    pub tick: u32,
}

impl MusicalPosition {
    /// `bar.beat.tick`, the one spelling both interfaces use.
    #[must_use]
    pub fn format(&self) -> String {
        format!("{}.{}.{:03}", self.bar, self.beat, self.tick)
    }
}

/// The tempo changes of a project, in order, with the conversions they imply.
///
/// A map with no changes is 120 BPM in 4/4 from the start — a project that
/// never mentions tempo still has one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TempoMap {
    changes: Vec<TempoChange>,
}

impl TempoMap {
    /// Builds a map from changes in any order, sorted and with anything before
    /// the first frame covered.
    #[must_use]
    pub fn new(changes: &[TempoChange]) -> Self {
        let mut changes = changes.to_vec();
        changes.sort_by_key(|change| change.frame);
        Self { changes }
    }

    #[must_use]
    pub fn changes(&self) -> &[TempoChange] {
        &self.changes
    }

    /// The tempo in force at a frame.
    #[must_use]
    pub fn at(&self, frame: u64) -> TempoChange {
        self.changes
            .iter()
            .rev()
            .find(|change| change.frame <= frame)
            .copied()
            .unwrap_or_default()
    }

    /// Beats from the start of the project to a frame, following every tempo
    /// change on the way.
    #[must_use]
    pub fn beats_at(&self, frame: u64, sample_rate: u32) -> f64 {
        let mut beats = 0.0;
        let mut cursor = 0_u64;
        let mut current = TempoChange::default();

        for change in &self.changes {
            if change.frame == 0 {
                current = *change;
                continue;
            }
            if change.frame >= frame {
                break;
            }
            beats += (change.frame - cursor) as f64 / current.frames_per_beat(sample_rate);
            cursor = change.frame;
            current = *change;
        }
        beats + (frame.saturating_sub(cursor)) as f64 / current.frames_per_beat(sample_rate)
    }

    /// The frame a number of beats from the start lands on.
    ///
    /// The inverse of [`Self::beats_at`], walking the same changes so a round
    /// trip returns where it started.
    #[must_use]
    pub fn frame_at_beats(&self, beats: f64, sample_rate: u32) -> u64 {
        let beats = beats.max(0.0);
        let mut consumed = 0.0;
        let mut cursor = 0_u64;
        let mut current = TempoChange::default();

        for change in &self.changes {
            if change.frame == 0 {
                current = *change;
                continue;
            }
            let span_beats = (change.frame - cursor) as f64 / current.frames_per_beat(sample_rate);
            if consumed + span_beats > beats {
                break;
            }
            consumed += span_beats;
            cursor = change.frame;
            current = *change;
        }
        cursor + ((beats - consumed) * current.frames_per_beat(sample_rate)).round() as u64
    }

    /// Where a frame falls, counted in bars and beats.
    ///
    /// A tempo or signature change starts a new bar: whatever was left of the
    /// bar it lands in counts as a bar of its own. That is what a score does,
    /// and it is what makes [`Self::frame_at_position`] its exact inverse.
    #[must_use]
    pub fn position_at(&self, frame: u64, sample_rate: u32) -> MusicalPosition {
        let mut bars = 0_u64;
        let mut beats_into_bar = 0.0;
        let mut cursor = 0_u64;
        let mut current = TempoChange::default();

        let accumulate =
            |from: u64, to: u64, tempo: &TempoChange, bars: &mut u64, beats_into_bar: &mut f64| {
                let beats =
                    (to - from) as f64 / tempo.frames_per_beat(sample_rate) + *beats_into_bar;
                let whole_bars = (beats / tempo.beats_per_bar()).floor();
                *bars += whole_bars as u64;
                *beats_into_bar = beats - whole_bars * tempo.beats_per_bar();
            };

        for change in &self.changes {
            if change.frame == 0 {
                current = *change;
                continue;
            }
            if change.frame >= frame {
                break;
            }
            accumulate(
                cursor,
                change.frame,
                &current,
                &mut bars,
                &mut beats_into_bar,
            );
            if beats_into_bar > f64::EPSILON {
                // The change begins a bar, so the short bar before it is still
                // a bar.
                bars += 1;
                beats_into_bar = 0.0;
            }
            cursor = change.frame;
            current = *change;
        }
        accumulate(
            cursor,
            frame.max(cursor),
            &current,
            &mut bars,
            &mut beats_into_bar,
        );

        let beat = beats_into_bar.floor();
        let tick = ((beats_into_bar - beat) * f64::from(TICKS_PER_BEAT)).round() as u32;
        // A tick that rounds up to a whole beat belongs to the next beat.
        let (beat, tick) = if tick >= TICKS_PER_BEAT {
            (beat + 1.0, 0)
        } else {
            (beat, tick)
        };
        MusicalPosition {
            bar: bars + 1,
            beat: beat as u64 + 1,
            tick,
        }
    }

    /// The frame a bar, beat and tick lands on. The inverse of
    /// [`Self::position_at`].
    #[must_use]
    pub fn frame_at_position(&self, position: MusicalPosition, sample_rate: u32) -> u64 {
        let mut bars_left = position.bar.saturating_sub(1);
        let mut frame = 0_u64;
        let mut cursor = 0_u64;
        let mut current = TempoChange::default();

        for change in &self.changes {
            if change.frame == 0 {
                current = *change;
                continue;
            }
            let span_beats = (change.frame - cursor) as f64 / current.frames_per_beat(sample_rate);
            // Rounded up, because a change begins a bar: a part-bar before it
            // still uses up a bar number.
            let span_bars = (span_beats / current.beats_per_bar()).ceil() as u64;
            if span_bars >= bars_left {
                break;
            }
            bars_left -= span_bars;
            frame = change.frame;
            cursor = change.frame;
            current = *change;
        }

        let beats = bars_left as f64 * current.beats_per_bar()
            + (position.beat.saturating_sub(1)) as f64
            + f64::from(position.tick) / f64::from(TICKS_PER_BEAT);
        frame + (beats * current.frames_per_beat(sample_rate)).round() as u64
    }

    /// Snaps a frame to the nearest division of a beat — 1 for beats, 4 for
    /// sixteenths, 3 for triplets of the beat.
    #[must_use]
    pub fn quantise(&self, frame: u64, divisions_per_beat: u32, sample_rate: u32) -> u64 {
        let divisions = f64::from(divisions_per_beat.max(1));
        let beats = self.beats_at(frame, sample_rate);
        let snapped = (beats * divisions).round() / divisions;
        self.frame_at_beats(snapped, sample_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn change(frame: u64, bpm: f64, beats_per_bar: u32) -> TempoChange {
        TempoChange {
            frame,
            beats_per_minute: bpm,
            beats_per_bar,
            beat_unit: 4,
        }
    }

    #[test]
    fn a_project_that_never_mentions_tempo_still_has_one() {
        let map = TempoMap::default();
        assert_eq!(map.at(0).beats_per_minute, DEFAULT_BEATS_PER_MINUTE);
        // At 120 BPM a beat is half a second: 24 000 frames at 48 kHz.
        assert!((map.beats_at(24_000, RATE) - 1.0).abs() < 1e-9);
        assert_eq!(map.frame_at_beats(1.0, RATE), 24_000);
    }

    #[test]
    fn the_first_frame_is_bar_one_beat_one() {
        let map = TempoMap::default();
        let position = map.position_at(0, RATE);
        assert_eq!(
            (position.bar, position.beat, position.tick),
            (1, 1, 0),
            "musicians count from one"
        );
        assert_eq!(position.format(), "1.1.000");
    }

    #[test]
    fn bars_and_beats_advance_with_the_time_signature() {
        let map = TempoMap::new(&[change(0, 120.0, 4)]);
        // Four beats in, at 24 000 frames a beat, is bar two.
        assert_eq!(map.position_at(96_000, RATE).bar, 2);
        assert_eq!(map.position_at(96_000, RATE).beat, 1);

        let waltz = TempoMap::new(&[change(0, 120.0, 3)]);
        assert_eq!(waltz.position_at(72_000, RATE).bar, 2);
    }

    #[test]
    fn a_tempo_change_takes_effect_from_its_frame() {
        // 120 BPM for two beats, then 240 BPM: beats come twice as fast.
        let map = TempoMap::new(&[change(0, 120.0, 4), change(48_000, 240.0, 4)]);
        assert!((map.beats_at(48_000, RATE) - 2.0).abs() < 1e-9);
        assert!(
            (map.beats_at(60_000, RATE) - 3.0).abs() < 1e-9,
            "12 000 frames is a whole beat at 240 BPM"
        );
    }

    #[test]
    fn frames_and_beats_round_trip_across_a_tempo_change() {
        let map = TempoMap::new(&[change(0, 90.0, 4), change(64_000, 150.0, 3)]);
        for frame in [0, 1_000, 63_999, 64_000, 64_001, 250_000] {
            let beats = map.beats_at(frame, RATE);
            let back = map.frame_at_beats(beats, RATE);
            assert!(
                back.abs_diff(frame) <= 1,
                "frame {frame} came back as {back}"
            );
        }
    }

    #[test]
    fn positions_and_frames_round_trip_across_a_signature_change() {
        let map = TempoMap::new(&[change(0, 120.0, 4), change(192_000, 120.0, 3)]);
        for frame in [0, 24_000, 96_000, 192_000, 216_000, 500_000] {
            let position = map.position_at(frame, RATE);
            let back = map.frame_at_position(position, RATE);
            assert!(
                back.abs_diff(frame) <= 1,
                "{} came back as {back} from {frame}",
                position.format()
            );
        }
    }

    #[test]
    fn quantising_snaps_to_the_nearest_division_and_leaves_what_is_already_on_it() {
        let map = TempoMap::default();
        // A sixteenth at 120 BPM is 6 000 frames.
        assert_eq!(map.quantise(6_200, 4, RATE), 6_000);
        assert_eq!(map.quantise(9_000, 4, RATE), 12_000, "halfway rounds up");
        assert_eq!(map.quantise(24_000, 4, RATE), 24_000);
        assert_eq!(map.quantise(0, 4, RATE), 0);
    }

    #[test]
    fn changes_are_ordered_however_they_arrive() {
        let map = TempoMap::new(&[change(96_000, 90.0, 4), change(0, 120.0, 4)]);
        assert_eq!(map.changes()[0].frame, 0);
        assert_eq!(map.at(100_000).beats_per_minute, 90.0);
    }
}
