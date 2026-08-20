# Jutsu Audio structured CLI

`jutsu-audio-cli` reads exactly one JSON request from standard input and writes exactly one JSON response to standard output. Protocol version `1` is stable for the MVP. Every entity result returns explicit UUIDs; agents never need to parse human prose.

Operations: `create_project`, `inspect_project`, `import_sample`, `add_clip`, `update_clip`, `delete_clip`, `export_wav`, `transport_request`, and `session_status`. Requests use snake_case tagged JSON with `protocol_version: 1`. Inspect output provides the complete project and the default `track_id`/`layer_id` needed for clip commands. `export_wav` accepts `encoding` (`pcm16` or `float32`) plus optional `start_frame` and `frame_count`.

Exit codes:

- `0`: structured success.
- `2`: malformed request or unsupported protocol version.
- `3`: project file, asset, or WAV failure.
- `4`: shared command validation or entity failure.
- `5`: a live session or another writer refused the edit (`session_unavailable`, `project_locked`, `revision_conflict`).

All exits, including failures, return an envelope with `ok`, `protocol_version`, and either `result` or `error { code, message }`.

## Live sessions

An editor with a project open owns that project. Every mutating operation checks for it first and reports which route it took as `delivery`:

- `session` — applied by the running editor, through its command engine, visible in its window immediately.
- `offline` — applied to the file under the project write lock, with no editor running.

The route is never chosen by the caller: writing the file behind an editor that has unsaved work would lose those edits. A session file left behind by a crashed editor is detected (nothing answers its port), cleaned up, and the operation falls through to the offline route.

`session_status` reports `attached`, plus the owner's project path, name, revision and unsaved flag when one is live. `transport_request` needs a `path` and is delivered to the live editor; with no editor running it is acknowledged with `delivery: "offline"` and dropped, because nothing is playing.

The protocol behind this is `docs/design/jutsu-audio-session-protocol-v1.md`.

## Tracks, layers and the mix

`add_track` appends a track with one empty layer and returns both IDs; `add_layer` appends a lane
to a named track. `set_track_mute`, `set_track_solo` and `set_clip_pan` change how a project
sums: solo wins over mute, pan runs `-1.0` (hard left) to `1.0` (hard right), and centre is unity
in both channels.

These are the same rules playback uses. Every surface — GUI playback, GUI export, `export_wav` —
mixes through `jutsu-audio-engine`'s `mix_project`, so a muted track is silent everywhere or
nowhere.

## Editing primitives

`split_clip`, `duplicate_clip`, `slip_clip`, `set_clip_fades` and `crossfade_clips` build their
command batches with `jutsu-audio-commands::edits`, the same code the editor's buttons use. One
operation is one batch, so it is one revision and one undo step.

`delete_clip` takes an optional `ripple` flag: with it, clips later in the same lane move earlier
by the deleted clip's length; without it the gap stays. Fades are given in project frames and are
trimmed to fit the clip — a fade longer than its clip comes back shorter than it was asked for,
and the response's revision reflects what was stored.

Refused edits — splitting outside a clip, cross-fading clips that do not overlap, naming a clip
that is gone — exit `4` with `command_failed` and change nothing.

## Markers and the loop region

`add_marker`, `move_marker` and `remove_marker` keep named positions on the timeline; markers
have stable IDs and keep them when they move. `set_loop_region` takes `start_frame`, `end_frame`
and an optional `enabled` (default `true`); `clear_loop_region` forgets the region entirely,
while `enabled: false` remembers where it was without playing it.

`export_wav` reports `diagnostics`: anything the mix had to work around — a sample that will not
decode, an extension this build does not have. The export still runs and still writes a file; a
degraded render that said nothing would be the worse outcome.

`export_wav` accepts `use_loop_region: true`, which writes exactly the frames the loop repeats
and fails with `export_failed` when there is no active loop. The editor's own Export WAV does
the same thing, and playback wraps on the same frame, so a loop sounds the same however it is
rendered.

## Synths

`list_extensions` takes no project and answers with every registered synth, effect and generator:
type ID, display name, state version, and each parameter's ID, value type, default and whether it
can be automated. It is the discovery surface — nothing has to be scraped from prose.

`add_synth_clip` creates the synth asset and the clip that plays it in one batch, so they undo
together. It takes `type_id`, the lane, `start_sample`, `duration_samples`, optional `parameters`,
and optional `notes` (`start_frame` and `duration_frames` are frames from the clip's own start).
`set_synth_parameters` replaces an asset's parameters; `set_clip_notes` replaces everything a clip
plays.

Parameters are checked against the registry before anything is applied. Exit code `6` carries
`unknown_extension` (with the types this build does have), `unknown_parameter` (with the ones the
extension declares) or `invalid_parameter` (wrong type, or a value the extension refuses, such as
a waveform name it does not know). Nothing is written when a request is refused.

## Procedural generation

`list_extensions` includes every generator with its presets; `describe_generator` answers with one
generator's full schema — parameter IDs, value types, defaults, `minimum`/`maximum`, and the
presets it ships. Between them there is nothing a caller needs to read prose for.

`preview_generator` renders a recipe without touching a project and reports `frame_count`, `peak`,
`rms` and a `fingerprint` of the samples; pass `output` to write the preview as a float WAV. The
fingerprint is the reproducibility check: the same generator, seed, length and parameters always
give the same one.

`run_generator` puts a recipe into a project as a generated asset plus the clip that plays it.
Entity IDs are derived from the recipe, so:

- `mode: "replace"` (the default) reruns a recipe over what it produced before, keeping the asset
  ID so every clip already using it follows the new version;
- `mode: "new"` with a `variant` number adds a variant beside the original, and the same variant
  number always names the same entity.

Generated audio is never stored in the project. The mix renders it from the asset's provenance —
generator, algorithm version, seed, parameters — which is why two exports of one project are
identical, and why a project can never disagree with its own recipe.

## Mixer, effects and automation

`describe_strip` answers with the parameters every track and bus has — `gain_db`, `pan`, `mute`,
`solo` — with their units, ranges and defaults. They are validated exactly as an extension's
parameters are, so the CLI and the editor accept and refuse the same values.

Routing: `add_bus`, `set_track_output`, `set_bus_output`. Levels: `set_track_parameter`,
`set_bus_parameter`.

Effects: `describe_effect` gives one effect's schema and presets; `add_effect` (with either
`{"track": {"track_id": …}}` or `{"bus": {"bus_id": …}}`), `remove_effect`, `move_effect`,
`set_effect_enabled`, `set_effect_wet` and `set_effect_parameters` manage a chain. Order is what a
chain is, so `move_effect` is an ordinary edit.

Automation: `add_automation_lane` takes a target, a parameter and optional breakpoints;
`set_automation_points` replaces a lane's curve in one command, and `remove_automation_lane`
deletes it. Points are stored in frame order whatever order they arrive in.

## Musical time

`set_tempo_map` replaces the project's tempo and time-signature changes in one command; an empty
list means the default, 120 BPM in 4/4. Changes are stored in frame order whatever order they
arrive in, and a change with a non-positive tempo is refused.

`convert_time` converts either way: give it a `frame` or a `position` (`bar`, `beat`, `tick`) and
it answers with both, plus seconds, beats, the tempo in force there, and the formatted
`bar.beat.tick`. That formatting comes from the same conversion the editor's status bar uses, so
the two can never disagree about where bar 9 starts.

A tempo or signature change begins a bar: whatever was left of the bar it lands in counts as a
short bar of its own. That is what makes position and frame exact inverses of each other.

## Patterns and note transforms

`add_pattern`, `set_pattern_notes` and `remove_pattern` manage reusable note sequences;
`set_clip_pattern` points a clip at one, or unlinks it when `pattern_id` is omitted. A pattern
repeats for the length of the clip playing it, so a two-second clip over a half-second pattern
plays it four times. Removing a pattern unlinks every clip that played it in the same batch, so a
project is never left pointing at something that is gone. A clip's own notes win over its
pattern, which is how a one-off variation is made without unlinking first.

Transforms rewrite a clip's notes in one command each — one undo step, and non-destructive in the
sense that undo puts back exactly what was there:

- `quantise_clip` snaps note starts to `divisions_per_beat` of the project's tempo (4 by default);
- `transpose_clip` moves every pitch by `semitones`;
- `humanise_clip` nudges timing and velocity by a bounded amount from an explicit `seed` — the
  same seed always gives the same result, so a humanised part is still reproducible;
- `loop_clip_notes` repeats a clip's notes at a fixed period.

## Samplers

`add_sampler` creates an instrument from a mapping of the project's own samples; `set_sampler_zones`
replaces that mapping in one command. A zone is an asset plus its `root_pitch_hz` and the pitch and
velocity range it answers for; ranges default to everything, so a one-sample instrument needs only
the asset and its root pitch. `loop_start_frame` and `loop_end_frame` together make a zone loop for
as long as its note is held; without them it plays once.

A clip plays a sampler exactly as it plays a synth: the clip carries the notes, through
`add_synth_clip`-style notes or a pattern. A zone whose asset cannot be read plays silence and
reports it rather than failing the mix, and a zone naming an asset the project does not have is
refused before anything is stored.

## Presets

`list_presets` answers with both kinds: the built-in presets the extensions ship (read-only, they
are code) and the user presets in a library directory — `<project dir>/presets` unless a `library`
path is given. Each user preset carries its tags, its payload, and any reason it will not fit
this build.

`save_preset` captures what something is set to right now: an `asset` (a synth, a generator or a
sampler instrument), or a `track_chain` / `bus_chain` for a whole effect rack. The preset kind
follows from the target. `apply_preset` puts one back — a chain preset replaces the rack in one
batch, so a strip is never half-configured.

`import_preset` and `export_preset` move a preset between libraries as a single file.

Incompatibilities are reported rather than assumed: a preset saved at another state version is
applied and listed in the response's `incompatibilities`, while a preset written by a newer build
of the format is refused with `incompatible_preset`, because guessing at a format you do not know
is worse than saying so.

## Bundles and relinking

`bundle_project` packs a project into a directory: the project file, every managed sample under
`assets/`, and the preset library beside it. Paths inside are rewritten to point within the
bundle, so it opens the same on another machine, in another folder, under another user name. A
sample that cannot be read is reported and the bundle is still written — a project missing one
sound beats no bundle at all.

`check_assets` reports what a project names but cannot read, and any path that would not survive
the trip: that is what "portable" is checked with.

`diagnose` collects everything a bug report needs about a project file: what schema version it
declares against what this build supports, whether it opens and why not, validation diagnostics,
every asset's presence, size, fingerprint match and decode error, the extension type IDs it
references (including ones this build does not have), and what recovery material is on disk. It
works on a project that will not open, and it never writes to the project it reports on. Given a
`destination`, it writes `diagnostics.json` and a copy of the project as found into that
directory. Rules behind it: `docs/design/crash-recovery-and-compatibility.md`.

`relink_assets` searches the paths it is given for the audio a project is missing, matching by
content fingerprint rather than by name, and repoints the project through an ordinary command —
so relinking undoes, and an open editor sees it. Anything still missing comes back in
`unresolved`.
