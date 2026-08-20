---
id: 02
title: Raise the engine to production-grade sound design
status: done
type: epic
priority: high
started: 2026-08-19
completed: 2026-08-19
---

The first epic proved the contracts end to end with the smallest sound-making parts that could
prove them: one oscillator with a linear attack and release, five single-knob effects, gain and
pan automation, one output per strip. Arrangement can carry that a long way, and did. It cannot
carry it further.

## Objective

Make the difference audible in the timbre rather than only in the arrangement: a synth whose
tone moves while a note is held, effect parameters that can be automated, dynamics that respond
to another strip, and the production surfaces a game audio pipeline is delivered through —
loudness figures, stems, loop points that survive export.

Every addition goes through the existing contracts: extensions stay typed and versioned,
project state stays migratable, playback and offline export stay identical, and the audio
callback stays free of allocation and locks.

## Notes

- 2026-08-19: All five phases done. What the engine could not do at the start of the day and can
  now: move a parameter while a note is held, automate an effect, duck one strip under another,
  send a copy of a signal somewhere without moving it, convolve against a real space, measure its
  own loudness, and hand a pipeline stems, loop points and variation sets. The limits that remain
  are written into `docs/release-checklist.md` beside the evidence rather than left in prose.
