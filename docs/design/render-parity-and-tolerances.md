# Render Parity and Tolerances

## The promise

What you hear is what you export. A project mixed once produces one snapshot, and both the audio
device and the offline exporter read that same snapshot — there is no second render path that
could drift from the first.

Proven by `crates/jutsu-audio-engine/tests/render_parity.rs`, which builds a reference project
spanning every path that can move audio (two tracks through a group bus, a sample clip, a synth
clip, a generated clip, insert effects on a track and a bus, and automation on a level) and
compares the device path against the exported file.

## Tolerances

| Case | Tolerance |
| --- | --- |
| Device format matches the mix (same rate, same channel count) | **exact** — bit-identical samples |
| Device rate or channel count differs | conversion applies; not compared to an export, and not used as an export path |
| Two mixes of the same project | **exact** — synths, generators and effects are all deterministic |
| Different device buffer sizes | **exact** — block size never changes the audio |

The verbatim path is what makes the first row exact: when the snapshot's format already matches
the device, playback copies rather than converts. An export always writes the snapshot's own
format, so an export is always on that path.

Conversion — resampling, channel folding — happens only on the way to a device that cannot take
the mix as it is. It is a playback convenience, never an export path, so it has no parity
obligation beyond sounding right.

## Latency and tail

`MixOutput::timing` reports two numbers:

- `latency_frames` — how much the effect chains delay the signal, summed along the path.
- `tail_frames` — how long the longest tail outlives its input.

Both are **reported, not compensated**. An offline render lays the whole timeline out at once, so
there is nothing to align; a caller synchronising against live playback needs the number rather
than a silent shift.

## What an export contains

An export covers the timeline: from frame zero (or the requested range) to the end of the last
clip. It does **not** extend to let a reverb or delay tail ring out past that point — a project
that ends on a decaying tail would otherwise have a length that depends on its effects.

To capture a tail, extend the timeline: lengthen the last clip, add silence, or set a loop region
that includes the space you want. `export_wav` with `use_loop_region` writes exactly the loop.

## Diagnostics

A mix that could not do everything asked of it still renders, and says what it did instead
through `MixOutput::diagnostics` — a missing effect passed audio through, a state version had
moved on, parameters were refused. Parity is about the audio that *was* produced; a diagnostic
explains any audio that was not.
