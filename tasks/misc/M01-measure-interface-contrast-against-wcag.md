---
id: M01
title: Measure interface contrast against WCAG
status: done
type: task
priority: medium
started: 2026-08-19
completed: 2026-08-19
---

`docs/release-checklist.md` records that the palette was chosen by eye and never measured. Compute
the WCAG contrast ratio for every text colour against the surface it is drawn on, assert the
result in a test so a palette change cannot quietly regress it, and fix any pair that falls short.

## Acceptance

Every foreground/background pair the interface actually uses is asserted at or above its WCAG AA
threshold, and the checklist limit is replaced by evidence.

## Notes

- 2026-08-19: `src/contrast.rs` computes WCAG relative luminance and contrast ratio, checked
  against published vectors, and asserts every foreground/background pair the interface actually
  draws. One pair failed: hint text at `#5c6472` measured 2.92:1 on a panel against a 3.0:1
  threshold, so `theme::FAINT` moved to `#686f7d` — the darkest that clears AA on panel, canvas
  and raised control alike. Rules and grid lines are held to a visibility floor instead, since
  they carry structure rather than information.
