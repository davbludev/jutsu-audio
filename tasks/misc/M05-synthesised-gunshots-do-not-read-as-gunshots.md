---
id: M05
title: Synthesised gunshots do not read as gunshots
status: todo
type: task
priority: medium
reported: 2026-08-20
---

Parked deliberately after four rounds. Everything measurable was matched and the result still did
not convince a listener, so the next attempt should start from a different idea rather than from
more tuning.

## What was tried

A reference recording (`UltimateGunshoots`, assault rifle, outdoors) was measured with
`analyse.py` and the synthesis was tuned until the profiles agreed:

| | Reference | Synthesised |
|---|---|---|
| RMS 1–3 ms | −0.9 dB | −1.9 dB |
| RMS 10–30 ms | −2.6 dB | −8.2 dB |
| RMS 100–300 ms | −18.9 dB | −15.8 dB |
| RMS 300–1000 ms | −30.2 dB | −29.0 dB |
| Loudest sample | 0.82 ms | 0.42 ms |
| Decay to −20 dB | 770 ms | ~800 ms |
| Front, octave bands | — | within 3–8 dB across the range |
| Distinct return | 155 ms, −3 dB | 155 ms, −2 dB |

Structurally the attempt was: a broadband front with a plateau, a ground bounce 21 ms later, a
large outdoor reverb with the sub filtered out of it, and one discrete return at 155 ms.

The verdict stayed "not a gunshot", with two specific notes: the reference's crack is clearer,
and the synthesised version has too much room.

## What the failure says

Octave bands and an RMS envelope are too coarse a description. What they cannot see:

- the fine spectral shape inside the first two or three milliseconds;
- the impulse structure of a real outdoor space — hundreds of discrete early arrivals off ground,
  walls and objects, not a diffuse algorithmic tail;
- whatever the ear uses to tell a pressure wave from a burst of filtered noise.

## Where to look next

- **Convolution against a real impulse response** is already in the build
  (`builtin.convolution`). A shot placed in a *recorded* space rather than an algorithmic one is
  the cheapest test of the "the room is the problem" half of this.
- **A source model closer to the physics** — a pressure N-wave rather than a filtered noise
  burst — is the other half.
- Before either, run `analyse.py` on the reference with a much finer time resolution than octave
  bands: the answer is somewhere the current measurement does not look.

## Notes

- 2026-08-20 — parked at the user's request: "давай пока сделаем так, в скилл запиши что выстрелы
  хорошими не будут, с этим потом разберёмся". The engine work done along the way stands on its
  own — see epic 03 and the `sfx.explosion` parameters `roar`, `crack_ms` and the crack plateau,
  all of which came out of this and are correct improvements regardless.
