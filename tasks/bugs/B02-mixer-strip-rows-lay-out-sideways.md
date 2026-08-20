---
id: B02
title: Mixer strip rows lay out sideways and overlap
status: done
type: bug
priority: high
started: 2026-08-19
completed: 2026-08-19
---

The mixer rack lays strips out side by side with `ui.horizontal_top`, so each strip's frame
inherited a horizontal layout. Every row inside a strip — title, meter, fader, pan, mute/solo,
routing, effect rack — was placed left to right instead of down the strip, and the labels landed
on top of each other. Reported from a screenshot: "0.0 dB" over "M", "everything ends here" over
"EFFECTS none".

## Acceptance

Strip rows read downwards, and a test fails if any two laid-out labels overlap.

## Notes

- 2026-08-19: `strip_frame` now wraps its contents in `ui.vertical`. The reason it went unnoticed
  is worth keeping: the widget test asserted every label was drawn and passed, because a sideways
  layout still draws them all. `ui_harness::Frame::overlaps` compares the boxes instead, and
  reproduces the reported picture exactly when the fix is reverted.
- 2026-08-19: Two more things the first fix exposed. The rack now scrolls in both directions, not
  just sideways: a strip grows with every insert, and with three effects it ran 338px down a
  268px panel, putting the fourth insert out of reach. And the harness hosts the rack in a
  fixed-height bottom panel like the real one — measured in a full-height panel, a strip that
  does not fit looks fine. `Frame::fits_in_bottom_panel` asserts both mixer cases.
