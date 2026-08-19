# Jutsu Audio Generator Recipe v1

## Purpose

A generated sound has to be reproducible: the same recipe, on another machine, in a year's time,
must produce the same audio and the same project edits. That is what makes procedural SFX safe to
keep in a project instead of baking to WAV, and what lets an agent regenerate a variant without
asking a human what the last one was.

Implemented by `crates/jutsu-audio-extensions/src/recipe.rs`.

## The recipe

```json
{
  "generator_type": "sfx.impact",
  "algorithm_version": 1,
  "seed": 7,
  "frame_count": 24000,
  "parameters": { "weight": { "type": "float", "value": 0.5 } }
}
```

- `generator_type` — a registered generator's stable type ID.
- `algorithm_version` — the generator's own version. A generator that changes what it produces
  bumps this; an old recipe then keeps naming the old sound rather than silently becoming a
  different one.
- `seed` — the root of every random choice.
- `frame_count` — how long to render, in project frames.
- `parameters` — the generator's declared parameters, validated against its descriptor.

Nothing else may affect the output. No clock, no file path, no machine state, no global RNG.

## Seed derivation

A generator that needs independent randomness for different parts asks the recipe for it:
`derive_seed("body")`, `derive_seed("tail")`. Derivation is splitmix64 over the root seed mixed
with an FNV-1a hash of the label — both specified here rather than borrowed from a hasher whose
output is allowed to change between releases.

Two labels never share a stream, and the same label always gives the same stream.

## Identity and derived IDs

`identity_seed` folds everything that decides the audio — generator, algorithm version, seed,
length and every parameter — into one number. Two recipes with the same identity render the same
samples.

Entity IDs come from that identity: `derive_uuid("asset")` gives the same UUID for the same
recipe every time. This is what makes *command output* byte-identical between runs, not just the
audio; a fresh v4 UUID per run would differ every time and leave nothing to compare. It is also
how `Replace` finds what it replaced, without a side table mapping recipes to assets.

## Provenance

A generated asset stores its provenance in `AudioAssetSource::Generated`: generator type,
algorithm version, seed, and the parameters it was run with. That is exactly the recipe minus its
length, which the clip carries. Reading an asset back is enough to run its recipe again.

Generated audio is not stored. The mix renders it from the provenance on the way to the speakers,
so a project stays small and can never disagree with its own recipe.

## Replace and regenerate

- `Replace` — rerun a recipe and overwrite the asset it produced before, keeping the asset ID so
  every clip already using it follows the new version. This is the "I want a different roll of the
  same dice" path: change the seed, keep the arrangement.
- `New` — leave the previous asset alone and add another, for auditioning a variant beside the
  original.

Both go through the command engine as ordinary batches, so both are one undo step and both are
visible to a live editor.

## Guarantees

- The same recipe produces byte-identical commands, including entity IDs.
- The same recipe renders identical samples, in real time and offline.
- Changing anything that affects the sound changes the identity, so a stale asset can be detected
  rather than assumed current.
- A generator that is not registered fails with a structured error naming the type, and the
  project keeps its provenance so another build can still render it.
