---
id: M03
title: Build and smoke-test macOS and Linux releases
status: todo
type: chore
priority: medium
---

`docs/release.md` declares macOS and Linux targets that have never been built. There is no
cross-compilation step — the audio backend binds to the host sound API — so each needs
`cargo package-release` and `cargo smoke` run on that machine, plus the by-hand items in
`docs/release.md`.

Blocked on access to those machines; nothing in this repository is missing.

## Acceptance

A packaged release per declared target, smoke-checked on its own platform, with the checklist
updated to record it.
