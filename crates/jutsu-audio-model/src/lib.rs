use std::collections::{BTreeMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod tempo;

pub use tempo::{DEFAULT_BEATS_PER_MINUTE, MusicalPosition, TICKS_PER_BEAT, TempoChange, TempoMap};

pub const CURRENT_PROJECT_SCHEMA_VERSION: u32 = 1;

macro_rules! entity_id {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

entity_id!(ProjectId);
entity_id!(AssetId);
entity_id!(TrackId);
entity_id!(LayerId);
entity_id!(ClipId);
entity_id!(BusId);
entity_id!(MarkerId);
entity_id!(AutomationId);
entity_id!(EffectId);
entity_id!(PatternId);

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Project {
    pub schema_version: u32,
    pub id: ProjectId,
    pub metadata: ProjectMetadata,
    pub assets: Vec<Asset>,
    pub buses: Vec<MixerBus>,
    pub master_bus_id: BusId,
    pub tracks: Vec<Track>,
    /// Named positions on the timeline, in project frames. Ordered by the
    /// project, not by position — a marker keeps its identity when it moves.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// The region playback repeats over, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_region: Option<LoopRegion>,
    /// Parameter values that move over time. One lane per target parameter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub automation: Vec<AutomationLane>,
    /// Tempo and time-signature changes, in frame order. Empty means 120 BPM
    /// in 4/4, which is what `TempoMap` returns for a project that has none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tempo: Vec<TempoChange>,
    /// Reusable note sequences. A clip can play one instead of holding its own
    /// notes, so editing the pattern changes every clip that uses it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patterns: Vec<Pattern>,
}

/// A named sequence of notes, played by any number of clips.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Pattern {
    pub id: PatternId,
    pub name: String,
    /// How long one repeat lasts, in project frames. A clip longer than this
    /// repeats the pattern; a clip shorter than it plays part of one.
    pub length_frames: u64,
    #[serde(default)]
    pub notes: Vec<ClipNote>,
}

/// What a lane writes to. Named by entity ID, so a lane survives everything
/// except deleting what it automates.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AutomationTarget {
    Track { track_id: TrackId },
    Bus { bus_id: BusId },
    Clip { clip_id: ClipId },
}

/// How a value travels from one breakpoint to the next.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Curve {
    /// Holds until the next breakpoint, then jumps. For switches and choices.
    Step,
    /// Straight line to the next breakpoint. The default, and what a fader does.
    #[default]
    Linear,
}

/// One point on a lane: a value at a frame, and how it reaches the next.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct Breakpoint {
    pub frame: u64,
    pub value: f64,
    #[serde(default)]
    pub curve: Curve,
}

/// One parameter of one entity, moving over time.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AutomationLane {
    pub id: AutomationId,
    pub target: AutomationTarget,
    /// The parameter ID this lane writes, e.g. `gain_db`.
    pub parameter: String,
    /// Breakpoints in frame order. An empty lane is inert rather than invalid:
    /// it is what a lane looks like before its first point is drawn.
    #[serde(default)]
    pub points: Vec<Breakpoint>,
}

impl AutomationLane {
    /// The value at a frame: held before the first point, interpolated
    /// between points, held after the last.
    ///
    /// `None` for an empty lane, which means "whatever the parameter already
    /// says" rather than any particular number.
    #[must_use]
    pub fn value_at(&self, frame: u64) -> Option<f64> {
        let first = self.points.first()?;
        if frame <= first.frame {
            return Some(first.value);
        }
        let last = self.points.last()?;
        if frame >= last.frame {
            return Some(last.value);
        }
        let index = self
            .points
            .partition_point(|point| point.frame <= frame)
            .saturating_sub(1);
        let start = &self.points[index];
        let end = self.points.get(index + 1)?;
        Some(match start.curve {
            Curve::Step => start.value,
            Curve::Linear => {
                let span = end.frame.saturating_sub(start.frame);
                if span == 0 {
                    end.value
                } else {
                    let progress = (frame - start.frame) as f64 / span as f64;
                    start.value + (end.value - start.value) * progress
                }
            }
        })
    }

    /// True when the points are in frame order, which is what `value_at`
    /// assumes and what validation enforces.
    #[must_use]
    pub fn is_ordered(&self) -> bool {
        self.points
            .windows(2)
            .all(|pair| pair[0].frame <= pair[1].frame)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Marker {
    pub id: MarkerId,
    pub name: String,
    pub frame: u64,
}

/// A half-open range of project frames: `start_frame` plays, `end_frame` does
/// not. Empty and reversed ranges are rejected by validation rather than
/// silently repaired, because a loop that plays nothing is a bug upstream.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct LoopRegion {
    pub start_frame: u64,
    pub end_frame: u64,
    /// A disabled region is remembered but not played, so toggling looping off
    /// and on again does not lose where the loop was.
    pub enabled: bool,
}

impl LoopRegion {
    #[must_use]
    pub const fn frame_count(&self) -> u64 {
        self.end_frame.saturating_sub(self.start_frame)
    }

    /// True when the region is worth playing: enabled and not empty.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.enabled && self.end_frame > self.start_frame
    }
}

impl Project {
    /// The project's tempo, ready to convert with.
    #[must_use]
    pub fn tempo_map(&self) -> TempoMap {
        TempoMap::new(&self.tempo)
    }

    /// Follows a bus's output chain looking for the bus it started from.
    ///
    /// A cycle would make the mix unrenderable — every bus waiting on itself —
    /// so it is rejected at validation rather than discovered in the callback.
    #[must_use]
    pub fn bus_route_loops(&self, start: BusId) -> bool {
        let mut visited = HashSet::new();
        let mut current = start;
        while let Some(next) = self
            .buses
            .iter()
            .find(|bus| bus.id == current)
            .and_then(|bus| bus.output_bus_id)
        {
            if next == start {
                return true;
            }
            if !visited.insert(next) {
                // A loop that does not include `start`: the bus this one feeds
                // is itself in a cycle, which its own diagnostic reports.
                return false;
            }
            current = next;
        }
        false
    }

    #[must_use]
    pub fn validate(&self) -> Vec<ValidationDiagnostic> {
        let mut diagnostics = Vec::new();

        if self.schema_version != CURRENT_PROJECT_SCHEMA_VERSION {
            diagnostics.push(ValidationDiagnostic::new(
                ValidationCode::UnsupportedSchemaVersion,
                "schema_version",
                Some(self.id.to_string()),
                format!(
                    "project schema version {} is unsupported; expected {}",
                    self.schema_version, CURRENT_PROJECT_SCHEMA_VERSION
                ),
            ));
        }

        validate_unique_ids(
            self.assets.iter().map(|asset| asset.id),
            "assets",
            &mut diagnostics,
        );
        validate_unique_ids(
            self.buses.iter().map(|bus| bus.id),
            "buses",
            &mut diagnostics,
        );
        validate_unique_ids(
            self.tracks.iter().map(|track| track.id),
            "tracks",
            &mut diagnostics,
        );
        validate_unique_ids(
            self.markers.iter().map(|marker| marker.id),
            "markers",
            &mut diagnostics,
        );
        validate_unique_ids(
            self.automation.iter().map(|lane| lane.id),
            "automation",
            &mut diagnostics,
        );
        validate_unique_ids(
            self.patterns.iter().map(|pattern| pattern.id),
            "patterns",
            &mut diagnostics,
        );
        validate_unique_ids(
            self.tracks
                .iter()
                .flat_map(|track| &track.effects)
                .chain(self.buses.iter().flat_map(|bus| &bus.effects))
                .map(|effect| effect.id),
            "effects",
            &mut diagnostics,
        );
        for (lane_index, lane) in self.automation.iter().enumerate() {
            if !lane.is_ordered() {
                diagnostics.push(ValidationDiagnostic::new(
                    ValidationCode::UnorderedAutomation,
                    format!("automation[{lane_index}].points"),
                    Some(lane.id.to_string()),
                    "automation breakpoints must be in frame order",
                ));
            }
        }

        if let Some(region) = self.loop_region
            && region.end_frame <= region.start_frame
        {
            diagnostics.push(ValidationDiagnostic::new(
                ValidationCode::InvalidLoopRegion,
                "loop_region",
                None,
                format!(
                    "loop ends at frame {} which is not after its start at {}",
                    region.end_frame, region.start_frame
                ),
            ));
        }

        let asset_ids: HashSet<_> = self.assets.iter().map(|asset| asset.id).collect();
        let bus_ids: HashSet<_> = self.buses.iter().map(|bus| bus.id).collect();

        if !bus_ids.contains(&self.master_bus_id) {
            diagnostics.push(ValidationDiagnostic::new(
                ValidationCode::MissingBusReference,
                "master_bus_id",
                Some(self.master_bus_id.to_string()),
                "master bus does not exist",
            ));
        }

        for (bus_index, bus) in self.buses.iter().enumerate() {
            if let Some(output_bus_id) = bus.output_bus_id
                && !bus_ids.contains(&output_bus_id)
            {
                diagnostics.push(ValidationDiagnostic::new(
                    ValidationCode::MissingBusReference,
                    format!("buses[{bus_index}].output_bus_id"),
                    Some(bus.id.to_string()),
                    format!("output bus {output_bus_id} does not exist"),
                ));
            }
            if self.bus_route_loops(bus.id) {
                diagnostics.push(ValidationDiagnostic::new(
                    ValidationCode::BusCycle,
                    format!("buses[{bus_index}].output_bus_id"),
                    Some(bus.id.to_string()),
                    "bus routing loops back on itself",
                ));
            }
        }

        for (lane_index, lane) in self.automation.iter().enumerate() {
            let exists = match lane.target {
                AutomationTarget::Track { track_id } => {
                    self.tracks.iter().any(|track| track.id == track_id)
                }
                AutomationTarget::Bus { bus_id } => bus_ids.contains(&bus_id),
                AutomationTarget::Clip { clip_id } => self
                    .tracks
                    .iter()
                    .flat_map(|track| &track.layers)
                    .flat_map(|layer| &layer.clips)
                    .any(|clip| clip.id == clip_id),
            };
            if !exists {
                diagnostics.push(ValidationDiagnostic::new(
                    ValidationCode::MissingAutomationTarget,
                    format!("automation[{lane_index}].target"),
                    Some(lane.id.to_string()),
                    "automation lane targets something that does not exist",
                ));
            }
        }

        let mut clip_ids = HashSet::new();
        for (track_index, track) in self.tracks.iter().enumerate() {
            if !bus_ids.contains(&track.output_bus_id) {
                diagnostics.push(ValidationDiagnostic::new(
                    ValidationCode::MissingBusReference,
                    format!("tracks[{track_index}].output_bus_id"),
                    Some(track.id.to_string()),
                    format!("output bus {} does not exist", track.output_bus_id),
                ));
            }

            validate_unique_ids(
                track.layers.iter().map(|layer| layer.id),
                format!("tracks[{track_index}].layers"),
                &mut diagnostics,
            );

            for (layer_index, layer) in track.layers.iter().enumerate() {
                for (clip_index, clip) in layer.clips.iter().enumerate() {
                    let path =
                        format!("tracks[{track_index}].layers[{layer_index}].clips[{clip_index}]");

                    if !clip_ids.insert(clip.id) {
                        diagnostics.push(ValidationDiagnostic::new(
                            ValidationCode::DuplicateEntityId,
                            format!("{path}.id"),
                            Some(clip.id.to_string()),
                            "duplicate clip ID in track",
                        ));
                    }
                    if let Some(pattern_id) = clip.pattern_id
                        && !self.patterns.iter().any(|pattern| pattern.id == pattern_id)
                    {
                        diagnostics.push(ValidationDiagnostic::new(
                            ValidationCode::MissingPatternReference,
                            format!("{path}.pattern_id"),
                            Some(clip.id.to_string()),
                            format!("pattern {pattern_id} does not exist"),
                        ));
                    }
                    if !asset_ids.contains(&clip.asset_id) {
                        diagnostics.push(ValidationDiagnostic::new(
                            ValidationCode::MissingAssetReference,
                            format!("{path}.asset_id"),
                            Some(clip.id.to_string()),
                            format!("asset {} does not exist", clip.asset_id),
                        ));
                    }
                    if clip.duration_samples == 0
                        || clip
                            .start_sample
                            .checked_add(clip.duration_samples)
                            .is_none()
                        || clip
                            .source_start_sample
                            .checked_add(clip.duration_samples)
                            .is_none()
                    {
                        diagnostics.push(ValidationDiagnostic::new(
                            ValidationCode::InvalidClipRange,
                            format!("{path}.duration_samples"),
                            Some(clip.id.to_string()),
                            "clip duration must be positive and sample ranges must not overflow",
                        ));
                    }
                }
            }
        }

        diagnostics
    }
}

fn validate_unique_ids<T>(
    ids: impl IntoIterator<Item = T>,
    path: impl Into<String>,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) where
    T: Copy + Eq + std::hash::Hash + fmt::Display,
{
    let path = path.into();
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(id) {
            diagnostics.push(ValidationDiagnostic::new(
                ValidationCode::DuplicateEntityId,
                format!("{path}.id"),
                Some(id.to_string()),
                "duplicate entity ID",
            ));
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ProjectMetadata {
    pub name: String,
    #[serde(default)]
    pub properties: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Asset {
    pub id: AssetId,
    pub name: String,
    pub source: AudioAssetSource,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AudioAssetSource {
    File {
        path: String,
    },
    ManagedFile {
        path: String,
        fingerprint: String,
        sample_rate: u32,
        channels: u16,
        frame_count: u64,
    },
    Generated {
        generator_type: String,
        algorithm_version: u32,
        seed: u64,
        /// What the generator was run with. Omitted when empty, so a project
        /// written before generators took parameters round-trips byte for byte.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        parameters: BTreeMap<String, ParameterValue>,
    },
    /// A synthesizer rather than a file: the clips referencing it carry the
    /// notes, and this carries what plays them.
    Synth {
        /// Registered extension type, e.g. `builtin.oscillator`.
        type_id: String,
        /// The descriptor version these parameters were written against.
        state_version: u32,
        #[serde(default)]
        parameters: BTreeMap<String, ParameterValue>,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Track {
    pub id: TrackId,
    pub name: String,
    pub output_bus_id: BusId,
    #[serde(default)]
    pub parameters: BTreeMap<String, ParameterValue>,
    #[serde(default)]
    pub layers: Vec<Layer>,
    /// Insert effects, in the order the audio passes through them. Omitted
    /// when empty, so a project without effects is unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectInsert>,
}

/// One effect in a chain: what it is, how it is set, and how much of it is
/// heard.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EffectInsert {
    pub id: EffectId,
    /// Registered extension type, e.g. `builtin.lowpass`.
    pub type_id: String,
    /// The descriptor version these parameters were written against. A build
    /// whose extension has moved on can say so rather than guess.
    pub state_version: u32,
    #[serde(default)]
    pub parameters: BTreeMap<String, ParameterValue>,
    /// Bypassed inserts stay in the chain, in place, doing nothing.
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    /// How much of the processed signal is heard, `0.0` dry to `1.0` wet.
    #[serde(default = "fully_wet")]
    pub wet: f64,
}

const fn enabled_by_default() -> bool {
    true
}

const fn fully_wet() -> f64 {
    1.0
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    #[serde(default)]
    pub clips: Vec<Clip>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Clip {
    pub id: ClipId,
    pub asset_id: AssetId,
    pub start_sample: u64,
    pub source_start_sample: u64,
    pub duration_samples: u64,
    #[serde(default)]
    pub parameters: BTreeMap<String, ParameterValue>,
    /// What a synth clip plays. Empty for a sample clip, and omitted from the
    /// file when empty, so nothing about existing projects changes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<ClipNote>,
    /// A pattern to play instead of `notes`, repeating for the clip's length.
    /// The clip's own notes win when it has any, so a pattern can be replaced
    /// by a one-off edit without unlinking it first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern_id: Option<PatternId>,
}

impl Clip {
    /// The notes this clip plays, in clip-relative frames.
    ///
    /// Its own notes when it has any; otherwise the pattern's, repeated to fill
    /// the clip. Repeating is what makes a pattern a loop rather than a
    /// one-shot, and it happens here so playback and export cannot disagree.
    #[must_use]
    pub fn resolved_notes(&self, patterns: &[Pattern]) -> Vec<ClipNote> {
        if !self.notes.is_empty() {
            return self.notes.clone();
        }
        let Some(pattern) = self
            .pattern_id
            .and_then(|id| patterns.iter().find(|pattern| pattern.id == id))
        else {
            return Vec::new();
        };
        if pattern.length_frames == 0 || pattern.notes.is_empty() {
            return pattern.notes.clone();
        }

        let mut notes = Vec::new();
        let mut offset = 0_u64;
        while offset < self.duration_samples {
            for note in &pattern.notes {
                let start = offset.saturating_add(note.start_frame);
                if start >= self.duration_samples {
                    continue;
                }
                notes.push(ClipNote {
                    start_frame: start,
                    // A note that would run past the clip is cut at its end
                    // rather than dropped: half a note is what you hear.
                    duration_frames: note
                        .duration_frames
                        .min(self.duration_samples.saturating_sub(start)),
                    ..*note
                });
            }
            offset = offset.saturating_add(pattern.length_frames);
        }
        notes
    }
}

/// One note inside a clip. Times are frames from the clip's own start, so
/// moving a clip moves its notes with it.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct ClipNote {
    pub start_frame: u64,
    pub duration_frames: u64,
    pub pitch_hz: f64,
    pub velocity: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MixerBus {
    pub id: BusId,
    pub name: String,
    pub output_bus_id: Option<BusId>,
    #[serde(default)]
    pub parameters: BTreeMap<String, ParameterValue>,
    /// Insert effects on the bus, applied to everything routed through it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectInsert>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ParameterValue {
    Float(f64),
    Integer(i64),
    Bool(bool),
    Text(String),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationCode {
    UnsupportedSchemaVersion,
    DuplicateEntityId,
    MissingAssetReference,
    MissingBusReference,
    InvalidClipRange,
    InvalidLoopRegion,
    BusCycle,
    UnorderedAutomation,
    MissingAutomationTarget,
    MissingPatternReference,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ValidationDiagnostic {
    pub code: ValidationCode,
    pub path: String,
    pub entity_id: Option<String>,
    pub message: String,
}

impl ValidationDiagnostic {
    fn new(
        code: ValidationCode,
        path: impl Into<String>,
        entity_id: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            path: path.into(),
            entity_id,
            message: message.into(),
        }
    }
}
