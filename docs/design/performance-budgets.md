# Performance Budgets

## What is guaranteed, and what is measured

Two different things, kept apart on purpose.

**Guaranteed** properties are asserted by tests and cannot regress without one going red:

- the audio callback never allocates — `crates/jutsu-audio-engine/tests/realtime_safety.rs`
  counts every allocation on the rendering thread and fails at one;
- the callback never locks or reads a file: it holds atomics and an `ArcSwap`, and nothing in its
  path opens anything;
- block size does not change the audio, and playback matches export exactly —
  `tests/render_parity.rs`.

**Measured** numbers are for comparing a change against the build before it, on the same machine.
They are printed, not asserted: a shared CI box has no business failing a build for being busy.

```bash
cargo bench -p jutsu-audio-engine
```

## Targets

| Budget | Target | Why |
| --- | --- | --- |
| Callback allocations | **0** | Anything else can block on the allocator's lock |
| Callback work per block | well under the block period | 512 frames at 48 kHz is 10.7 ms; the verbatim path is a `copy_from_slice` |
| Mixdown throughput | **faster than real time** for a supported project | a re-mix after an edit should feel instant, not scheduled |
| Re-mix latency after an edit | ≤ 150 ms of settling (`MIX_DEBOUNCE`), 50 ms for an external edit | fast enough to feel live, slow enough not to re-mix per keystroke |
| Memory | one decoded copy per distinct source, capped by the worker's LRU cache | a project with the same sample on twenty clips holds it once |

"Supported project" for the mixdown target means roughly what the stress harness calls *16 tracks
mixed*: sixteen tracks of samples and synths, effects on the tracks, a group bus and a master
chain. On the machine this was written on that renders about fifteen times faster than real time.

## Where the time goes

The stress harness renders ten seconds of audio for a few project shapes and reports the ratio to
real time. It is deliberately unsubtle: sample tracks, synth tracks with many notes, effects on
tracks and buses. If a change makes any row noticeably worse, that is the row to look at.

Generators are re-run on every mix rather than cached — fine for one-shots, and the documented
place to look first if long ambient beds ever make a re-mix drag.

## Stress coverage in tests

Behaviour under load is covered where it can be asserted rather than timed:

- many voices past the polyphony limit (`builtin_synths.rs`, `sampler.rs`) — bounded, finite,
  deterministic;
- effects at both ends of every declared parameter range for repeated passes
  (`builtin_effects.rs`) — no NaN, nothing outside full scale;
- rapid concurrent edits from two clients against a live editor (`tests/session_workflows.rs`) —
  no lost updates;
- seeks, loop wraps and mixes published mid-playback (`realtime_safety.rs`) — still no
  allocation.
