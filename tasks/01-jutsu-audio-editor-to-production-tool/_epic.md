---
id: 01
title: Build Jutsu Audio from minimal editor to extensible production tool
status: done
type: epic
priority: high
started: 2026-08-14
completed: 2026-08-19
---

Incremental Rust desktop audio editor roadmap with shared GUI/CLI core, live synchronization, deterministic generation, and extensible audio systems.

## Objective

Deliver Jutsu Audio in usable vertical slices. Each phase must preserve one shared project/audio core for GUI and CLI, deterministic serialized state, real-time-safe playback, structured machine interfaces, and extension points for synths, effects, and generators. Complete child phases in order through explicit dependencies; no implementation is part of this planning task.

## Notes

- 2026-08-19: All eight phases done. What is not finished is recorded rather than implied:
  `docs/release-checklist.md` lists the remaining limits — macOS and Linux packaging unbuilt and
  unsigned, no screen-reader or contrast audit, no automated GUI-widget test, and mixdown
  throughput printed rather than enforced.
