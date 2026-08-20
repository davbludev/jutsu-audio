//! Running the interface headlessly, so what a user sees can be asserted.
//!
//! Every other test drives the layer *under* the widgets — the command engine,
//! the session, the mix. This drives the widgets themselves: it runs a real
//! `egui::Context` with no window, feeds it keys and clicks, and reads back the
//! text that was actually laid out for drawing.
//!
//! Reading the laid-out text is the point. A label that never renders, a button
//! whose name went missing, a modal that says nothing about why it appeared —
//! those are exactly the accessibility failures nothing else here would catch,
//! and they are invisible to a test that only checks state.
//!
//! Test-only, and deliberately dependency-free: `egui::Context::run` is enough,
//! so this needs no UI test framework and no display.

use eframe::egui::{self, Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};

/// A window big enough for the panels that assume one.
const SCREEN: Vec2 = Vec2::new(1280.0, 800.0);

/// One frame's worth of what the interface produced: every string laid out for
/// drawing, and where it landed.
pub struct Frame {
    pub text: Vec<(String, Rect)>,
}

impl Frame {
    /// Whether any laid-out string contains `needle`. Substring rather than
    /// equality: egui splits a paragraph into as many galleys as it needs.
    #[must_use]
    pub fn says(&self, needle: &str) -> bool {
        self.text.iter().any(|(line, _)| line.contains(needle))
    }

    /// Pairs of laid-out strings whose boxes sit on top of each other.
    ///
    /// Text that overlaps text is unreadable, and it is what a broken layout
    /// looks like from the outside: a panel that lays its rows out in the wrong
    /// direction still draws every label, so nothing but position catches it.
    #[must_use]
    pub fn overlaps(&self) -> Vec<(String, String)> {
        let mut found = Vec::new();
        for (index, (one, one_box)) in self.text.iter().enumerate() {
            for (two, two_box) in self.text.iter().skip(index + 1) {
                // Shrunk slightly: galley boxes carry a little padding, and
                // adjacent rows touching at the edge is not an overlap.
                let overlap = one_box.shrink(1.0).intersects(two_box.shrink(1.0));
                if overlap {
                    found.push((one.clone(), two.clone()));
                }
            }
        }
        found
    }

    /// Where to click to hit the control labelled `needle`. Roughly the middle
    /// of its text, which is inside the button drawn around it.
    #[must_use]
    pub fn position_of(&self, needle: &str) -> Option<Pos2> {
        self.text
            .iter()
            .find(|(line, _)| line.contains(needle))
            .map(|(_, at)| at.center())
    }

    /// Everything drawn, for a failure message worth reading.
    #[must_use]
    pub fn transcript(&self) -> String {
        self.text
            .iter()
            .map(|(line, _)| line.as_str())
            .collect::<Vec<_>>()
            .join(" ⏐ ")
    }
}

/// Drives a headless interface across frames.
pub struct Harness {
    context: egui::Context,
    events: Vec<Event>,
}

impl Default for Harness {
    fn default() -> Self {
        let context = egui::Context::default();
        crate::theme::configure(&context);
        Self {
            context,
            events: Vec::new(),
        }
    }
}

impl Harness {
    /// Queues a key press for the next frame.
    pub fn press(&mut self, key: Key) -> &mut Self {
        self.key(key, Modifiers::NONE)
    }

    pub fn key(&mut self, key: Key, modifiers: Modifiers) -> &mut Self {
        self.events.push(Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        });
        self.events.push(Event::Key {
            key,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers,
        });
        self
    }

    /// Queues a click at a point, the two events egui expects for one.
    pub fn click(&mut self, at: Pos2) -> &mut Self {
        self.events.push(Event::PointerMoved(at));
        for pressed in [true, false] {
            self.events.push(Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        self
    }

    /// Runs one frame and returns what it drew.
    ///
    /// egui lays out on the frame *after* a widget first appears, so a caller
    /// asserting on a fresh panel runs two frames — `settle` does that.
    pub fn frame(&mut self, build: impl FnMut(&egui::Context)) -> Frame {
        let mut build = build;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            events: std::mem::take(&mut self.events),
            ..Default::default()
        };
        let output = self.context.run(input, |context| build(context));

        let mut text = Vec::new();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut text);
        }
        Frame { text }
    }

    /// The same, for a panel that draws into a `Ui` rather than owning a
    /// window: the mixer and the timeline both take one.
    pub fn panel<T>(&mut self, build: impl FnMut(&mut egui::Ui) -> T) -> (Frame, Option<T>) {
        let mut build = build;
        let mut produced = None;
        let frame = self.settle(|context| {
            egui::CentralPanel::default().show(context, |ui| {
                produced = Some(build(ui));
            });
        });
        (frame, produced)
    }

    /// Runs the interface until its layout has settled, and returns the last
    /// frame. Two passes: the first creates the widgets, the second draws them
    /// at the size the first worked out.
    ///
    /// Queued input is held back for the second pass. Delivering a click to the
    /// layout pass would land it before the widget it was aimed at existed, and
    /// the caller would be looking at the wrong frame's answer.
    pub fn settle(&mut self, build: impl FnMut(&egui::Context)) -> Frame {
        let mut build = build;
        let queued = std::mem::take(&mut self.events);
        self.frame(&mut build);
        self.events = queued;
        self.frame(&mut build)
    }
}

fn collect_text(shape: &egui::Shape, into: &mut Vec<(String, Rect)>) {
    match shape {
        egui::Shape::Text(text) => into.push((
            text.galley.text().to_owned(),
            Rect::from_min_size(text.pos, text.galley.size()),
        )),
        // Panels and modals nest their contents, so a flat scan would miss
        // everything inside them.
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                collect_text(shape, into);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{audio_setup, recovery, shortcuts_help};
    use jutsu_audio_project::ProjectStore;

    #[test]
    fn the_keyboard_reference_lists_its_keys_with_what_they_do() {
        let mut harness = Harness::default();
        let frame = harness.settle(|context| {
            let _ = shortcuts_help::prompt(context);
        });

        assert!(frame.says("Keyboard"), "{}", frame.transcript());
        for group in shortcuts_help::GROUPS {
            assert!(
                frame.says(group.title),
                "the {} group never rendered: {}",
                group.title,
                frame.transcript()
            );
            for (keys, what) in group.keys {
                assert!(frame.says(keys), "{keys} is not shown");
                assert!(
                    frame.says(what),
                    "{keys} is shown without saying what it does"
                );
            }
        }
    }

    #[test]
    fn the_keyboard_reference_closes_on_escape() {
        let mut harness = Harness::default();
        let mut closed = harness.settle(|context| {
            let _ = shortcuts_help::prompt(context);
        });
        assert!(!closed.says("never"), "{}", closed.transcript());

        // A reference nobody can dismiss the obvious way is its own problem.
        let mut answered = false;
        harness.press(Key::Escape);
        closed = harness.frame(|context| {
            answered |= shortcuts_help::prompt(context);
        });
        assert!(answered, "escape did not close it: {}", closed.transcript());
    }

    #[test]
    fn the_audio_notice_says_what_is_wrong_what_still_works_and_what_to_do() {
        let mut harness = Harness::default();
        let frame = harness.settle(|context| {
            let _ = audio_setup::prompt(context, "device is in use by another application");
        });

        assert!(
            frame.says("No audio output device"),
            "{}",
            frame.transcript()
        );
        assert!(
            frame.says("exporting a WAV all run without"),
            "the notice must say what still works: {}",
            frame.transcript()
        );
        assert!(
            frame.says("device is in use by another application"),
            "and repeat what the system said: {}",
            frame.transcript()
        );
        // Both ways out are offered, in words rather than icons.
        assert!(frame.says("Try again") && frame.says("Continue without playback"));
    }

    #[test]
    fn the_recovery_prompt_names_the_project_and_both_outcomes() {
        let mut harness = Harness::default();
        let recovered = recovery::Recovery {
            project: Box::new(ProjectStore::new_project("Chase cue")),
        };
        let frame = harness.settle(|context| {
            let _ = recovery::prompt(context, &recovered);
        });

        assert!(frame.says("Chase cue"), "{}", frame.transcript());
        assert!(
            frame.says("has \u{201c}not\u{201d}") || frame.says("not been changed"),
            "the saved file's fate must be stated: {}",
            frame.transcript()
        );
        assert!(frame.says("Restore recovered edits") && frame.says("Keep the saved project"));
    }

    #[test]
    fn the_audio_notice_retries_when_its_button_is_clicked() {
        let mut harness = Harness::default();
        let frame = harness.settle(|context| {
            let _ = audio_setup::prompt(context, "no device");
        });
        let button = frame
            .position_of("Try again")
            .expect("the retry button is drawn");

        // Clicked rather than simulated: a label nothing can hit is not a
        // button, however good it looks.
        let mut decision = None;
        harness.click(button);
        let _ = harness.frame(|context| {
            decision = decision.or(audio_setup::prompt(context, "no device"));
        });
        assert_eq!(decision, Some(audio_setup::Decision::Retry));
    }

    /// A project with one named track, one bus and one clip on the timeline.
    fn arranged() -> jutsu_audio_model::Project {
        let mut project = ProjectStore::new_project("Cue");
        project.tracks[0].name = "Footsteps".into();
        let asset_id = jutsu_audio_model::AssetId::new();
        project.assets.push(jutsu_audio_model::Asset {
            id: asset_id,
            name: "Step".into(),
            source: jutsu_audio_model::AudioAssetSource::File {
                path: "step.wav".into(),
            },
        });
        project.tracks[0].layers[0]
            .clips
            .push(jutsu_audio_model::Clip {
                id: jutsu_audio_model::ClipId::new(),
                asset_id,
                start_sample: 0,
                source_start_sample: 0,
                duration_samples: 48_000,
                parameters: std::collections::BTreeMap::new(),
                notes: Vec::new(),
                pattern_id: None,
            });
        project
    }

    #[test]
    fn the_timeline_draws_its_tracks_and_selects_the_clip_that_is_clicked() {
        let project = arranged();
        let clip_id = project.tracks[0].layers[0].clips[0].id;
        let waveforms = std::collections::HashMap::new();
        let context = crate::timeline::TimelineContext {
            project: &project,
            sample_rate: 48_000,
            selected_clip: None,
            playhead: 0,
            waveforms: &waveforms,
        };

        let mut view = crate::timeline::TimelineView::default();
        let mut harness = Harness::default();
        let (frame, _) = harness.panel(|ui| view.show(ui, &context));
        assert!(frame.says("Footsteps"), "{}", frame.transcript());
        assert!(
            frame.says("Step"),
            "a clip is labelled with what it plays: {}",
            frame.transcript()
        );

        // The clip label sits on the clip, so clicking it is clicking the clip.
        let at = frame.position_of("Step").expect("the clip is drawn");
        harness.click(at);
        let (_, actions) = harness.panel(|ui| view.show(ui, &context));
        assert!(
            actions
                .expect("the panel ran")
                .contains(&crate::timeline::TimelineAction::SelectClip(clip_id)),
            "clicking a clip must select it"
        );
    }

    #[test]
    fn the_mixer_draws_a_strip_per_track_and_adds_a_bus_when_asked() {
        let project = arranged();
        let meters = jutsu_audio_engine::Meters::default();
        let registries = crate::extensions::registries();

        let mut harness = Harness::default();
        let (frame, _) =
            harness.panel(|ui| crate::mixer_panel::show(ui, &project, &meters, registries));
        assert!(
            frame.says("MIXER") && frame.says("Footsteps"),
            "{}",
            frame.transcript()
        );
        assert!(
            frame.says("Master"),
            "the master strip is always there: {}",
            frame.transcript()
        );
        // Mute and solo read as letters, so the state is not carried by colour.
        assert!(frame.says("M") && frame.says("S"));
        // Every strip lays its rows out downwards. When it laid them out across
        // instead, every label still drew — on top of the next one.
        let overlaps = frame.overlaps();
        assert!(
            overlaps.is_empty(),
            "labels are drawn on top of each other: {overlaps:?}"
        );

        let at = frame
            .position_of("+ Bus")
            .expect("the add-bus button is drawn");
        harness.click(at);
        let (_, actions) =
            harness.panel(|ui| crate::mixer_panel::show(ui, &project, &meters, registries));
        assert!(
            actions
                .expect("the panel ran")
                .iter()
                .any(|action| matches!(action, crate::mixer_panel::MixerAction::AddBus)),
            "clicking + Bus must ask for a bus"
        );
    }
}
