# Jutsu Audio Nocturne UI redesign

## Goal

Make the native desktop editor feel premium, modern, and purpose-built while preserving every existing MVP workflow. The approved visual reference is the Nocturne concept generated during design review.

## Scope

- Restyle and reorganize the existing eframe application shell in `src/main.rs`.
- Keep project persistence, commands, asset import, playback, clip editing, and WAV export behavior unchanged.
- Add no UI framework or runtime dependency.
- Keep all project mutations routed through `ProjectCommandEngine`.

## Layout

The window uses five coordinated regions:

1. A slim top menu/title bar for project identity and global actions.
2. A narrow navigation rail for editor-area identity and future navigation affordances.
3. A sample browser with search treatment, richer asset rows, metadata, selection state, and a clear import action.
4. A dominant timeline with toolbar, time ruler, track header, detailed waveform clips, selection, playhead, and a centered floating transport.
5. A contextual clip inspector plus a bottom status strip for project state, zoom, sample rate, and save status.

The empty state uses the same composition and teaches the import-to-timeline flow without inserting fake project content.

## Visual system

- Background: near-black with deep plum panel separation.
- Elevated surfaces: restrained purple-black layers with subtle borders and shadows.
- Primary signal: luminous violet for selection, playback, waveform focus, and active controls.
- Secondary signal: coral only for destructive actions or a contrasting clip.
- Text: warm off-white with lavender-gray secondary labels.
- Corners: modest radii; panels remain structural rather than becoming detached dashboard cards.
- Typography: strong section hierarchy, compact metadata, and monospaced timecode where useful.
- Spacing: dense enough for editing, with consistent 4/8 px-derived rhythm and larger separation only between major regions.

## Interaction behavior

- Existing Open, Save, Import, Export, playback, selection, drag, zoom, split, delete, and gain interactions remain available.
- Transport remains immediately reachable and visually dominant without obscuring clips.
- Selected asset and clip states have distinct border, fill, and text contrast.
- Destructive Delete styling is visually separate from primary actions.
- Current keyboard/pointer behavior remains intact; visual helpers must not introduce project mutation paths.

## Implementation boundaries

- Centralize palette and spacing tokens in the GUI module.
- Extract only small drawing/layout helpers needed by multiple regions; avoid speculative component architecture.
- Prefer native egui painting and installed icon/text capabilities.
- Use cached waveform peaks when exposed by current project APIs. If not accessible without changing shared crates, retain the deterministic waveform fallback and document that limitation in the handoff.

## Error and state presentation

Existing status messages remain the source of truth. The redesign gives them a stable bottom-strip location and preserves clear failure text for open, save, import, edit, playback, and export errors. Missing selection or project path continues to produce the current safe no-op/status behavior.

## Verification

- Run formatting, Clippy, tests, and benchmarks through `cargo quality`.
- Launch the native app and capture the main window in both empty and populated states when practical.
- Compare the populated render against the approved Nocturne concept for hierarchy, palette, density, timeline prominence, transport placement, inspector treatment, and status strip.
- Smoke-test import using a WAV from `demo_assets` without modifying the source file.

## Acceptance criteria

- The native editor visibly matches the approved Nocturne direction rather than the previous flat MVP shell.
- Existing MVP actions remain functional.
- No shared model, command, project, engine, CLI, or export behavior regresses.
- The UI remains usable at the existing 1080 x 680 minimum viewport.
- `cargo quality` passes.
