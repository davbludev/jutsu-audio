# Jutsu Audio Project Schema v1

## Scope

Schema v1 defines portable domain state shared by GUI, CLI, persistence, command processing, playback compilation, and offline export. Implementation lives in `jutsu-audio-model`. Presentation state and runtime DSP state are excluded.

## Version and identity

- `schema_version` is explicit; current value is `1`.
- Project, asset, track, layer, clip, and mixer-bus IDs are strongly typed UUID newtypes.
- IDs survive serialization round trips and remain stable across edits, ordering changes, CLI/GUI synchronization, save/open, and migrations.
- IDs are generated only for new entities. Copy/duplicate commands must create new IDs.
- Entity references store IDs, never collection indexes or display names.

## Project aggregate

A project contains:

- project ID and metadata;
- audio assets;
- mixer buses and one master-bus reference;
- ordered tracks;
- ordered layers within tracks;
- ordered clips within layers.

Metadata contains human name plus deterministic string properties. Extension-specific structured state will use separately versioned extension contracts rather than untyped project metadata.

## Assets

Each asset has stable ID, display name, and source descriptor. Schema v1 supports file sources and deterministic generated sources containing generator type, algorithm version, and seed. Decoded audio, hashes, waveform peaks, absolute cache locations, and open file handles are derived/runtime state.

## Tracks, layers, and clips

Tracks route to a mixer-bus ID and contain layers. Layers contain clips. A clip references an asset ID and stores project start sample, source start sample, positive duration, and typed parameter values.

Sample positions use unsigned 64-bit integers. Validation rejects zero-duration clips and ranges that overflow. Later musical time remains a view/conversion layer over canonical sample positions until tempo-map task extends schema deliberately.

## Routing and parameters

Mixer buses have stable IDs, optional output-bus references, and typed parameters. Project stores master-bus ID. Tracks route by bus ID.

Parameter values are tagged values: float, integer, Boolean, or text. Parameter keys are stable strings. Parameter descriptors, bounds, units, and extension ownership belong to extension/parameter contracts; schema stores values without embedding GUI controls.

## Structured validation

Validation returns a list of diagnostics, never only a Boolean or formatted error. Each diagnostic contains:

- stable machine code;
- precise field path;
- optional entity ID;
- human-readable message.

Schema v1 codes cover unsupported schema version, duplicate entity ID, missing asset reference, missing bus reference, and invalid clip range.

Validation is side-effect free and reports all discovered issues in one pass. Invalid user/project data does not panic.

## Required invariants

- schema version equals a supported version before normal editing;
- IDs are unique within their typed project-wide entity collection;
- every clip asset reference resolves;
- every track/bus output reference resolves;
- master-bus reference resolves;
- clip duration is positive;
- project and source sample ranges do not overflow;
- JSON serialization round trip preserves complete project value and identity.

## Deferred schema work

File-format framing, migration orchestration, unknown-field preservation, asset fingerprints, routing-cycle checks, automation, effect state, synth events, tempo maps, presets, and extension state belong to their scheduled tasks. Add them through explicit schema versions and migrations, not ad hoc fields.

## Markers and the loop region (additive)

`markers` and `loop_region` are optional members of the project object. Both are omitted when
unused, so a project without either serializes exactly as it did before they existed and older
files load unchanged — no schema version bump.

A marker is `{ id, name, frame }`, where `frame` is a project frame. IDs are stable across moves.

`loop_region` is `{ start_frame, end_frame, enabled }`, half-open: `start_frame` plays,
`end_frame` does not. Validation rejects a region whose end is not after its start. `enabled`
exists so switching looping off does not lose where the loop was.

## Automation (additive)

`automation` is an optional list of lanes, omitted when empty. A lane is:

```json
{
  "id": "…",
  "target": { "type": "track", "track_id": "…" },
  "parameter": "gain_db",
  "points": [ { "frame": 0, "value": -60.0, "curve": "linear" } ]
}
```

`target` is `track`, `bus` or `clip`; `parameter` is the parameter ID the lane writes. Points are
in frame order — validation rejects a lane that is not, and the command that sets points sorts
them rather than making a caller do it.

A value is held before the first point, interpolated to the next (`linear`) or held until it
(`step`), and held after the last. A lane with no points is inert: the stored parameter value
still stands. A lane whose target no longer exists is a validation error, not silence.

## Tempo (additive)

`tempo` is an optional list of changes, omitted when empty. Each is
`{ frame, beats_per_minute, beats_per_bar, beat_unit }`. An absent or empty list means 120 BPM in
4/4 — a project that never mentions tempo still has one, and never needs to write it down.

Conversions live in `jutsu-audio-model::tempo`: frames to beats, beats to frames, frames to
`bar.beat.tick` and back. Ticks are 960 per beat, which divides cleanly for triplets and
sixteenths.
