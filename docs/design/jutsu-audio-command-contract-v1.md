# Jutsu Audio Command Contract v1

## Purpose

All authoritative project edits use `jutsu-audio-commands`. GUI, CLI, IPC, migrations, generators, undo/redo, and tests must submit the same versioned command envelopes. Consumers receive committed outcomes or structured errors; they never mutate `Project` directly.

## Envelope

A command envelope contains:

- `protocol_version`: command wire-contract version, currently `1`;
- `command_id`: caller-generated UUID for correlation and future idempotency;
- `expected_revision`: optimistic-concurrency precondition;
- `commands`: ordered, non-empty atomic batch.

Commands are externally tagged with stable snake_case `type` names. IDs and values use model serialization. Human presentation must wrap this structure rather than change it.

## Atomic application

`ProjectCommandEngine` owns project state and revision. Application order:

1. reject unsupported protocol version;
2. reject stale expected revision;
3. reject empty batch;
4. clone current project;
5. apply commands in order to candidate state;
6. validate complete candidate through `jutsu-audio-model`;
7. commit candidate and increment revision once;
8. return ordered change events.

Any failure leaves project state and revision unchanged. Final-state validation lets one batch create an entity and then reference it while preventing partial or dangling updates.

## Initial command surface

Foundation commands prove shared behavior for:

- project metadata update;
- asset addition/removal;
- clip addition to identified track/layer.

Later feature tasks add edit variants at this boundary. They must not create alternate mutation paths.

## Outcomes and events

Successful outcome contains original command ID, committed revision, and ordered change events. Each event contains batch-local sequence, added/updated/removed kind, entity kind, and stable entity ID.

Events describe committed domain changes. GUI projection, audio-plan compilation, autosave, IPC subscriptions, and undo history consume them after commit. Runtime telemetry does not use this channel.

## Structured failures

Stable machine error codes cover protocol mismatch, revision conflict, empty batch, missing entity, invalid final project, and revision overflow. Errors may include command index, expected/actual revisions, and model validation diagnostics.

No user input or invalid command path may panic. Error text aids humans; integrations branch on codes and structured fields.

## Determinism and threading

Engine needs no GUI, audio device, filesystem, async runtime, clock, or random source. Caller supplies all IDs, seeds, and values. Given identical initial project, revision, and envelope, state and outcome are identical.

Engine is a synchronous single-writer component. Project actor serializes calls around it. Audio callback never invokes it. Candidate cloning is allowed here because work runs off audio callback.

## Extension rules

Every new mutation must:

- be represented by a serializable command;
- validate identifiers and values;
- participate in batch rollback;
- emit committed change events;
- have red/green tests for success, failure, and atomicity where relevant;
- preserve stable protocol compatibility or increment protocol version deliberately.

Idempotent request replay, inverse commands, durable history, and session transport belong to their scheduled tasks, built around command IDs and outcomes defined here.
