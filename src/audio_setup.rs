//! The first-run audio notice.
//!
//! Opening the default output device is the one part of startup that depends on
//! the machine rather than the project, and it is the one a new user is most
//! likely to hit: a fresh install on a box with no sound card, a remote
//! session, a device another application has taken exclusively.
//!
//! Failing silently would leave someone pressing play and hearing nothing with
//! no idea why. So: say it once, say what still works — everything except
//! playback, including export — and offer to try again once they have plugged
//! something in.

use eframe::egui::{self, RichText};

use crate::theme;

/// What the user decided. `None` while the notice is still open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    /// Look for an output device again.
    Retry,
    /// Carry on without playback. Not asked again this session.
    Continue,
}

/// Draws the notice. Returns the decision once one is made.
pub fn prompt(context: &egui::Context, error: &str) -> Option<Decision> {
    let mut decision = None;
    egui::Modal::new(egui::Id::new("audio-setup")).show(context, |ui| {
        ui.set_width(430.0);
        ui.label(
            RichText::new("No audio output device")
                .size(13.0)
                .color(theme::TEXT)
                .strong(),
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "Jutsu Audio could not open an output device, so playback is unavailable. \
                 Everything else works: editing, saving, and exporting a WAV all run without \
                 one.",
            )
            .size(11.5)
            .color(theme::DIM),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("The system reported: {error}"))
                .font(theme::mono(10.5))
                .color(theme::FAINT),
        );
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if theme::flat_button(ui, "Try again")
                .on_hover_text("Looks for the system default output device again")
                .clicked()
            {
                decision = Some(Decision::Retry);
            }
            if theme::flat_button(ui, "Continue without playback").clicked() {
                decision = Some(Decision::Continue);
            }
        });
    });
    decision
}
