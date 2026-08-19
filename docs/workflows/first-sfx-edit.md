# First SFX edit, end to end

The scenario Jutsu Audio has to do well before anything else: take a raw sample, layer and shape
it into one sound, hear it, and hand out a WAV. Every step below exists on both surfaces — the
desktop editor and the structured CLI — and both drive the same command engine, the same mixdown
and the same transport.

`tests/sfx_workflow.rs` runs this as one automated scenario. It needs no audio device: the only
step that touches hardware is preview, and that check reports what the machine has instead of
failing the run.

## The scenario

1. **Create and import.** `create_project`, then `import_sample` copies the WAV into the project
   directory, fingerprints it, and writes its peak cache. A second import of identical audio
   reports `duplicate` and adds nothing.
2. **Layer.** `add_layer` gives the track a second lane, and `add_clip` places the sample in each.
   Lanes exist so overlapping takes are separate objects rather than one flattened region.
3. **Trim.** `update_clip` shortens the first clip. Nothing is destroyed: the source is untouched
   and the clip is a window onto it, so the trim is undoable and re-openable forever.
4. **Fade.** `crossfade_clips` fades the first clip out and the second in across their whole
   overlap. `set_clip_fades` sets either fade directly; both are trimmed to fit the clip.
5. **Loop.** `set_loop_region` marks the span worth auditioning. Playback wraps on the exact loop
   frame, and an export of that region contains exactly those frames.
6. **Preview.** The editor plays the mixdown through the default output device. Playback and
   export share one snapshot, so what is heard is what is written.
7. **Adjust live.** With the editor open it owns the project, and CLI edits are applied through
   it: `set_clip_pan` here returns `"delivery": "session"`, appears in the editor immediately, and
   does *not* touch the file — the editor still owns when the file is written.
8. **Save.** The editor writes the project atomically and clears its recovery sidecar.
9. **Export.** `export_wav` with `use_loop_region: true` writes the looped span. The frame count
   in the response is the frame count in the file.

## What the scenario asserts

- Both layers survive the round trip, and so do the fades the cross-fade wrote.
- A CLI edit made while the editor is open reaches the editor and leaves the file alone until the
  editor saves.
- The exported file is exactly the loop, is not silence, and stays inside full scale.
- A refused edit — here a loop that ends before it starts — changes nothing on disk.

## Running it

```bash
cargo test --test sfx_workflow
```
