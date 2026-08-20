# Release Checklist

What is covered, what proves it, and what is still missing. Every "covered" line names the test
or document standing behind it, so this is a record rather than a claim. Limits are listed as
plainly as the coverage — a checklist that only says yes is not a checklist.

Recorded against **326 automated tests**, all green under `cargo quality` on
`x86_64-pc-windows-msvc`.

## Keyboard control

| What | Evidence |
| --- | --- |
| Transport, editing, markers, loop, save, undo/redo all have keys | `JutsuAudioApp::shortcuts` in `src/main.rs`; the full list is `src/shortcuts_help.rs::GROUPS` |
| Every clip is reachable without a mouse — Tab and Shift+Tab walk the arrangement in time order, wrapping | `shortcuts_help::tests::tab_walks_the_arrangement_in_time_order_and_wraps`, `shift_tab_walks_the_other_way` |
| Deleting the selected clip does not strand the keyboard | `shortcuts_help::tests::a_selection_that_is_gone_starts_the_walk_over` |
| The key list is discoverable in the app | `?` or `F1` opens `shortcuts_help::prompt` |
| Typing in a field never triggers a shortcut | `shortcuts` returns early on `wants_keyboard_input` |

**Limits.** Selecting a *range* of clips, and moving a selected clip, still need the mouse. Panel
focus order is egui's default rather than a designed one.

## Focus and labels

| What | Evidence |
| --- | --- |
| Screen-reader metadata is published | AccessKit, through eframe — every widget is built from egui primitives that carry their own accessible name |
| Controls carry text, not only glyphs | mixer strips, transport and inspector build from `theme::flat_button` and `RichText` labels; `ui_harness::tests` reads back the text actually laid out and asserts on it |
| Non-obvious controls explain themselves on hover | `on_hover_text` on the mute/solo chips, the audio-device status, the recovery choices |
| Meaning is never carried by colour alone | mute and solo read `M`/`S`; the audio status reads `no audio device`; diagnostics are words |
| Every foreground the interface draws clears WCAG 2.1 AA on the surface it lands on | `contrast::tests::every_colour_the_interface_draws_meets_its_threshold`, over the pairs in `src/contrast.rs` |
| The measurement itself is checked against published vectors | `contrast::tests::the_published_vectors_come_out_right` |

**Limits.** No screen-reader run has been done on any platform (`tasks/misc/M04`). Contrast is
asserted for the pairs the interface uses; a new colour is only covered once it is added to
`PAIRS`.

## Scalable interface

| What | Evidence |
| --- | --- |
| The whole interface scales, 0.2× to 5× | egui's `zoom_with_keyboard` (on by default): `Ctrl +`, `Ctrl -`, `Ctrl 0` |
| Timeline zoom is independent of interface scale, and clamped to a usable range | `timeline::tests::zoom_is_clamped_to_a_usable_range`, `zooming_keeps_the_time_under_the_pointer_in_place` |
| Long names are elided rather than overrunning their column | `tests::long_labels_are_elided_rather_than_overrunning_their_column` |
| Grid labels stay readable as the view changes | `tests::grid_labels_switch_from_milliseconds_to_seconds`, `timeline::tests::grid_gets_coarser_as_the_view_zooms_out` |

**Limits.** No minimum window size is enforced; panels can be squeezed until they are unusable.

## Error recovery

| What | Evidence |
| --- | --- |
| A crash never loses both the saved file and the parked work | `power_loss_between_autosave_and_save_keeps_both_the_saved_and_the_unsaved_state` |
| A failed write cannot replace a good project | `a_torn_write_cannot_replace_a_good_project_with_a_broken_one` |
| A project from a newer build is refused untouched; an older one migrates with a backup | `a_project_from_a_newer_build_is_refused_without_touching_it`, `a_migration_keeps_the_file_it_migrated_from` |
| A damaged sample silences its own clip only | `a_damaged_sample_silences_its_own_clip_and_nothing_else` |
| A missing extension does not stop editing or export | `a_project_needing_an_extension_this_build_lacks_still_edits_and_exports` |
| An unreadable project can still be reported on | `a_project_that_will_not_open_can_still_be_reported_on` |
| No audio device is said out loud, with a retry | `src/audio_setup.rs`, wired at startup and again on play |
| Rules written down | `docs/design/crash-recovery-and-compatibility.md` |

**Limits.** Autosave keeps two generations, not a history. Recovery is offered at open, so a
crash immediately after a *manual* discard has nothing to offer back.

## Human workflows

| What | Evidence |
| --- | --- |
| A sound effect, start to finish | `a_sample_becomes_a_layered_trimmed_faded_looped_and_exported_sound`; written up in `docs/workflows/first-sfx-edit.md` |
| A music cue, start to finish | `a_pattern_becomes_an_arranged_mixed_and_exported_cue`; `docs/workflows/first-music-cue.md` |
| A refused edit mid-workflow leaves the project alone | `a_refused_edit_in_the_middle_of_the_workflow_leaves_the_project_alone` |
| Generated SFX are reproducible across runs and machines | `a_golden_seed_previews_identically_every_run`, `a_generated_clip_is_audible_in_an_export_and_stays_the_same_across_exports` |

| No panel draws one label on top of another | `ui_harness::Frame::overlaps`, asserted for the mixer — the check that caught `tasks/bugs/B02` |
| The panels themselves draw and respond | `ui_harness::tests` — the timeline labels its tracks and clips and selects the clip that is clicked, the mixer draws a strip per track and asks for a bus when its button is clicked, and each modal says what it is for |

**Limits.** The transport and the inspector are drawn inside `JutsuAudioApp` rather than in a
panel function, so the harness cannot reach them without restructuring `main.rs`; they are
covered only through the state they change.

## CLI and GUI live together

| What | Evidence |
| --- | --- |
| An editor owns its project; the CLI edits through it | `edits_report_the_route_they_took_and_the_revision_they_produced` |
| Two writers cannot both win | `cli_session::tests::a_project_another_writer_holds_is_refused_rather_than_overwritten` |
| A crashed editor does not lock the project forever | `a_session_file_left_by_a_crashed_editor_falls_back_to_an_offline_edit` |
| A retried request is applied once | `a_replayed_request_id_is_answered_once_by_the_editor` |
| A stale revision is refused without side effects | `a_stale_expected_revision_is_refused_and_leaves_the_project_alone` |
| Batches are all-or-nothing and refuse to run behind an editor | `a_batch_that_fails_partway_leaves_the_project_exactly_as_it_was`, `a_batch_refuses_to_run_behind_a_live_editor` |
| A script needs no GUI and no prose | `a_representative_script_works_from_discovery_alone`; `describe_protocol_lists_every_operation_the_build_accepts` |

## Sound design

| What | Evidence |
| --- | --- |
| A synth whose tone moves while a note is held: ADSR, resonant filter with its own envelope, unison | `crates/jutsu-audio-extensions/src/subtractive.rs` tests — a held note's high-frequency energy falls by more than a factor of three as the filter closes, and the stack detunes around the played pitch rather than off it |
| Its oscillators are band-limited | `band_limited` in the same module: polynomial correction on every edge, so a saw is an instrument rather than an aliasing pattern |
| An effect parameter can be automated | `crates/jutsu-audio-engine/tests/effect_automation.rs` — a cutoff lane opens a filter across a render, an un-automated insert renders from its stored value, and a delay's tail survives the block boundaries the sweep is made of |
| An insert can listen to another strip | `crates/jutsu-audio-engine/tests/sidechain.rs` — a keyed compressor ducks its tone under each kick, an unkeyed one does not move, and a key that is not there leaves the mix rendering |
| A delay can be given beats instead of milliseconds | `sync_beats` in `crates/jutsu-audio-extensions/src/effects/delay.rs`, resolved against the tempo the host offers each block |
| EQ, saturation, chorus and a lookahead limiter | `crates/jutsu-audio-extensions/src/effects/`; every one conformance-checked by `tests/conformance.rs` |

| A track can send a copy of itself somewhere | `crates/jutsu-audio-engine/tests/sends.rs` — a unity send doubles what the master hears without taking anything from the output, a send carries its own level, and pre-fader and post-fader are told apart by turning the fader down |
| Convolution against a real impulse response | `crates/jutsu-audio-extensions/src/effects/convolution.rs` tests — a one-spike impulse returns the signal unchanged, a spike at frame 400 delays by exactly 400, a decaying impulse leaves a tail, and a missing impulse passes audio through |

**Limits.** Sends come from tracks, not from buses. The convolver truncates an impulse at eight
seconds, resamples linearly, and leaves the last partial block of a render silent — all three are
written down in its own module rather than only here. Nothing renders the reverb tail past the
end of the timeline; `tail_frames` reports how long it would be.

## Export

| What | Evidence |
| --- | --- |
| Loudness is measured, not estimated | `crates/jutsu-audio-engine/src/loudness.rs` — integrated LUFS against the published 1 kHz check (−23 dBFS reads −23 LUFS within 0.1), plus sample and four-times-oversampled true peak |
| Stems, from the same render as the master | `tests/cli_protocol.rs::stems_are_written_per_track_and_sum_back_to_the_master` — the files add back up to the mix sample for sample |
| The loop survives the export | `an_exported_wav_carries_the_projects_loop_points`, `exporting_the_loop_region_marks_the_whole_file_as_the_loop` — written as a `smpl` chunk and read back out of the file itself |
| A repeated one-shot is not the same sound every time | `a_variation_set_cycles_its_versions_and_repeats_exactly` — three seeds, five placements, cycling, and the same request names the same set again |
| Playback and export produce identical audio | `crates/jutsu-audio-engine/tests/offline_export.rs`, `render_parity.rs` |
| Block size does not change what is heard | `block_size_does_not_change_what_is_played` |
| Export covers the timeline and not the tail beyond it | `an_export_covers_the_timeline_and_not_the_tail_beyond_it` |
| What the mix had to work around is reported, never swallowed | `export_wav` returns `diagnostics`; `a_damaged_sample_silences_its_own_clip_and_nothing_else` |
| Export works with no audio device at all | `docs/release.md` smoke checklist; `xtask/src/smoke.rs` exports without opening a device |

## Performance

| What | Evidence |
| --- | --- |
| The audio callback never allocates — verbatim, converting, seeking, looping, swapping a mix, or underrunning | `crates/jutsu-audio-engine/tests/realtime_safety.rs`, five tests |
| The callback never locks | `callback_observes_snapshot_exchange_without_locks` |
| A new mix fades in rather than clicking | `a_mix_published_during_playback_fades_in_rather_than_stepping` |
| Mixdown throughput, measured not asserted | `cargo bench -p jutsu-audio-engine`; budgets in `docs/design/performance-budgets.md` |

**Limits.** Throughput numbers are printed, not enforced — a shared machine has no business
failing a build for being busy. There is no automated regression alarm on them.

## Release

| What | Evidence |
| --- | --- |
| Reproducible package with checksums, notices and install notes | `cargo package-release`; `xtask/tests/release_package.rs` |
| The packaged binaries actually run | `cargo smoke <dir>`; verified on `x86_64-pc-windows-msvc` |
| Install, PATH, upgrade and uninstall are documented for the user | generated `INSTALL.md` |
| Windows installs with one command: Start Menu entry, PATH, in-place upgrade, reversible uninstall | `installer/install.ps1`; `xtask/tests/release_package.rs` — it parses, it never writes the machine environment, and the Windows release carries it |
| Process, platforms and signing | `docs/release.md` |

**Limits, and the honest bottom line.** The macOS and Linux targets in `docs/release.md` have not
been built or smoke-tested — only Windows has. Nothing is signed or notarised; the layout is
ready for someone with the keys — so the installer, like the binaries, trips SmartScreen on a
machine that has not seen it before. The by-hand items in `docs/release.md` — hearing playback,
opening an export elsewhere — have not been recorded on any machine yet,
and no accessibility audit with real assistive technology has been run. Those four gaps are
tracked as `tasks/misc/M01`–`M04` rather than left in prose.
