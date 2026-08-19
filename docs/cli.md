# Jutsu Audio structured CLI

`jutsu-audio-cli` reads exactly one JSON request from standard input and writes exactly one JSON response to standard output. Protocol version `1` is stable for the MVP. Every entity result returns explicit UUIDs; agents never need to parse human prose.

Operations: `create_project`, `inspect_project`, `import_sample`, `add_clip`, `update_clip`, `delete_clip`, `export_wav`, `transport_request`, and `session_status`. Requests use snake_case tagged JSON with `protocol_version: 1`. Inspect output provides the complete project and the default `track_id`/`layer_id` needed for clip commands. `export_wav` accepts `encoding` (`pcm16` or `float32`) plus optional `start_frame` and `frame_count`.

Exit codes:

- `0`: structured success.
- `2`: malformed request or unsupported protocol version.
- `3`: project file, asset, or WAV failure.
- `4`: shared command validation or entity failure.
- `5`: a live session or another writer refused the edit (`session_unavailable`, `project_locked`, `revision_conflict`).

All exits, including failures, return an envelope with `ok`, `protocol_version`, and either `result` or `error { code, message }`.

## Live sessions

An editor with a project open owns that project. Every mutating operation checks for it first and reports which route it took as `delivery`:

- `session` — applied by the running editor, through its command engine, visible in its window immediately.
- `offline` — applied to the file under the project write lock, with no editor running.

The route is never chosen by the caller: writing the file behind an editor that has unsaved work would lose those edits. A session file left behind by a crashed editor is detected (nothing answers its port), cleaned up, and the operation falls through to the offline route.

`session_status` reports `attached`, plus the owner's project path, name, revision and unsaved flag when one is live. `transport_request` needs a `path` and is delivered to the live editor; with no editor running it is acknowledged with `delivery: "offline"` and dropped, because nothing is playing.

The protocol behind this is `docs/design/jutsu-audio-session-protocol-v1.md`.

## Tracks, layers and the mix

`add_track` appends a track with one empty layer and returns both IDs; `add_layer` appends a lane
to a named track. `set_track_mute`, `set_track_solo` and `set_clip_pan` change how a project
sums: solo wins over mute, pan runs `-1.0` (hard left) to `1.0` (hard right), and centre is unity
in both channels.

These are the same rules playback uses. Every surface — GUI playback, GUI export, `export_wav` —
mixes through `jutsu-audio-engine`'s `mix_project`, so a muted track is silent everywhere or
nowhere.

## Editing primitives

`split_clip`, `duplicate_clip`, `slip_clip`, `set_clip_fades` and `crossfade_clips` build their
command batches with `jutsu-audio-commands::edits`, the same code the editor's buttons use. One
operation is one batch, so it is one revision and one undo step.

`delete_clip` takes an optional `ripple` flag: with it, clips later in the same lane move earlier
by the deleted clip's length; without it the gap stays. Fades are given in project frames and are
trimmed to fit the clip — a fade longer than its clip comes back shorter than it was asked for,
and the response's revision reflects what was stored.

Refused edits — splitting outside a clip, cross-fading clips that do not overlap, naming a clip
that is gone — exit `4` with `command_failed` and change nothing.

## Markers and the loop region

`add_marker`, `move_marker` and `remove_marker` keep named positions on the timeline; markers
have stable IDs and keep them when they move. `set_loop_region` takes `start_frame`, `end_frame`
and an optional `enabled` (default `true`); `clear_loop_region` forgets the region entirely,
while `enabled: false` remembers where it was without playing it.

`export_wav` accepts `use_loop_region: true`, which writes exactly the frames the loop repeats
and fails with `export_failed` when there is no active loop. The editor's own Export WAV does
the same thing, and playback wraps on the same frame, so a loop sounds the same however it is
rendered.
