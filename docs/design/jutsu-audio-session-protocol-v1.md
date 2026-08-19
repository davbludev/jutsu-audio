# Jutsu Audio Session Protocol v1

## Purpose

One process owns a project at a time. When the GUI has a project open it is the single writer:
it holds the write lock, it owns the command engine, and every other process — the CLI, a
future agent, a test — routes its edits through the owner instead of writing the file behind
its back. When no session is live, a client takes the same write lock itself and edits the file
directly. Both paths apply commands through `jutsu-audio-commands`; neither mutates `Project`.

Implemented by `crates/jutsu-audio-session`.

## Endpoint

The owner listens on a TCP socket bound to `127.0.0.1` with an ephemeral port. Loopback TCP is
the only local transport with the same shape on Windows, macOS and Linux in the standard
library, and it costs no dependency. The listener never binds a routable interface.

Framing is newline-delimited JSON: one request object per line in, one response object per line
out, UTF-8, no length prefix. A connection may carry many request/response pairs and is closed
by either side at any frame boundary.

## Discovery

The owner publishes a sidecar file next to the project file, named by appending `.session` to
the whole project file name (`song.jutsu-audio.json.session`). It contains:

- `protocol_version` — this contract's version, currently `1`;
- `port` — the loopback port to dial;
- `token` — the shared secret for this session;
- `project_path` — the project the session owns;
- `process_id` — diagnostics only.

The file is removed when the session ends. `PublishedSession` owns that removal, so a normal
exit never leaves a stale endpoint behind.

## Authentication boundary

The token is the whole boundary, and it is a boundary against confusion, not against a local
attacker: any process that can read the session file already runs as the user who owns the
project. The token exists so a client cannot drive the wrong session — a recycled port, a
descriptor read before the owner restarted — and so an unrelated program that happens to
connect is rejected rather than obeyed. Every request carries it; a mismatch answers
`unauthorized` and the connection is dropped.

## Requests and revisions

Every request carries `protocol_version`, `token`, a caller-generated `request_id`, and a
payload. Version mismatch is rejected before anything else; the payload is not interpreted.

Payloads in v1:

- `status` — project path, name, current revision, unsaved flag. Read-only.
- `apply` — a `ProjectCommand` batch with an optional `expected_revision`.
- `transport` — play, pause, stop, seek against the owner's transport.

`expected_revision` is the optimistic-concurrency precondition from the command contract. It is
optional on the wire for one reason: a client that has never read the project should be able to
append without a round trip. Supplying it turns a blind write into a checked one, and a
mismatch answers `revision_conflict` carrying both `expected_revision` and `actual_revision`,
so the client can re-read and retry. Omitting it means "apply against whatever is current" and
is only safe for edits that do not depend on prior state.

The owner never silently reconciles a conflict. A stale writer is told the revision moved.

## Request IDs and idempotency

`request_id` is a caller-generated UUID echoed in the response. A client that loses a
connection mid-request cannot know whether the batch was applied, so replaying the same
`request_id` returns the first response instead of applying the batch again, with
`replayed: true` on the payload. The owner remembers a bounded number of recent request IDs;
beyond that window a replay is treated as a new request, which the client's
`expected_revision` then catches.

## Responses

A response carries `status` (`ok` or `error`), `protocol_version`, the originating
`request_id`, and either a payload or an error. Error codes are
`unsupported_protocol_version`, `unauthorized`, `malformed_request`, `revision_conflict`,
`command_failed`, and `session_closed`. `command_failed` wraps a structured command-engine
failure; the project is unchanged, as the command contract guarantees.

## Write lock and stale recovery

`ProjectLock` is a sidecar file named by appending `.lock` to the project file name, created
with `create_new` so creation is the atomic test-and-set. It records the holder's process ID
and the millisecond timestamp at which it was taken.

A lock is considered stale when its recorded timestamp is more than 60 seconds old, or when the
file cannot be parsed at all — the latter can only come from a crash mid-write, and honouring
it forever would strand the project. A stale lock is broken and re-taken; a live holder that
races the break simply wins the following `create_new`.

Session staleness is decided differently, and more cheaply: a client that finds a session file
but cannot connect to its port knows the owner is gone. It deletes the session file and falls
back to the offline lock path. No process-liveness probing is needed, so no platform-specific
code is.

## Undo ordering

Undo history belongs to the project, not to whoever made the edit. Every batch committed while
the editor is open — from the UI or over this protocol — is recorded in one chronological stack
with its inverse, so undo reverses the last thing that happened to the project. An undo is
itself an ordinary forward batch: it takes the next revision, and a client watching revisions
sees it like any other edit.

## Guarantees

- A second writer cannot silently overwrite newer state: it either goes through the owner and
  is revision-checked, or it holds the exclusive lock while the owner does not exist.
- A failed request mutates nothing.
- A replayed request ID applies once.
- Both transport and protocol are versioned, and the version is checked before the payload is
  interpreted.
