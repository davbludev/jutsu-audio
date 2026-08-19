# Quality Gates and Deterministic Fixture Policy

## Required command

Run `cargo quality` from repository root before completing and committing implementation tasks. Cross-platform Rust xtask runs, in order:

1. `cargo fmt --all -- --check`
2. Clippy for workspace, all targets/features, with warnings denied
3. all workspace tests, targets, and features
4. compile all workspace benchmark targets

First failure stops gate with nonzero status.

## Deterministic fixtures

Versioned golden projects live at `fixtures/projects/vN/`. They use fixed UUIDs, explicit seeds, explicit algorithm versions, stable ordered maps, canonical pretty JSON, and one trailing newline. Tests normalize line endings only, then compare exact bytes.

Baseline `v1/seeded-project.json` validates schema, stable references, generator seed `0x4a55545355415544`, and byte-identical decode/encode.

Migration tasks preserve original fixtures and add expected destination bytes. They prove exact source-to-destination output, current-version idempotence, stable entity IDs, and seeded provenance. Old fixtures are never rewritten to make new code pass.

## Test and benchmark conventions

Unit tests cover private algorithms. Integration tests cover public crate contracts without GUI, physical audio devices, IPC, or filesystem unless that boundary is subject.

Tests fix IDs, seeds, sample rates, block sizes, and tolerances; they do not depend on clocks, hash iteration, locale, device enumeration, or OS randomness.

Benchmarks name workload size/sample format/seed, separate setup from measured work, and require no hardware. Gate compiles benchmarks; performance tasks run and record them because timing thresholds depend on runner.

## Real-time safety convention

Audio callback permits no filesystem/network access, blocking lock/wait, logging/formatting, panic recovery, unbounded work, or steady-state allocation/deallocation. Callback tests use preallocated buffers and bounded work. When runtime callback lands, instrumentation must cover allocation, blocking fallbacks, queue overflow contracts, non-finite output, and declared workload bounds.

## Reproducible toolchain

`rust-toolchain.toml` pins Rust 1.97.1 plus rustfmt and Clippy. `Cargo.lock` is committed. Automation uses Rust rather than platform-specific shell scripts. Dependency additions require maintenance and permissive-license checks.
