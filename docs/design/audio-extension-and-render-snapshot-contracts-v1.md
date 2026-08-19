# Audio Extension and Render Snapshot Contracts v1

## Boundary

`jutsu-audio-extensions` defines typed synth, effect, and generator contracts. `jutsu-audio-engine` defines immutable render-plan snapshots. Project state, GUI widgets, CLI parsing, filesystem access, audio-device access, and dynamic-library loading remain outside these contracts.

## Stable extension identity

Each extension uses a serialized lowercase dotted `ExtensionTypeId`, such as `builtin.noise`. Invalid IDs are rejected during construction and deserialization. Type ID plus extension kind identifies a factory. Duplicate registration never replaces existing factory silently.

Rust dynamic-library ABI is not promised. Initial registrations are compile-time factories. Future sandbox/process/WASM loading must adapt into same typed factory contracts.

## Descriptors and parameters

Every factory exposes an `ExtensionDescriptor` containing:

- stable type ID;
- synth/effect/generator kind;
- display name;
- positive serialized state version;
- ordered parameter descriptors.

Each parameter declares stable ID, display name, value type, default value, first supported state version, and automation capability. Registration rejects mismatched registry kind, duplicate/invalid parameter IDs, invalid version ranges, and defaults whose value type does not match descriptor.

Instantiation validates supplied parameter IDs and value types before calling factory. Errors contain stable code, optional kind/type/parameter context, and human message.

## Typed factories

Separate `SynthFactory`, `EffectFactory`, and `GeneratorFactory` traits prevent cross-kind instantiation. Instances are `Send`:

- synth renders bounded mono buffers;
- effect processes bounded mono buffers;
- generator creates deterministic mono output from explicit seed and frame count.

Mono methods establish minimal contract only. Channel layouts, sample rate lifecycle, note/voice semantics, real-time allocation rules, generator provenance, cancellation, and richer DSP contexts belong to scheduled feature tasks.

## Registries

`ExtensionRegistries` owns three typed maps. It supports validated registration, descriptor discovery, and instantiation. Missing types return `UnavailableType` with requested kind and type ID so projects can preserve unavailable extension state and GUI/CLI can report actionable diagnostics.

Registry mutation happens during application setup or controlled extension reload, never on audio callback.

## Immutable render snapshot

`RenderSnapshot` is built off audio callback and published as a shareable `Send + Sync` value. Fields are private; consumers receive read-only slices/getters. Snapshot contains:

- source project ID and committed revision;
- sample rate and channel count;
- immutable processor nodes;
- immutable connections;
- output node.

Processor specs support sample clips, typed synth/effect references with state versions and parameters, and mixer buses. Runtime DSP instances and mutable transport state are not stored in snapshot.

Build validation rejects zero audio formats, duplicate node IDs, missing connection endpoints, and missing output node. Future graph compilation adds cycle/routing/latency validation without weakening this boundary.

## Publication and runtime ownership

Project actor commits revision. Audio control worker compiles that exact revision into a complete snapshot and resolves extension descriptors/factories. Only successful complete snapshots are published. Audio callback atomically observes old or new complete snapshot; it never sees partially built graph.

DSP instances derived from snapshot are callback-owned mutable runtime state. Snapshot replacement and instance transition policies must be bounded and non-blocking. Missing extensions fail compilation clearly off callback; they do not panic or substitute hidden behavior.

## Extension requirements

New synths, effects, and generators must:

- use stable type and parameter IDs;
- version serialized state and algorithm changes deliberately;
- provide deterministic behavior where contract promises it;
- validate parameters before runtime;
- avoid unrestricted project, GUI, filesystem, IPC, or audio-device access;
- pass descriptor, registration, instantiation, missing-type, and compatibility tests.

Built-ins use same contracts as external extensions.
