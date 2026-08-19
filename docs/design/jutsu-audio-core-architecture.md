# Jutsu Audio core architecture

## Decision

Use a Cargo workspace with small layered library crates and two thin application crates. Keep one authoritative project-command path for GUI, CLI, IPC, tests, and future automation. Use compile-time extension registries initially; do not commit to a Rust dynamic-library ABI.

This is preferred over:

1. **One application crate with internal modules:** fastest initial setup, but GUI/CLI boundaries are easy to violate and later extraction becomes costly.
2. **Many feature crates from day one:** strongest isolation, but creates excessive coordination and unstable public APIs before the domain is known.
3. **Layered workspace (chosen):** enough enforced separation for shared core and real-time safety while allowing internal modules to split into crates only when proven useful.

## Workspace layout

```text
Cargo.toml                       workspace manifest
crates/
  jutsu-audio-model/             pure project/domain data and validation types
  jutsu-audio-commands/          edit commands, atomic application, revisions, change events
  jutsu-audio-extensions/        synth/effect/generator contracts, descriptors, registries
  jutsu-audio-engine/            render snapshots, transport, real-time and offline rendering
  jutsu-audio-project/           project files, migrations, assets, autosave and recovery
  jutsu-audio-session/           local IPC protocol, session discovery, locks and client/server
apps/
  jutsu-audio-cli/               structured CLI and human-readable opt-in presentation
  jutsu-audio-gui/               desktop UI, project actor host and audio-device host
```

Initial work may keep closely related implementation as modules inside these crates. New crates require a real ownership, dependency, build-time, or reuse benefit.

## Responsibilities and boundaries

### `jutsu-audio-model`

Owns stable entity IDs, time/sample units, project entities, parameter values, asset references, validation diagnostics, and schema-independent domain invariants. No GUI, CLI, filesystem, audio device, async runtime, or DSP dependencies.

### `jutsu-audio-commands`

Owns versioned command envelopes, command validation/application, revision preconditions, atomic batches, inverse/history metadata, and ordered change events. Depends on `model` only. Every project mutation enters here, including GUI edits, CLI edits, migrations, generators, and undo/redo.

### `jutsu-audio-extensions`

Owns typed descriptors and runtime contracts for synths, effects, and generators; stable type IDs; algorithm/state versions; parameter schemas; registries; missing-extension diagnostics. Depends on `model`. Built-ins implement same contracts as third-party extensions.

Initial extension loading is compile-time registration. A later stable out-of-process or WASM boundary may be added after requirements are proven. Rust `cdylib` ABI is not a foundation contract.

### `jutsu-audio-engine`

Owns immutable render-plan compilation, transport, audio graph runtime, resampling/channel handling, metering events, system audio adapter, and offline renderer. Depends on `model` and `extensions`; it never mutates authoritative project state.

Audio device integration stays behind a narrow adapter so tests and offline export need no hardware.

### `jutsu-audio-project`

Owns atomic save/open, format versions and migrations, asset import/fingerprints, relative paths, caches, autosave, journaling, recovery, and project locking primitives. Depends on `model`, `commands`, and `extensions` descriptors. Persistence serializes domain and extension state but never contains GUI state.

### `jutsu-audio-session`

Owns versioned local IPC messages, discovery metadata, authentication token handling, request IDs, revision conflicts, subscriptions, and stale-session recovery. Depends on public command/model DTOs. Transport details do not leak into command logic.

### Application crates

GUI renders projections of authoritative state and emits commands. CLI parses input, discovers schemas/capabilities, sends commands to a live GUI session when present, or uses project/command services under an exclusive offline lock. Neither application duplicates validation or editing behavior.

## Allowed dependency direction

```text
model
  commands -> model
  extensions -> model
  engine -> model + extensions
  project -> model + commands + extensions
  session -> model + commands
  cli -> model + commands + extensions + project + session
  gui -> model + commands + extensions + engine + project + session
```

Forbidden dependencies:

- Core crates never depend on GUI or CLI.
- `model` never depends on persistence, IPC, DSP, or presentation.
- `commands` never performs filesystem, IPC, or audio-device work.
- `engine` never writes project state or project files.
- GUI and CLI never mutate project structs directly.
- Extension implementations never receive unrestricted project, filesystem, GUI, or audio-device access.

Use dependency checks or architecture tests once workspace exists.

## Thread and ownership model

### Project actor: single authoritative writer

One project actor owns mutable `ProjectState`, current revision, command history, dirty state, and ordered change publication. GUI actions and IPC requests enter one queue. A command validates against its expected revision and either commits fully or returns structured diagnostics. File saves snapshot committed state without granting another writer.

When GUI has a project open, it hosts this actor and session server. Offline CLI acquires exclusive project ownership before creating the same actor locally.

### GUI main thread

Owns windows, widgets, focus, selection, and view projections only. It sends commands and consumes ordered state/change events. Stable entity IDs preserve selection across external CLI edits.

### Audio control worker

Compiles committed project revisions into immutable render plans outside the audio callback. Only complete plans are published to audio runtime. Superseded compilation may be cancelled or discarded by revision.

### Audio callback

Owns mutable DSP runtime and transport position required for the current stream. Receives immutable plans and bounded control messages through lock-free or wait-free channels. Emits bounded meters, positions, and diagnostics through non-blocking queues.

Hard callback rules:

- no filesystem or network/IPC access;
- no blocking locks or waits;
- no heap allocation or deallocation in steady state;
- no logging, formatting, panic unwinding, or unbounded work;
- no project mutation or command validation.

Queues have explicit capacity and overflow behavior. Low-value telemetry may be dropped/coalesced; commands and state changes are never sent through lossy callback queues.

### Asset and waveform workers

Perform decoding, hashing, waveform generation, and cache work. Results return to project actor as explicit events/commands tied to asset ID and expected revision.

### IPC runtime

Accepts local CLI clients, authenticates session requests, enforces protocol versions/request IDs, and forwards command envelopes to project actor. It never owns project state.

### Offline render workers

Render immutable project snapshots without audio hardware. Export uses same graph compilation and DSP semantics as real-time playback, with explicit differences only for scheduling, block size, and latency/tail flushing.

## Primary data flows

### GUI edit

`GUI intent -> command envelope -> project actor -> validated commit/revision -> change event -> GUI projection + audio-plan compilation -> callback plan swap`

### CLI edit while GUI is open

`CLI JSON -> session client -> GUI session server -> project actor -> same commit/event path -> structured CLI result`

### CLI edit while GUI is closed

`CLI -> exclusive project lock -> open/migrate -> local project actor -> command -> atomic save -> unlock`

### Playback and export

Both compile from an immutable committed project snapshot through `engine`. Real-time output targets audio device; export targets a file sink. DSP and automation evaluation remain shared.

## Error and compatibility contracts

Public boundaries return structured diagnostics with stable machine code, message, entity/path context, and optional remediation data. Panics indicate programmer faults, not user input errors.

Version separately:

- project file format;
- command/IPC envelope;
- extension type state and algorithm;
- preset/generator recipe.

Unknown extension state must be preservable even when it cannot render. Migration is explicit, testable, and backup-safe. Seeds use a specified algorithm/version and derived sub-seeds so unrelated generator changes do not perturb existing results.

## Architecture acceptance checks

Before completing later foundation work, prove:

- GUI and CLI link to shared model/command crates and contain no duplicate edit rules.
- Unit tests apply identical command fixtures without GUI, IPC, filesystem, or audio device.
- A mock synth, effect, and generator register by stable type ID and expose typed descriptors.
- A render snapshot can be built off callback thread and swapped without blocking callback.
- Dependency inspection shows no forbidden reverse dependency.
- Thread-safety test/benchmark detects allocation, blocking, queue overflow, and stale revision behavior at named boundaries.

## Deferred decisions

GUI toolkit, audio backend crate, serialization format, async runtime, plugin sandbox format, and supported operating systems remain implementation choices for their owning tasks. Select them only after checking maintenance, license, platform support, and real-time behavior. This architecture does not require a specific vendor library.

## Desktop UI product requirement

Jutsu Audio desktop app must present a modern, polished, visually coherent interface suitable for sustained professional creative work. Visual quality is a product requirement, not optional post-release decoration.

Required qualities:

- clear hierarchy, balanced spacing, restrained color, consistent typography, icons, controls, and interaction states;
- responsive editing at supported project sizes, with no UI-thread audio, decoding, persistence, or waveform work;
- scalable layouts for common desktop window sizes and high-DPI displays;
- keyboard-accessible editing, visible focus, readable contrast, useful labels/tooltips, and non-color-only status communication;
- cohesive timeline, waveform, inspector, mixer, automation, preset, browser, transport, progress, empty, loading, and error states;
- immediate, stable visual projection of GUI and external CLI edits without disruptive full-view resets;
- reusable design tokens and shared components so new synths, effects, generators, and parameter editors inherit consistent presentation;
- deliberate destructive-action confirmation and recoverable workflows without modal overload.

UI-specific state such as panel layout, zoom, selection, and theme remains separate from portable audio project state. GUI must consume shared model, commands, registries, and structured diagnostics rather than duplicate product logic.

Before selecting GUI toolkit or committing to final interaction patterns, create representative timeline, mixer, inspector, and procedural-generation prototypes. Validate visual coherence, editing speed, keyboard flow, high-DPI scaling, and large-project responsiveness. Release gates must include rendered visual review across supported themes, scales, window sizes, and core human workflows.
