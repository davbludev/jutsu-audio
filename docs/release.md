# Cutting a Release

```bash
cargo quality                                        # the gate, first
cargo package-release                                # dist/jutsu-audio-<version>-<target>/
cargo smoke dist/jutsu-audio-<version>-<target>      # run what was just built
```

`cargo package-release` builds both binaries with `--release --locked`, copies them into a
release directory with `docs/cli.md` and `docs/extension-sdk.md`, and generates `INSTALL.md`,
`THIRD-PARTY-NOTICES.md` and `SHA256SUMS`. The code is `xtask/src/package.rs`.

## Declared platforms

One release directory per platform, built on that platform:

| Target | Built on |
| --- | --- |
| `x86_64-pc-windows-msvc` | Windows |
| `x86_64-apple-darwin`, `aarch64-apple-darwin` | macOS |
| `x86_64-unknown-linux-gnu` | Linux |

There is no cross-compilation step: the audio backend binds to the host's own sound API, so a
release is only as trustworthy as the machine it was built and smoke-tested on.

## Reproducible

Two builds of one commit on one machine produce identical checksums. What makes that true:

- `--locked`, so the dependency graph is the lock file's and not today's registry;
- `rust-toolchain.toml`, so it is the same compiler;
- sorted iteration everywhere in the packaging code, and no dates written into any artifact.

If `SHA256SUMS` differs between two builds of the same commit, that is a bug in packaging, not a
quirk of the toolchain.

## Signed-ready

Nothing here signs anything, and no key material lives in this repository. The layout is fixed
and the checksums are written, so signing is a step someone with the keys runs afterwards:
Authenticode on the Windows binaries, `codesign` plus notarisation on the macOS ones, a detached
signature over `SHA256SUMS` for Linux. Sign after `cargo smoke`, never before — signing a build
that does not run is worse than not signing.

## Clean-machine checklist

`cargo smoke` covers what can be automated: the CLI answers `--version`, `describe_protocol`
lists its operations, a project is created, a generator runs, a WAV exports and is non-empty. It
runs the *packaged* binaries and touches nothing in the repository.

By hand, on a machine that has never built this, once per platform:

1. Unpack, verify with `sha256sum --check SHA256SUMS`.
2. Launch `jutsu-audio`. It opens with an empty project.
3. With no audio device (or with the device disabled): the first-run notice appears, says
   playback is unavailable, and **Try again** works once a device is plugged in.
4. Import a WAV, place a clip, press play, hear it.
5. Export a WAV and open it somewhere else.
6. Put the directory on PATH and run `jutsu-audio-cli --version` from another folder.
7. Delete the directory. Confirm projects, exports and preset libraries elsewhere are untouched.

## Version numbers

The release takes the version from the `jutsu-audio` package in `Cargo.toml` — not from `xtask`,
which is a build tool nobody downloads.
