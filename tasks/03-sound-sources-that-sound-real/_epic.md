---
id: 03
title: Sound sources that sound real
status: done
type: epic
priority: high
---

Every `sfx.*` generator in this build is made of exactly two ingredients: white noise through a
**one-pole** low-pass, and a **single sine** at a fixed frequency. `crates/jutsu-audio-extensions/src/generators/dsp.rs`
is the whole toolbox — `LowPass`, `decay`, `sine`, `advance_phase`, `lerp`, `normalise`,
`seeded_noise` — and every generator is a few lines combining them.

That is why a listener with no audio vocabulary described three of the eight demos as
"just white noise" and the weapon layer as "someone poking a broken drum". Both descriptions are
accurate, not a misunderstanding:

- A one-pole filter rolls off at 6 dB per octave. Noise through it is still broadband noise, so
  `sfx.ambience` and the blast half of `sfx.explosion` read as hiss whatever the parameters say.
- `sfx.impact` has a body at a fixed `body_hz` (60–180 Hz). Real impacts drop in pitch as they
  decay; a fixed sine plus a noise click is exactly the sound of a struck drum head with no
  tuning.
- Nothing anywhere has a transient shaper, saturation, band-limited noise, more than one body
  partial, or an amplitude envelope beyond a single exponential.

The tonal side of the build does not have this problem — `builtin.subtractive` has envelopes, a
state-variable filter, unison and a filter envelope, and the demos built on it were judged
plausible. The gap is entirely in the sample-based sound sources.

## What this epic has to deliver

A game cue built from these generators should be able to read as a *serious* game, not a
stylised arcade one. Concretely, the missing pieces:

- **Pitch envelopes** on any body partial — the fall is what makes weight.
- **A real filter** in the generator toolbox: multi-pole with resonance, the same
  topology-preserving SVF the subtractive synth already uses, rather than a second one-pole.
- **Band-limited and multi-band noise**, so a noise layer can occupy a register instead of all of
  them.
- **Transient shaping** — an attack stage separate from the decay, so a hit has a front.
- **Saturation inside a generator**, which is most of what makes a gunshot read as loud rather
  than as a click.
- **More than one partial** per body, detunable, so metal rings and wood does not.

## What was built

The toolbox first, then every generator against it — small enough to land as one change rather
than as phases.

- `crates/jutsu-audio-extensions/src/filter.rs` — the SVF, extracted from `subtractive.rs` where
  it was private, widened to low/band/high with resonance, and covered by its own frequency
  response tests. The synth now shares it rather than carrying a second copy.
- `generators/dsp.rs` — `envelope` (attack plus decay), `glide` (interpolation in octaves rather
  than hertz), `BandNoise` (noise with a register, level-compensated for bandwidth so a low band
  is not quietly missing), `Partials` (several partials at chosen ratios), `saturate`.
- `sfx.impact` — pitch drop, attack, tone, ring, drive. Covers a kick, a snare, a gunshot body,
  a slam and a metal clang; presets for each.
- `sfx.explosion` — a fixed six-millisecond crack added *after* the drive so saturation cannot
  flatten it, a roar sweeping down through a real filter, a rumble that outlasts both, and a
  distance control with a slope steep enough to be heard.
- `sfx.laser` — harmonics, a sub body that outlasts the sweep, a noise front, and drive. The
  "sounds like a whistle" complaint is what `body` and `harmonics` exist to answer.
- `sfx.ambience` — three independently drifting bands instead of one filtered noise stream, with
  `depth` and `focus`. A dark bed now crosses zero under 3 000 times a second against white
  noise's 24 000, which is the difference between a place and a hiss.
- `sfx.pickup` — harmonics, a sparkle layer and a tail.

## Notes

- 2026-08-20 — built and closed. Four tests failed on the first run and two of them were finding
  real defects rather than bad thresholds: the upper partials of an impact ignored the attack
  envelope (so a slow attack still clicked), and `ring` shortened the upper partials instead of
  lengthening them (so metal could never ring). A third measurement problem was instructive in
  itself: a crack is a peak event, not an energy event, and against a two-second roar it cannot
  be measured any other way.
- 2026-08-19 — filed after the eight-piece contrast sheet in
  `%USERPROFILE%\Documents\Jutsu Audio\lab.jutsu-audio.json` was reviewed by ear. Verdicts:
  combat read as arcade rather than serious; horror worked only with the ambience layer muted;
  arcade read as "just beeps"; cinematic was plausible but situational; nature and the weapon set
  read as white noise; UI sounds read as arcade-only. The one consistent line through all of it
  is that everything built on `sfx.*` failed and everything built on `builtin.subtractive`
  did not.
