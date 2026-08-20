---
id: B04
title: Recovery is offered for an autosave older than the project, and accepting it destroys newer work
status: todo
type: bug
priority: high
reported: 2026-08-20
---

## What happens

`autosave::recover` in `crates/jutsu-audio-project/src/autosave.rs` loads the recovery sidecar
whenever one exists. Nothing compares it against the project file it sits beside — not the
revision, not the modification time. So an autosave left behind by an earlier session is offered
as "recovery" even when the project on disk is newer, and accepting it silently replaces newer
work with older.

The prompt does not say what the recovery file contains or what accepting it would replace, so
there is no way for the person answering it to tell which side is newer.

## How it was hit

2026-08-20, working on `%USERPROFILE%\Documents\Jutsu Audio\lab.jutsu-audio.json`:

1. The editor was killed (routine during this work) with an autosave beside the project.
2. The CLI rebuilt the project offline — a completely different piece of audio, 19 156 bytes,
   written at 00:27.
3. The stale autosave from the previous session was still there: 116 946 bytes, 00:23.
4. Opening the editor offered recovery. Accepting it restored the **older** file, and the work
   from step 2 was gone.

## Why it matters

Every kill of the editor leaves an autosave behind — that is what it is for. Any edit made
afterwards through the CLI, which is the normal way an agent works on a project, is then one
misread dialog away from being discarded. The failure is silent and the dialog gives the user
nothing to decide with.

## Where to look

- `crates/jutsu-audio-project/src/autosave.rs` — `recover` returns the sidecar unconditionally;
  it has both paths available and could compare revisions.
- `src/recovery.rs` and `src/main.rs` around `Recovery` / `Decision` — what the prompt says and
  what each answer does.
- `src/worker.rs:220` — the other caller of `recover`.

## Suggested shape

- Only offer recovery when the autosave's `revision` is **ahead** of the project file's, or when
  the project file cannot be read at all. An autosave at or behind the project's revision has
  nothing in it worth recovering; discard it silently.
- Tell the user what they are choosing between — project name, revision and timestamp on both
  sides — so the dialog is answerable.
- Consider making the choice reversible: keep the replaced file as a further sidecar rather than
  overwriting it.

## Notes

- 2026-08-20 — filed after it cost a listening round. The generation-before-this-one file
  (`previous_autosave_path`) already exists for a related reason: protecting against a complete
  write of state the user would not want back. This is the same class of problem one step out.
