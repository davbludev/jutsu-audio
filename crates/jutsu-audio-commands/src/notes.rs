//! Note transforms: quantise, transpose, scale velocity, humanise.
//!
//! Each takes a clip and returns one command batch, like every other edit — so
//! a transform undoes in one step and reaches an attached session. Nothing here
//! is destructive: the notes are replaced wholesale, and undo puts back exactly
//! what was there.
//!
//! Humanising is seeded, so "random" here means "the same random every time".

use jutsu_audio_model::{ClipId, ClipNote, Project, TempoMap};

use crate::edits::locate;
use crate::{CommandError, CommandErrorCode, ProjectCommand};

fn not_found(clip_id: ClipId) -> CommandError {
    CommandError {
        code: CommandErrorCode::EntityNotFound,
        message: format!("clip {clip_id} does not exist"),
        command_index: None,
        expected_revision: None,
        actual_revision: None,
        diagnostics: Vec::new(),
    }
}

/// The notes a clip plays right now, resolved through its pattern if it has one.
pub fn notes_of(project: &Project, clip_id: ClipId) -> Result<Vec<ClipNote>, CommandError> {
    let (_, _, clip) = locate(project, clip_id).ok_or_else(|| not_found(clip_id))?;
    Ok(clip.resolved_notes(&project.patterns))
}

/// Replaces a clip's notes with the result of a transform.
fn replace(clip_id: ClipId, notes: Vec<ClipNote>) -> Vec<ProjectCommand> {
    vec![ProjectCommand::SetClipNotes { clip_id, notes }]
}

/// Snaps every note start to the nearest division of a beat.
///
/// Positions are clip-relative, so a clip that does not start on a beat keeps
/// its own grid — quantising is relative to the timeline, not to the clip.
pub fn quantise(
    project: &Project,
    clip_id: ClipId,
    divisions_per_beat: u32,
    sample_rate: u32,
) -> Result<Vec<ProjectCommand>, CommandError> {
    let (_, _, clip) = locate(project, clip_id).ok_or_else(|| not_found(clip_id))?;
    let start = clip.start_sample;
    let tempo = project.tempo_map();
    let notes = clip
        .resolved_notes(&project.patterns)
        .into_iter()
        .map(|note| {
            let absolute = start.saturating_add(note.start_frame);
            let snapped = tempo.quantise(absolute, divisions_per_beat, sample_rate);
            ClipNote {
                start_frame: snapped.saturating_sub(start),
                ..note
            }
        })
        .collect();
    Ok(replace(clip_id, notes))
}

/// Moves every note by a number of semitones, keeping their timing.
pub fn transpose(
    project: &Project,
    clip_id: ClipId,
    semitones: f64,
) -> Result<Vec<ProjectCommand>, CommandError> {
    let ratio = 2_f64.powf(semitones / 12.0);
    let notes = notes_of(project, clip_id)?
        .into_iter()
        .map(|note| ClipNote {
            // Clamped to the audible range: a transpose that runs off the end
            // of hearing should stop there rather than alias.
            pitch_hz: (note.pitch_hz * ratio).clamp(8.0, 20_000.0),
            ..note
        })
        .collect();
    Ok(replace(clip_id, notes))
}

/// Multiplies every velocity, keeping it inside `0.0..=1.0`.
pub fn scale_velocity(
    project: &Project,
    clip_id: ClipId,
    factor: f64,
) -> Result<Vec<ProjectCommand>, CommandError> {
    let notes = notes_of(project, clip_id)?
        .into_iter()
        .map(|note| ClipNote {
            velocity: (note.velocity * factor as f32).clamp(0.0, 1.0),
            ..note
        })
        .collect();
    Ok(replace(clip_id, notes))
}

/// Nudges timing and velocity by a bounded, seeded amount.
///
/// The same seed and the same notes always give the same result, so a humanised
/// pattern is still reproducible — which is what makes it safe to commit.
pub fn humanise(
    project: &Project,
    clip_id: ClipId,
    seed: u64,
    timing_frames: u64,
    velocity_amount: f64,
) -> Result<Vec<ProjectCommand>, CommandError> {
    let velocity_amount = velocity_amount.clamp(0.0, 1.0) as f32;
    let notes = notes_of(project, clip_id)?
        .into_iter()
        .enumerate()
        .map(|(index, note)| {
            // Two independent streams per note, so nudging timing does not also
            // decide the velocity.
            let timing = signed_unit(seed, index as u64, 1);
            let velocity = signed_unit(seed, index as u64, 2);
            let offset = (timing * timing_frames as f64).round();
            let start_frame = if offset >= 0.0 {
                note.start_frame.saturating_add(offset as u64)
            } else {
                note.start_frame.saturating_sub(offset.abs() as u64)
            };
            ClipNote {
                start_frame,
                velocity: (note.velocity + velocity as f32 * velocity_amount).clamp(0.0, 1.0),
                ..note
            }
        })
        .collect();
    Ok(replace(clip_id, notes))
}

/// A deterministic value in `-1.0..=1.0` from a seed, an index and a stream.
fn signed_unit(seed: u64, index: u64, stream: u64) -> f64 {
    let mut value = seed
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(index.wrapping_mul(0xbf58_476d_1ce4_e5b9))
        .wrapping_add(stream.wrapping_mul(0x94d0_49bb_1331_11eb));
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^= value >> 31;
    // Top 32 bits onto -1.0..1.0.
    ((value >> 32) as f64 / f64::from(u32::MAX)).mul_add(2.0, -1.0)
}

/// Repeats a clip's notes to fill a longer clip, at a fixed period.
///
/// The looping a pattern does automatically, available as a one-off transform
/// for a clip that holds its own notes.
pub fn loop_notes(
    project: &Project,
    clip_id: ClipId,
    period_frames: u64,
    repeats: u32,
) -> Result<Vec<ProjectCommand>, CommandError> {
    if period_frames == 0 {
        return Err(CommandError {
            code: CommandErrorCode::ProjectValidationFailed,
            message: "a loop needs a period longer than zero frames".into(),
            command_index: None,
            expected_revision: None,
            actual_revision: None,
            diagnostics: Vec::new(),
        });
    }
    let source = notes_of(project, clip_id)?;
    let mut notes = Vec::with_capacity(source.len() * (repeats as usize + 1));
    for repeat in 0..=u64::from(repeats) {
        let offset = repeat.saturating_mul(period_frames);
        notes.extend(source.iter().map(|note| ClipNote {
            start_frame: note.start_frame.saturating_add(offset),
            ..*note
        }));
    }
    Ok(replace(clip_id, notes))
}

/// The tempo map a transform quantises against. Exposed so a caller can show
/// the same grid the transform will use.
#[must_use]
pub fn tempo_map(project: &Project) -> TempoMap {
    project.tempo_map()
}
