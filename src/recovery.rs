//! The recovery prompt.
//!
//! Unsaved work found next to a project after a crash is always shown as a
//! choice. The saved file is what the user last chose to keep, so nothing
//! replaces it until they say so.

use eframe::egui::{self, RichText};
use jutsu_audio_model::Project;

use crate::theme;

/// Unsaved work waiting for the user to accept or throw away.
pub struct Recovery {
    pub project: Box<Project>,
}

/// What the user decided. `None` while the prompt is still open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    /// Load the recovered work into the editor, leaving the file alone until
    /// the next save.
    Restore,
    /// Keep the saved project and delete the recovery file.
    Discard,
}

/// Draws the prompt. Returns the decision once one is made.
pub fn prompt(context: &egui::Context, recovery: &Recovery) -> Option<Decision> {
    let mut decision = None;
    egui::Modal::new(egui::Id::new("recovery")).show(context, |ui| {
        ui.set_width(430.0);
        ui.label(
            RichText::new("Unsaved work was recovered")
                .size(13.0)
                .color(theme::TEXT)
                .strong(),
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!(
                "Jutsu Audio closed with unsaved edits to “{}”. The saved project on disk has \
                 not been changed.",
                recovery.project.metadata.name
            ))
            .size(11.5)
            .color(theme::DIM),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "Restoring loads the recovered edits into the editor; they replace the file only \
                 when you save.",
            )
            .size(11.0)
            .color(theme::FAINT),
        );
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if theme::flat_button(ui, "Restore recovered edits").clicked() {
                decision = Some(Decision::Restore);
            }
            if theme::flat_button(ui, "Keep the saved project")
                .on_hover_text("Deletes the recovery file")
                .clicked()
            {
                decision = Some(Decision::Discard);
            }
        });
    });
    decision
}
