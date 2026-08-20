# Project index

Where to start looking. Pointers only — verify before relying on an entry, and fix it here if
it has moved.

## Desktop GUI (`src/`)

- `src/main.rs` — app state and chrome (top bar, library, inspector, status bar with the
  transport). `JutsuAudioApp::apply` is the *only* path into the command engine; it never
  touches the disk, it stamps `save_due` / `mix_due` deadlines that `dispatch_pending_work`
  fires once editing settles.
- `src/worker.rs` — every disk read, decode, mixdown and file dialog. One thread, `Job` in,
  `JobResult` out. `mixdown` sums the whole timeline into the snapshot that *both* playback and
  export consume. Holds an LRU cache of decoded files, so re-mixing after an edit is a memcpy.
  File dialogs are `rfd::AsyncFileDialog` + `pollster::block_on`, because the sync API cannot be
  driven off the main thread portably.
- `src/timeline.rs` — timeline view state, painting and hit-testing. Returns `TimelineAction`
  rather than mutating, so edits still funnel through `main.rs`. Also home to
  `project_sample_rate` / `project_duration_frames`, which the rest of the GUI counts in.
- `src/session_host.rs` — the GUI's side of the session protocol. Answers CLI requests from the
  same `ProjectCommandEngine` the user's edits go through, and returns `ExternalEffect`s that
  `main.rs::poll_session` folds into dirty state, mixdown and transport. Started/stopped by
  `main.rs::sync_session` whenever `project_path` changes.
- `src/theme.rs` — Console palette, egui style, and shared drawing helpers (peak meter, time
  formatting, elision). Change colours here, nowhere else.
- `src/session_host.rs` — the editor's side of the session protocol, and the only place the GUI
  answers external requests. Lives in `src/lib.rs` (not the binary) so
  `tests/session_workflows.rs` can stand up a real editor against a real socket.
- `src/extensions.rs` — the one set of registries both surfaces share, built once on first use.
- `src/mixer_panel.rs` — the mixer strips, meters, routing and effect racks. Reports actions;
  `main.rs::apply_mixer_action` is what turns them into commands.
- `src/cli_mixer.rs` — the same surface for the CLI, validating through `parameters`.
- `crates/jutsu-audio-project::bundle` — packing a project for another machine, and relinking
  moved audio by fingerprint.
- `crates/jutsu-audio-project::presets` — the user preset library (one file per preset) and the
  compatibility report; `src/cli_presets.rs` is its machine surface.
- `examples/pocket-extensions` — the third-party extension example, a workspace member outside
  `crates/`. Depends only on the published extension surface; its tests are the conformance run
  a pack author writes. Rules: `docs/extension-sdk.md`.
- `crates/jutsu-audio-extensions::conformance` — the checks any extension runs, built-ins
  included (`crates/jutsu-audio-extensions/tests/conformance.rs`).
- `xtask/src/package.rs` + `smoke.rs` — `cargo package-release` builds a release directory
  (binaries, docs, generated INSTALL/notices/SHA256SUMS); `cargo smoke <dir>` runs the packaged
  binaries like a user would. Process and platform list: `docs/release.md`.
- `src/audio_setup.rs` — the first-run "no output device" notice, with a retry that reopens the
  default device. Wired in `main.rs::audio_prompt`.
- `src/cli_batch.rs` — `describe_protocol` (the operation table a test pins against serde's
  accepted variants) and `batch` (rollback by restoring the project bytes; dry run, JSONL
  progress on stderr, timeout as cancellation).
- `src/cli_generator.rs` — generator discovery, recipe validation, preview rendering and the
  derived-ID rules `run_generator` uses.
- `src/cli_synth.rs` — registry-backed validation and discovery for the CLI; `src/synth_panel.rs`
  is the same idea for the inspector, building widgets from the descriptor.
- `src/cli.rs` + `src/bin/jutsu-audio-cli.rs` — the machine surface. Reached through
  `src/lib.rs`, which exists only to expose `cli` to the binary and to `tests/cli_protocol.rs`.
  Request variants are tagged with `"operation"`, not `"type"`.
- `src/cli_session.rs` — the only place that decides between editing through a live editor and
  editing the file under the write lock. Every mutating CLI operation goes through
  `cli_session::apply`; the `delivery` field in the response says which route it took.

Running the GUI against a real project: `cargo run -- path/to/project.jutsu-audio.json`.

## Crates

- `crates/jutsu-audio-model` — the aggregate and `Project::validate`. Leaf crate; nothing else
  may be depended on from here.
- `crates/jutsu-audio-commands` — `ProjectCommandEngine::apply` clones the project, applies the
  batch, validates, and only then swaps it in. That clone is what gives rollback.
- `crates/jutsu-audio-commands::edits` — timeline editing primitives (split, duplicate, ripple
  delete, slip, fades, cross-fade, paste). Each returns one batch, which is one undo step. The
  GUI and the CLI both build their edits here; neither assembles commands by hand.
- `crates/jutsu-audio-project` — save/load/migrate, WAV import, and the waveform peak cache.
  The cache lives at `<project dir>/.jutsu-audio-cache/waveforms/<fingerprint>.json`; reach it
  through `AssetManager::waveform_cache_path` / `load_waveform` / `rebuild_waveform` rather than
  rebuilding the path. Its shape and zoom levels are `src/waveform.rs`: `level_for` picks the
  window to draw at, and `load_waveform` refuses a cache from an older format so it is rebuilt.
- `crates/jutsu-audio-engine::mixdown` — the only place a project becomes audio. `mix_project`
  sums tracks/layers/clips with mute, solo, per-clip gain and pan, resampling sources onto the
  project rate. The GUI worker and the CLI export both call it; nothing else should sum.
- `crates/jutsu-audio-engine` — transport, snapshot exchange, `PlaybackRenderer`,
  `OfflineExporter`. The renderer is built by `SystemAudioOutput::open_default` from the device's
  own format and converts the snapshot onto it; when the formats already match it takes a
  verbatim-copy path, which is what keeps real-time output bit-identical to offline export
  (`tests/offline_export.rs` asserts that).
- `crates/jutsu-audio-commands` also holds `history.rs`: `CommandHistory::apply` records a
  batch's inverse, and undo/redo replay through the engine. GUI and session edits share one
  stack, so undo reverses whatever happened last whoever did it.
- `crates/jutsu-audio-project::autosave` — the `.autosave` sidecar plus the generation before it
  (`.autosave.1`). Written by the GUI worker, removed by a successful save, offered at open
  through `src/recovery.rs`; `recover` falls back to the older generation when the newest will
  not read.
- `crates/jutsu-audio-project::report` — the diagnostic report and bundle behind the CLI's
  `diagnose`. Reads the project file directly rather than opening it, so reporting never
  migrates or writes; rules in `docs/design/crash-recovery-and-compatibility.md`, broken by
  `tests/fault_injection.rs`.
- `crates/jutsu-audio-session` — single-writer session layer. `protocol.rs` is the wire
  contract (newline-delimited JSON over loopback TCP), `discovery.rs` the `.session` sidecar a
  client dials, `lock.rs` the `.lock` sidecar an offline writer takes, `server.rs`/`client.rs`
  the two ends. The server never touches a `Project`: it hands `SessionCall`s to the owner.
  Contract:
  `docs/design/jutsu-audio-session-protocol-v1.md`.
- `crates/jutsu-audio-extensions` — compile-time synth/effect/generator registries. `voice.rs` is
  the note lifecycle (events, envelope, seeded noise); `builtin.rs` the reference synths,
  registered by `register_builtin`. A synth's whole state is its parameters plus its descriptor's
  `state_version` — there is no separate instance serialization. `recipe.rs` is the seeded
  generator contract; `generators/` holds the SFX generators, one file each over a shared
  `dsp.rs`; `effects/` the built-in filters, dynamics, delay and reverb; `parameters.rs` the one
  parameter API (units, ranges, presets, validation) that mixer strips and extensions share.

## Tests worth knowing about

- `tests/sfx_workflow.rs` — the end-to-end SFX scenario, documented in
  `docs/workflows/first-sfx-edit.md`. Runs without an audio device.
- `tests/music_workflow.rs` — the same for a music cue: tempo, patterns, sampler, mix, live edit,
  reopen, export. `docs/workflows/first-music-cue.md`.
- `tests/session_workflows.rs` — concurrent editor/CLI behaviour over a real socket.
- `tests/support/mod.rs` — the shared harness both use: a live `Editor`, the in-process CLI, and
  a deterministic test WAV.

## Conventions worth knowing before editing

- Clip `start_sample` and `duration_samples` are **project** frames; `source_start_sample` is in
  the source file's own frames. The project rate is inferred from the first managed asset
  (`timeline::project_sample_rate`) — there is no rate field in the schema yet.
- Track mute and solo live in `Track::parameters` under `"mute"` / `"solo"`, clip gain and pan in
  `Clip::parameters` under `"gain_db"` / `"pan"`. The keys are named in
  `jutsu-audio-engine::mixdown`; read them through its helpers rather than by hand.
- `cargo quality` is the gate; the four steps are listed in `xtask/src/lib.rs:quality_steps`.
