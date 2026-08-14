# Quality and reproducibility

## Required gate

Run from repository root:

```text
cargo quality
```

This cross-platform Cargo alias runs formatting verification, Clippy with warnings denied, all workspace tests/targets/features, and compilation of every benchmark target. Any nonzero step stops the gate and fails the command.

Run the complete gate before completing a Project Flow task or creating its commit. Focused tests support red/green development but never replace the complete gate.

## Test placement

- Unit tests cover private algorithms beside implementation.
- Integration tests under each crate's `tests/` cover public contracts without GUI, audio hardware, filesystem, or IPC unless those boundaries are the test subject.
- Tests use fixed IDs, seeds, sample rates, block sizes, and explicit tolerances. Never depend on wall-clock time, collection hash order, device enumeration, locale, or random OS entropy.
- Each bug fix begins with a failing regression test. New behavior begins with a failing contract test.

## Golden projects and migrations

Golden project inputs live under `fixtures/projects/vN/`. They use fixed UUIDs and explicit generator seeds. Checked-in JSON uses canonical `serde_json::to_string_pretty` output plus one trailing newline; tests compare exact normalized bytes.

Never rewrite an old fixture to match a new schema. A migration adds:

1. unchanged source fixture under its original version;
2. expected canonical output under destination version;
3. test proving source migration equals expected bytes;
4. second migration proving idempotent current-version loading;
5. validation proving stable entity IDs and seeded provenance survive.

Current baseline is `fixtures/projects/v1/seeded-project.json`. Its generated asset uses fixed algorithm version and seed, making later migration/output checks reproducible across supported platforms.

## Benchmarks

Benchmarks live in the owning crate's `benches/` directory and name workload size, sample rate, channel count, block size, and seed. Benchmarks must separate setup/graph compilation from measured callback/render work and must not require physical audio hardware.

`cargo quality` compiles benchmarks. Performance tasks run and record relevant benchmarks explicitly because timing thresholds vary by runner. Regressions use stable fixture workloads and report both baseline and candidate measurements.

## Real-time safety

Audio-callback code must perform no filesystem/network access, blocking waits or locks, logging/formatting, panic recovery, unbounded work, or steady-state allocation/deallocation. Tests and benchmarks at callback boundaries must use preallocated buffers and fixed bounded workloads.

When callback implementation begins, add instrumentation tests that fail on allocation, blocking fallback, queue overflow contract violations, non-finite output, or work exceeding declared bounds. Keep device adapters outside deterministic DSP tests.

## Toolchain and dependencies

`rust-toolchain.toml` pins Rust plus Clippy/rustfmt. `Cargo.lock` is committed. New dependencies require current maintenance and permissive commercial-use license verification before addition.
