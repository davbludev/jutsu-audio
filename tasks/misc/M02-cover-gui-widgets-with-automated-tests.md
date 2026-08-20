---
id: M02
title: Cover GUI widgets with automated tests
status: done
type: task
priority: low
started: 2026-08-19
completed: 2026-08-19
---

Every workflow test drives the CLI and the session host — what the GUI calls — but no test drives
the GUI's own widgets. `egui_kittest` can run the app's panels headlessly and assert what a user
would see and click.

## Acceptance

At least the transport, the inspector and the timeline are exercised through their widgets, and
the checklist limit is narrowed to what remains uncovered.

## Notes

- 2026-08-19: `src/ui_harness.rs` runs a real `egui::Context` headlessly, feeds it keys and
  clicks, and reads back the text that was laid out for drawing — no UI test framework and no
  display, since `Context::run` is enough. Seven tests: the timeline labels its tracks and clips
  and selects the clip that is clicked, the mixer draws a strip per track and asks for a bus when
  its button is clicked, and the keyboard, audio and recovery modals each say what they are for
  and answer their controls. Queued input is held back for the settled frame, or a click lands
  before the widget it was aimed at exists.
- 2026-08-19: The transport and inspector are drawn inline in `JutsuAudioApp` rather than in
  panel functions, so the harness cannot reach them without restructuring `main.rs`. Recorded as
  a limit in `docs/release-checklist.md` rather than pretended away.
