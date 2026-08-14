# Jutsu Audio project guidance

## Project index

- `Cargo.toml`: workspace membership.
- `src/main.rs`: modern eframe desktop shell, sample timeline, inspector, transport, and project actions.
- `crates/jutsu-audio-model`: portable project schema, stable IDs, and validation.
- `crates/jutsu-audio-project`: atomic persistence, migrations, WAV assets, waveform cache, and portable paths.
- `crates/jutsu-audio-commands`: atomic revisioned command application and change events.
- `crates/jutsu-audio-extensions`: typed synth, effect, and generator contracts/registries.
- `crates/jutsu-audio-engine`: immutable render snapshots, lock-free playback transport, system output, and audio-graph foundations.
- `fixtures/projects`: versioned deterministic golden projects.
- `xtask`: cross-platform repository automation; `cargo quality` runs required gates.
- `docs/quality.md`: tests, fixtures, migrations, benchmarks, and real-time-safety conventions.
- `.project-flow`: live Jutsu Project Flow tasks, documents, links, annotations, and activity.

## MCP index

- Jutsu Project Flow is task/document source of truth. Establish and verify one project session, inspect ready task plus linked documents/reviews, start before edits, attach exact validation evidence, complete only after acceptance, verify next ready task, then close session.
- Prefer plural Project Flow tools for multiple atomic changes. Complete children before parent. Never edit `.project-flow` records manually.

## Local rules

- Keep GUI and CLI thin over shared model/command/audio crates.
- Never mutate project state outside command engine.
- Keep blocking, allocation, I/O, logging, and project mutation outside audio callback.
- Run `cargo quality` before task completion and commit.
- Commit/push only when explicitly requested; never push by default.
