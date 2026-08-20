---
id: M06
title: sfx.object presets are not confirmed by ear
status: todo
type: task
priority: medium
reported: 2026-08-20
---

`sfx.object` was added after a listener named eight deliberately different struck sounds — a drip,
a knock, metal, glass, stone, a creak, a coin, a cardboard box — as variations of one tuned
instrument. The cause was found in the code and is not in doubt: every generator in the crate
builds sound from three sine partials or from filtered noise, and three partials fuse into a
single perceived pitch.

The primitive that replaced it (a bank of 12–36 scattered resonances, struck by contact noise,
with a direct path for the contact and a stick-slip mode for friction) is covered by unit tests.
**The preset values are not.** They have not yet been through a listener, and the first round of
them was wrong in ways the tests could not see.

## What the listener said about the first round

| Asked for | Heard as |
|---|---|
| Water drip | close, but not quite |
| Knock on wood | too much like the drip |
| Metal | more like glass |
| Glass | a child's xylophone |
| Stone | good — could become an axe into wood if pushed |
| Creak | not a creak at all |
| Coin | "a pattern", could not name it |
| Cardboard box | light taps on thin wood |

Two structural causes came out of that and are fixed: the contact was inaudible (everything went
through the resonators), and a creak is friction, not a strike. What is still open is whether the
new preset values land.

## Next

- Play the presets to a listener and record what each is *named*, not whether it is liked.
  A wrong name is the useful signal; "I like it" is not.
- Expect `contact` and `material` to move first.
- A reference recording of a real drip and a real knock would settle the drip/knock confusion
  quickly — the same measurement loop as `tasks/misc/M05-…`, which worked for narrowing even
  where it did not finally convince.

## Notes

- 2026-08-20 — opened alongside the generator. `analyse.py` in the `jutsu-audio` skill now prints
  a heard-band repeat score, which is the number that separates "an object" from "a note".
