//! The mixer: one strip per track and bus, with its level, pan, routing, and
//! the effects it runs.
//!
//! Nothing here mutates. Every control reports an action, and the app turns
//! that into a command — so a fader move undoes like any other edit and reaches
//! anyone attached to the session.

use std::collections::BTreeMap;

use eframe::egui::{self, RichText, Vec2};
use jutsu_audio_engine::Meters;
use jutsu_audio_extensions::ExtensionRegistries;
use jutsu_audio_extensions::parameters::{GAIN_DB, MUTE, PAN, SOLO};
use jutsu_audio_model::{BusId, EffectId, ParameterValue, Project, TrackId};

use crate::theme;

/// What the user did to a strip.
#[derive(Clone, Debug, PartialEq)]
pub enum MixerAction {
    SetTrackParameter {
        track_id: TrackId,
        key: String,
        value: ParameterValue,
    },
    SetBusParameter {
        bus_id: BusId,
        key: String,
        value: ParameterValue,
    },
    SetTrackOutput {
        track_id: TrackId,
        output_bus_id: BusId,
    },
    SetBusOutput {
        bus_id: BusId,
        output_bus_id: Option<BusId>,
    },
    AddBus,
    AddEffect {
        target: EffectSlot,
        type_id: String,
    },
    RemoveEffect {
        effect_id: EffectId,
    },
    ToggleEffect {
        effect_id: EffectId,
        enabled: bool,
    },
    MoveEffect {
        effect_id: EffectId,
        to_index: usize,
    },
}

/// Which chain an effect belongs to, in the shape the panel works in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectSlot {
    Track(TrackId),
    Bus(BusId),
}

/// Draws the mixer. Returns everything the user asked for this frame.
pub fn show(
    ui: &mut egui::Ui,
    project: &Project,
    meters: &Meters,
    registries: &ExtensionRegistries,
) -> Vec<MixerAction> {
    let mut actions = Vec::new();

    ui.horizontal(|ui| {
        ui.add_space(10.0);
        theme::column_label(ui, "Mixer");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(10.0);
            if theme::tool_button(ui, "+ Bus", false)
                .on_hover_text("Add a bus feeding the master")
                .clicked()
            {
                actions.push(MixerAction::AddBus);
            }
        });
    });
    ui.add_space(4.0);

    egui::ScrollArea::horizontal()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.add_space(10.0);
                for track in &project.tracks {
                    let level = meters.tracks.get(&track.id).copied().unwrap_or(0.0);
                    actions.extend(track_strip(ui, project, track, level, registries));
                }
                for bus in &project.buses {
                    let level = meters.buses.get(&bus.id).copied().unwrap_or(0.0);
                    actions.extend(bus_strip(ui, project, bus, level, registries));
                }
            });
        });

    actions
}

const STRIP_WIDTH: f32 = 132.0;

fn track_strip(
    ui: &mut egui::Ui,
    project: &Project,
    track: &jutsu_audio_model::Track,
    level: f32,
    registries: &ExtensionRegistries,
) -> Vec<MixerAction> {
    let mut actions = Vec::new();
    strip_frame(ui, &track.name, level, |ui| {
        if let Some((key, value)) = fader_row(ui, &track.parameters) {
            actions.push(MixerAction::SetTrackParameter {
                track_id: track.id,
                key,
                value,
            });
        }
        if let Some((key, value)) = switch_row(ui, &track.parameters) {
            actions.push(MixerAction::SetTrackParameter {
                track_id: track.id,
                key,
                value,
            });
        }
        if let Some(Some(output_bus_id)) =
            routing_row(ui, project, Some(track.output_bus_id), track.id.to_string())
        {
            actions.push(MixerAction::SetTrackOutput {
                track_id: track.id,
                output_bus_id,
            });
        }
        actions.extend(effect_rack(
            ui,
            EffectSlot::Track(track.id),
            &track.effects,
            registries,
        ));
    });
    actions
}

fn bus_strip(
    ui: &mut egui::Ui,
    project: &Project,
    bus: &jutsu_audio_model::MixerBus,
    level: f32,
    registries: &ExtensionRegistries,
) -> Vec<MixerAction> {
    let mut actions = Vec::new();
    let is_master = bus.id == project.master_bus_id;
    let title = if is_master {
        format!("{} · master", bus.name)
    } else {
        bus.name.clone()
    };
    strip_frame(ui, &title, level, |ui| {
        if let Some((key, value)) = fader_row(ui, &bus.parameters) {
            actions.push(MixerAction::SetBusParameter {
                bus_id: bus.id,
                key,
                value,
            });
        }
        if is_master {
            ui.label(
                RichText::new("everything ends here")
                    .size(10.0)
                    .color(theme::FAINT),
            );
        } else if let Some(output) = routing_row(ui, project, bus.output_bus_id, bus.id.to_string())
        {
            actions.push(MixerAction::SetBusOutput {
                bus_id: bus.id,
                output_bus_id: output,
            });
        }
        actions.extend(effect_rack(
            ui,
            EffectSlot::Bus(bus.id),
            &bus.effects,
            registries,
        ));
    });
    actions
}

/// One strip's frame: a title, a meter, and whatever the caller draws inside.
fn strip_frame(ui: &mut egui::Ui, title: &str, level: f32, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::PANEL)
        .stroke(egui::Stroke::new(1.0_f32, theme::RULE))
        .corner_radius(theme::RADIUS)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            // The rack lays strips out side by side, so the frame inherits a
            // horizontal layout. Without this the strip's own rows — title,
            // meter, fader, routing — would run across the screen instead of
            // down the strip, and land on top of each other.
            ui.vertical(|ui| {
                ui.set_width(STRIP_WIDTH);
                ui.label(
                    RichText::new(theme::elide(title, STRIP_WIDTH))
                        .size(11.0)
                        .color(theme::TEXT),
                );
                ui.add_space(4.0);
                meter(ui, level);
                ui.add_space(6.0);
                contents(ui);
            });
        });
    ui.add_space(6.0);
}

/// A peak bar for the level this strip contributed to the last mix.
fn meter(ui: &mut egui::Ui, level: f32) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 6.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, theme::RADIUS, theme::RAISED);
    if level <= 0.0 {
        return;
    }
    // Decibels, not amplitude: a linear bar spends most of its length on the
    // top few dB and reads as either full or empty.
    let db = 20.0 * level.max(1e-5).log10();
    let filled = ((db + 60.0) / 66.0).clamp(0.0, 1.0);
    let mut bar = rect;
    bar.set_width(rect.width() * filled);
    painter.rect_filled(
        bar,
        theme::RADIUS,
        if level > 1.0 {
            theme::DANGER
        } else {
            theme::LIVE
        },
    );
}

/// Level and pan.
fn fader_row(
    ui: &mut egui::Ui,
    parameters: &BTreeMap<String, ParameterValue>,
) -> Option<(String, ParameterValue)> {
    let mut edit = None;

    let mut gain = float_of(parameters, GAIN_DB, 0.0);
    let response = ui.add(
        egui::Slider::new(&mut gain, -60.0..=12.0)
            .show_value(true)
            .suffix(" dB")
            .trailing_fill(true),
    );
    if settled(&response) {
        edit = Some((GAIN_DB.to_owned(), ParameterValue::Float(gain)));
    }
    if response.double_clicked() {
        edit = Some((GAIN_DB.to_owned(), ParameterValue::Float(0.0)));
    }

    let mut pan = float_of(parameters, PAN, 0.0);
    let response = ui.add(
        egui::Slider::new(&mut pan, -1.0..=1.0)
            .show_value(true)
            .text("pan"),
    );
    if settled(&response) {
        edit = Some((PAN.to_owned(), ParameterValue::Float(pan)));
    }
    if response.double_clicked() {
        edit = Some((PAN.to_owned(), ParameterValue::Float(0.0)));
    }
    edit
}

/// Mute and solo.
fn switch_row(
    ui: &mut egui::Ui,
    parameters: &BTreeMap<String, ParameterValue>,
) -> Option<(String, ParameterValue)> {
    let mut edit = None;
    ui.horizontal(|ui| {
        let muted = bool_of(parameters, MUTE);
        if theme::tool_button(ui, "M", muted)
            .on_hover_text("Mute")
            .clicked()
        {
            edit = Some((MUTE.to_owned(), ParameterValue::Bool(!muted)));
        }
        let soloed = bool_of(parameters, SOLO);
        if theme::tool_button(ui, "S", soloed)
            .on_hover_text("Solo — solo wins over mute")
            .clicked()
        {
            edit = Some((SOLO.to_owned(), ParameterValue::Bool(!soloed)));
        }
    });
    edit
}

/// Where this strip sends. `Some(None)` means the user chose "nowhere".
fn routing_row(
    ui: &mut egui::Ui,
    project: &Project,
    current: Option<BusId>,
    id: String,
) -> Option<Option<BusId>> {
    let mut chosen = None;
    let label = current
        .and_then(|bus_id| project.buses.iter().find(|bus| bus.id == bus_id))
        .map_or_else(|| "nowhere".to_owned(), |bus| bus.name.clone());

    ui.add_space(4.0);
    egui::ComboBox::from_id_salt(("routing", id))
        .selected_text(RichText::new(theme::elide(&label, 96.0)).size(10.5))
        .width(STRIP_WIDTH - 8.0)
        .show_ui(ui, |ui| {
            for bus in &project.buses {
                if Some(bus.id) == current {
                    continue;
                }
                if ui.selectable_label(false, &bus.name).clicked() {
                    chosen = Some(Some(bus.id));
                }
            }
        });
    chosen
}

/// The inserts on this strip, and a menu of what could be added.
fn effect_rack(
    ui: &mut egui::Ui,
    slot: EffectSlot,
    inserts: &[jutsu_audio_model::EffectInsert],
    registries: &ExtensionRegistries,
) -> Vec<MixerAction> {
    let mut actions = Vec::new();
    ui.add_space(6.0);
    theme::column_label(ui, "Effects");
    ui.add_space(2.0);

    if inserts.is_empty() {
        ui.label(RichText::new("none").size(10.0).color(theme::FAINT));
    }

    for (index, insert) in inserts.iter().enumerate() {
        ui.horizontal(|ui| {
            let name = registries
                .effect_descriptor(
                    &jutsu_audio_extensions::ExtensionTypeId::new(insert.type_id.clone())
                        .unwrap_or_else(|_| {
                            jutsu_audio_extensions::ExtensionTypeId::new("unknown.effect")
                                .expect("a valid fallback")
                        }),
                )
                .map_or_else(|| insert.type_id.clone(), |d| d.display_name.clone());

            if theme::tool_button(ui, if insert.enabled { "•" } else { "○" }, insert.enabled)
                .on_hover_text(if insert.enabled { "Bypass" } else { "Enable" })
                .clicked()
            {
                actions.push(MixerAction::ToggleEffect {
                    effect_id: insert.id,
                    enabled: !insert.enabled,
                });
            }
            ui.label(RichText::new(theme::elide(&name, 62.0)).size(10.5).color(
                if insert.enabled {
                    theme::TEXT
                } else {
                    theme::FAINT
                },
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::tool_button(ui, "×", false)
                    .on_hover_text("Remove")
                    .clicked()
                {
                    actions.push(MixerAction::RemoveEffect {
                        effect_id: insert.id,
                    });
                }
                if index > 0
                    && theme::tool_button(ui, "↑", false)
                        .on_hover_text("Move earlier in the chain")
                        .clicked()
                {
                    actions.push(MixerAction::MoveEffect {
                        effect_id: insert.id,
                        to_index: index - 1,
                    });
                }
            });
        });
    }

    ui.add_space(2.0);
    egui::ComboBox::from_id_salt(("add-effect", format!("{slot:?}")))
        .selected_text(RichText::new("+ Effect").size(10.5))
        .width(STRIP_WIDTH - 8.0)
        .show_ui(ui, |ui| {
            for type_id in registries.effect_type_ids() {
                let label = registries
                    .effect_descriptor(type_id)
                    .map_or_else(|| type_id.as_str().to_owned(), |d| d.display_name.clone());
                if ui.selectable_label(false, label).clicked() {
                    actions.push(MixerAction::AddEffect {
                        target: slot,
                        type_id: type_id.as_str().to_owned(),
                    });
                }
            }
        });
    actions
}

fn settled(response: &egui::Response) -> bool {
    response.changed() && (response.drag_stopped() || response.lost_focus())
}

fn float_of(parameters: &BTreeMap<String, ParameterValue>, id: &str, fallback: f64) -> f64 {
    match parameters.get(id) {
        Some(ParameterValue::Float(value)) => *value,
        _ => fallback,
    }
}

fn bool_of(parameters: &BTreeMap<String, ParameterValue>, id: &str) -> bool {
    matches!(parameters.get(id), Some(ParameterValue::Bool(true)))
}
