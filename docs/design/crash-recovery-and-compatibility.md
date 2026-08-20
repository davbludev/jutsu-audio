# Crash Recovery and Project Compatibility

One rule underneath all of this: **nothing is lost silently.** Either the work is still there,
or the tool says out loud what it could not do. A project that opens wrong is a bug; a project
that opens *slightly* wrong without saying so is a worse one.

`tests/fault_injection.rs` breaks something in each of these areas and asserts the rule holds.

## Writing

Every project write is atomic — `atomic_write` writes a temporary file beside the target and
renames it. A write that dies partway leaves the previous file intact and no temporary behind.
Validation runs *before* the write, so an invalid project is refused rather than half-saved.

## Unsaved work

The editor parks unsaved state in a `.autosave` sidecar and keeps the generation before it in
`.autosave.1`:

| File | What it holds |
| --- | --- |
| `song.jutsu-audio.json` | the last explicit save |
| `song.jutsu-audio.json.autosave` | the newest parked state |
| `song.jutsu-audio.json.autosave.1` | the one before it |

Atomicity already prevents a torn autosave. The second generation covers the other case: a
*complete* autosave of state the user would not want back, or a sidecar damaged after the fact.
`autosave::recover` prefers the newest readable generation, so a broken one costs one edit
rather than the session. A successful save, or a declined recovery, clears both.

Recovery never rewrites the saved file. Until the user decides, the file on disk is exactly what
they last saved.

## Schema versions

- **Older than this build** — migrated in memory, then written back, with the file as it arrived
  kept beside it as `…backup.v<version>`. The backup is the original bytes, not the migrated
  result.
- **Newer than this build** — refused, with a message naming both versions, and the file is left
  byte for byte as it was. An old build must never rewrite a project a newer one wrote.
- **A version with no migration path** — the same: refused, untouched.

Migration writes only after the migrated project parses *and* validates. A failure at any step
returns before anything is written.

## Extensions this build does not have

Synths, effects and generators are stored by type ID, state version and opaque parameters. A
build with none of a vendor's extensions installed still round-trips them exactly: the project
model carries them through load, edit and save untouched, and
`report::collect` lists every referenced type ID so a user can find out what they are missing.

Rendering degrades instead of failing — a missing effect passes audio through, a missing synth
plays silence — and each degradation produces a `MixDiagnostic` naming the entity. Export
returns those diagnostics in its response.

## Damaged audio

A clip whose source will not read or decode plays silence, and the mix carries a
`SourceUnreadable` diagnostic naming the clip. One damaged sample cannot silence a session or
fail an export. `check_assets` lists the same files as needing attention, with the fingerprint
mismatch or the missing path.

## Diagnostic bundles

```bash
echo '{"protocol_version":1,"operation":"diagnose","path":"song.jutsu-audio.json","destination":"report/"}' | jutsu-audio-cli
```

`report::collect` gathers what a bug report needs: declared and supported schema versions,
whether the project opens and why not, validation diagnostics, per-asset presence, size,
fingerprint match and decode errors, referenced extension type IDs, and what recovery material
is on disk. It works on a project that will not open — which is when it is needed — and it never
writes to the project it is reporting on. With a `destination`, it also drops a copy of the file
as found beside the report.
