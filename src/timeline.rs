//! The timeline surface: ruler, track headers, lanes, clips, playhead.
//!
//! It owns its own view state (scroll and zoom) and never mutates the project.
//! Interactions come back to the caller as a [`TimelineAction`], so every edit
//! still goes through the command engine.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Vec2, pos2};
use jutsu_audio_model::{
    AssetId, AudioAssetSource, Clip, ClipId, LayerId, ParameterValue, Project, TrackId,
};
use jutsu_audio_project::CachedWaveform;

use crate::theme;
use crate::theme::elide;

const HEADER_WIDTH: f32 = 128.0;
const RULER_HEIGHT: f32 = 26.0;
const LANE_HEIGHT: f32 = 84.0;
const SCROLLBAR_HEIGHT: f32 = 9.0;
const CLIP_INSET: f32 = 6.0;
const CLIP_TITLE_HEIGHT: f32 = 15.0;
const MIN_ZOOM: f32 = 4.0;
const MAX_ZOOM: f32 = 4_000.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tool {
    Select,
    Pan,
}

impl Tool {
    pub const ALL: [Self; 2] = [Self::Select, Self::Pan];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Pan => "Pan",
        }
    }
}

/// Peaks for one asset, or why they are not on screen yet.
#[derive(Clone, Debug)]
pub enum WaveformState {
    Pending,
    Ready(Arc<CachedWaveform>),
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TimelineAction {
    SelectClip(ClipId),
    ClearSelection,
    MoveClip {
        clip_id: ClipId,
        start_sample: u64,
    },
    Seek(u64),
    ToggleTrackMute(TrackId),
    ToggleTrackSolo(TrackId),
    DropAsset {
        asset_id: AssetId,
        track_id: TrackId,
        layer_id: LayerId,
        start_sample: u64,
    },
}

pub struct TimelineContext<'a> {
    pub project: &'a Project,
    pub sample_rate: u32,
    pub selected_clip: Option<ClipId>,
    pub playhead: u64,
    pub waveforms: &'a HashMap<AssetId, WaveformState>,
}

#[derive(Clone, Copy, Debug)]
struct ClipDrag {
    clip_id: ClipId,
    origin_start: u64,
    preview_start: u64,
}

pub struct TimelineView {
    pub tool: Tool,
    pub snap: bool,
    pixels_per_second: f32,
    scroll_x: f32,
    scroll_y: f32,
    drag: Option<ClipDrag>,
}

impl Default for TimelineView {
    fn default() -> Self {
        Self {
            tool: Tool::Select,
            snap: true,
            pixels_per_second: 160.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
            drag: None,
        }
    }
}

impl TimelineView {
    #[must_use]
    pub fn zoom_label(&self) -> String {
        format!("{:.0} px/s", self.pixels_per_second)
    }

    #[must_use]
    pub const fn pixels_per_second(&self) -> f32 {
        self.pixels_per_second
    }

    pub fn zoom_in(&mut self) {
        self.set_zoom(self.pixels_per_second * 1.5, None, 0.0);
    }

    pub fn zoom_out(&mut self) {
        self.set_zoom(self.pixels_per_second / 1.5, None, 0.0);
    }

    /// Fits `duration_frames` of material into `width` pixels of lane.
    pub fn zoom_to_fit(&mut self, duration_frames: u64, sample_rate: u32, width: f32) {
        let seconds = duration_frames as f64 / f64::from(sample_rate.max(1));
        if seconds <= 0.0 || width <= 1.0 {
            return;
        }
        self.pixels_per_second =
            ((width as f64 * 0.94) / seconds).clamp(MIN_ZOOM as f64, MAX_ZOOM as f64) as f32;
        self.scroll_x = 0.0;
    }

    /// Sets zoom, keeping the time under `anchor_x` pinned to that pixel.
    fn set_zoom(&mut self, zoom: f32, anchor_x: Option<f32>, content_left: f32) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        match anchor_x {
            Some(x) => {
                let offset = x - content_left;
                let time_under_pointer =
                    f64::from(self.scroll_x + offset) / f64::from(self.pixels_per_second);
                self.pixels_per_second = zoom;
                self.scroll_x = (time_under_pointer * f64::from(zoom)) as f32 - offset;
            }
            None => self.pixels_per_second = zoom,
        }
        self.scroll_x = self.scroll_x.max(0.0);
    }

    #[must_use]
    fn seconds_at(&self, x: f32, content_left: f32) -> f64 {
        f64::from(x - content_left + self.scroll_x) / f64::from(self.pixels_per_second)
    }

    #[must_use]
    fn x_of(&self, seconds: f64, content_left: f32) -> f32 {
        content_left + (seconds * f64::from(self.pixels_per_second)) as f32 - self.scroll_x
    }

    /// The grid clip edges and the playhead land on, or `None` when snap is off.
    #[must_use]
    fn snap_grid(&self) -> Option<f64> {
        self.snap.then(|| grid_seconds(self.pixels_per_second))
    }

    #[must_use]
    fn frame_at(&self, x: f32, content_left: f32, sample_rate: u32) -> u64 {
        let seconds = snap_to(self.seconds_at(x, content_left).max(0.0), self.snap_grid());
        (seconds * f64::from(sample_rate)).max(0.0) as u64
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        context: &TimelineContext<'_>,
    ) -> Vec<TimelineAction> {
        let mut actions = Vec::new();
        let rect = ui.available_rect_before_wrap();
        let background = ui.allocate_rect(rect, Sense::click_and_drag());

        let content_left = rect.left() + HEADER_WIDTH;
        let ruler_rect = Rect::from_min_max(
            pos2(content_left, rect.top()),
            pos2(rect.right(), rect.top() + RULER_HEIGHT),
        );
        let lanes_rect = Rect::from_min_max(
            pos2(content_left, ruler_rect.bottom()),
            pos2(rect.right(), rect.bottom() - SCROLLBAR_HEIGHT),
        );
        let header_rect = Rect::from_min_max(
            pos2(rect.left(), ruler_rect.bottom()),
            pos2(content_left, lanes_rect.bottom()),
        );

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, theme::RADIUS, theme::BG);

        let lane_count = context
            .project
            .tracks
            .iter()
            .map(|track| track.layers.len().max(1))
            .sum::<usize>()
            .max(1);
        let content_height = lane_count as f32 * LANE_HEIGHT;
        self.scroll_y = self
            .scroll_y
            .clamp(0.0, (content_height - lanes_rect.height()).max(0.0));

        let duration_seconds =
            project_duration_frames(context.project) as f64 / f64::from(context.sample_rate.max(1));
        let content_width = (duration_seconds * f64::from(self.pixels_per_second)) as f32;
        // Always allow a screen of empty space past the end to drop into.
        let max_scroll = (content_width + lanes_rect.width() * 0.5 - lanes_rect.width()).max(0.0);
        self.scroll_x = self.scroll_x.clamp(0.0, max_scroll);

        self.handle_scroll_and_zoom(ui, rect, content_left, max_scroll);

        self.paint_ruler(&painter, ruler_rect, content_left);
        self.paint_lanes(&painter, lanes_rect, lane_count);
        self.paint_loop_region(&painter, ruler_rect, lanes_rect, content_left, context);
        actions.extend(self.clips(ui, lanes_rect, content_left, context));
        self.paint_playhead(&painter, ruler_rect, lanes_rect, content_left, context);
        self.paint_markers(&painter, ruler_rect, content_left, context);
        self.paint_headers(&painter, header_rect, context);
        actions.extend(self.header_controls(ui, header_rect, context));

        if let Some(action) = self.handle_ruler_seek(ui, ruler_rect, content_left, context) {
            actions.push(action);
        }
        if let Some(action) = self.handle_drop(ui, &background, lanes_rect, content_left, context) {
            actions.push(action);
        }
        self.handle_background(ui, &background, lanes_rect, &mut actions);
        self.paint_scrollbar(
            &painter,
            Rect::from_min_max(
                pos2(content_left, rect.bottom() - SCROLLBAR_HEIGHT),
                pos2(rect.right(), rect.bottom()),
            ),
            content_width,
            max_scroll,
        );

        if project_duration_frames(context.project) == 0 {
            self.paint_empty_state(&painter, lanes_rect, context);
        }

        // A drag that ended off-surface — the window lost focus, the pointer
        // left the lane — leaves a clip stuck in preview. Clear it at the end of
        // the frame, after `clips` has had its chance to see `drag_stopped`;
        // clearing earlier would swallow every legitimate drop, because the
        // button is already up on the frame the drag stops.
        if self.drag.is_some() && !ui.input(|input| input.pointer.any_down()) {
            self.drag = None;
        }

        painter.rect_stroke(
            rect,
            theme::RADIUS,
            Stroke::new(1.0_f32, theme::RULE),
            StrokeKind::Inside,
        );
        actions
    }

    // ─── input ──────────────────────────────────────────────────────────────

    fn handle_scroll_and_zoom(
        &mut self,
        ui: &egui::Ui,
        rect: Rect,
        content_left: f32,
        max_scroll: f32,
    ) {
        let pointer = ui.ctx().pointer_hover_pos();
        let Some(pointer) = pointer.filter(|position| rect.contains(*position)) else {
            return;
        };
        let (scroll, modifiers) = ui.input(|input| (input.smooth_scroll_delta, input.modifiers));
        if scroll == Vec2::ZERO {
            return;
        }
        if modifiers.command || modifiers.ctrl {
            let factor = (scroll.y * 0.004).exp();
            self.set_zoom(
                self.pixels_per_second * factor,
                Some(pointer.x.max(content_left)),
                content_left,
            );
        } else if modifiers.shift {
            self.scroll_x = (self.scroll_x - scroll.y - scroll.x).clamp(0.0, max_scroll);
        } else {
            self.scroll_x = (self.scroll_x - scroll.x).clamp(0.0, max_scroll);
            self.scroll_y -= scroll.y;
        }
    }

    fn handle_ruler_seek(
        &self,
        ui: &egui::Ui,
        ruler_rect: Rect,
        content_left: f32,
        context: &TimelineContext<'_>,
    ) -> Option<TimelineAction> {
        let response = ui.interact(
            ruler_rect,
            egui::Id::new("jutsu_timeline_ruler"),
            Sense::click_and_drag(),
        );
        let position = response.interact_pointer_pos()?;
        (response.clicked() || response.dragged()).then(|| {
            TimelineAction::Seek(self.frame_at(position.x, content_left, context.sample_rate))
        })
    }

    fn handle_background(
        &mut self,
        ui: &egui::Ui,
        background: &egui::Response,
        lanes_rect: Rect,
        actions: &mut Vec<TimelineAction>,
    ) {
        if self.tool == Tool::Pan || ui.input(|input| input.pointer.middle_down()) {
            if background.dragged() {
                self.scroll_x = (self.scroll_x - background.drag_delta().x).max(0.0);
                self.scroll_y -= background.drag_delta().y;
            }
            if background.hovered() {
                ui.ctx().set_cursor_icon(if background.dragged() {
                    egui::CursorIcon::Grabbing
                } else {
                    egui::CursorIcon::Grab
                });
            }
            return;
        }
        if background.clicked()
            && background
                .interact_pointer_pos()
                .is_some_and(|position| lanes_rect.contains(position))
        {
            actions.push(TimelineAction::ClearSelection);
        }
    }

    fn handle_drop(
        &self,
        ui: &egui::Ui,
        background: &egui::Response,
        lanes_rect: Rect,
        content_left: f32,
        context: &TimelineContext<'_>,
    ) -> Option<TimelineAction> {
        let hovering = background.dnd_hover_payload::<AssetId>().is_some();
        let released = background.dnd_release_payload::<AssetId>();
        if !hovering && released.is_none() {
            return None;
        }
        let pointer = ui.ctx().pointer_hover_pos()?;
        let (track_id, layer_id, lane_index) = self.lane_at(pointer.y, lanes_rect, context)?;
        let start_sample = self.frame_at(
            pointer.x.max(content_left),
            content_left,
            context.sample_rate,
        );

        if hovering {
            let painter = ui.painter_at(lanes_rect);
            let top = lanes_rect.top() + lane_index as f32 * LANE_HEIGHT - self.scroll_y;
            painter.rect_filled(
                Rect::from_min_size(
                    pos2(lanes_rect.left(), top),
                    Vec2::new(lanes_rect.width(), LANE_HEIGHT),
                ),
                CornerRadius::ZERO,
                Color32::from_rgba_unmultiplied(0xf0, 0xa5, 0x31, 18),
            );
            let x = self.x_of(
                start_sample as f64 / f64::from(context.sample_rate.max(1)),
                content_left,
            );
            painter.line_segment(
                [pos2(x, top), pos2(x, top + LANE_HEIGHT)],
                Stroke::new(2.0_f32, theme::SIGNAL),
            );
        }

        released.map(|asset_id| TimelineAction::DropAsset {
            asset_id: *asset_id,
            track_id,
            layer_id,
            start_sample,
        })
    }

    fn lane_at(
        &self,
        y: f32,
        lanes_rect: Rect,
        context: &TimelineContext<'_>,
    ) -> Option<(TrackId, LayerId, usize)> {
        let offset = y - lanes_rect.top() + self.scroll_y;
        if offset < 0.0 {
            return None;
        }
        let wanted = (offset / LANE_HEIGHT) as usize;
        let mut index = 0;
        for track in &context.project.tracks {
            for layer in &track.layers {
                if index == wanted {
                    return Some((track.id, layer.id, index));
                }
                index += 1;
            }
        }
        // Past the last lane: fall back to the last real one so a drop still lands.
        context.project.tracks.last().and_then(|track| {
            track
                .layers
                .last()
                .map(|layer| (track.id, layer.id, index.saturating_sub(1)))
        })
    }

    // ─── clips ──────────────────────────────────────────────────────────────

    fn clips(
        &mut self,
        ui: &mut egui::Ui,
        lanes_rect: Rect,
        content_left: f32,
        context: &TimelineContext<'_>,
    ) -> Vec<TimelineAction> {
        let mut actions = Vec::new();
        let painter = ui.painter_at(lanes_rect);
        let rate = f64::from(context.sample_rate.max(1));
        let mut lane_index = 0;

        for track in &context.project.tracks {
            for layer in &track.layers {
                let lane_top = lanes_rect.top() + lane_index as f32 * LANE_HEIGHT - self.scroll_y;
                lane_index += 1;
                if lane_top > lanes_rect.bottom() || lane_top + LANE_HEIGHT < lanes_rect.top() {
                    continue;
                }
                for clip in &layer.clips {
                    let start = if self.drag.is_some_and(|drag| drag.clip_id == clip.id) {
                        self.drag.expect("checked above").preview_start
                    } else {
                        clip.start_sample
                    };
                    let left = self.x_of(start as f64 / rate, content_left);
                    let width = (clip.duration_samples as f64 / rate
                        * f64::from(self.pixels_per_second)) as f32;
                    let clip_rect = Rect::from_min_size(
                        pos2(left, lane_top + CLIP_INSET),
                        Vec2::new(width.max(3.0), LANE_HEIGHT - CLIP_INSET * 2.0),
                    );
                    // Cull anything fully outside the lane viewport rather than
                    // squashing it against the edge.
                    if !clip_rect.intersects(lanes_rect) {
                        continue;
                    }

                    let hit = clip_rect.intersect(lanes_rect);
                    let response = ui.interact(
                        hit,
                        egui::Id::new(("jutsu_clip", clip.id.as_uuid())),
                        Sense::click_and_drag(),
                    );
                    self.paint_clip(&painter, clip_rect, lanes_rect, clip, context, &response);

                    if self.tool == Tool::Select {
                        if response.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                        }
                        if response.drag_started() {
                            self.drag = Some(ClipDrag {
                                clip_id: clip.id,
                                origin_start: clip.start_sample,
                                preview_start: clip.start_sample,
                            });
                        }
                        if let Some(drag) = self.drag.filter(|drag| drag.clip_id == clip.id) {
                            if let Some(total) = response.total_drag_delta() {
                                self.drag = Some(ClipDrag {
                                    preview_start: shifted_start(
                                        drag.origin_start,
                                        total.x,
                                        self.pixels_per_second,
                                        context.sample_rate,
                                        self.snap_grid(),
                                    ),
                                    ..drag
                                });
                                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                            }
                            if response.drag_stopped() {
                                // Re-read: the block above may have just moved
                                // the preview, and `drag` is a copy from before.
                                actions.push(TimelineAction::MoveClip {
                                    clip_id: clip.id,
                                    start_sample: self
                                        .drag
                                        .map_or(drag.preview_start, |latest| latest.preview_start),
                                });
                                self.drag = None;
                            }
                        }
                        if response.clicked() {
                            actions.push(TimelineAction::SelectClip(clip.id));
                        }
                    }
                }
            }
        }
        actions
    }

    fn paint_clip(
        &self,
        painter: &egui::Painter,
        clip_rect: Rect,
        lanes_rect: Rect,
        clip: &Clip,
        context: &TimelineContext<'_>,
        response: &egui::Response,
    ) {
        let selected = context.selected_clip == Some(clip.id);
        let painter = painter.with_clip_rect(lanes_rect);
        painter.rect_filled(clip_rect, theme::RADIUS, theme::RAISED);
        painter.rect_stroke(
            clip_rect,
            theme::RADIUS,
            Stroke::new(
                1.0_f32,
                if selected {
                    theme::SIGNAL
                } else if response.hovered() {
                    theme::DIM
                } else {
                    theme::RULE
                },
            ),
            StrokeKind::Inside,
        );

        let title_rect = Rect::from_min_size(
            clip_rect.min,
            Vec2::new(clip_rect.width(), CLIP_TITLE_HEIGHT),
        );
        painter.rect_filled(
            title_rect,
            CornerRadius::ZERO,
            Color32::from_rgba_unmultiplied(0xff, 0xff, 0xff, 8),
        );
        let asset = context
            .project
            .assets
            .iter()
            .find(|asset| asset.id == clip.asset_id);
        if clip_rect.width() > 34.0 {
            // Clipped to its own title bar: a long name is cut, never drawn
            // across the clip beside it.
            let painter = painter.with_clip_rect(title_rect.intersect(lanes_rect));
            painter.text(
                pos2(clip_rect.left() + 5.0, title_rect.center().y),
                egui::Align2::LEFT_CENTER,
                elide(
                    asset.map_or("Missing sample", |asset| asset.name.as_str()),
                    clip_rect.width() - 10.0,
                ),
                theme::mono(9.5),
                if selected { theme::SIGNAL } else { theme::DIM },
            );
        }

        let wave_rect = Rect::from_min_max(
            pos2(clip_rect.left() + 2.0, title_rect.bottom() + 2.0),
            pos2(clip_rect.right() - 2.0, clip_rect.bottom() - 2.0),
        );
        if wave_rect.width() < 2.0 || wave_rect.height() < 2.0 {
            return;
        }
        let color = if selected { theme::SIGNAL } else { theme::DIM };
        match asset.map(|asset| context.waveforms.get(&asset.id)) {
            Some(Some(WaveformState::Ready(waveform))) => {
                self.paint_peaks(&painter, wave_rect, lanes_rect, clip, waveform, color);
            }
            Some(Some(WaveformState::Failed)) => {
                painter.text(
                    wave_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "peaks unavailable",
                    theme::mono(9.0),
                    theme::FAINT,
                );
            }
            _ => {
                // Pending: a flat baseline reads as "loading", not as silence.
                painter.line_segment(
                    [
                        pos2(wave_rect.left(), wave_rect.center().y),
                        pos2(wave_rect.right(), wave_rect.center().y),
                    ],
                    Stroke::new(1.0_f32, theme::RULE),
                );
            }
        }
    }

    /// Draws one vertical min/max bar per visible pixel column, straight from
    /// the cached peaks. Only the columns inside `viewport` are touched.
    fn paint_peaks(
        &self,
        painter: &egui::Painter,
        wave_rect: Rect,
        viewport: Rect,
        clip: &Clip,
        waveform: &CachedWaveform,
        color: Color32,
    ) {
        if waveform.peaks.is_empty() || waveform.window_frames == 0 {
            return;
        }
        let first = wave_rect.left().max(viewport.left()).floor();
        let last = wave_rect.right().min(viewport.right()).ceil();
        if last <= first {
            return;
        }
        // Source frames covered by one pixel of lane.
        let source_frames_per_pixel =
            f64::from(waveform.metadata.sample_rate) / f64::from(self.pixels_per_second);
        // Zoomed out, read from a coarser level: one fold per column instead of
        // thousands, which is what keeps a long timeline scrolling.
        let (window_frames, peaks) = waveform.level_for(source_frames_per_pixel);
        let window = window_frames as f64;
        let centre = wave_rect.center().y;
        let half_height = wave_rect.height() * 0.5;
        let stroke = Stroke::new(1.0_f32, color);

        let mut x = first;
        while x < last {
            let pixels_in = f64::from(x - wave_rect.left());
            let source_frame =
                clip.source_start_sample as f64 + pixels_in * source_frames_per_pixel;
            let from = (source_frame / window).floor().max(0.0) as usize;
            let to = ((source_frame + source_frames_per_pixel) / window).ceil() as usize;
            let to = to.clamp(from + 1, peaks.len());
            if from >= peaks.len() {
                break;
            }
            let (minimum, maximum) = peaks[from..to]
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), peak| {
                    (low.min(peak.minimum), high.max(peak.maximum))
                });
            if minimum.is_finite() && maximum.is_finite() {
                let top = centre - maximum.clamp(-1.0, 1.0) * half_height;
                let bottom = centre - minimum.clamp(-1.0, 1.0) * half_height;
                painter.line_segment([pos2(x, top), pos2(x, bottom.max(top + 0.75))], stroke);
            }
            x += 1.0;
        }
    }

    // ─── chrome ─────────────────────────────────────────────────────────────

    fn paint_ruler(&self, painter: &egui::Painter, ruler_rect: Rect, content_left: f32) {
        let painter = painter.with_clip_rect(ruler_rect);
        painter.rect_filled(ruler_rect, CornerRadius::ZERO, theme::BG);
        let grid = grid_seconds(self.pixels_per_second);
        let first = (self.seconds_at(ruler_rect.left(), content_left) / grid).floor() as i64;
        let last = (self.seconds_at(ruler_rect.right(), content_left) / grid).ceil() as i64;
        for step in first.max(0)..=last.max(0) {
            let seconds = step as f64 * grid;
            let x = self.x_of(seconds, content_left);
            let major = step % 4 == 0;
            painter.line_segment(
                [
                    pos2(x, ruler_rect.bottom() - if major { 8.0 } else { 4.0 }),
                    pos2(x, ruler_rect.bottom()),
                ],
                Stroke::new(1.0_f32, if major { theme::DIM } else { theme::RULE }),
            );
            if major {
                painter.text(
                    pos2(x + 4.0, ruler_rect.top() + 8.0),
                    egui::Align2::LEFT_TOP,
                    theme::format_ruler(seconds),
                    theme::mono(9.5),
                    theme::FAINT,
                );
            }
        }
        painter.line_segment(
            [
                pos2(ruler_rect.left(), ruler_rect.bottom()),
                pos2(ruler_rect.right(), ruler_rect.bottom()),
            ],
            Stroke::new(1.0_f32, theme::RULE),
        );
    }

    fn paint_lanes(&self, painter: &egui::Painter, lanes_rect: Rect, lane_count: usize) {
        let painter = painter.with_clip_rect(lanes_rect);
        let grid = grid_seconds(self.pixels_per_second);
        let content_left = lanes_rect.left();
        let start = (self.seconds_at(lanes_rect.left(), content_left) / grid).floor() as i64;
        let end = (self.seconds_at(lanes_rect.right(), content_left) / grid).ceil() as i64;

        for lane in 0..lane_count {
            let top = lanes_rect.top() + lane as f32 * LANE_HEIGHT - self.scroll_y;
            if lane % 2 == 1 {
                painter.rect_filled(
                    Rect::from_min_size(
                        pos2(lanes_rect.left(), top),
                        Vec2::new(lanes_rect.width(), LANE_HEIGHT),
                    ),
                    CornerRadius::ZERO,
                    Color32::from_rgba_unmultiplied(0xff, 0xff, 0xff, 4),
                );
            }
            painter.line_segment(
                [
                    pos2(lanes_rect.left(), top + LANE_HEIGHT),
                    pos2(lanes_rect.right(), top + LANE_HEIGHT),
                ],
                Stroke::new(1.0_f32, theme::RULE),
            );
        }
        // The time grid belongs over the tracks. Running it down through the
        // empty space below the last lane reads as another, emptier lane.
        let grid_bottom = (lanes_rect.top() + lane_count as f32 * LANE_HEIGHT - self.scroll_y)
            .min(lanes_rect.bottom());
        for step in start.max(0)..=end.max(0) {
            let x = self.x_of(step as f64 * grid, content_left);
            painter.line_segment(
                [pos2(x, lanes_rect.top()), pos2(x, grid_bottom)],
                Stroke::new(
                    1.0_f32,
                    if step % 4 == 0 {
                        theme::RULE
                    } else {
                        theme::GRID
                    },
                ),
            );
        }
    }

    /// The top of the first lane of each track, paired with the track. Mute and
    /// solo belong to the track, so their chips sit on that first row only.
    fn track_rows(&self, header_rect: Rect, context: &TimelineContext<'_>) -> Vec<(TrackId, f32)> {
        let mut lane = 0;
        let mut rows = Vec::new();
        for track in &context.project.tracks {
            let top = header_rect.top() + lane as f32 * LANE_HEIGHT - self.scroll_y;
            rows.push((track.id, top));
            lane += track.layers.len().max(1);
        }
        rows
    }

    /// Where the mute and solo chips sit on a track's first lane.
    fn chip_rects(header_rect: Rect, top: f32) -> [Rect; 2] {
        let y = top + LANE_HEIGHT - 24.0;
        let mute = Rect::from_min_size(pos2(header_rect.left() + 10.0, y), Vec2::new(22.0, 16.0));
        [mute, mute.translate(Vec2::new(26.0, 0.0))]
    }

    /// Mute and solo. Painted by [`Self::paint_headers`]; this is the half that
    /// listens, so the timeline still reports actions rather than mutating.
    fn header_controls(
        &self,
        ui: &mut egui::Ui,
        header_rect: Rect,
        context: &TimelineContext<'_>,
    ) -> Vec<TimelineAction> {
        let mut actions = Vec::new();
        for (track_id, top) in self.track_rows(header_rect, context) {
            if top > header_rect.bottom() || top + LANE_HEIGHT < header_rect.top() {
                continue;
            }
            let [mute, solo] = Self::chip_rects(header_rect, top);
            let mute = ui.interact(mute, egui::Id::new(("mute", track_id)), Sense::click());
            if mute.on_hover_text("Mute this track").clicked() {
                actions.push(TimelineAction::ToggleTrackMute(track_id));
            }
            let solo = ui.interact(solo, egui::Id::new(("solo", track_id)), Sense::click());
            if solo
                .on_hover_text("Solo this track — solo wins over mute")
                .clicked()
            {
                actions.push(TimelineAction::ToggleTrackSolo(track_id));
            }
        }
        actions
    }

    fn paint_headers(
        &self,
        painter: &egui::Painter,
        header_rect: Rect,
        context: &TimelineContext<'_>,
    ) {
        let painter = painter.with_clip_rect(header_rect);
        painter.rect_filled(header_rect, CornerRadius::ZERO, theme::PANEL);
        painter.line_segment(
            [
                pos2(header_rect.right(), header_rect.top()),
                pos2(header_rect.right(), header_rect.bottom()),
            ],
            Stroke::new(1.0_f32, theme::RULE),
        );

        let mut lane = 0;
        for (track_index, track) in context.project.tracks.iter().enumerate() {
            for layer in &track.layers {
                let top = header_rect.top() + lane as f32 * LANE_HEIGHT - self.scroll_y;
                lane += 1;
                if top > header_rect.bottom() || top + LANE_HEIGHT < header_rect.top() {
                    continue;
                }
                painter.line_segment(
                    [
                        pos2(header_rect.left(), top + LANE_HEIGHT),
                        pos2(header_rect.right(), top + LANE_HEIGHT),
                    ],
                    Stroke::new(1.0_f32, theme::RULE),
                );
                painter.text(
                    pos2(header_rect.left() + 10.0, top + 13.0),
                    egui::Align2::LEFT_TOP,
                    format!("{}  {}", track_index + 1, elide(&track.name, 92.0)),
                    theme::body(),
                    theme::TEXT,
                );
                painter.text(
                    pos2(header_rect.left() + 10.0, top + 30.0),
                    egui::Align2::LEFT_TOP,
                    elide(&layer.name, 100.0),
                    theme::mono(9.5),
                    theme::FAINT,
                );
                let clips = layer.clips.len();
                painter.text(
                    pos2(header_rect.left() + 10.0, top + 46.0),
                    egui::Align2::LEFT_TOP,
                    if clips == 1 {
                        "1 clip".to_owned()
                    } else {
                        format!("{clips} clips")
                    },
                    theme::mono(9.5),
                    theme::FAINT,
                );
            }
        }

        for (track_id, top) in self.track_rows(header_rect, context) {
            let Some(track) = context
                .project
                .tracks
                .iter()
                .find(|track| track.id == track_id)
            else {
                continue;
            };
            if top > header_rect.bottom() || top + LANE_HEIGHT < header_rect.top() {
                continue;
            }
            let [mute, solo] = Self::chip_rects(header_rect, top);
            paint_chip(&painter, mute, "M", track_flag(track, "mute"));
            paint_chip(&painter, solo, "S", track_flag(track, "solo"));
        }
    }

    /// Shades the looped span across the ruler and the lanes, so it reads as a
    /// region of time rather than as two lines.
    fn paint_loop_region(
        &self,
        painter: &egui::Painter,
        ruler_rect: Rect,
        lanes_rect: Rect,
        content_left: f32,
        context: &TimelineContext<'_>,
    ) {
        let Some(region) = context.project.loop_region else {
            return;
        };
        let rate = f64::from(context.sample_rate.max(1));
        let left = self.x_of(region.start_frame as f64 / rate, content_left);
        let right = self.x_of(region.end_frame as f64 / rate, content_left);
        if right <= ruler_rect.left() || left >= ruler_rect.right() {
            return;
        }
        let band = Rect::from_min_max(
            pos2(left.max(ruler_rect.left()), ruler_rect.top()),
            pos2(right.min(ruler_rect.right()), lanes_rect.bottom()),
        );
        let painter = painter.with_clip_rect(band);
        // A disabled loop is drawn fainter rather than hidden: it is still
        // where the loop will be when it is switched back on.
        let alpha = if region.enabled { 22 } else { 10 };
        painter.rect_filled(
            band,
            CornerRadius::ZERO,
            Color32::from_rgba_unmultiplied(0x4f, 0xd6, 0xff, alpha),
        );
        let edge = Stroke::new(
            1.0_f32,
            if region.enabled {
                theme::SIGNAL
            } else {
                theme::RULE
            },
        );
        painter.line_segment([pos2(left, band.top()), pos2(left, band.bottom())], edge);
        painter.line_segment([pos2(right, band.top()), pos2(right, band.bottom())], edge);
    }

    /// Markers sit on the ruler: a tick and, where there is room before the
    /// next one, a name.
    fn paint_markers(
        &self,
        painter: &egui::Painter,
        ruler_rect: Rect,
        content_left: f32,
        context: &TimelineContext<'_>,
    ) {
        let painter = painter.with_clip_rect(ruler_rect);
        let rate = f64::from(context.sample_rate.max(1));
        let mut ordered: Vec<&jutsu_audio_model::Marker> = context.project.markers.iter().collect();
        ordered.sort_by_key(|marker| marker.frame);

        for (index, marker) in ordered.iter().enumerate() {
            let x = self.x_of(marker.frame as f64 / rate, content_left);
            if x < ruler_rect.left() - 1.0 || x > ruler_rect.right() {
                continue;
            }
            painter.line_segment(
                [
                    pos2(x, ruler_rect.top()),
                    pos2(x, ruler_rect.top() + RULER_HEIGHT * 0.5),
                ],
                Stroke::new(1.0_f32, theme::ACCENT),
            );
            let room = ordered
                .get(index + 1)
                .map_or(ruler_rect.right() - x, |next| {
                    self.x_of(next.frame as f64 / rate, content_left) - x
                })
                .min(ruler_rect.right() - x);
            if room > 24.0 {
                painter.text(
                    pos2(x + 3.0, ruler_rect.top() + 1.0),
                    egui::Align2::LEFT_TOP,
                    elide(&marker.name, room - 6.0),
                    theme::mono(9.0),
                    theme::ACCENT,
                );
            }
        }
    }

    fn paint_playhead(
        &self,
        painter: &egui::Painter,
        ruler_rect: Rect,
        lanes_rect: Rect,
        content_left: f32,
        context: &TimelineContext<'_>,
    ) {
        let seconds = context.playhead as f64 / f64::from(context.sample_rate.max(1));
        let x = self.x_of(seconds, content_left);
        if x < content_left - 1.0 || x > lanes_rect.right() + 1.0 {
            return;
        }
        let painter = painter.with_clip_rect(Rect::from_min_max(
            pos2(content_left, ruler_rect.top()),
            lanes_rect.max,
        ));
        painter.line_segment(
            [pos2(x, ruler_rect.top()), pos2(x, lanes_rect.bottom())],
            Stroke::new(1.0_f32, theme::SIGNAL),
        );
        painter.add(egui::Shape::convex_polygon(
            vec![
                pos2(x - 4.5, ruler_rect.top()),
                pos2(x + 4.5, ruler_rect.top()),
                pos2(x, ruler_rect.top() + 6.0),
            ],
            theme::SIGNAL,
            Stroke::NONE,
        ));
    }

    fn paint_scrollbar(
        &self,
        painter: &egui::Painter,
        track_rect: Rect,
        content_width: f32,
        max_scroll: f32,
    ) {
        painter.rect_filled(track_rect, CornerRadius::ZERO, theme::BG);
        if max_scroll <= 0.0 || content_width <= 0.0 {
            return;
        }
        let total = content_width + track_rect.width() * 0.5;
        let visible = (track_rect.width() / total).clamp(0.05, 1.0);
        let thumb_width = track_rect.width() * visible;
        let offset =
            (self.scroll_x / max_scroll).clamp(0.0, 1.0) * (track_rect.width() - thumb_width);
        painter.rect_filled(
            Rect::from_min_size(
                pos2(track_rect.left() + offset, track_rect.top() + 2.0),
                Vec2::new(thumb_width, track_rect.height() - 4.0),
            ),
            CornerRadius::same(1),
            theme::RULE,
        );
    }

    fn paint_empty_state(
        &self,
        painter: &egui::Painter,
        lanes_rect: Rect,
        context: &TimelineContext<'_>,
    ) {
        let centre = lanes_rect.center();
        let (headline, hint) = if context.project.assets.is_empty() {
            (
                "Nothing on the timeline yet",
                "Import a WAV, then drag it here",
            )
        } else {
            (
                "Nothing on the timeline yet",
                "Drag a sample from the left, or double-click one",
            )
        };
        painter.text(
            pos2(centre.x, centre.y - 9.0),
            egui::Align2::CENTER_CENTER,
            headline,
            egui::FontId::proportional(13.0),
            theme::DIM,
        );
        painter.text(
            pos2(centre.x, centre.y + 10.0),
            egui::Align2::CENTER_CENTER,
            hint,
            egui::FontId::proportional(11.0),
            theme::FAINT,
        );
    }
}

// ─── helpers ────────────────────────────────────────────────────────────────

/// Seconds between minor grid lines, chosen so they land roughly 64 px apart.
#[must_use]
pub fn grid_seconds(pixels_per_second: f32) -> f64 {
    const CANDIDATES: [f64; 13] = [
        0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0,
    ];
    let target = 64.0 / f64::from(pixels_per_second.max(0.001));
    CANDIDATES
        .into_iter()
        .find(|candidate| *candidate >= target)
        .unwrap_or(60.0)
}

/// Clip start after a horizontal drag, clamped at the project start.
#[must_use]
pub fn shifted_start(
    origin_start: u64,
    drag_x: f32,
    pixels_per_second: f32,
    sample_rate: u32,
    snap_seconds: Option<f64>,
) -> u64 {
    let rate = f64::from(sample_rate.max(1));
    let frames = f64::from(drag_x) / f64::from(pixels_per_second.max(0.001)) * rate;
    let seconds = (origin_start as f64 + frames).max(0.0) / rate;
    (snap_to(seconds, snap_seconds) * rate).max(0.0) as u64
}

/// Rounds `seconds` onto `grid`, or leaves it alone when snap is off.
#[must_use]
fn snap_to(seconds: f64, grid: Option<f64>) -> f64 {
    match grid {
        Some(grid) if grid > 0.0 => (seconds / grid).round() * grid,
        _ => seconds,
    }
}

/// Longest clip end in the project, in project frames.
#[must_use]
pub fn project_duration_frames(project: &Project) -> u64 {
    project
        .tracks
        .iter()
        .flat_map(|track| &track.layers)
        .flat_map(|layer| &layer.clips)
        .map(|clip| clip.start_sample.saturating_add(clip.duration_samples))
        .max()
        .unwrap_or(0)
}

/// The rate the timeline counts in. Clip frames are project frames, so one rate
/// has to win: the first managed asset's, falling back to 48 kHz.
#[must_use]
pub fn project_sample_rate(project: &Project) -> u32 {
    project
        .assets
        .iter()
        .find_map(|asset| match asset.source {
            AudioAssetSource::ManagedFile { sample_rate, .. } => Some(sample_rate),
            _ => None,
        })
        .unwrap_or(48_000)
}

/// True when assets disagree about sample rate, which the status bar warns about.
#[must_use]
pub fn has_mixed_sample_rates(project: &Project) -> bool {
    let mut seen: Option<u32> = None;
    for asset in &project.assets {
        if let AudioAssetSource::ManagedFile { sample_rate, .. } = asset.source {
            match seen {
                Some(rate) if rate != sample_rate => return true,
                Some(_) => {}
                None => seen = Some(sample_rate),
            }
        }
    }
    false
}

#[must_use]
pub fn clip_gain_db(clip: &Clip) -> f64 {
    match clip.parameters.get("gain_db") {
        Some(ParameterValue::Float(value)) => *value,
        _ => 0.0,
    }
}

/// One header chip: filled while the flag is on, outlined while it is off, so
/// state is readable without hovering.
fn paint_chip(painter: &egui::Painter, rect: Rect, label: &str, on: bool) {
    painter.rect_filled(
        rect,
        theme::RADIUS,
        if on { theme::SIGNAL } else { theme::RAISED },
    );
    painter.rect_stroke(
        rect,
        theme::RADIUS,
        Stroke::new(1.0_f32, theme::RULE),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        theme::mono(9.5),
        if on { theme::BG } else { theme::DIM },
    );
}

/// A track flag as the mix reads it: absent counts as off.
#[must_use]
pub fn track_flag(track: &jutsu_audio_model::Track, key: &str) -> bool {
    matches!(
        track.parameters.get(key),
        Some(jutsu_audio_model::ParameterValue::Bool(true))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_gets_coarser_as_the_view_zooms_out() {
        assert!(grid_seconds(2_000.0) < grid_seconds(160.0));
        assert!(grid_seconds(160.0) < grid_seconds(6.0));
        assert_eq!(grid_seconds(0.001), 60.0);
    }

    #[test]
    fn dragging_a_clip_left_of_zero_clamps_to_the_project_start() {
        assert_eq!(shifted_start(48_000, -320.0, 160.0, 48_000, None), 0);
        assert_eq!(shifted_start(48_000, 160.0, 160.0, 48_000, None), 96_000);
        assert_eq!(shifted_start(0, -1.0, 160.0, 48_000, None), 0);
    }

    #[test]
    fn a_snapped_drag_lands_on_the_grid_not_between_it() {
        // 160 px/s, one second of grid: a 40 px nudge rounds back to where it
        // started, a 100 px nudge rounds on to the next line.
        assert_eq!(
            shifted_start(48_000, 40.0, 160.0, 48_000, Some(1.0)),
            48_000
        );
        assert_eq!(
            shifted_start(48_000, 100.0, 160.0, 48_000, Some(1.0)),
            96_000
        );
        // Snap never drags a clip before the project start.
        assert_eq!(shifted_start(4_800, -100.0, 160.0, 48_000, Some(1.0)), 0);
    }

    #[test]
    fn zooming_keeps_the_time_under_the_pointer_in_place() {
        let mut view = TimelineView {
            scroll_x: 480.0,
            ..Default::default()
        };
        let content_left = 100.0;
        let anchor = 400.0;
        let before = view.seconds_at(anchor, content_left);
        view.set_zoom(view.pixels_per_second * 2.5, Some(anchor), content_left);
        let after = view.seconds_at(anchor, content_left);
        assert!(
            (before - after).abs() < 0.02,
            "expected {before} to stay under the pointer, got {after}"
        );
    }

    #[test]
    fn zoom_is_clamped_to_a_usable_range() {
        let mut view = TimelineView::default();
        for _ in 0..40 {
            view.zoom_out();
        }
        assert_eq!(view.pixels_per_second, MIN_ZOOM);
        for _ in 0..40 {
            view.zoom_in();
        }
        assert_eq!(view.pixels_per_second, MAX_ZOOM);
    }
}
