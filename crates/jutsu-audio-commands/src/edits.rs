//! Timeline editing primitives.
//!
//! Each function turns "what the user meant" into one command batch. Batches
//! are why undo is one logical step: a split is a shorten *and* an insert, and
//! the two either both land or neither does.
//!
//! Nothing here mutates. Callers hand the batch to the engine, which is still
//! the only thing that changes a project.

use jutsu_audio_model::{Clip, ClipId, LayerId, ParameterValue, Project, TrackId};

use crate::{CommandError, CommandErrorCode, ProjectCommand};

/// Clip parameter holding the fade-in length in project frames.
pub const FADE_IN_KEY: &str = "fade_in_samples";
/// Clip parameter holding the fade-out length in project frames.
pub const FADE_OUT_KEY: &str = "fade_out_samples";

/// What happens to the clips after a deleted one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeleteMode {
    /// Leave the gap where the clip was.
    Leave,
    /// Close the gap: everything later in the same lane moves earlier by the
    /// deleted clip's length.
    Ripple,
}

fn not_found(what: String) -> CommandError {
    CommandError {
        code: CommandErrorCode::EntityNotFound,
        message: what,
        command_index: None,
        expected_revision: None,
        actual_revision: None,
        diagnostics: Vec::new(),
    }
}

fn invalid(what: String) -> CommandError {
    CommandError {
        code: CommandErrorCode::ProjectValidationFailed,
        message: what,
        command_index: None,
        expected_revision: None,
        actual_revision: None,
        diagnostics: Vec::new(),
    }
}

/// A clip with the lane it lives in.
#[must_use]
pub fn locate(project: &Project, clip_id: ClipId) -> Option<(TrackId, LayerId, &Clip)> {
    project.tracks.iter().find_map(|track| {
        track.layers.iter().find_map(|layer| {
            layer
                .clips
                .iter()
                .find(|clip| clip.id == clip_id)
                .map(|clip| (track.id, layer.id, clip))
        })
    })
}

fn require(project: &Project, clip_id: ClipId) -> Result<(TrackId, LayerId, &Clip), CommandError> {
    locate(project, clip_id).ok_or_else(|| not_found(format!("clip {clip_id} does not exist")))
}

#[must_use]
pub fn gain_db(clip: &Clip) -> f64 {
    match clip.parameters.get("gain_db") {
        Some(ParameterValue::Float(value)) => *value,
        _ => 0.0,
    }
}

/// Copies clips into the same lane, offset later by `offset_frames`.
///
/// The copies carry the originals' parameters — gain, pan, fades — because a
/// duplicate that sounds different from what was duplicated is a bug.
pub fn duplicate(
    project: &Project,
    clip_ids: &[ClipId],
    offset_frames: u64,
) -> Result<Vec<ProjectCommand>, CommandError> {
    clip_ids
        .iter()
        .map(|clip_id| {
            let (track_id, layer_id, clip) = require(project, *clip_id)?;
            Ok(ProjectCommand::AddClip {
                track_id,
                layer_id,
                clip: Clip {
                    id: ClipId::new(),
                    start_sample: clip.start_sample.saturating_add(offset_frames),
                    ..clip.clone()
                },
            })
        })
        .collect()
}

/// Removes clips, optionally closing the gap behind each one.
///
/// Ripple only moves clips in the same lane that start at or after the removed
/// clip's end — a clip that overlaps the removed one keeps its place, since
/// there is no gap to close for it.
pub fn delete(
    project: &Project,
    clip_ids: &[ClipId],
    mode: DeleteMode,
) -> Result<Vec<ProjectCommand>, CommandError> {
    let mut commands = Vec::new();
    let mut shifted: Vec<(ClipId, u64)> = Vec::new();

    for clip_id in clip_ids {
        let (_, layer_id, clip) = require(project, *clip_id)?;
        let removed_end = clip.start_sample.saturating_add(clip.duration_samples);
        let length = clip.duration_samples;
        commands.push(ProjectCommand::RemoveClip { clip_id: *clip_id });

        if mode == DeleteMode::Ripple {
            for later in lane_clips(project, layer_id) {
                if clip_ids.contains(&later.id) || later.start_sample < removed_end {
                    continue;
                }
                let start = shifted
                    .iter()
                    .find(|(id, _)| *id == later.id)
                    .map_or(later.start_sample, |(_, start)| *start);
                let start = start.saturating_sub(length);
                shifted.retain(|(id, _)| *id != later.id);
                shifted.push((later.id, start));
            }
        }
    }

    for (clip_id, start_sample) in shifted {
        let (_, _, clip) = require(project, clip_id)?;
        commands.push(ProjectCommand::UpdateClip {
            clip_id,
            start_sample,
            source_start_sample: clip.source_start_sample,
            duration_samples: clip.duration_samples,
            gain_db: gain_db(clip),
        });
    }

    Ok(commands)
}

/// Moves the material under clips without moving the clips: the window on the
/// timeline stays put, the part of the source it shows slides.
///
/// A slip that would read before the start of the source is clamped there
/// rather than refused; dragging past the edge is a normal thing to do.
pub fn slip(
    project: &Project,
    clip_ids: &[ClipId],
    delta_frames: i64,
) -> Result<Vec<ProjectCommand>, CommandError> {
    clip_ids
        .iter()
        .map(|clip_id| {
            let (_, _, clip) = require(project, *clip_id)?;
            let source_start_sample = if delta_frames >= 0 {
                clip.source_start_sample
                    .saturating_add(delta_frames.unsigned_abs())
            } else {
                clip.source_start_sample
                    .saturating_sub(delta_frames.unsigned_abs())
            };
            Ok(ProjectCommand::UpdateClip {
                clip_id: *clip_id,
                start_sample: clip.start_sample,
                source_start_sample,
                duration_samples: clip.duration_samples,
                gain_db: gain_db(clip),
            })
        })
        .collect()
}

/// Cuts a clip in two at a project frame. The halves keep the source running
/// continuously across the cut, so a split is inaudible until something moves.
pub fn split(
    project: &Project,
    clip_id: ClipId,
    at_frame: u64,
) -> Result<Vec<ProjectCommand>, CommandError> {
    let (track_id, layer_id, clip) = require(project, clip_id)?;
    let end = clip.start_sample.saturating_add(clip.duration_samples);
    if at_frame <= clip.start_sample || at_frame >= end {
        return Err(invalid(format!(
            "frame {at_frame} is not inside clip {clip_id}"
        )));
    }
    let head = at_frame - clip.start_sample;
    let tail = clip.duration_samples - head;

    Ok(vec![
        ProjectCommand::UpdateClip {
            clip_id,
            start_sample: clip.start_sample,
            source_start_sample: clip.source_start_sample,
            duration_samples: head,
            gain_db: gain_db(clip),
        },
        ProjectCommand::AddClip {
            track_id,
            layer_id,
            clip: Clip {
                id: ClipId::new(),
                start_sample: at_frame,
                source_start_sample: clip.source_start_sample.saturating_add(head),
                duration_samples: tail,
                ..clip.clone()
            },
        },
    ])
}

/// Sets both fades on a clip. Lengths longer than the clip are clamped, and the
/// two together never exceed its length — a fade-in that runs past the fade-out
/// has no defined shape.
pub fn set_fades(
    project: &Project,
    clip_id: ClipId,
    fade_in_frames: u64,
    fade_out_frames: u64,
) -> Result<Vec<ProjectCommand>, CommandError> {
    let (_, _, clip) = require(project, clip_id)?;
    let (fade_in_samples, fade_out_samples) =
        clamp_fades(clip.duration_samples, fade_in_frames, fade_out_frames);
    Ok(vec![ProjectCommand::SetClipFades {
        clip_id,
        fade_in_samples,
        fade_out_samples,
    }])
}

/// Fades that fit: each is capped at the clip, and if they still overlap the
/// pair is scaled down to share the clip between them.
#[must_use]
pub fn clamp_fades(duration: u64, fade_in: u64, fade_out: u64) -> (u64, u64) {
    let fade_in = fade_in.min(duration);
    let fade_out = fade_out.min(duration);
    let total = fade_in.saturating_add(fade_out);
    if total <= duration {
        return (fade_in, fade_out);
    }
    let overflow = total - duration;
    // Take the overflow out of the longer fade first, so a small fade the user
    // set deliberately survives.
    if fade_in >= fade_out {
        (fade_in - overflow.min(fade_in), fade_out)
    } else {
        (fade_in, fade_out - overflow.min(fade_out))
    }
}

/// Fades two overlapping clips into each other across the whole overlap: the
/// earlier one out, the later one in.
pub fn crossfade(
    project: &Project,
    first: ClipId,
    second: ClipId,
) -> Result<Vec<ProjectCommand>, CommandError> {
    let (_, _, one) = require(project, first)?;
    let (_, _, other) = require(project, second)?;
    let (early, late) = if one.start_sample <= other.start_sample {
        (one, other)
    } else {
        (other, one)
    };
    let early_end = early.start_sample.saturating_add(early.duration_samples);
    let overlap = early_end.saturating_sub(late.start_sample);
    if overlap == 0 {
        return Err(invalid(format!(
            "clips {first} and {second} do not overlap, so there is nothing to cross-fade"
        )));
    }
    let overlap = overlap
        .min(early.duration_samples)
        .min(late.duration_samples);

    let (early_in, early_out) = clamp_fades(early.duration_samples, fade_in(early), overlap);
    let (late_in, late_out) = clamp_fades(late.duration_samples, overlap, fade_out(late));
    Ok(vec![
        ProjectCommand::SetClipFades {
            clip_id: early.id,
            fade_in_samples: early_in,
            fade_out_samples: early_out,
        },
        ProjectCommand::SetClipFades {
            clip_id: late.id,
            fade_in_samples: late_in,
            fade_out_samples: late_out,
        },
    ])
}

/// Pastes copied clips into a lane, the earliest landing on `at_frame` and the
/// rest keeping their spacing.
pub fn paste(
    clips: &[Clip],
    track_id: TrackId,
    layer_id: LayerId,
    at_frame: u64,
) -> Vec<ProjectCommand> {
    let earliest = clips
        .iter()
        .map(|clip| clip.start_sample)
        .min()
        .unwrap_or(0);
    clips
        .iter()
        .map(|clip| ProjectCommand::AddClip {
            track_id,
            layer_id,
            clip: Clip {
                id: ClipId::new(),
                start_sample: at_frame.saturating_add(clip.start_sample - earliest),
                ..clip.clone()
            },
        })
        .collect()
}

/// Every clip in one lane, in project order.
fn lane_clips(project: &Project, layer_id: LayerId) -> &[Clip] {
    project
        .tracks
        .iter()
        .flat_map(|track| &track.layers)
        .find(|layer| layer.id == layer_id)
        .map_or(&[], |layer| layer.clips.as_slice())
}

#[must_use]
pub fn fade_in(clip: &Clip) -> u64 {
    frames(clip, FADE_IN_KEY)
}

#[must_use]
pub fn fade_out(clip: &Clip) -> u64 {
    frames(clip, FADE_OUT_KEY)
}

fn frames(clip: &Clip, key: &str) -> u64 {
    match clip.parameters.get(key) {
        Some(ParameterValue::Integer(value)) => u64::try_from(*value).unwrap_or(0),
        Some(ParameterValue::Float(value)) if *value >= 0.0 => *value as u64,
        _ => 0,
    }
}
