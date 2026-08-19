//! The inspector section for a synth clip: the extension's own parameters, and
//! the notes the clip plays.
//!
//! Widgets are built from the registry descriptor rather than hard-coded, so a
//! synth registered later gets a usable editor without touching this file.
//! Edits are reported as whole values on release, the same way the timing
//! fields commit, so a drag is one command rather than one per frame.

use std::collections::BTreeMap;

use eframe::egui::{self, RichText};
use jutsu_audio_extensions::{ExtensionDescriptor, ParameterType};
use jutsu_audio_model::{ClipNote, ParameterValue};

use crate::theme;

/// What the user changed, once they let go of it.
#[derive(Clone, Debug, PartialEq)]
pub enum SynthAction {
    SetParameters(BTreeMap<String, ParameterValue>),
    SetNotes(Vec<ClipNote>),
}

/// Draws the section. `parameters` is what the asset currently holds; anything
/// the descriptor declares and the asset does not gets its default.
pub fn show(
    ui: &mut egui::Ui,
    descriptor: &ExtensionDescriptor,
    parameters: &BTreeMap<String, ParameterValue>,
    notes: &[ClipNote],
    sample_rate: u32,
) -> Option<SynthAction> {
    ui.add_space(18.0);
    ui.horizontal(|ui| {
        theme::column_label(ui, "Synth");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(descriptor.type_id.as_str())
                    .font(theme::mono(9.0))
                    .color(theme::FAINT),
            );
        });
    });
    ui.add_space(6.0);

    let mut action = parameter_rows(ui, descriptor, parameters);
    if action.is_none() {
        action = note_rows(ui, notes, sample_rate);
    }
    action
}

fn parameter_rows(
    ui: &mut egui::Ui,
    descriptor: &ExtensionDescriptor,
    parameters: &BTreeMap<String, ParameterValue>,
) -> Option<SynthAction> {
    let mut edited = parameters.clone();
    let mut committed = false;

    for parameter in &descriptor.parameters {
        let current = edited
            .get(&parameter.id)
            .cloned()
            .unwrap_or_else(|| parameter.default_value.clone());
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(&parameter.display_name)
                    .size(11.0)
                    .color(theme::DIM),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(value) = value_widget(ui, parameter.value_type, &current) {
                    edited.insert(parameter.id.clone(), value);
                    committed = true;
                }
            });
        });
        ui.add_space(3.0);
    }

    committed.then_some(SynthAction::SetParameters(edited))
}

/// One editor for one parameter value. `Some` only once the edit settles, so a
/// drag does not produce a command per frame.
fn value_widget(
    ui: &mut egui::Ui,
    value_type: ParameterType,
    current: &ParameterValue,
) -> Option<ParameterValue> {
    match (value_type, current) {
        (ParameterType::Float, ParameterValue::Float(value)) => {
            let mut value = *value;
            let response = ui.add_sized([86.0, 22.0], egui::DragValue::new(&mut value).speed(0.5));
            settled(&response).then_some(ParameterValue::Float(value))
        }
        (ParameterType::Integer, ParameterValue::Integer(value)) => {
            let mut value = *value;
            let response = ui.add_sized([86.0, 22.0], egui::DragValue::new(&mut value).speed(1.0));
            settled(&response).then_some(ParameterValue::Integer(value))
        }
        (ParameterType::Bool, ParameterValue::Bool(value)) => {
            let mut value = *value;
            ui.checkbox(&mut value, "")
                .changed()
                .then_some(ParameterValue::Bool(value))
        }
        (ParameterType::Text, ParameterValue::Text(value)) => {
            let mut value = value.clone();
            let response = ui.add_sized(
                [110.0, 22.0],
                egui::TextEdit::singleline(&mut value).font(theme::mono(10.0)),
            );
            (response.lost_focus() && response.changed()).then_some(ParameterValue::Text(value))
        }
        // A stored value whose type no longer matches the descriptor: show it,
        // do not let the user edit it into a worse state.
        _ => {
            ui.label(
                RichText::new("type mismatch")
                    .font(theme::mono(9.0))
                    .color(theme::DANGER),
            );
            None
        }
    }
}

fn settled(response: &egui::Response) -> bool {
    response.changed() && (response.drag_stopped() || response.lost_focus())
}

fn note_rows(ui: &mut egui::Ui, notes: &[ClipNote], sample_rate: u32) -> Option<SynthAction> {
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        theme::column_label(ui, "Notes");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(format!("{}", notes.len()))
                    .font(theme::mono(9.0))
                    .color(theme::FAINT),
            );
        });
    });
    ui.add_space(6.0);

    if notes.is_empty() {
        ui.label(
            RichText::new("This clip plays nothing yet")
                .size(10.5)
                .color(theme::FAINT),
        );
    }

    let mut edited = notes.to_vec();
    let mut committed = false;
    let mut removed = None;

    for (index, note) in edited.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let pitch = ui.add_sized(
                [70.0, 22.0],
                egui::DragValue::new(&mut note.pitch_hz)
                    .speed(1.0)
                    .range(20.0..=20_000.0)
                    .suffix(" Hz"),
            );
            let start = ui.add_sized(
                [70.0, 22.0],
                egui::DragValue::new(&mut note.start_frame)
                    .speed(f64::from(sample_rate) / 200.0)
                    .prefix("@"),
            );
            let length = ui.add_sized(
                [70.0, 22.0],
                egui::DragValue::new(&mut note.duration_frames)
                    .speed(f64::from(sample_rate) / 200.0)
                    .range(1..=u64::from(u32::MAX)),
            );
            committed |= settled(&pitch) || settled(&start) || settled(&length);
            if theme::flat_button(ui, "×")
                .on_hover_text("Remove this note")
                .clicked()
            {
                removed = Some(index);
            }
        });
        ui.add_space(3.0);
    }

    if let Some(index) = removed {
        edited.remove(index);
        return Some(SynthAction::SetNotes(edited));
    }

    ui.add_space(4.0);
    if theme::flat_button(ui, "+ Note")
        .on_hover_text("Adds a note after the last one")
        .clicked()
    {
        let start = edited
            .last()
            .map_or(0, |note| note.start_frame + note.duration_frames);
        edited.push(ClipNote {
            start_frame: start,
            duration_frames: u64::from(sample_rate) / 4,
            pitch_hz: 440.0,
            velocity: 1.0,
        });
        return Some(SynthAction::SetNotes(edited));
    }

    committed.then_some(SynthAction::SetNotes(edited))
}
