# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo quality                  # required gate: fmt --check, clippy -D warnings, workspace tests, bench compile
cargo run                      # eframe desktop GUI
cargo run --bin jutsu-audio-cli < request.json    # structured CLI: one JSON in, one JSON out
```

`cargo quality` is a `.cargo/config.toml` alias for `cargo run -p xtask -- quality`; the four
steps live in `xtask/src/lib.rs:quality_steps`. Run the whole gate before completing a task or
making a commit — focused tests support red/green work but never replace it.

Focused runs while iterating:

```bash
cargo test -p jutsu-audio-engine --test playback        # one integration test file
cargo test -p jutsu-audio-model golden                  # one test by name filter
cargo test --bin jutsu-audio                            # GUI-crate unit tests in src/main.rs
cargo test --test cli_protocol                          # root-package CLI protocol tests
```

## Architecture

A Cargo workspace of layered library crates plus two thin application surfaces. Dependencies
point one direction only: `model` is the leaf, `commands` / `project` / `engine` build on it,
and the GUI and CLI sit on top of all of them. `docs/design/jutsu-audio-core-architecture.md`
records why.

- `crates/jutsu-audio-model` — portable project aggregate, typed stable IDs, parameter values,
  validation diagnostics. Serialization format is versioned; see
  `docs/design/jutsu-audio-project-schema-v1.md`.
- `crates/jutsu-audio-commands` — the **only** legal way to mutate a project. Every edit is a
  versioned `CommandEnvelope` applied atomically against an expected revision, producing a
  single revision increment and ordered change events, rolling back on validation failure.
  Contract: `docs/design/jutsu-audio-command-contract-v1.md`.
- `crates/jutsu-audio-project` — atomic save/load, schema migrations, WAV asset import,
  waveform cache, portable relative paths. `fixtures/projects` holds versioned golden projects
  that migration tests replay.
- `crates/jutsu-audio-engine` — immutable `PlaybackSnapshot` render state, lock-free transport,
  system output, and `OfflineExporter`. Real-time playback and offline export must produce
  identical audio; `crates/jutsu-audio-engine/tests/offline_export.rs` asserts that parity.
- `crates/jutsu-audio-extensions` — typed synth / effect / generator registries with versioned
  descriptors, resolved at compile time (no dynamic-library ABI commitment).
  Contract: `docs/design/audio-extension-and-render-snapshot-contracts-v1.md`.
- `src/main.rs` — the eframe desktop shell (timeline, inspector, transport). Heavy work such as
  WAV decode and snapshot building runs on a background worker over stdlib channels; the UI
  thread only publishes results whose request ID is still current.
- `src/cli.rs` + `src/bin/jutsu-audio-cli.rs` — machine-facing surface. Exactly one JSON request
  on stdin, one JSON response on stdout, `protocol_version: 1`, explicit UUIDs in every result.
  Operations and exit codes: `docs/cli.md`.

### Invariants

- Never mutate project state outside the command engine — not in the GUI, not in the CLI.
- Keep the audio callback free of blocking, allocation, I/O, logging, and project mutation.
- Keep GUI and CLI thin; shared behavior belongs in a crate both can call.
- Tests use fixed IDs, seeds, sample rates, and explicit tolerances — never wall-clock time,
  hash order, device enumeration, or OS entropy. `docs/quality.md` has the full policy.
- Commit or push only when explicitly asked.

## Tasks

`tasks/` is the only task tracker. `tasks/README.md` holds the full conventions; the rules
that matter every time:

- One directory per epic. The dotted zero-padded `id` is also the filename prefix, so `ls`
  sorts a directory back into its tree: `tasks/01-jutsu-audio-editor-to-production-tool/01.02.01-….md`.
- A bug that belongs to a feature stays with that feature. `tasks/bugs/` is only for bugs with
  no epic home, and is where new bug reports go. `tasks/misc/` holds epic-less chores.
- IDs never change. Renaming a slug is fine; renumbering is not.
- Status is the `status:` frontmatter line (`todo` / `doing` / `done`) and nothing else.
  Never record status a second time anywhere, and never generate an index of tasks.
- Find work with `grep -rl "^status: doing" tasks/`, not by reading files.
- Editing a task: edit only the lines that change. Never rewrite a whole task file.
- Progress: append one dated bullet under `## Notes`. Never rewrite `## Notes`.
- New task: copy the frontmatter block, take the next free number under its parent.
