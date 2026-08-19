---
id: B01
title: Prevent timeline clip selection from blocking UI
status: done
type: bug
priority: high
started: 2026-08-14
completed: 2026-08-14
---

Decode and build playback snapshots off eframe UI thread so selection remains responsive.

## Objective

Move timeline clip WAV decoding, gain processing, and playback snapshot construction to one background worker. Selection updates immediately, stops/clears prior audio, and Play reports loading until matching snapshot arrives. Discard stale queued/results.

## Acceptance

Timeline click path performs no synchronous WAV decode; deterministic tests cover stale/current/error result handling; cargo quality passes.

## Notes

- 2026-08-14 — Implemented one stdlib-channel playback worker in src/main.rs. Timeline selection now stops transport, clears prior snapshot, queues owned decode/build request, and immediately shows loading; Play remains stopped while loading. Worker drains queued stale requests when idle, builds snapshot off the eframe thread, requests repaint, and UI publishes only current request ID. Added deterministic regression tests for stale/current/error routing and loading-state completion. TDD evidence: both new tests first failed with missing symbols, then passed. Validation: `cargo test --bin jutsu-audio` passed 6 tests; `cargo quality` passed fmt, Clippy -D warnings, all workspace tests, and bench compilation; final `git diff --check` passed.
