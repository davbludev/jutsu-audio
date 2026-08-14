use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, Color32, CornerRadius, FontId, RichText, Stroke, Vec2};
use jutsu_audio_commands::{
    COMMAND_PROTOCOL_VERSION, CommandEnvelope, CommandId, ProjectCommand, ProjectCommandEngine,
};
use jutsu_audio_engine::{
    ExportEncoding, ExportRange, OfflineExporter, PlaybackRenderer, PlaybackSnapshot,
    SnapshotExchange, SystemAudioOutput, TransportController,
};
use jutsu_audio_model::{AssetId, AudioAssetSource, Clip, ClipId, ParameterValue, Project};
use jutsu_audio_project::{AssetManager, ImportMode, ImportStatus, ProjectStore};

const ACCENT: Color32 = Color32::from_rgb(255, 103, 72);
const VIOLET: Color32 = Color32::from_rgb(142, 102, 255);
const PANEL: Color32 = Color32::from_rgb(22, 24, 27);
const CANVAS: Color32 = Color32::from_rgb(16, 18, 20);
const TEXT_MUTED: Color32 = Color32::from_rgb(151, 154, 161);
const SAMPLE_RATE: f32 = 48_000.0;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1536.0, 960.0])
            .with_min_inner_size([1080.0, 680.0])
            .with_title("Jutsu Audio"),
        ..Default::default()
    };
    eframe::run_native(
        "Jutsu Audio",
        options,
        Box::new(|context| Ok(Box::new(JutsuAudioApp::new(context)))),
    )
}

struct JutsuAudioApp {
    commands: ProjectCommandEngine,
    project_path: Option<PathBuf>,
    selected_asset: Option<AssetId>,
    selected_clip: Option<ClipId>,
    transport: TransportController,
    snapshots: SnapshotExchange,
    _audio_output: Option<SystemAudioOutput>,
    status: String,
    gain_db: f64,
    zoom: f32,
}

impl JutsuAudioApp {
    fn new(context: &eframe::CreationContext<'_>) -> Self {
        configure_style(&context.egui_ctx);
        let project = ProjectStore::new_project("Untitled Project");
        let commands = ProjectCommandEngine::new(project).expect("new project is valid");
        let transport = TransportController::new();
        let snapshots = SnapshotExchange::new(None);
        let renderer = PlaybackRenderer::new(snapshots.reader(), transport.reader());
        let audio = SystemAudioOutput::open_default(renderer).ok();
        Self {
            commands,
            project_path: None,
            selected_asset: None,
            selected_clip: None,
            transport,
            snapshots,
            _audio_output: audio,
            status: "Ready".into(),
            gain_db: 0.0,
            zoom: 1.0,
        }
    }

    fn project(&self) -> &Project {
        self.commands.project()
    }

    fn apply(&mut self, commands: Vec<ProjectCommand>) -> bool {
        let envelope = CommandEnvelope {
            protocol_version: COMMAND_PROTOCOL_VERSION,
            command_id: CommandId::new(),
            expected_revision: self.commands.revision(),
            commands,
        };
        match self.commands.apply(envelope) {
            Ok(_) => {
                self.persist();
                self.refresh_playback();
                true
            }
            Err(error) => {
                self.status = format!("Edit failed: {}", error.message);
                false
            }
        }
    }

    fn persist(&mut self) {
        let Some(path) = &self.project_path else {
            self.status = "Edited — choose Save to persist".into();
            return;
        };
        match ProjectStore::save(path, self.commands.project()) {
            Ok(()) => self.status = "Saved".into(),
            Err(error) => self.status = format!("Save failed: {}", error.message),
        }
    }

    fn open_project(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Jutsu Audio project", &["json"])
            .pick_file()
        else {
            return;
        };
        match ProjectStore::open(&path).and_then(|opened| {
            ProjectCommandEngine::new(opened.project).map_err(|error| {
                jutsu_audio_project::ProjectFileError {
                    code: jutsu_audio_project::ProjectFileErrorCode::InvalidProject,
                    path: path.clone(),
                    message: error.message,
                    diagnostics: error.diagnostics,
                }
            })
        }) {
            Ok(commands) => {
                self.commands = commands;
                self.project_path = Some(path);
                self.selected_clip = None;
                self.selected_asset = None;
                self.status = "Project opened".into();
                self.refresh_playback();
            }
            Err(error) => self.status = format!("Open failed: {}", error.message),
        }
    }

    fn save_project(&mut self) {
        if self.project_path.is_none() {
            self.project_path = rfd::FileDialog::new()
                .set_file_name("project.jutsu-audio.json")
                .save_file();
        }
        self.persist();
    }

    fn import_wav(&mut self) {
        let Some(source) = rfd::FileDialog::new()
            .add_filter("WAV audio", &["wav"])
            .pick_file()
        else {
            return;
        };
        if self.project_path.is_none() {
            self.project_path = rfd::FileDialog::new()
                .set_file_name("project.jutsu-audio.json")
                .save_file();
        }
        let Some(project_path) = &self.project_path else {
            self.status = "Import cancelled: project needs a save location".into();
            return;
        };
        match AssetManager::prepare_wav_import(
            self.commands.project(),
            project_path,
            source,
            ImportMode::CopyIntoProject,
        ) {
            Ok(prepared) => match prepared.status {
                ImportStatus::Prepared => {
                    let asset = prepared.asset.expect("prepared import has asset");
                    self.selected_asset = Some(asset.id);
                    self.apply(vec![ProjectCommand::AddAsset { asset }]);
                }
                ImportStatus::Duplicate(id) => {
                    self.selected_asset = Some(id);
                    self.status = "Sample already exists in this project".into();
                }
            },
            Err(error) => self.status = format!("Import failed: {}", error.message),
        }
    }

    fn export_selected(&mut self) {
        let Some(snapshot) = self.snapshots.current() else {
            self.status = "Select a playable clip before export".into();
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("WAV audio", &["wav"])
            .set_file_name("jutsu-audio-export.wav")
            .save_file()
        else {
            return;
        };
        match OfflineExporter::export_wav(
            snapshot,
            path,
            ExportRange::full(),
            ExportEncoding::Pcm16,
        ) {
            Ok(report) => self.status = format!("Exported {} frames", report.frame_count),
            Err(error) => self.status = format!("Export failed: {}", error.message),
        }
    }

    fn add_selected_asset(&mut self, start_sample: u64) {
        let Some(asset_id) = self.selected_asset else {
            return;
        };
        let Some(asset) = self
            .project()
            .assets
            .iter()
            .find(|asset| asset.id == asset_id)
        else {
            return;
        };
        let duration_samples = match asset.source {
            AudioAssetSource::ManagedFile { frame_count, .. } => frame_count.max(1),
            _ => 48_000,
        };
        let Some(track) = self.project().tracks.first() else {
            return;
        };
        let Some(layer) = track.layers.first() else {
            return;
        };
        let track_id = track.id;
        let layer_id = layer.id;
        let clip = Clip {
            id: ClipId::new(),
            asset_id,
            start_sample,
            source_start_sample: 0,
            duration_samples,
            parameters: [("gain_db".into(), ParameterValue::Float(0.0))]
                .into_iter()
                .collect(),
        };
        self.selected_clip = Some(clip.id);
        self.apply(vec![ProjectCommand::AddClip {
            track_id,
            layer_id,
            clip,
        }]);
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

    fn update_selected_clip(&mut self, start: u64, source_start: u64, duration: u64) {
        let Some(id) = self.selected_clip else { return };
        self.apply(vec![ProjectCommand::UpdateClip {
            clip_id: id,
            start_sample: start,
            source_start_sample: source_start,
            duration_samples: duration.max(1),
            gain_db: self.gain_db,
        }]);
    }

    fn split_selected(&mut self) {
        let Some(clip) = self.selected_clip().cloned() else {
            return;
        };
        if clip.duration_samples < 2 {
            return;
        }
        let half = clip.duration_samples / 2;
        let mut right = clip.clone();
        right.id = ClipId::new();
        right.start_sample += half;
        right.source_start_sample += half;
        right.duration_samples -= half;
        let Some(track) = self.project().tracks.first() else {
            return;
        };
        let Some(layer) = track.layers.first() else {
            return;
        };
        self.apply(vec![
            ProjectCommand::UpdateClip {
                clip_id: clip.id,
                start_sample: clip.start_sample,
                source_start_sample: clip.source_start_sample,
                duration_samples: half,
                gain_db: self.gain_db,
            },
            ProjectCommand::AddClip {
                track_id: track.id,
                layer_id: layer.id,
                clip: right,
            },
        ]);
    }

    fn delete_selected(&mut self) {
        let Some(id) = self.selected_clip.take() else {
            return;
        };
        self.apply(vec![ProjectCommand::RemoveClip { clip_id: id }]);
    }

    fn refresh_playback(&mut self) {
        let Some(clip) = self.selected_clip().cloned() else {
            self.snapshots.clear();
            return;
        };
        let Some(asset) = self
            .project()
            .assets
            .iter()
            .find(|asset| asset.id == clip.asset_id)
        else {
            return;
        };
        let AudioAssetSource::ManagedFile { path, .. } = &asset.source else {
            return;
        };
        let Some(project_path) = &self.project_path else {
            return;
        };
        let source = project_path.parent().unwrap_or(Path::new(".")).join(path);
        match AssetManager::decode_wav_samples(&source) {
            Ok((metadata, samples)) => {
                let channels = usize::from(metadata.channels);
                let start = usize::try_from(clip.source_start_sample)
                    .unwrap_or(usize::MAX)
                    .saturating_mul(channels);
                let end = start
                    .saturating_add(
                        usize::try_from(clip.duration_samples)
                            .unwrap_or(usize::MAX)
                            .saturating_mul(channels),
                    )
                    .min(samples.len());
                if start < end {
                    let gain = 10_f32.powf(self.gain_db as f32 / 20.0);
                    let edited: Arc<[f32]> = samples[start..end]
                        .iter()
                        .map(|sample| sample * gain)
                        .collect();
                    if let Ok(snapshot) =
                        PlaybackSnapshot::new(metadata.sample_rate, metadata.channels, edited)
                    {
                        self.snapshots.publish(Arc::new(snapshot));
                    }
                }
            }
            Err(error) => self.status = format!("Playback unavailable: {}", error.message),
        }
    }
}

impl eframe::App for JutsuAudioApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.top_bar(context);
        self.status_bar(context);
        self.asset_panel(context);
        self.inspector(context);
        self.timeline(context);
        context.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

impl JutsuAudioApp {
    fn top_bar(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::top("top")
            .exact_height(82.0)
            .show(context, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.add_space(18.0);
                    ui.label(RichText::new("〽").size(27.0).color(ACCENT));
                    ui.label(RichText::new("Jutsu Audio").size(24.0).strong());
                    ui.separator();
                    ui.label(RichText::new(&self.project().metadata.name).size(18.0));
                    ui.add_space(ui.available_width().max(0.0) * 0.25);
                    if ui.button("Stop").clicked() {
                        self.transport.stop();
                    }
                    if ui
                        .button(RichText::new("▶").color(ACCENT))
                        .on_hover_text("Play")
                        .clicked()
                    {
                        self.transport.play();
                    }
                    if ui.button("Pause").clicked() {
                        self.transport.pause();
                    }
                    ui.add_sized(
                        [150.0, 42.0],
                        egui::Label::new(
                            RichText::new(format_time(self.transport.position_frames()))
                                .size(22.0)
                                .monospace(),
                        ),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(12.0);
                        if ui.button("Save").clicked() {
                            self.save_project();
                        }
                        if ui.button("Export WAV").clicked() {
                            self.export_selected();
                        }
                        if ui.button("Open").clicked() {
                            self.open_project();
                        }
                    });
                });
            });
    }

    fn status_bar(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(34.0)
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.label(RichText::new(&self.status).color(TEXT_MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("Underruns {}", self.transport.underrun_count()))
                                .color(TEXT_MUTED),
                        );
                    });
                });
            });
    }

    fn asset_panel(&mut self, context: &egui::Context) {
        egui::SidePanel::left("assets")
            .exact_width(300.0)
            .show(context, |ui| {
                ui.add_space(15.0);
                ui.horizontal(|ui| {
                    ui.heading("Assets");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Import WAV").clicked() {
                            self.import_wav();
                        }
                    });
                });
                ui.add_space(10.0);
                ui.separator();
                let assets: Vec<_> = self
                    .project()
                    .assets
                    .iter()
                    .map(|asset| {
                        (
                            asset.id,
                            asset.name.clone(),
                            match asset.source {
                                AudioAssetSource::ManagedFile { frame_count, .. } => frame_count,
                                _ => 0,
                            },
                        )
                    })
                    .collect();
                for (id, name, frames) in assets {
                    let selected = self.selected_asset == Some(id);
                    let response = ui.selectable_label(
                        selected,
                        RichText::new(format!("⌁  {name}     {}", format_time(frames))).size(14.0),
                    );
                    if response.clicked() {
                        self.selected_asset = Some(id);
                    }
                    if response.double_clicked() {
                        self.selected_asset = Some(id);
                        self.add_selected_asset(0);
                    }
                }
                if self.project().assets.is_empty() {
                    ui.add_space(28.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("Import a WAV to begin").color(TEXT_MUTED));
                    });
                }
            });
    }

    fn inspector(&mut self, context: &egui::Context) {
        egui::SidePanel::right("inspector")
            .exact_width(340.0)
            .show(context, |ui| {
                ui.add_space(15.0);
                ui.heading("Inspector");
                ui.add_space(12.0);
                ui.separator();
                let Some(clip) = self.selected_clip().cloned() else {
                    ui.add_space(30.0);
                    ui.label(RichText::new("Select a clip to edit").color(TEXT_MUTED));
                    return;
                };
                let asset_name = self
                    .project()
                    .assets
                    .iter()
                    .find(|asset| asset.id == clip.asset_id)
                    .map(|asset| asset.name.as_str())
                    .unwrap_or("Clip");
                ui.label(RichText::new("Clip").color(TEXT_MUTED));
                ui.add_sized(
                    [ui.available_width(), 40.0],
                    egui::TextEdit::singleline(&mut asset_name.to_owned()).interactive(false),
                );
                ui.add_space(14.0);
                let mut start = clip.start_sample;
                let mut duration = clip.duration_samples;
                ui.label(RichText::new("Start (frames)").color(TEXT_MUTED));
                let start_changed = ui
                    .add(egui::DragValue::new(&mut start).speed(240.0))
                    .changed();
                ui.label(RichText::new("Duration (frames)").color(TEXT_MUTED));
                let duration_changed = ui
                    .add(
                        egui::DragValue::new(&mut duration)
                            .range(1..=u64::MAX)
                            .speed(240.0),
                    )
                    .changed();
                ui.add_space(20.0);
                ui.separator();
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    ui.label("Gain");
                    ui.label(RichText::new(format!("{:.1} dB", self.gain_db)).color(TEXT_MUTED));
                });
                let gain_changed = ui
                    .add(egui::Slider::new(&mut self.gain_db, -24.0..=24.0).show_value(false))
                    .changed();
                if start_changed || duration_changed || gain_changed {
                    self.update_selected_clip(start, clip.source_start_sample, duration);
                }
                ui.add_space(26.0);
                if ui
                    .add_sized(
                        [ui.available_width(), 44.0],
                        egui::Button::new(RichText::new("✂  Split").color(ACCENT)),
                    )
                    .clicked()
                {
                    self.split_selected();
                }
                ui.add_space(8.0);
                if ui
                    .add_sized(
                        [ui.available_width(), 44.0],
                        egui::Button::new(RichText::new("⌫  Delete").color(ACCENT)),
                    )
                    .clicked()
                {
                    self.delete_selected();
                }
            });
    }

    fn timeline(&mut self, context: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(CANVAS))
            .show(context, |ui| {
                ui.add_space(13.0);
                ui.horizontal(|ui| {
                    ui.heading("Timeline");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("+").clicked() {
                            self.zoom = (self.zoom * 1.25).min(4.0);
                        }
                        ui.label(format!("{:.1}x", self.zoom));
                        if ui.button("-").clicked() {
                            self.zoom = (self.zoom / 1.25).max(0.5);
                        }
                    });
                });
                ui.add_space(12.0);
                let available = ui.available_size();
                let (rect, response) =
                    ui.allocate_exact_size(available, egui::Sense::click_and_drag());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 0.0, CANVAS);
                let ruler_y = rect.top() + 46.0;
                painter.line_segment(
                    [
                        egui::pos2(rect.left(), ruler_y),
                        egui::pos2(rect.right(), ruler_y),
                    ],
                    Stroke::new(1.0_f32, Color32::from_gray(55)),
                );
                let pixels_per_second = 320.0 * self.zoom;
                for tick in 0..=8 {
                    let x = rect.left() + tick as f32 * pixels_per_second / 4.0;
                    painter.line_segment(
                        [egui::pos2(x, ruler_y - 8.0), egui::pos2(x, rect.bottom())],
                        Stroke::new(
                            1.0_f32,
                            Color32::from_gray(if tick % 4 == 0 { 48 } else { 35 }),
                        ),
                    );
                    if tick % 2 == 0 {
                        painter.text(
                            egui::pos2(x + 4.0, ruler_y - 26.0),
                            egui::Align2::LEFT_CENTER,
                            format!("{:.2}s", tick as f32 / 4.0),
                            FontId::monospace(11.0),
                            TEXT_MUTED,
                        );
                    }
                }
                let clips: Vec<_> = self
                    .project()
                    .tracks
                    .iter()
                    .flat_map(|track| &track.layers)
                    .flat_map(|layer| &layer.clips)
                    .cloned()
                    .collect();
                for clip in clips {
                    let x =
                        rect.left() + clip.start_sample as f32 / SAMPLE_RATE * pixels_per_second;
                    let width =
                        (clip.duration_samples as f32 / SAMPLE_RATE * pixels_per_second).max(44.0);
                    let clip_rect = egui::Rect::from_min_size(
                        egui::pos2(x, ruler_y + 35.0),
                        Vec2::new(width, 190.0),
                    );
                    let selected = self.selected_clip == Some(clip.id);
                    painter.rect_filled(
                        clip_rect,
                        CornerRadius::same(8),
                        Color32::from_rgb(36, 28, 57),
                    );
                    painter.rect_stroke(
                        clip_rect,
                        CornerRadius::same(8),
                        Stroke::new(
                            if selected { 2.0_f32 } else { 1.0_f32 },
                            if selected {
                                VIOLET
                            } else {
                                Color32::from_gray(75)
                            },
                        ),
                        egui::StrokeKind::Inside,
                    );
                    let name = self
                        .project()
                        .assets
                        .iter()
                        .find(|asset| asset.id == clip.asset_id)
                        .map(|asset| asset.name.as_str())
                        .unwrap_or("Sample");
                    painter.text(
                        clip_rect.left_top() + Vec2::new(14.0, 18.0),
                        egui::Align2::LEFT_TOP,
                        name,
                        FontId::proportional(14.0),
                        Color32::WHITE,
                    );
                    draw_waveform(&painter, clip_rect.shrink2(Vec2::new(14.0, 42.0)));
                    let id = ui.make_persistent_id(clip.id.to_string());
                    let clip_response = ui.interact(clip_rect, id, egui::Sense::click_and_drag());
                    if clip_response.clicked() {
                        self.selected_clip = Some(clip.id);
                        self.gain_db = clip
                            .parameters
                            .get("gain_db")
                            .and_then(|v| match v {
                                ParameterValue::Float(v) => Some(*v),
                                _ => None,
                            })
                            .unwrap_or(0.0);
                        self.refresh_playback();
                    }
                    if clip_response.drag_stopped() {
                        let frames = (clip_response.drag_delta().x / pixels_per_second
                            * SAMPLE_RATE)
                            .round() as i64;
                        let start = (clip.start_sample as i64 + frames).max(0) as u64;
                        self.selected_clip = Some(clip.id);
                        self.update_selected_clip(
                            start,
                            clip.source_start_sample,
                            clip.duration_samples,
                        );
                    }
                }
                if response.clicked()
                    && self.selected_asset.is_some()
                    && self
                        .project()
                        .tracks
                        .iter()
                        .all(|track| track.layers.iter().all(|layer| layer.clips.is_empty()))
                {
                    let start = ((response.interact_pointer_pos().map_or(rect.left(), |p| p.x)
                        - rect.left())
                        / pixels_per_second
                        * SAMPLE_RATE)
                        .max(0.0) as u64;
                    self.add_selected_asset(start);
                }
                let play_x = rect.left()
                    + self.transport.position_frames() as f32 / SAMPLE_RATE * pixels_per_second;
                painter.line_segment(
                    [
                        egui::pos2(play_x, ruler_y - 10.0),
                        egui::pos2(play_x, rect.bottom()),
                    ],
                    Stroke::new(1.5_f32, Color32::from_rgb(244, 240, 232)),
                );
                painter.text(
                    egui::pos2(rect.left() + 22.0, rect.bottom() - 80.0),
                    egui::Align2::LEFT_CENTER,
                    "Select an asset, then click the timeline or double-click the asset",
                    FontId::proportional(13.0),
                    TEXT_MUTED,
                );
            });
    }
}

fn configure_style(context: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = PANEL;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = Color32::from_rgb(13, 15, 17);
    visuals.selection.bg_fill = Color32::from_rgb(64, 46, 96);
    visuals.selection.stroke = Stroke::new(1.0_f32, VIOLET);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
    visuals.widgets.active.corner_radius = CornerRadius::same(6);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    context.set_visuals(visuals);
    let mut style = (*context.style()).clone();
    style.spacing.item_spacing = Vec2::new(10.0, 10.0);
    style.spacing.button_padding = Vec2::new(14.0, 9.0);
    context.set_style(style);
}

fn draw_waveform(painter: &egui::Painter, rect: egui::Rect) {
    let center = rect.center().y;
    let points = 90;
    for index in 0..points {
        let t = index as f32 / points as f32;
        let envelope = (1.0 - t).powf(0.8) * 0.82 + 0.08;
        let noise = ((index as f32 * 1.71).sin().abs() * 0.6
            + (index as f32 * 0.37).cos().abs() * 0.4)
            * envelope;
        let x = egui::lerp(rect.x_range(), t);
        let amplitude = noise * rect.height() * 0.48;
        painter.line_segment(
            [
                egui::pos2(x, center - amplitude),
                egui::pos2(x, center + amplitude),
            ],
            Stroke::new(2.0_f32, ACCENT),
        );
    }
}

fn format_time(frames: u64) -> String {
    let milliseconds = frames.saturating_mul(1_000) / 48_000;
    format!(
        "{:02}:{:02}.{:03}",
        milliseconds / 60_000,
        (milliseconds / 1_000) % 60,
        milliseconds % 1_000
    )
}
