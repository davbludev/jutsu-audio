---
id: M02
title: Cover GUI widgets with automated tests
status: todo
type: task
priority: low
---

Every workflow test drives the CLI and the session host — what the GUI calls — but no test drives
the GUI's own widgets. `egui_kittest` can run the app's panels headlessly and assert what a user
would see and click.

## Acceptance

At least the transport, the inspector and the timeline are exercised through their widgets, and
the checklist limit is narrowed to what remains uncovered.
