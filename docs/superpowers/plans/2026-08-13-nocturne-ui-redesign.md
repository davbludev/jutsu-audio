# Jutsu Audio Nocturne UI Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Work inline unless the user explicitly authorizes subagents.

**Goal:** Replace the functional MVP shell with the approved premium Nocturne editor while preserving all existing audio, project, and command behavior.

**Architecture:** Keep the redesign inside the native eframe GUI. Reuse `JutsuAudioApp` state and every existing action method; change only composition and painting. Add a small set of module-local visual tokens and drawing helpers so repeated styling stays consistent without introducing a component framework.

**Tech Stack:** Rust, eframe/egui, existing Jutsu Audio workspace crates.

## Global Constraints

- Work on the current branch; do not create or switch branches.
- Add no UI framework or runtime dependency.
- Keep all project mutations routed through `ProjectCommandEngine`.
- Preserve Open, Save, Import, Export, playback, selection, drag, zoom, split, delete, and gain behavior.
- Keep blocking work, allocation, I/O, logging, and project mutation outside the audio callback.
- Keep the existing 1080 x 680 minimum viewport usable.
- Do not commit unless the user explicitly requests it.

## File map

- Modify `.gitignore`: keep visual-companion artifacts out of source control.
- Modify `src/main.rs`: visual tokens, layout, controls, and painter helpers for the native editor.
- Reference `docs/superpowers/specs/2026-08-13-nocturne-ui-redesign-design.md`: approved behavior and acceptance criteria.
- Reference `.superpowers/brainstorm/*/content/nocturne-concept.png`: approved visual target; ignored and not shipped.

---

### Task 1: Establish Nocturne visual system and shell

**Files:**

- Modify: `src/main.rs:10-21`
- Modify: `src/main.rs:363-438`
- Modify: `src/main.rs:732-749`

**Interfaces:**

- Consumes: existing `JutsuAudioApp` fields and action methods.
- Produces: module-local palette constants, `panel_frame(fill: Color32) -> egui::Frame`, `section_label(ui: &mut egui::Ui, text: &str)`, and the final top/bottom shell used by later tasks.

- [ ] **Step 1: Record baseline behavior**

Run:

```powershell
cargo test --workspace
```

Expected: all existing workspace tests pass before visual changes.

- [ ] **Step 2: Replace palette and configure native visuals**

Use these exact primary tokens and derive only nearby neutral shades where egui widgets require them:

```rust
const BG: Color32 = Color32::from_rgb(11, 9, 16);
const PANEL: Color32 = Color32::from_rgb(23, 18, 31);
const ELEVATED: Color32 = Color32::from_rgb(37, 27, 49);
const VIOLET: Color32 = Color32::from_rgb(167, 123, 255);
const CORAL: Color32 = Color32::from_rgb(255, 128, 102);
const TEXT: Color32 = Color32::from_rgb(244, 239, 248);
const TEXT_MUTED: Color32 = Color32::from_rgb(156, 147, 170);
const BORDER: Color32 = Color32::from_rgb(55, 43, 68);
```

Update `configure_style` with 8 px widget radii, 8 px item spacing, 12 x 8 px button padding, violet selection, readable inactive/hovered widget fills, and warm text colors.

- [ ] **Step 3: Build the structural shell**

Change `update`, `top_bar`, and `status_bar` so the frame order is:

```text
top title/menu bar
bottom status/audio strip
left navigation rail
left sample browser
right clip inspector
central timeline
```

Keep top-bar project actions wired directly to the current methods. Show project name/path context, transport menu grouping, and compact buttons without adding dormant menus.

In the bottom strip show existing `status`, `format_time(self.transport.position_frames())`, zoom, `48 kHz`, and a violet level-meter treatment. The level meter is presentation-only and must not add audio-thread communication.

- [ ] **Step 4: Check shell at minimum size**

Run:

```powershell
cargo fmt --check
cargo check --all-targets --all-features
```

Expected: both commands exit 0; no clipping-causing fixed central width is introduced.

### Task 2: Redesign sample browser and clip inspector

**Files:**

- Modify: `src/main.rs:439-567`

**Interfaces:**

- Consumes: `selected_asset`, `selected_clip`, `gain_db`, `import_wav`, `add_selected_asset`, `update_selected_clip`, `split_selected`, and `delete_selected`.
- Produces: Nocturne sample-browser and inspector regions while retaining identical command calls.

- [ ] **Step 1: Redesign sample browser**

Keep current asset iteration and selection logic. Render:

```text
Samples                         Import +
[ Search sounds                         ]
IMPORTED
[selected marker] asset name      duration
                  WAV metadata
...
Drag and drop WAV files here or click Import
```

Use the existing import button for the drop-zone click; do not claim OS drag-and-drop unless implemented by existing eframe input. If asset metadata is not already available without I/O, show only data held in the project model.

- [ ] **Step 2: Redesign selected and empty inspector states**

For a selected clip, group asset identity, gain, start, duration, Split, and Delete. Preserve current `DragValue` and slider update paths. Use violet outline styling for Split and coral outline styling for Delete.

For no selection, show a quiet empty state explaining that selecting a clip reveals controls. Do not fabricate disabled controls.

- [ ] **Step 3: Verify unchanged editing paths**

Run:

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Expected: all tests pass and Clippy exits 0 with no warnings.

### Task 3: Build premium timeline, waveforms, and floating transport

**Files:**

- Modify: `src/main.rs:568-731`
- Modify: `src/main.rs:750-770`

**Interfaces:**

- Consumes: existing clip collection, selection, drag, zoom, placement, playhead, transport, and `SAMPLE_RATE` behavior.
- Produces: `draw_waveform(painter: &egui::Painter, rect: egui::Rect, color: Color32, seed: u64)`, `draw_transport_overlay(&mut self, ui: &mut egui::Ui, rect: egui::Rect)`, and the final timeline surface.

- [ ] **Step 1: Recompose timeline chrome**

Render a compact header with `Timeline`, editing-tool affordances that correspond to existing selection/drag behavior, and zoom controls. Keep inactive future tools out.

Add a readable ruler, major/minor grid, track header column, lane background, and empty lane drop guidance. Preserve the current conversion between frames, seconds, pixels, and zoom.

- [ ] **Step 2: Upgrade clip painting**

Give clips a dark tinted fill, top label strip, selection glow/border, resize-handle marks, and waveform color. Use violet for the primary clip and coral for a contrasting later clip, determined stably from clip order rather than project mutation.

Keep deterministic waveform fallback but make it asset-specific by passing a stable seed derived from clip or asset identity already available in memory. Do not add file I/O to painting. If cached peaks are not exposed through the current API, leave shared crates unchanged.

- [ ] **Step 3: Add floating transport**

Place a compact elevated transport near the bottom-center of the timeline. Wire Play/Pause, Stop, and timecode to the existing `TransportController`; do not duplicate transport state. Ensure overlay consumes input only inside its own rectangle and does not break clip dragging elsewhere.

- [ ] **Step 4: Preserve pointer editing**

Confirm clip click, clip drag, first-clip placement, zoom, playhead, split, and delete still route through existing code paths. Resolve overlapping egui interaction IDs with stable IDs derived from clip IDs and control names.

- [ ] **Step 5: Compile and run focused checks**

Run:

```powershell
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Expected: every command exits 0.

### Task 4: Native visual QA and final validation

**Files:**

- Modify: `src/main.rs` only for visual or interaction defects found during this task.
- Modify: `AGENTS.md` only if the GUI entry point or responsibility changed materially.

**Interfaces:**

- Consumes: completed Nocturne editor.
- Produces: validated empty and populated native-app states with no known MVP regression.

- [ ] **Step 1: Launch empty state**

Run:

```powershell
cargo run
```

Capture the native window at a representative desktop size and at the 1080 x 680 minimum. Check text contrast, panel hierarchy, control alignment, and absence of clipped or overlapping regions.

- [ ] **Step 2: Smoke-test populated state**

Use `demo_assets/LightCannon_A_Shoot.wav` through the UI import flow. Confirm the source WAV is not modified. Add it to the timeline, select it, adjust gain, drag it, play/stop, split, and export to an ignored temporary path under `target/`.

- [ ] **Step 3: Compare against approved concept**

Compare native screenshots with `nocturne-concept.png` for:

```text
deep plum neutral hierarchy
violet signal restraint
sample browser density
timeline dominance and waveform richness
floating transport placement
inspector grouping
bottom status strip
```

Fix only concrete mismatches or usability defects. Do not add concept-only features absent from the MVP.

- [ ] **Step 4: Run final gate**

Run:

```powershell
cargo quality
git diff --check
git status --short
```

Expected: `cargo quality` and `git diff --check` exit 0. Status contains only redesign files and approved design/plan documentation; `.superpowers/` remains ignored.

- [ ] **Step 5: Report completion**

Report changed files, validation commands, visual comparison result, intentional concept deviations, and remaining limitations. Do not commit unless the user explicitly asks.
