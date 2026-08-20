//! The keyboard reference, and the keyboard navigation that needs one.
//!
//! Every editing action has a key, which is only useful if a user can find out
//! what it is: `?` or `F1` opens this. The list here is the documentation of
//! what `JutsuAudioApp::shortcuts` in `main.rs` handles — add a key there, add
//! a line here.

use eframe::egui::{self, RichText};
use jutsu_audio_model::{ClipId, Project};

use crate::theme;

/// One group of keys, as the overlay shows them.
pub struct Group {
    pub title: &'static str,
    pub keys: &'static [(&'static str, &'static str)],
}

pub const GROUPS: &[Group] = &[
    Group {
        title: "Transport",
        keys: &[
            ("Space", "Play or pause"),
            ("Esc", "Stop and return to the start of the loop"),
            ("Home / End", "Jump to the start or the end"),
            (", / .", "Previous or next marker"),
            ("M", "Drop a marker at the playhead"),
            ("L", "Toggle the loop region"),
        ],
    },
    Group {
        title: "Selection and editing",
        keys: &[
            ("Tab / Shift+Tab", "Select the next or previous clip"),
            ("Delete", "Delete the selected clip"),
            ("Shift+Delete", "Delete it and close the gap"),
            ("Ctrl+C / Ctrl+V", "Copy and paste clips"),
            ("Ctrl+Z / Ctrl+Y", "Undo and redo"),
            ("Ctrl+S", "Save"),
        ],
    },
    Group {
        title: "View",
        keys: &[
            ("+ / -", "Zoom the timeline in and out"),
            ("F", "Fit the whole project on screen"),
            (
                "Ctrl + / Ctrl -",
                "Make the whole interface larger or smaller",
            ),
            ("Ctrl+0", "Reset the interface size"),
            ("? or F1", "This list"),
        ],
    },
];

/// Draws the reference. Returns `true` once the user closes it.
pub fn prompt(context: &egui::Context) -> bool {
    let mut closed = false;
    let response = egui::Modal::new(egui::Id::new("shortcuts")).show(context, |ui| {
        ui.set_width(460.0);
        ui.label(
            RichText::new("Keyboard")
                .size(13.0)
                .color(theme::TEXT)
                .strong(),
        );
        ui.add_space(8.0);
        for group in GROUPS {
            ui.label(
                RichText::new(group.title)
                    .size(11.0)
                    .color(theme::SIGNAL)
                    .strong(),
            );
            ui.add_space(3.0);
            egui::Grid::new(group.title)
                .num_columns(2)
                .spacing([18.0, 3.0])
                .show(ui, |ui| {
                    for (keys, what) in group.keys {
                        ui.label(
                            RichText::new(*keys)
                                .font(theme::mono(10.5))
                                .color(theme::DIM),
                        );
                        ui.label(RichText::new(*what).size(11.0).color(theme::FAINT));
                        ui.end_row();
                    }
                });
            ui.add_space(8.0);
        }
        ui.add_space(2.0);
        if theme::flat_button(ui, "Close").clicked() {
            closed = true;
        }
    });
    // Clicking away or pressing Escape closes it too: a reference nobody can
    // dismiss the obvious way is its own usability problem.
    closed || response.should_close()
}

/// The clip after `current` in timeline order, wrapping at the end.
///
/// Timeline order rather than project order, so Tab walks the arrangement the
/// way it looks rather than the way it was built. Ties break on clip ID, which
/// keeps the walk stable when two clips start together.
#[must_use]
pub fn step_selection(project: &Project, current: Option<ClipId>, forward: bool) -> Option<ClipId> {
    let mut clips: Vec<(u64, ClipId)> = project
        .tracks
        .iter()
        .flat_map(|track| &track.layers)
        .flat_map(|layer| &layer.clips)
        .map(|clip| (clip.start_sample, clip.id))
        .collect();
    clips.sort_by(|one, two| one.0.cmp(&two.0).then_with(|| one.1.cmp(&two.1)));
    if clips.is_empty() {
        return None;
    }

    let position = current.and_then(|id| clips.iter().position(|(_, clip)| *clip == id));
    let next = match (position, forward) {
        // Nothing selected: forward starts at the first clip, back at the last.
        (None, true) => 0,
        (None, false) => clips.len() - 1,
        (Some(index), true) => (index + 1) % clips.len(),
        (Some(index), false) => (index + clips.len() - 1) % clips.len(),
    };
    Some(clips[next].1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jutsu_audio_project::ProjectStore;

    /// A project with clips at the given start frames, spread over two layers
    /// so the walk has to look past the project's own nesting.
    fn project_with(starts: &[u64]) -> Project {
        let mut project = ProjectStore::new_project("Keyboard");
        let asset_id = jutsu_audio_model::AssetId::new();
        project.assets.push(jutsu_audio_model::Asset {
            id: asset_id,
            name: "Blip".into(),
            source: jutsu_audio_model::AudioAssetSource::File {
                path: "blip.wav".into(),
            },
        });
        let second = jutsu_audio_model::Layer {
            id: jutsu_audio_model::LayerId::new(),
            name: "Layer 2".into(),
            clips: Vec::new(),
        };
        project.tracks[0].layers.push(second);
        for (index, start) in starts.iter().enumerate() {
            let clip = jutsu_audio_model::Clip {
                id: jutsu_audio_model::ClipId::new(),
                asset_id,
                start_sample: *start,
                source_start_sample: 0,
                duration_samples: 100,
                parameters: std::collections::BTreeMap::new(),
                notes: Vec::new(),
                pattern_id: None,
            };
            project.tracks[0].layers[index % 2].clips.push(clip);
        }
        project
    }

    fn starts(project: &Project) -> Vec<(u64, ClipId)> {
        let mut all: Vec<(u64, ClipId)> = project
            .tracks
            .iter()
            .flat_map(|track| &track.layers)
            .flat_map(|layer| &layer.clips)
            .map(|clip| (clip.start_sample, clip.id))
            .collect();
        all.sort_by_key(|(start, _)| *start);
        all
    }

    #[test]
    fn tab_walks_the_arrangement_in_time_order_and_wraps() {
        // Deliberately built out of order: what matters is where they sound.
        let project = project_with(&[300, 100, 200]);
        let order = starts(&project);

        let first = step_selection(&project, None, true).expect("a first clip");
        assert_eq!(
            first, order[0].1,
            "forward from nothing starts at the front"
        );
        let second = step_selection(&project, Some(first), true).expect("a second");
        assert_eq!(second, order[1].1);
        let third = step_selection(&project, Some(second), true).expect("a third");
        assert_eq!(third, order[2].1);
        assert_eq!(
            step_selection(&project, Some(third), true),
            Some(order[0].1),
            "and wraps rather than dead-ending"
        );
    }

    #[test]
    fn shift_tab_walks_the_other_way() {
        let project = project_with(&[100, 200, 300]);
        let order = starts(&project);

        let last = step_selection(&project, None, false).expect("a last clip");
        assert_eq!(last, order[2].1, "back from nothing starts at the end");
        assert_eq!(
            step_selection(&project, Some(order[0].1), false),
            Some(order[2].1),
            "and wraps at the front"
        );
    }

    #[test]
    fn an_empty_timeline_has_nothing_to_select() {
        assert_eq!(
            step_selection(&ProjectStore::new_project("Empty"), None, true),
            None
        );
    }

    #[test]
    fn a_selection_that_is_gone_starts_the_walk_over() {
        let project = project_with(&[100, 200]);
        let deleted = ClipId::new();
        assert_eq!(
            step_selection(&project, Some(deleted), true),
            Some(starts(&project)[0].1),
            "deleting the selected clip must not strand the keyboard"
        );
    }
}
