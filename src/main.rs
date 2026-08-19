//! Jutsu Audio desktop shell.
//!
//! The UI thread only draws and dispatches. Every decode, mixdown, disk write
//! and file dialog happens on the worker (see [`worker`]), and every project
//! mutation goes through the command engine — never directly.

mod external_changes;
mod recovery;
mod theme;
mod timeline;
mod worker;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText, Stroke, Vec2};
use jutsu_audio_commands::{
    COMMAND_PROTOCOL_VERSION, ChangeEvent, CommandEnvelope, CommandHistory, CommandId, EntityKind,
    ProjectCommand, ProjectCommandEngine,
};
use jutsu_audio_engine::{
    SnapshotExchange, SystemAudioOutput, TransportController, TransportState,
};
use jutsu_audio_model::{
    AssetId, AudioAssetSource, Clip, ClipId, LayerId, ParameterValue, Project, TrackId,
};
use jutsu_audio_project::{ImportStatus, ProjectStore, autosave};
use jutsu_audio_session::TransportAction;

use jutsu_audio::session_host::{ExternalEffect, SessionHost};
use recovery::{Decision, Recovery};

use timeline::{
    TimelineAction, TimelineContext, TimelineView, Tool, WaveformState, clip_gain_db,
    has_mixed_sample_rates, project_duration_frames, project_sample_rate,
};
use worker::{Job, JobResult, MixRequest, Worker, resolve_asset_path};

/// How long editing has to settle before the project is written to disk.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(700);
/// How long editing has to settle before the timeline is re-mixed for playback.
const MIX_DEBOUNCE: Duration = Duration::from_millis(150);
/// How long editing has to settle before unsaved work is parked in the
/// recovery sidecar. Shorter than a save: this one is for crashes.
const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(2_000);
/// How long an edit that arrived over the session socket waits before the
/// timeline is re-mixed. Short: nobody is still typing on the other end.
const EXTERNAL_LATENCY: Duration = Duration::from_millis(50);
/// Meter fall-off per frame. Fast enough to follow, slow enough to read.
const METER_DECAY: f32 = 0.90;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1536.0, 900.0])
            .with_min_inner_size([1040.0, 620.0])
            .with_title("Jutsu Audio"),
        ..Default::default()
    };
    // `jutsu-audio path/to/project.json` opens that project instead of a new one,
    // which is also what a desktop "open with" association hands us.
    let opened = std::env::args_os().nth(1).map(PathBuf::from);
    eframe::run_native(
        "Jutsu Audio",
        options,
        Box::new(move |context| Ok(Box::new(JutsuAudioApp::new(context, opened)))),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tone {
    Info,
    Working,
    Error,
}

#[derive(Clone, Debug)]
struct Status {
    text: String,
    tone: Tone,
}

impl Status {
    fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Info,
        }
    }

    fn working(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Working,
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Error,
        }
    }

    fn color(&self) -> Color32 {
        match self.tone {
            Tone::Info => theme::DIM,
            Tone::Working => theme::SIGNAL,
            Tone::Error => theme::DANGER,
        }
    }
}

/// Inspector edit buffer. Kept separate from the project so a drag can preview
/// without producing a command per frame.
#[derive(Clone, Copy, Debug)]
struct ClipEdit {
    clip_id: ClipId,
    gain_db: f64,
    start: u64,
    duration: u64,
    source_start: u64,
    dirty: bool,
}

impl ClipEdit {
    fn from_clip(clip: &Clip) -> Self {
        Self {
            clip_id: clip.id,
            gain_db: clip_gain_db(clip),
            start: clip.start_sample,
            duration: clip.duration_samples,
            source_start: clip.source_start_sample,
            dirty: false,
        }
    }
}

struct JutsuAudioApp {
    commands: ProjectCommandEngine,
    history: CommandHistory,
    project_path: Option<PathBuf>,

    selected_asset: Option<AssetId>,
    selected_clip: Option<ClipId>,
    edit: Option<ClipEdit>,
    filter: String,

    transport: TransportController,
    snapshots: SnapshotExchange,
    _audio: Option<SystemAudioOutput>,
    audio_error: Option<String>,
    meter: f32,

    worker: Worker,
    next_mix_id: u64,
    active_mix: Option<u64>,
    mix_due: Option<Instant>,
    save_due: Option<Instant>,
    autosave_due: Option<Instant>,
    save_in_flight: bool,
    unsaved: bool,
    dialog_open: bool,
    exporting: bool,

    waveforms: HashMap<AssetId, WaveformState>,
    timeline: TimelineView,
    status: Status,
    /// Live while this window owns a project on disk. `None` for an unsaved
    /// project: there is no path for a client to name yet.
    session: Option<SessionHost>,
    /// Unsaved work found after a crash, waiting for the user to decide.
    recovery: Option<Recovery>,
}

impl JutsuAudioApp {
    fn new(context: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
        theme::configure(&context.egui_ctx);
        // A project named on the command line replaces the blank one, but a bad
        // path must not stop the editor from starting.
        let (project, project_path, status) = match open {
            Some(path) => match ProjectStore::open(&path) {
                Ok(opened) => (opened.project, Some(path), Status::info("Project opened")),
                Err(error) => (
                    ProjectStore::new_project("Untitled Project"),
                    None,
                    Status::error(format!("Could not open that project: {}", error.message)),
                ),
            },
            None => (
                ProjectStore::new_project("Untitled Project"),
                None,
                Status::info("Ready"),
            ),
        };
        let commands = ProjectCommandEngine::new(project).unwrap_or_else(|_| {
            ProjectCommandEngine::new(ProjectStore::new_project("Untitled Project"))
                .expect("a new project is valid")
        });
        // A project named on the command line can also carry unsaved work from
        // a crash. Offered, never applied on its own.
        let recovery = project_path
            .as_ref()
            .and_then(|path| autosave::recover(path).ok().flatten())
            .map(|recovered| Recovery {
                project: Box::new(recovered.project),
            });
        let transport = TransportController::new();
        let snapshots = SnapshotExchange::new(None);
        let (audio, audio_error) =
            match SystemAudioOutput::open_default(snapshots.reader(), transport.reader()) {
                Ok(output) => (Some(output), None),
                Err(error) => (None, Some(format!("{error:?}"))),
            };
        Self {
            commands,
            history: CommandHistory::new(),
            project_path,
            selected_asset: None,
            selected_clip: None,
            edit: None,
            filter: String::new(),
            transport,
            snapshots,
            _audio: audio,
            audio_error,
            meter: 0.0,
            worker: Worker::spawn(context.egui_ctx.clone()),
            next_mix_id: 0,
            active_mix: None,
            // A project opened at startup still needs its audio built.
            mix_due: Some(Instant::now()),
            save_due: None,
            autosave_due: None,
            save_in_flight: false,
            unsaved: false,
            dialog_open: false,
            exporting: false,
            waveforms: HashMap::new(),
            timeline: TimelineView::default(),
            status,
            session: None,
            recovery,
        }
    }

    fn project(&self) -> &Project {
        self.commands.project()
    }

    fn sample_rate(&self) -> u32 {
        project_sample_rate(self.project())
    }

    // ─── mutation ───────────────────────────────────────────────────────────

    /// The single door into the command engine. Everything that edits the
    /// project comes through here, and nothing here touches the disk.
    fn apply(&mut self, commands: Vec<ProjectCommand>) -> bool {
        let envelope = CommandEnvelope {
            protocol_version: COMMAND_PROTOCOL_VERSION,
            command_id: CommandId::new(),
            expected_revision: self.commands.revision(),
            commands,
        };
        match self.history.apply(&mut self.commands, envelope) {
            Ok(_) => {
                self.mark_edited(MIX_DEBOUNCE);
                true
            }
            Err(error) => {
                self.status = Status::error(format!("Edit rejected: {}", error.message));
                // The engine rolled back, so the edit buffer is now ahead of
                // the project. Drop it and let the next frame rebuild it.
                self.edit = None;
                false
            }
        }
    }

    /// Records that the project changed: dirty flag, and the three deadlines
    /// that follow an edit. `mix_after` differs for interactive and external
    /// edits, and none of the deadlines is ever pushed back by a later edit —
    /// a burst must not starve the work.
    fn mark_edited(&mut self, mix_after: Duration) {
        let now = Instant::now();
        self.unsaved = true;
        self.save_due.get_or_insert(now + SAVE_DEBOUNCE);
        self.autosave_due.get_or_insert(now + AUTOSAVE_DEBOUNCE);
        self.mix_due.get_or_insert(now + mix_after);
    }

    /// Reverses the last edit made to this project, whoever made it: the
    /// history is chronological and shared with the session socket.
    fn undo(&mut self) {
        match self.history.undo(&mut self.commands) {
            Some(Ok(_)) => {
                self.mark_edited(MIX_DEBOUNCE);
                self.edit = None;
                self.status = Status::info("Undone");
            }
            Some(Err(error)) => {
                self.status = Status::error(format!("Undo failed: {}", error.message));
            }
            None => self.status = Status::info("Nothing to undo"),
        }
    }

    fn redo(&mut self) {
        match self.history.redo(&mut self.commands) {
            Some(Ok(_)) => {
                self.mark_edited(MIX_DEBOUNCE);
                self.edit = None;
                self.status = Status::info("Redone");
            }
            Some(Err(error)) => {
                self.status = Status::error(format!("Redo failed: {}", error.message));
            }
            None => self.status = Status::info("Nothing to redo"),
        }
    }

    /// A track flag as the mix reads it. Absent counts as off.
    fn track_flag(&self, track_id: TrackId, key: &str) -> bool {
        self.project()
            .tracks
            .iter()
            .find(|track| track.id == track_id)
            .is_some_and(|track| timeline::track_flag(track, key))
    }

    /// Appends a track with one empty layer, ready to drop a sample onto.
    fn add_track(&mut self) {
        let track = jutsu_audio_model::Track {
            id: TrackId::new(),
            name: format!("Track {}", self.project().tracks.len() + 1),
            output_bus_id: self.project().master_bus_id,
            parameters: std::collections::BTreeMap::new(),
            layers: vec![jutsu_audio_model::Layer {
                id: LayerId::new(),
                name: "Layer 1".into(),
                clips: Vec::new(),
            }],
        };
        if self.apply(vec![ProjectCommand::AddTrack { track }]) {
            self.status = Status::info("Track added");
        }
    }

    /// Appends a lane to the track holding the selection, or to the last track
    /// when nothing is selected.
    fn add_layer(&mut self) {
        let Some(track_id) = self
            .selected_clip
            .and_then(|clip_id| self.lane_of(clip_id))
            .map(|(track_id, _)| track_id)
            .or_else(|| self.project().tracks.last().map(|track| track.id))
        else {
            self.status = Status::error("Add a track first");
            return;
        };
        let count = self
            .project()
            .tracks
            .iter()
            .find(|track| track.id == track_id)
            .map_or(0, |track| track.layers.len());
        let layer = jutsu_audio_model::Layer {
            id: LayerId::new(),
            name: format!("Layer {}", count + 1),
            clips: Vec::new(),
        };
        if self.apply(vec![ProjectCommand::AddLayer { track_id, layer }]) {
            self.status = Status::info("Layer added");
        }
    }

    fn selected_clip(&self) -> Option<&Clip> {
        let id = self.selected_clip?;
        self.project()
            .tracks
            .iter()
            .flat_map(|track| &track.layers)
            .flat_map(|layer| &layer.clips)
            .find(|clip| clip.id == id)
    }

    fn select_clip(&mut self, clip_id: Option<ClipId>) {
        if self.selected_clip == clip_id {
            return;
        }
        self.selected_clip = clip_id;
        // Rebuild the edit buffer from the project, never carry the old clip's
        // values across — that is how gain used to leak between clips.
        self.edit = self.selected_clip().map(ClipEdit::from_clip);
    }

    /// Keeps the inspector buffer in step with the project when the user is not
    /// actively editing it.
    fn sync_edit(&mut self) {
        let current = self.selected_clip().cloned();
        match (current, self.edit) {
            (Some(clip), Some(edit)) if edit.clip_id == clip.id && !edit.dirty => {
                self.edit = Some(ClipEdit::from_clip(&clip));
            }
            (Some(clip), Some(edit)) if edit.clip_id != clip.id => {
                self.edit = Some(ClipEdit::from_clip(&clip));
            }
            (Some(clip), None) => self.edit = Some(ClipEdit::from_clip(&clip)),
            (None, Some(_)) => self.edit = None,
            _ => {}
        }
    }

    fn commit_edit(&mut self) {
        let Some(mut edit) = self.edit else { return };
        edit.dirty = false;
        edit.duration = edit.duration.max(1);
        self.edit = Some(edit);
        self.apply(vec![ProjectCommand::UpdateClip {
            clip_id: edit.clip_id,
            start_sample: edit.start,
            source_start_sample: edit.source_start,
            duration_samples: edit.duration,
            gain_db: edit.gain_db,
        }]);
    }

    fn add_asset_to_timeline(
        &mut self,
        asset_id: AssetId,
        track_id: TrackId,
        layer_id: LayerId,
        start_sample: u64,
    ) {
        let rate = self.sample_rate();
        let Some(asset) = self
            .project()
            .assets
            .iter()
            .find(|asset| asset.id == asset_id)
        else {
            return;
        };
        // Clip lengths live in project frames, so material recorded at another
        // rate has to be converted on the way in.
        let duration_samples = match asset.source {
            AudioAssetSource::ManagedFile {
                frame_count,
                sample_rate,
                ..
            } => ((frame_count as f64 * f64::from(rate)) / f64::from(sample_rate.max(1))) as u64,
            _ => u64::from(rate),
        }
        .max(1);

        let clip = Clip {
            id: ClipId::new(),
            asset_id,
            start_sample,
            source_start_sample: 0,
            duration_samples,
            parameters: [("gain_db".to_owned(), ParameterValue::Float(0.0))]
                .into_iter()
                .collect(),
        };
        let clip_id = clip.id;
        if self.apply(vec![ProjectCommand::AddClip {
            track_id,
            layer_id,
            clip,
        }]) {
            self.select_clip(Some(clip_id));
            self.status = Status::info("Clip added");
        }
    }

    /// Drops the selected asset onto the first lane, for the double-click and
    /// keyboard paths that have no pointer position to read.
    fn add_selected_asset_at_playhead(&mut self) {
        let Some(asset_id) = self.selected_asset else {
            return;
        };
        let Some((track_id, layer_id)) = self.first_lane() else {
            self.status = Status::error("This project has no track to add a clip to");
            return;
        };
        let start = self.transport.position_frames();
        self.add_asset_to_timeline(asset_id, track_id, layer_id, start);
    }

    fn first_lane(&self) -> Option<(TrackId, LayerId)> {
        let track = self.project().tracks.first()?;
        Some((track.id, track.layers.first()?.id))
    }

    fn split_selected(&mut self) {
        let Some(clip) = self.selected_clip().cloned() else {
            return;
        };
        let playhead = self.transport.position_frames();
        // Split at the playhead when it sits inside the clip, at the midpoint
        // otherwise, so the button always does something predictable.
        let offset = if playhead > clip.start_sample
            && playhead < clip.start_sample + clip.duration_samples
        {
            playhead - clip.start_sample
        } else {
            clip.duration_samples / 2
        };
        if offset == 0 || offset >= clip.duration_samples {
            self.status = Status::error("This clip is too short to split");
            return;
        }
        let Some((track_id, layer_id)) = self.lane_of(clip.id) else {
            return;
        };
        let mut right = clip.clone();
        right.id = ClipId::new();
        right.start_sample += offset;
        right.source_start_sample += offset;
        right.duration_samples -= offset;

        if self.apply(vec![
            ProjectCommand::UpdateClip {
                clip_id: clip.id,
                start_sample: clip.start_sample,
                source_start_sample: clip.source_start_sample,
                duration_samples: offset,
                // Keep the clip's own gain — not whatever the inspector shows.
                gain_db: clip_gain_db(&clip),
            },
            ProjectCommand::AddClip {
                track_id,
                layer_id,
                clip: right,
            },
        ]) {
            self.edit = None;
            self.status = Status::info("Clip split");
        }
    }

    fn duplicate_selected(&mut self) {
        let Some(clip) = self.selected_clip().cloned() else {
            return;
        };
        let Some((track_id, layer_id)) = self.lane_of(clip.id) else {
            return;
        };
        let mut copy = clip.clone();
        copy.id = ClipId::new();
        copy.start_sample = clip.start_sample + clip.duration_samples;
        let new_id = copy.id;
        if self.apply(vec![ProjectCommand::AddClip {
            track_id,
            layer_id,
            clip: copy,
        }]) {
            self.select_clip(Some(new_id));
            self.status = Status::info("Clip duplicated");
        }
    }

    fn delete_selected(&mut self) {
        let Some(clip_id) = self.selected_clip else {
            return;
        };
        if self.apply(vec![ProjectCommand::RemoveClip { clip_id }]) {
            self.select_clip(None);
            self.status = Status::info("Clip deleted");
        }
    }

    fn lane_of(&self, clip_id: ClipId) -> Option<(TrackId, LayerId)> {
        self.project().tracks.iter().find_map(|track| {
            track.layers.iter().find_map(|layer| {
                layer
                    .clips
                    .iter()
                    .any(|clip| clip.id == clip_id)
                    .then_some((track.id, layer.id))
            })
        })
    }

    // ─── background work ────────────────────────────────────────────────────

    // ─── live session ───────────────────────────────────────────────────────

    /// Keeps the hosted session pointed at the project currently open. Opening,
    /// saving somewhere new, or closing all move it.
    fn sync_session(&mut self, context: &egui::Context) {
        let wanted = self.project_path.as_deref();
        if self.session.as_ref().map(SessionHost::path) == wanted {
            return;
        }
        self.session = None;
        let Some(path) = wanted else { return };
        let context = context.clone();
        match SessionHost::start(path, move || context.request_repaint()) {
            Ok(host) => self.session = Some(host),
            Err(message) => {
                // The editor still works; only the machine surface is missing.
                self.status = Status::error(format!("Live session unavailable: {message}"));
            }
        }
    }

    /// Answers whatever arrived over the session socket and folds the result
    /// into the editor, exactly as if the user had made the edit here.
    fn poll_session(&mut self) {
        let Self {
            session,
            commands,
            history,
            ..
        } = self;
        let Some(host) = session.as_ref() else { return };
        let effects = host.poll(commands, history, self.unsaved);
        for effect in effects {
            match effect {
                ExternalEffect::Applied { revision, changes } => {
                    self.absorb_external(&changes);
                    self.mark_edited(EXTERNAL_LATENCY);
                    self.status = Status::info(format!("External edit applied (r{revision})"));
                }
                ExternalEffect::Transport {
                    action,
                    position_frames,
                } => match action {
                    TransportAction::Play => self.play(),
                    TransportAction::Pause => self.transport.pause(),
                    TransportAction::Stop => self.transport.stop(),
                    TransportAction::Seek => self.transport.seek(position_frames),
                },
            }
        }
    }

    /// Follows an external edit into the state the project does not own.
    /// Selection is keyed by entity ID, so an update keeps it and only a
    /// removal drops it.
    fn absorb_external(&mut self, changes: &[ChangeEvent]) {
        if self.selected_clip.is_some_and(|clip| {
            external_changes::removes(changes, EntityKind::Clip, &clip.to_string())
        }) {
            self.select_clip(None);
        }
        if self.selected_asset.is_some_and(|asset| {
            external_changes::removes(changes, EntityKind::Asset, &asset.to_string())
        }) {
            self.selected_asset = None;
        }
        let removed = external_changes::removed_assets(changes);
        if !removed.is_empty() {
            self.waveforms
                .retain(|asset_id, _| !removed.contains(&asset_id.to_string().as_str()));
        }
    }

    fn dispatch_pending_work(&mut self) {
        let now = Instant::now();

        if self.mix_due.is_some_and(|due| now >= due) {
            self.mix_due = None;
            self.request_mixdown();
        }

        if !self.save_in_flight
            && self.save_due.is_some_and(|due| now >= due)
            && let Some(path) = self.project_path.clone()
        {
            self.save_due = None;
            self.save_in_flight = true;
            self.send(Job::Save {
                path,
                project: Box::new(self.project().clone()),
            });
        }

        if self.autosave_due.is_some_and(|due| now >= due)
            && let Some(path) = self.project_path.clone()
        {
            self.autosave_due = None;
            // Parked even while a save is in flight: the save may be writing
            // state that a further edit has already moved on from.
            self.send(Job::Autosave {
                path,
                project: Box::new(self.project().clone()),
            });
        }

        self.request_missing_waveforms();
    }

    fn request_mixdown(&mut self) {
        let Some(project_path) = self.project_path.clone() else {
            return;
        };
        let sample_rate = self.sample_rate();
        self.next_mix_id = self.next_mix_id.wrapping_add(1);
        let id = self.next_mix_id;
        if self.send(Job::Mixdown(MixRequest {
            id,
            sample_rate,
            project: Box::new(self.project().clone()),
            project_path,
        })) {
            self.active_mix = Some(id);
        }
    }

    fn request_missing_waveforms(&mut self) {
        let Some(project_path) = self.project_path.clone() else {
            return;
        };
        let wanted: Vec<(AssetId, PathBuf, String)> = self
            .project()
            .assets
            .iter()
            .filter(|asset| !self.waveforms.contains_key(&asset.id))
            .filter_map(|asset| {
                let AudioAssetSource::ManagedFile { fingerprint, .. } = &asset.source else {
                    return None;
                };
                Some((
                    asset.id,
                    resolve_asset_path(&project_path, &asset.source)?,
                    fingerprint.clone(),
                ))
            })
            .collect();
        for (asset_id, source, fingerprint) in wanted {
            self.waveforms.insert(asset_id, WaveformState::Pending);
            self.send(Job::Waveform {
                asset_id,
                project_path: project_path.clone(),
                source,
                fingerprint,
            });
        }
    }

    fn send(&mut self, job: Job) -> bool {
        if self.worker.send(job) {
            return true;
        }
        self.status = Status::error("Background worker stopped — restart Jutsu Audio");
        false
    }

    fn drain_results(&mut self) {
        while let Some(result) = self.worker.try_recv() {
            match result {
                JobResult::Mixdown { id, result } => {
                    if self.active_mix != Some(id) {
                        continue; // A newer mix is already on its way.
                    }
                    self.active_mix = None;
                    match result {
                        Ok(snapshot) => {
                            self.snapshots.publish(snapshot);
                            // Only clear a "working" message. An edit that just
                            // reported "Clip added" should keep saying so.
                            if self.status.tone == Tone::Working {
                                self.status = Status::info("Ready");
                            }
                        }
                        Err(message) => {
                            self.status =
                                Status::error(format!("Playback build failed: {message}"));
                        }
                    }
                }
                JobResult::MixdownEmpty { id } => {
                    if self.active_mix == Some(id) {
                        self.active_mix = None;
                        self.snapshots.clear();
                        self.transport.stop();
                    }
                }
                JobResult::Saved { path, result } => {
                    self.save_in_flight = false;
                    match result {
                        Ok(()) => {
                            self.project_path = Some(path);
                            // Another edit may have landed while the write was
                            // in flight; only clear the flag if none did.
                            if self.save_due.is_none() {
                                self.unsaved = false;
                                // The save already removed the sidecar; no
                                // point parking state that is now on disk.
                                self.autosave_due = None;
                                self.status = Status::info("Saved");
                            }
                            self.mix_due.get_or_insert_with(Instant::now);
                        }
                        Err(message) => {
                            self.status = Status::error(format!("Save failed: {message}"))
                        }
                    }
                    self.dialog_open = false;
                }
                JobResult::Opened(outcome) => {
                    self.dialog_open = false;
                    match *outcome {
                        Ok(opened) => match ProjectCommandEngine::new(opened.project) {
                            Ok(commands) => {
                                self.commands = commands;
                                self.project_path = Some(opened.path);
                                self.reset_for_new_project();
                                self.recovery = opened.recovered.map(|project| Recovery {
                                    project: Box::new(project),
                                });
                                self.status = if self.recovery.is_some() {
                                    Status::info("Unsaved work was recovered")
                                } else {
                                    Status::info("Project opened")
                                };
                            }
                            Err(error) => {
                                self.status =
                                    Status::error(format!("Project is invalid: {}", error.message));
                            }
                        },
                        Err(message) => {
                            self.status = Status::error(format!("Open failed: {message}"));
                        }
                    }
                }
                JobResult::Imported(outcome) => {
                    self.dialog_open = false;
                    match *outcome {
                        Ok(import) => {
                            self.project_path = Some(import.project_path);
                            if import.saved_project {
                                self.unsaved = false;
                            }
                            match (import.status, import.asset) {
                                (ImportStatus::Prepared, Some(asset)) => {
                                    let id = asset.id;
                                    if self.apply(vec![ProjectCommand::AddAsset { asset }]) {
                                        self.selected_asset = Some(id);
                                        self.status = Status::info("Sample imported");
                                    }
                                }
                                (ImportStatus::Duplicate(id), _) => {
                                    self.selected_asset = Some(id);
                                    self.status =
                                        Status::info("That sample is already in this project");
                                }
                                (ImportStatus::Prepared, None) => {
                                    self.status = Status::info("Project location saved");
                                }
                            }
                        }
                        Err(message) => {
                            self.status = Status::error(format!("Import failed: {message}"));
                        }
                    }
                }
                JobResult::Exported(result) => {
                    self.exporting = false;
                    self.dialog_open = false;
                    self.status = match result {
                        Ok(frames) => Status::info(format!(
                            "Exported {}",
                            theme::format_time(frames, self.sample_rate())
                        )),
                        Err(message) => Status::error(format!("Export failed: {message}")),
                    };
                }
                JobResult::Waveform { asset_id, result } => {
                    self.waveforms.insert(
                        asset_id,
                        match result {
                            Ok(waveform) => WaveformState::Ready(waveform),
                            Err(_) => WaveformState::Failed,
                        },
                    );
                }
                JobResult::Autosaved(result) => {
                    if let Err(message) = result {
                        self.status = Status::error(format!("Recovery file failed: {message}"));
                    }
                }
                JobResult::Cancelled => {
                    self.dialog_open = false;
                    self.exporting = false;
                    self.save_in_flight = false;
                }
            }
        }
    }

    fn reset_for_new_project(&mut self) {
        self.selected_asset = None;
        self.selected_clip = None;
        self.edit = None;
        self.waveforms.clear();
        self.filter.clear();
        self.transport.stop();
        self.snapshots.clear();
        self.timeline = TimelineView::default();
        self.unsaved = false;
        self.save_due = None;
        self.autosave_due = None;
        // Inverses only make sense against the project they were computed for.
        self.history.clear();
        self.recovery = None;
        self.mix_due = Some(Instant::now());
    }

    // ─── commands from the chrome ───────────────────────────────────────────

    fn open_project(&mut self) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;
        self.status = Status::working("Choosing a project…");
        self.send(Job::Open);
    }

    fn save_project(&mut self) {
        match self.project_path.clone() {
            Some(path) => {
                if self.save_in_flight {
                    self.save_due = Some(Instant::now());
                    return;
                }
                self.save_in_flight = true;
                self.save_due = None;
                self.send(Job::Save {
                    path,
                    project: Box::new(self.project().clone()),
                });
            }
            None => {
                if self.dialog_open {
                    return;
                }
                self.dialog_open = true;
                self.save_in_flight = true;
                self.status = Status::working("Choosing where to save…");
                self.send(Job::SaveAs {
                    project: Box::new(self.project().clone()),
                });
            }
        }
    }

    fn import_wav(&mut self) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;
        self.status = Status::working(if self.project_path.is_some() {
            "Choosing a WAV…"
        } else {
            "Choosing where to save this project…"
        });
        self.send(Job::Import {
            project: Box::new(self.project().clone()),
            project_path: self.project_path.clone(),
        });
    }

    fn export_mix(&mut self) {
        if self.dialog_open {
            return;
        }
        let Some(snapshot) = self.snapshots.current() else {
            self.status = Status::error("Nothing to export — add a clip to the timeline first");
            return;
        };
        self.dialog_open = true;
        self.exporting = true;
        self.status = Status::working("Choosing an export destination…");
        self.send(Job::Export { snapshot });
    }

    fn toggle_playback(&mut self) {
        match self.transport.state() {
            TransportState::Playing => self.transport.pause(),
            _ => self.play(),
        }
    }

    fn play(&mut self) {
        if self.snapshots.current().is_none() {
            self.status = if self.active_mix.is_some() || self.mix_due.is_some() {
                Status::working("Building playback audio…")
            } else {
                Status::error("Nothing to play — add a clip to the timeline")
            };
            return;
        }
        if self.audio_error.is_some() {
            return;
        }
        self.transport.play();
    }

    fn total_frames(&self) -> u64 {
        self.snapshots.current().map_or_else(
            || project_duration_frames(self.project()),
            |snapshot| snapshot.frame_count(),
        )
    }
}

impl eframe::App for JutsuAudioApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_results();
        self.sync_session(context);
        self.poll_session();
        self.dispatch_pending_work();
        self.sync_edit();
        self.meter = (self.meter * METER_DECAY).max(self.transport.peak_level());

        self.top_bar(context);
        self.status_bar(context);
        self.library_panel(context);
        self.inspector_panel(context);
        self.timeline_panel(context);
        self.recovery_prompt(context);
        if self.recovery.is_none() {
            self.shortcuts(context);
        }

        // Only keep the frame loop hot while something is actually moving.
        let busy = self.transport.state() == TransportState::Playing
            || self.meter > 0.0005
            || self.active_mix.is_some();
        if busy {
            context.request_repaint_after(Duration::from_millis(16));
        } else if self.mix_due.is_some() || self.save_due.is_some() {
            context.request_repaint_after(Duration::from_millis(80));
        }
    }
}

impl JutsuAudioApp {
    /// Offers recovered work, and acts on the answer. Restoring loads the
    /// recovered project into the editor and leaves the file untouched until
    /// the next save; keeping the saved project deletes the recovery file.
    fn recovery_prompt(&mut self, context: &egui::Context) {
        let Some(recovery) = self.recovery.as_ref() else {
            return;
        };
        let Some(decision) = recovery::prompt(context, recovery) else {
            return;
        };
        let recovery = self.recovery.take().expect("the prompt was open");
        match decision {
            Decision::Restore => match ProjectCommandEngine::new(*recovery.project) {
                Ok(commands) => {
                    self.commands = commands;
                    self.history.clear();
                    self.selected_clip = None;
                    self.edit = None;
                    self.mark_edited(MIX_DEBOUNCE);
                    self.status = Status::info("Recovered edits restored - save to keep them");
                }
                Err(error) => {
                    self.status =
                        Status::error(format!("Recovered work is invalid: {}", error.message));
                }
            },
            Decision::Discard => {
                if let Some(path) = self.project_path.clone() {
                    self.send(Job::DiscardAutosave { path });
                }
                self.status = Status::info("Kept the saved project");
            }
        }
    }

    fn shortcuts(&mut self, context: &egui::Context) {
        if context.wants_keyboard_input() {
            return;
        }
        let (space, delete, save, zoom_in, zoom_out, fit, stop, undo, redo) =
            context.input(|input| {
                (
                    input.key_pressed(egui::Key::Space),
                    input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace),
                    input.modifiers.command && input.key_pressed(egui::Key::S),
                    input.key_pressed(egui::Key::Plus) || input.key_pressed(egui::Key::Equals),
                    input.key_pressed(egui::Key::Minus),
                    input.key_pressed(egui::Key::F),
                    input.key_pressed(egui::Key::Escape),
                    input.modifiers.command
                        && !input.modifiers.shift
                        && input.key_pressed(egui::Key::Z),
                    input.modifiers.command
                        && (input.key_pressed(egui::Key::Y)
                            || (input.modifiers.shift && input.key_pressed(egui::Key::Z))),
                )
            });
        if undo {
            self.undo();
        }
        if redo {
            self.redo();
        }
        if space {
            self.toggle_playback();
        }
        if stop {
            self.transport.stop();
        }
        if delete {
            self.delete_selected();
        }
        if save {
            self.save_project();
        }
        if zoom_in {
            self.timeline.zoom_in();
        }
        if zoom_out {
            self.timeline.zoom_out();
        }
        if fit {
            let width = context.content_rect().width() - 700.0;
            self.timeline
                .zoom_to_fit(self.total_frames(), self.sample_rate(), width.max(200.0));
        }
    }

    // ─── top bar ────────────────────────────────────────────────────────────

    fn top_bar(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::top("top")
            .frame(
                theme::panel(theme::PANEL)
                    .inner_margin(egui::Margin::symmetric(10, 0))
                    .stroke(Stroke::new(1.0_f32, theme::RULE)),
            )
            .exact_height(38.0)
            .show(context, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(
                        RichText::new("JUTSU")
                            .size(12.0)
                            .color(theme::TEXT)
                            .strong()
                            .extra_letter_spacing(1.0),
                    );
                    ui.label(RichText::new("·").size(12.0).color(theme::SIGNAL).strong());
                    ui.label(
                        RichText::new("AUDIO")
                            .size(12.0)
                            .color(theme::TEXT)
                            .strong()
                            .extra_letter_spacing(1.0),
                    );
                    ui.add_space(10.0);

                    let rate = self.sample_rate();
                    ui.label(
                        RichText::new(&self.project().metadata.name)
                            .size(11.5)
                            .color(theme::DIM),
                    );
                    ui.label(RichText::new("/").size(11.0).color(theme::FAINT));
                    ui.label(
                        RichText::new(format!("{:.1} kHz", f64::from(rate) / 1000.0))
                            .size(11.0)
                            .color(theme::FAINT),
                    );
                    if self.unsaved {
                        ui.label(RichText::new("· unsaved").size(11.0).color(theme::SIGNAL));
                    } else if self.project_path.is_none() {
                        ui.label(
                            RichText::new("· not on disk")
                                .size(11.0)
                                .color(theme::FAINT),
                        );
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can_export = self.snapshots.current().is_some() && !self.exporting;
                        let export = ui.add_enabled_ui(can_export, |ui| {
                            theme::flat_button(
                                ui,
                                if self.exporting {
                                    "Exporting…"
                                } else {
                                    "Export WAV"
                                },
                            )
                        });
                        if export.inner.clicked() {
                            self.export_mix();
                        }
                        if !can_export && !self.exporting {
                            export
                                .response
                                .on_hover_text("Add a clip to the timeline to export a mix");
                        }
                        if theme::flat_button(ui, "Save")
                            .on_hover_text("Ctrl+S")
                            .clicked()
                        {
                            self.save_project();
                        }
                        if theme::flat_button(ui, "Open").clicked() {
                            self.open_project();
                        }
                        let redo = ui.add_enabled_ui(self.history.can_redo(), |ui| {
                            theme::flat_button(ui, "Redo").on_hover_text("Ctrl+Shift+Z")
                        });
                        if redo.inner.clicked() {
                            self.redo();
                        }
                        let undo = ui.add_enabled_ui(self.history.can_undo(), |ui| {
                            theme::flat_button(ui, "Undo").on_hover_text("Ctrl+Z")
                        });
                        if undo.inner.clicked() {
                            self.undo();
                        }
                    });
                });
            });
    }

    // ─── status bar ─────────────────────────────────────────────────────────

    fn status_bar(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .frame(
                theme::panel(theme::PANEL)
                    .inner_margin(egui::Margin::symmetric(10, 0))
                    .stroke(Stroke::new(1.0_f32, theme::RULE)),
            )
            .exact_height(30.0)
            .show(context, |ui| {
                ui.horizontal_centered(|ui| {
                    let state = self.transport.state();
                    if ui
                        .add(transport_button(
                            "Stop",
                            state == TransportState::Stopped,
                            false,
                        ))
                        .clicked()
                    {
                        self.transport.stop();
                    }
                    if ui
                        .add(transport_button(
                            if state == TransportState::Playing {
                                "Pause"
                            } else {
                                "Play"
                            },
                            false,
                            state == TransportState::Playing,
                        ))
                        .on_hover_text("Space")
                        .clicked()
                    {
                        self.toggle_playback();
                    }

                    let rate = self.sample_rate();
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(theme::format_time(self.transport.position_frames(), rate))
                            .font(theme::mono(12.0))
                            .color(theme::TEXT),
                    );
                    ui.label(
                        RichText::new(format!(
                            "/ {}",
                            theme::format_time(self.total_frames(), rate)
                        ))
                        .font(theme::mono(11.0))
                        .color(theme::FAINT),
                    );

                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(&self.status.text)
                            .size(11.0)
                            .color(self.status.color()),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        match &self.audio_error {
                            Some(error) => {
                                ui.label(
                                    RichText::new("no audio device")
                                        .font(theme::mono(10.5))
                                        .color(theme::DANGER),
                                )
                                .on_hover_text(format!(
                                    "Playback is unavailable: {error}\nExport still works."
                                ));
                            }
                            None => {
                                let xruns = self.transport.underrun_count();
                                ui.label(
                                    RichText::new(format!("xrun {xruns}"))
                                        .font(theme::mono(10.5))
                                        .color(if xruns == 0 {
                                            theme::LIVE
                                        } else {
                                            theme::DANGER
                                        }),
                                )
                                .on_hover_text("Buffer underruns since launch");
                            }
                        }
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(format!("{} dBFS", theme::format_dbfs(self.meter)))
                                .font(theme::mono(10.5))
                                .color(theme::DIM),
                        );
                        theme::peak_meter(ui, self.meter);
                        ui.add_space(8.0);
                        if has_mixed_sample_rates(self.project()) {
                            ui.label(
                                RichText::new("mixed rates")
                                    .font(theme::mono(10.5))
                                    .color(theme::SIGNAL),
                            )
                            .on_hover_text(format!(
                                "Samples in this project disagree on sample rate. \
                                 The timeline counts in {} Hz and everything else is resampled.",
                                self.sample_rate()
                            ));
                            ui.add_space(8.0);
                        }
                    });
                });
            });
    }

    // ─── library ────────────────────────────────────────────────────────────

    fn library_panel(&mut self, context: &egui::Context) {
        egui::SidePanel::left("library")
            .frame(
                theme::panel(theme::PANEL)
                    .inner_margin(egui::Margin::symmetric(0, 0))
                    .stroke(Stroke::new(1.0_f32, theme::RULE)),
            )
            .exact_width(238.0)
            .resizable(false)
            .show(context, |ui| {
                let rate_of = |source: &AudioAssetSource| match source {
                    AudioAssetSource::ManagedFile { sample_rate, .. } => *sample_rate,
                    _ => 0,
                };

                ui.allocate_ui_with_layout(
                    Vec2::new(ui.available_width(), 28.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add_space(10.0);
                        theme::column_label(ui, "Samples");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(self.project().assets.len().to_string())
                                    .font(theme::mono(10.0))
                                    .color(theme::FAINT),
                            );
                        });
                    },
                );
                separator(ui);

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    ui.add_sized(
                        [ui.available_width() - 10.0, 22.0],
                        egui::TextEdit::singleline(&mut self.filter)
                            .hint_text(RichText::new("Filter samples").size(11.0))
                            .font(egui::TextStyle::Body)
                            .margin(egui::Margin::symmetric(7, 3)),
                    );
                });
                ui.add_space(8.0);

                let needle = self.filter.to_lowercase();
                let visible: Vec<AssetRow> = self
                    .project()
                    .assets
                    .iter()
                    .filter(|asset| {
                        needle.is_empty() || asset.name.to_lowercase().contains(&needle)
                    })
                    .map(|asset| {
                        let (frames, detail) = match &asset.source {
                            AudioAssetSource::ManagedFile {
                                sample_rate,
                                channels,
                                frame_count,
                                ..
                            } => (
                                *frame_count,
                                format!(
                                    "{:.1}k · {}",
                                    f64::from(*sample_rate) / 1000.0,
                                    channel_label(*channels)
                                ),
                            ),
                            AudioAssetSource::File { .. } => (0, "linked file".to_owned()),
                            AudioAssetSource::Generated { generator_type, .. } => {
                                (0, generator_type.clone())
                            }
                        };
                        AssetRow {
                            id: asset.id,
                            name: asset.name.clone(),
                            detail,
                            frames,
                            // Durations read in the sample's own rate, not the
                            // project's — this is the file, not a clip.
                            rate: rate_of(&asset.source).max(1),
                        }
                    })
                    .collect();

                let list_height = ui.available_height() - 44.0;
                egui::ScrollArea::vertical()
                    .max_height(list_height.max(60.0))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.project().assets.is_empty() {
                            empty_note(
                                ui,
                                "No samples yet",
                                "Import a WAV to start building a timeline",
                            );
                        } else if visible.is_empty() {
                            empty_note(
                                ui,
                                "No matches",
                                &format!("Nothing here is called “{}”", self.filter),
                            );
                        }
                        for row in visible {
                            if self.asset_row(ui, &row) {
                                self.add_selected_asset_at_playhead();
                            }
                        }
                    });

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.add_space(10.0);
                        if ui
                            .add_sized(
                                [ui.available_width() - 10.0, 26.0],
                                egui::Button::new(
                                    RichText::new("+  Import WAV").size(11.5).color(theme::TEXT),
                                )
                                .fill(theme::RAISED)
                                .stroke(Stroke::new(1.0_f32, theme::RULE))
                                .corner_radius(theme::RADIUS),
                            )
                            .on_hover_text("Copies the file into this project's folder")
                            .clicked()
                        {
                            self.import_wav();
                        }
                    });
                });
            });
    }

    /// One library row. Returns true when it was double-clicked.
    fn asset_row(&mut self, ui: &mut egui::Ui, row: &AssetRow) -> bool {
        let AssetRow {
            id,
            name,
            detail,
            frames,
            rate,
        } = row;
        let (id, frames, rate) = (*id, *frames, *rate);
        let selected = self.selected_asset == Some(id);
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), 38.0),
            egui::Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        if selected || response.hovered() {
            painter.rect_filled(rect, egui::CornerRadius::ZERO, theme::RAISED);
        }
        if selected {
            painter.rect_filled(
                egui::Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())),
                egui::CornerRadius::ZERO,
                theme::SIGNAL,
            );
        }
        let duration = theme::format_time(frames, rate);
        let duration_width = ui.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(duration.clone(), theme::mono(9.5), theme::FAINT)
                .size()
                .x
        });
        // Leave room for the duration column so long sample names cannot run
        // underneath it.
        let name_width = rect.width() - 27.0 - duration_width;
        painter.text(
            egui::pos2(rect.left() + 10.0, rect.top() + 8.0),
            egui::Align2::LEFT_TOP,
            theme::elide_measured(ui, name, &theme::body(), name_width),
            theme::body(),
            if selected { theme::TEXT } else { theme::DIM },
        );
        painter.text(
            egui::pos2(rect.left() + 10.0, rect.top() + 23.0),
            egui::Align2::LEFT_TOP,
            detail,
            theme::mono(9.5),
            theme::FAINT,
        );
        painter.text(
            egui::pos2(rect.right() - 9.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            duration,
            theme::mono(9.5),
            theme::FAINT,
        );

        response.dnd_set_drag_payload(id);
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if response.clicked() {
            self.selected_asset = Some(id);
        }
        if response.double_clicked() {
            self.selected_asset = Some(id);
            return true;
        }
        false
    }

    // ─── inspector ──────────────────────────────────────────────────────────

    fn inspector_panel(&mut self, context: &egui::Context) {
        egui::SidePanel::right("inspector")
            .frame(
                theme::panel(theme::PANEL)
                    .inner_margin(egui::Margin::ZERO)
                    .stroke(Stroke::new(1.0_f32, theme::RULE)),
            )
            .exact_width(248.0)
            .resizable(false)
            .show(context, |ui| {
                let rate = self.sample_rate();
                let clip = self.selected_clip().cloned();

                self.inspector_header(ui, clip.as_ref());
                separator(ui);

                let Some(clip) = clip else {
                    ui.add_space(30.0);
                    empty_note(
                        ui,
                        "Nothing selected",
                        "Click a clip on the timeline to edit its level and timing",
                    );
                    return;
                };
                let Some(edit) = self.edit else { return };

                // Everything below the header lives in an explicitly bounded,
                // padded rect. Without one, right-aligned rows anchor to the
                // parent's unbounded width and spill outside the panel.
                let body = ui
                    .available_rect_before_wrap()
                    .shrink2(Vec2::new(INSPECTOR_PADDING, 0.0));
                let outcome = ui
                    .scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                        self.inspector_body(ui, &clip, edit, rate)
                    })
                    .inner;

                if outcome.changed {
                    self.edit = Some(outcome.edit);
                }
                // Commit once the interaction settles, not once per frame.
                if outcome.edit.dirty && (outcome.released || !ui.ctx().is_using_pointer()) {
                    self.edit = Some(outcome.edit);
                    self.commit_edit();
                }
                match outcome.action {
                    Some(ClipAction::Split) => self.split_selected(),
                    Some(ClipAction::Duplicate) => self.duplicate_selected(),
                    Some(ClipAction::Delete) => self.delete_selected(),
                    None => {}
                }
            });
    }

    fn inspector_header(&self, ui: &mut egui::Ui, clip: Option<&Clip>) {
        let title = clip.and_then(|clip| {
            self.project()
                .assets
                .iter()
                .find(|asset| asset.id == clip.asset_id)
                .map(|asset| asset.name.clone())
        });
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 28.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.add_space(INSPECTOR_PADDING);
                theme::column_label(ui, "Clip");
                if let Some(title) = title {
                    ui.add_space(4.0);
                    let room = ui.available_width() - INSPECTOR_PADDING;
                    ui.label(
                        RichText::new(theme::elide_measured(ui, &title, &theme::body(), room))
                            .size(11.0)
                            .color(theme::TEXT),
                    );
                }
            },
        );
    }

    /// Draws the level, timing and action sections. Returns what the user did
    /// instead of mutating, so the panel closure keeps a single borrow of self.
    fn inspector_body(
        &self,
        ui: &mut egui::Ui,
        clip: &Clip,
        mut edit: ClipEdit,
        rate: u32,
    ) -> InspectorOutcome {
        let mut changed = false;
        let mut released = false;
        let mut action = None;

        ui.add_space(14.0);
        theme::column_label(ui, "Level");
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Gain").size(11.5).color(theme::DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{:+.1} dB", edit.gain_db))
                        .font(theme::mono(11.0))
                        .color(theme::TEXT),
                );
            });
        });
        ui.add_space(2.0);
        ui.spacing_mut().slider_width = ui.available_width() - 4.0;
        let gain = ui.add(
            egui::Slider::new(&mut edit.gain_db, -60.0..=12.0)
                .show_value(false)
                .trailing_fill(true),
        );
        changed |= gain.changed();
        released |= gain.drag_stopped() || gain.lost_focus();
        if gain.double_clicked() {
            edit.gain_db = 0.0;
            changed = true;
            released = true;
        }
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("-60")
                    .font(theme::mono(9.0))
                    .color(theme::FAINT),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new("+12")
                        .font(theme::mono(9.0))
                        .color(theme::FAINT),
                );
            });
        });
        ui.add_space(3.0);
        ui.label(
            RichText::new("Double-click the slider for unity gain")
                .size(10.0)
                .color(theme::FAINT),
        );

        ui.add_space(18.0);
        ui.horizontal(|ui| {
            theme::column_label(ui, "Timing");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("frames @ {rate} Hz"))
                        .font(theme::mono(9.0))
                        .color(theme::FAINT),
                );
            });
        });
        ui.add_space(6.0);
        for row in TimingRow::ALL {
            let value = match row {
                TimingRow::Start => &mut edit.start,
                TimingRow::Length => &mut edit.duration,
                TimingRow::Offset => &mut edit.source_start,
            };
            let caption = format!("{}  {}", row.label(), theme::format_time(*value, rate));
            ui.horizontal(|ui| {
                ui.label(RichText::new(caption).size(11.0).color(theme::DIM));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let response = ui.add_sized(
                        [86.0, 22.0],
                        egui::DragValue::new(value)
                            // One drag pixel moves 5 ms, whatever the rate.
                            .speed(f64::from(rate) / 200.0)
                            .range(row.range()),
                    );
                    changed |= response.changed();
                    released |= response.drag_stopped() || response.lost_focus();
                });
            });
            ui.add_space(3.0);
        }

        ui.add_space(18.0);
        theme::column_label(ui, "Actions");
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let width = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
            if ui
                .add_sized([width, 26.0], flat("Split"))
                .on_hover_text("Splits at the playhead when it sits inside the clip")
                .clicked()
            {
                action = Some(ClipAction::Split);
            }
            if ui
                .add_sized([width, 26.0], flat("Duplicate"))
                .on_hover_text("Places a copy directly after this clip")
                .clicked()
            {
                action = Some(ClipAction::Duplicate);
            }
        });
        ui.add_space(6.0);
        if theme::danger_button(ui, "Delete clip", ui.available_width())
            .on_hover_text("Delete")
            .clicked()
        {
            action = Some(ClipAction::Delete);
        }

        ui.add_space(20.0);
        theme::column_label(ui, "Source");
        ui.add_space(4.0);
        ui.label(
            RichText::new(match self.waveforms.get(&clip.asset_id) {
                Some(WaveformState::Ready(_)) => "peaks cached",
                Some(WaveformState::Pending) => "reading peaks...",
                Some(WaveformState::Failed) => "peaks unavailable",
                None => "not loaded",
            })
            .font(theme::mono(9.5))
            .color(theme::FAINT),
        );

        if changed {
            edit.dirty = true;
        }
        InspectorOutcome {
            edit,
            changed,
            released,
            action,
        }
    }

    // ─── timeline ───────────────────────────────────────────────────────────

    fn timeline_panel(&mut self, context: &egui::Context) {
        egui::CentralPanel::default()
            .frame(theme::panel(theme::BG).inner_margin(egui::Margin::ZERO))
            .show(context, |ui| {
                let mut fit_requested = false;
                ui.allocate_ui_with_layout(
                    Vec2::new(ui.available_width(), 30.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add_space(10.0);
                        for tool in Tool::ALL {
                            if theme::tool_button(ui, tool.label(), self.timeline.tool == tool)
                                .clicked()
                            {
                                self.timeline.tool = tool;
                            }
                        }
                        ui.add_space(10.0);
                        if theme::tool_button(
                            ui,
                            &format!(
                                "Snap {}",
                                if self.timeline.snap {
                                    format_grid(timeline::grid_seconds(
                                        self.timeline.pixels_per_second(),
                                    ))
                                } else {
                                    "off".to_owned()
                                }
                            ),
                            self.timeline.snap,
                        )
                        .on_hover_text("Snap clip edges and the playhead to the visible grid")
                        .clicked()
                        {
                            self.timeline.snap = !self.timeline.snap;
                        }

                        ui.add_space(10.0);
                        if theme::tool_button(ui, "+ Track", false)
                            .on_hover_text("Add a track to the timeline")
                            .clicked()
                        {
                            self.add_track();
                        }
                        if theme::tool_button(ui, "+ Lane", false)
                            .on_hover_text("Add a lane to the selected track")
                            .clicked()
                        {
                            self.add_layer();
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(10.0);
                            if theme::tool_button(ui, "+", false).clicked() {
                                self.timeline.zoom_in();
                            }
                            ui.label(
                                RichText::new(self.timeline.zoom_label())
                                    .font(theme::mono(10.0))
                                    .color(theme::FAINT),
                            );
                            if theme::tool_button(ui, "−", false).clicked() {
                                self.timeline.zoom_out();
                            }
                            ui.add_space(6.0);
                            if theme::tool_button(ui, "Fit", false)
                                .on_hover_text("F")
                                .clicked()
                            {
                                fit_requested = true;
                            }
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new("Ctrl+wheel zoom · Shift+wheel scroll")
                                    .font(theme::mono(9.5))
                                    .color(theme::FAINT),
                            );
                        });
                    },
                );
                separator(ui);

                let available = ui.available_rect_before_wrap();
                if fit_requested {
                    self.timeline.zoom_to_fit(
                        self.total_frames(),
                        self.sample_rate(),
                        available.width() - 140.0,
                    );
                }

                let actions = {
                    let context = TimelineContext {
                        project: self.commands.project(),
                        sample_rate: project_sample_rate(self.commands.project()),
                        selected_clip: self.selected_clip,
                        playhead: self.transport.position_frames(),
                        waveforms: &self.waveforms,
                    };
                    self.timeline.show(ui, &context)
                };
                for action in actions {
                    self.handle_timeline_action(action);
                }
            });
    }

    fn handle_timeline_action(&mut self, action: TimelineAction) {
        match action {
            TimelineAction::SelectClip(clip_id) => self.select_clip(Some(clip_id)),
            TimelineAction::ClearSelection => self.select_clip(None),
            TimelineAction::MoveClip {
                clip_id,
                start_sample,
            } => {
                let Some(clip) = self
                    .project()
                    .tracks
                    .iter()
                    .flat_map(|track| &track.layers)
                    .flat_map(|layer| &layer.clips)
                    .find(|clip| clip.id == clip_id)
                    .cloned()
                else {
                    return;
                };
                if clip.start_sample == start_sample {
                    return;
                }
                self.select_clip(Some(clip_id));
                self.apply(vec![ProjectCommand::UpdateClip {
                    clip_id,
                    start_sample,
                    source_start_sample: clip.source_start_sample,
                    duration_samples: clip.duration_samples,
                    gain_db: clip_gain_db(&clip),
                }]);
                self.edit = None;
            }
            TimelineAction::Seek(frame) => self.transport.seek(frame),
            TimelineAction::ToggleTrackMute(track_id) => {
                let muted = self.track_flag(track_id, "mute");
                self.apply(vec![ProjectCommand::SetTrackMute {
                    track_id,
                    muted: !muted,
                }]);
            }
            TimelineAction::ToggleTrackSolo(track_id) => {
                let soloed = self.track_flag(track_id, "solo");
                self.apply(vec![ProjectCommand::SetTrackSolo {
                    track_id,
                    soloed: !soloed,
                }]);
            }
            TimelineAction::DropAsset {
                asset_id,
                track_id,
                layer_id,
                start_sample,
            } => {
                self.selected_asset = Some(asset_id);
                self.add_asset_to_timeline(asset_id, track_id, layer_id, start_sample);
            }
        }
    }
}

// ─── small shared widgets ───────────────────────────────────────────────────

fn separator(ui: &mut egui::Ui) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::ZERO, theme::RULE);
}

fn flat(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).size(11.5).color(theme::TEXT))
        .fill(theme::RAISED)
        .stroke(Stroke::new(1.0_f32, theme::RULE))
        .corner_radius(theme::RADIUS)
}

fn transport_button(label: &str, muted: bool, live: bool) -> egui::Button<'static> {
    egui::Button::new(RichText::new(label.to_owned()).size(11.0).color(if live {
        theme::BG
    } else if muted {
        theme::FAINT
    } else {
        theme::TEXT
    }))
    .fill(if live { theme::LIVE } else { theme::RAISED })
    .stroke(Stroke::new(
        1.0_f32,
        if live { theme::LIVE } else { theme::RULE },
    ))
    .corner_radius(theme::RADIUS)
    .min_size(Vec2::new(46.0, 20.0))
}

fn empty_note(ui: &mut egui::Ui, headline: &str, hint: &str) {
    ui.add_space(16.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(headline).size(12.0).color(theme::DIM));
        ui.add_space(2.0);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        ui.label(RichText::new(hint).size(10.5).color(theme::FAINT));
    });
}

const INSPECTOR_PADDING: f32 = 10.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClipAction {
    Split,
    Duplicate,
    Delete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimingRow {
    Start,
    Length,
    Offset,
}

impl TimingRow {
    const ALL: [Self; 3] = [Self::Start, Self::Length, Self::Offset];

    const fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Length => "Length",
            Self::Offset => "Offset",
        }
    }

    /// A clip with zero length is invalid, so length starts at one frame.
    const fn range(self) -> std::ops::RangeInclusive<u64> {
        match self {
            Self::Length => 1..=u64::MAX,
            _ => 0..=u64::MAX,
        }
    }
}

struct InspectorOutcome {
    edit: ClipEdit,
    changed: bool,
    released: bool,
    action: Option<ClipAction>,
}

/// One row of the sample library, flattened out of the project so drawing does
/// not borrow it.
struct AssetRow {
    id: AssetId,
    name: String,
    detail: String,
    frames: u64,
    rate: u32,
}

fn channel_label(channels: u16) -> &'static str {
    match channels {
        1 => "mono",
        2 => "stereo",
        _ => "multi",
    }
}

fn format_grid(seconds: f64) -> String {
    if seconds >= 1.0 {
        format!("{seconds:.0}s")
    } else {
        format!("{:.0}ms", seconds * 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip_with(gain: f64) -> Clip {
        Clip {
            id: ClipId::new(),
            asset_id: AssetId::new(),
            start_sample: 100,
            source_start_sample: 5,
            duration_samples: 400,
            parameters: [("gain_db".to_owned(), ParameterValue::Float(gain))]
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn the_edit_buffer_takes_gain_from_the_clip_it_belongs_to() {
        let clip = clip_with(-12.0);
        let edit = ClipEdit::from_clip(&clip);
        assert_eq!(edit.clip_id, clip.id);
        assert_eq!(edit.gain_db, -12.0);
        assert_eq!(edit.start, 100);
        assert_eq!(edit.duration, 400);
        assert_eq!(edit.source_start, 5);
        assert!(!edit.dirty);
    }

    #[test]
    fn a_clip_without_a_gain_parameter_reads_as_unity() {
        let mut clip = clip_with(0.0);
        clip.parameters.clear();
        assert_eq!(ClipEdit::from_clip(&clip).gain_db, 0.0);
    }

    #[test]
    fn long_labels_are_elided_rather_than_overrunning_their_column() {
        assert_eq!(theme::elide("kick", 200.0), "kick");
        let long = theme::elide("MUSCSong_Antique Graveyard_GoAg_SWSH_Abminor", 120.0);
        assert!(long.ends_with('…'));
        assert!(long.chars().count() <= 22, "got {long:?}");
    }

    #[test]
    fn grid_labels_switch_from_milliseconds_to_seconds() {
        assert_eq!(format_grid(0.25), "250ms");
        assert_eq!(format_grid(2.0), "2s");
    }

    #[test]
    fn time_formatting_follows_the_project_rate() {
        assert_eq!(theme::format_time(48_000, 48_000), "00:01.000");
        assert_eq!(theme::format_time(44_100, 44_100), "00:01.000");
        // A zero rate must not divide by zero.
        assert_eq!(theme::format_time(48_000, 0), "00:01.000");
    }

    #[test]
    fn peak_levels_render_as_dbfs_with_a_silent_floor() {
        assert_eq!(theme::format_dbfs(1.0).trim(), "0.0");
        assert_eq!(theme::format_dbfs(0.0).trim(), "-inf");
    }
}
