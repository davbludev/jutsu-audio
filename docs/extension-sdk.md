# Writing an Extension

An extension is a Rust crate that depends on `jutsu-audio-extensions` and implements one of three
traits. There is no dynamic-library ABI to hold still: a build registers the packs it ships with,
which is why a descriptor can change shape between versions without breaking anyone's `.so`.

A complete worked example — one synth, one effect, one generator, plus its conformance tests —
is `examples/pocket-extensions`. It depends only on the published surface, so if it compiles,
the surface is enough.

## The three traits

| Trait | Renders | Called on the audio thread |
| --- | --- | --- |
| `Synth` | notes → audio | yes |
| `Effect` | audio → audio, in place, one channel per instance | yes |
| `Generator` | a seed → a finished buffer | no |

Each has a factory (`SynthFactory`, `EffectFactory`, `GeneratorFactory`) that carries the
descriptor and instantiates the thing. Register the factories with `ExtensionRegistries`.

### Rules for `Synth` and `Effect`

These run in the audio callback, so `render` and `process` must not **allocate, lock, block, log,
or do I/O**. Size every buffer in `prepare`, which is called before the first block and whenever
the rate changes.

`reset` must return the instance to the state a fresh one would be in. That is what makes an
offline export match what playback just produced — the engine asserts that parity in
`crates/jutsu-audio-engine/tests/render_parity.rs`, and a stale filter state is the usual reason
it fails.

`latency_frames` and `tail_frames` are how a chain lines audio up. Report the truth; zero is the
truth for most effects.

### Rules for `Generator`

A project stores the seed, not the audio. The same seed must give the same samples on every
machine, forever — so seed your own randomness (splitmix64 in eight lines beats a dependency),
never read the clock, and never iterate a `HashMap`. Ignoring the seed entirely is allowed: a
generator that plays a written phrase is a legitimate design, and `sfx.pickup` does exactly that.

## Descriptors and the version policy

A descriptor declares the type ID, the display name, a `state_version`, and the parameters.

**Type IDs** are lowercase dotted identifiers: `vendor.thing`. Pick a prefix nobody else will use;
`builtin.` and `sfx.` are taken. Two extensions with one ID make a project ambiguous, so a
duplicate registration is refused rather than silently overwritten.

**`state_version` describes the parameter set, not the sound.** It starts at 1.

- Adding a parameter: bump `state_version` and set `introduced_in_state_version` to the new number
  on the new parameter. A project written by the older build still loads — the missing parameter
  takes its default.
- Changing a default, a range or a display name: no bump. Existing projects keep the values they
  stored.
- Removing or renaming a parameter, or changing its type: bump, and keep reading the old name if
  you want old projects to sound the same. Nothing else can do that for you.
- Changing what the same parameters *sound* like is not a state version question. If the change
  is big enough to matter to finished work, ship a new type ID.

A build that meets a project using an extension it does not have keeps the whole thing —
type ID, state version, parameters — untouched through load, edit and save, and reports the
missing extension as a mix diagnostic instead of failing. That behaviour is pinned in
`tests/fault_injection.rs`; the rules are in
`docs/design/crash-recovery-and-compatibility.md`.

**Ranges** on numeric parameters are enforced by the host before your `instantiate` is called, so
the body may read its parameters without checking them. Declare the range you actually accept:
an automation lane writes anywhere inside it.

## Conformance

```rust
let findings = jutsu_audio_extensions::conformance::check_synth(&MyFactory::default());
assert!(findings.is_empty(), "{findings:#?}");
```

`check_synth`, `check_effect` and `check_generator` run the same checks the built-in extensions
are held to (`crates/jutsu-audio-extensions/tests/conformance.rs` runs them over every built-in):

- the descriptor's own defaults instantiate, and sit inside their own declared ranges;
- both ends of every declared range are accepted, and past the maximum is refused;
- two instances with the same parameters produce identical audio;
- `reset` reproduces the first render exactly;
- no NaN, no infinity, nothing past full scale;
- a synth with no notes, and a fresh effect handed silence, produce silence;
- a generator honours its frame count and repeats itself for a given seed.

Every check is fixed-rate, fixed-block, fixed-seed. A finding is a defect, never a flake.

## Registering a pack

```rust
pub fn register(registries: &mut ExtensionRegistries) -> Result<(), ExtensionError> {
    registries.register_synth(Arc::new(MySynthFactory::default()))?;
    registries.register_effect(Arc::new(MyEffectFactory::default()))
}
```

One entry point per pack, so a host adds it the way its author intended. The application's own
registry lives in `src/extensions.rs`.
