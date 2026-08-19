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
- `src/cli.rs` + `src/bin/jutsu-audio-cli.rs` — the machine surface. Reached through
  `src/lib.rs`, which exists only to expose `cli` to the binary and to `tests/cli_protocol.rs`.
  Request variants are tagged with `"operation"`, not `"type"`.

Running the GUI against a real project: `cargo run -- path/to/project.jutsu-audio.json`.

## Crates

- `crates/jutsu-audio-model` — the aggregate and `Project::validate`. Leaf crate; nothing else
  may be depended on from here.
- `crates/jutsu-audio-commands` — `ProjectCommandEngine::apply` clones the project, applies the
  batch, validates, and only then swaps it in. That clone is what gives rollback.
- `crates/jutsu-audio-project` — save/load/migrate, WAV import, and the waveform peak cache.
  The cache lives at `<project dir>/.jutsu-audio-cache/waveforms/<fingerprint>.json`; reach it
  through `AssetManager::waveform_cache_path` / `load_waveform` / `rebuild_waveform` rather than
  rebuilding the path.
- `crates/jutsu-audio-engine` — transport, snapshot exchange, `PlaybackRenderer`,
  `OfflineExporter`. The renderer is built by `SystemAudioOutput::open_default` from the device's
  own format and converts the snapshot onto it; when the formats already match it takes a
  verbatim-copy path, which is what keeps real-time output bit-identical to offline export
  (`tests/offline_export.rs` asserts that).
- `crates/jutsu-audio-session` — single-writer session layer. `protocol.rs` is the wire
  contract (newline-delimited JSON over loopback TCP), `discovery.rs` the `.session` sidecar a
  client dials, `lock.rs` the `.lock` sidecar an offline writer takes, `server.rs`/`client.rs`
  the two ends. The server never touches a `Project`: it hands `SessionCall`s to the owner.
  Contract:
  `docs/design/jutsu-audio-session-protocol-v1.md`.
- `crates/jutsu-audio-extensions` — compile-time synth/effect/generator registries.

## Conventions worth knowing before editing

- Clip `start_sample` and `duration_samples` are **project** frames; `source_start_sample` is in
  the source file's own frames. The project rate is inferred from the first managed asset
  (`timeline::project_sample_rate`) — there is no rate field in the schema yet.
- Track mute, solo and gain are intentionally absent from the UI: no command exists to change
  them and the render path would ignore them.
- `cargo quality` is the gate; the four steps are listed in `xtask/src/lib.rs:quality_steps`.
