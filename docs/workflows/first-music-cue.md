# First music cue, end to end

The music counterpart to `first-sfx-edit.md`: write a pattern, arrange it into a cue, play it with
a synth and a sampler, mix it, adjust it live, save, reopen, and export.

`tests/music_workflow.rs` runs this as one automated scenario, and asserts that two exports of the
finished project are identical sample for sample. No audio device is needed.

## The scenario

1. **Set a tempo.** `set_tempo_map` puts the project at 100 BPM in 4/4. Everything after this is
   written in frames, but a caller that thinks in bars converts with `convert_time`.
2. **Write a pattern.** `add_pattern` stores a one-bar bass riff — three notes with their own
   velocities.
3. **Arrange it.** A synth clip four bars long plays the pattern; `set_clip_pattern` links them,
   and the pattern repeats to fill the clip. Twelve notes from three, without copying anything.
4. **Add a kit.** `import_sample` brings a hit in, `add_sampler` maps it, and a clip on a second
   track plays sixteen beats of it.
5. **Tighten and loosen.** `quantise_clip` snaps the kit to sixteenths of the project's tempo;
   `humanise_clip` with an explicit seed nudges it back off the grid by a bounded amount. Seeded,
   so the same "human" feel comes back on every render.
6. **Mix.** A bus takes both tracks, the bass sits back a few decibels, a reverb goes on the bus,
   and an automation lane fades the bus out over the last bar.
7. **Adjust live.** With the editor holding the project, `transpose_clip` drops the bass an
   octave. The response says `"delivery": "session"`, the editor has it immediately, and the file
   on disk still holds what was last saved.
8. **Save and reopen.** The editor writes the project; reopening it finds the tracks, the pattern,
   the automation and the tempo exactly as they were.
9. **Export.** `export_wav` writes the cue. Exporting twice produces identical audio: patterns,
   the sampler, effects and automation are deterministic together.

## What the scenario asserts

- A pattern of three notes becomes twelve across four bars, without duplicating note data.
- A CLI edit made while the editor is open reaches the editor and leaves the file alone until it
  saves.
- The reopened project holds every part of what was built — tracks, pattern, automation, tempo.
- The exported cue is audible, inside full scale, and identical between two exports.
- The automated fade is audible in the file: the last beats are far quieter than the opening bars.

A second test bundles the project and exports from the bundle, so a cue that plays here plays
wherever it is sent.

## Running it

```bash
cargo test --test music_workflow
```
