//! The one place a project becomes audio.
//!
//! Playback, preview and offline export all call [`mix_project`], so they
//! cannot drift apart: same project, same sources, same samples. Nothing here
//! reads the disk — the caller supplies decoded sources, which is what lets the
//! editor keep a decode cache and the CLI stay stateless.
//!
//! Summing order is the project's own order — tracks, then layers, then clips —
//! so a mix is reproducible from the file alone.

use std::sync::Arc;

use jutsu_audio_extensions::{ExtensionRegistries, ExtensionTypeId, NoteEvent};
use jutsu_audio_model::{AssetId, AudioAssetSource, Clip, ParameterValue, Project, Track};

use crate::{PlaybackSnapshot, SnapshotError};

/// Everything mixes to stereo for now; the mixer phase introduces real bus
/// channel counts.
pub const MIX_CHANNELS: u16 = 2;

/// Track parameter read as "do not play this track".
pub const MUTE_KEY: &str = "mute";
/// Track parameter read as "play only tracks with this set".
pub const SOLO_KEY: &str = "solo";
/// Clip parameter read as gain in decibels.
pub const GAIN_DB_KEY: &str = "gain_db";
/// Clip parameter read as stereo position, `-1.0` hard left to `1.0` hard right.
pub const PAN_KEY: &str = "pan";
/// Clip parameter read as the fade-in length in project frames.
pub const FADE_IN_KEY: &str = "fade_in_samples";
/// Clip parameter read as the fade-out length in project frames.
pub const FADE_OUT_KEY: &str = "fade_out_samples";

/// One decoded source, interleaved, at whatever rate it was stored in.
#[derive(Clone, Debug)]
pub struct SourceAudio {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Arc<[f32]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixErrorCode {
    /// A clip names an asset the loader could not provide.
    SourceUnavailable,
    /// A synth or generator clip names an extension that is not registered, or
    /// parameters the extension refuses.
    SynthUnavailable,
    /// The timeline is longer than this machine can hold in memory.
    TooLong,
    /// The mixed buffer is not a valid playback snapshot.
    InvalidAudioFormat,
}

#[derive(Clone, Debug)]
pub struct MixError {
    pub code: MixErrorCode,
    pub message: String,
}

impl MixError {
    fn new(code: MixErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<SnapshotError> for MixError {
    fn from(error: SnapshotError) -> Self {
        Self::new(MixErrorCode::InvalidAudioFormat, error.message)
    }
}

/// Sums a project into one interleaved stereo snapshot at `sample_rate`.
///
/// `load` is called once per sample clip and may cache; it returns the decoded
/// source for an asset. Synth clips do not go through it: they are rendered
/// here from their notes, through `extensions`. `Ok(None)` means there is
/// nothing audible — an empty timeline, or every audible track muted — which is
/// not an error.
pub fn mix_project(
    project: &Project,
    sample_rate: u32,
    extensions: &ExtensionRegistries,
    mut load: impl FnMut(AssetId) -> Result<SourceAudio, String>,
) -> Result<Option<PlaybackSnapshot>, MixError> {
    let audible = audible_tracks(project);
    let clips: Vec<&Clip> = audible
        .iter()
        .flat_map(|track| &track.layers)
        .flat_map(|layer| &layer.clips)
        .collect();

    let total_frames = clips
        .iter()
        .map(|clip| clip.start_sample.saturating_add(clip.duration_samples))
        .max()
        .unwrap_or(0);
    if total_frames == 0 {
        return Ok(None);
    }
    let total_frames = usize::try_from(total_frames)
        .ok()
        .and_then(|frames| frames.checked_mul(usize::from(MIX_CHANNELS)))
        .ok_or_else(|| {
            MixError::new(
                MixErrorCode::TooLong,
                "the timeline is longer than this machine can render",
            )
        })?
        / usize::from(MIX_CHANNELS);

    let mut mix = vec![0.0_f32; total_frames * usize::from(MIX_CHANNELS)];
    for clip in clips {
        match rendered_source(project, clip) {
            Some(source) => {
                let source = render_extension_clip(extensions, source, clip, sample_rate)?;
                // The rendered buffer *is* the clip, so it is read from its
                // start rather than from the clip's source offset.
                let mut placed = clip.clone();
                placed.source_start_sample = 0;
                render_clip(&mut mix, total_frames, &placed, &source, sample_rate);
            }
            None => {
                let source = load(clip.asset_id).map_err(|message| {
                    MixError::new(
                        MixErrorCode::SourceUnavailable,
                        format!("clip {} cannot be rendered: {message}", clip.id),
                    )
                })?;
                render_clip(&mut mix, total_frames, clip, &source, sample_rate);
            }
        }
    }

    PlaybackSnapshot::new(sample_rate, MIX_CHANNELS, Arc::from(mix))
        .map(Some)
        .map_err(MixError::from)
}

/// The asset source a clip renders from an extension rather than a file: a
/// synth played by the clip's notes, or a generator run from its seed.
fn rendered_source<'a>(project: &'a Project, clip: &Clip) -> Option<&'a AudioAssetSource> {
    let asset = project
        .assets
        .iter()
        .find(|asset| asset.id == clip.asset_id)?;
    matches!(
        asset.source,
        AudioAssetSource::Synth { .. } | AudioAssetSource::Generated { .. }
    )
    .then_some(&asset.source)
}

/// Renders one clip from an extension into a mono buffer as long as the clip.
///
/// Every clip gets a fresh instance, reset before it plays: a mix is the same
/// however many times it is rendered, and in whatever order.
///
/// ponytail: a generator is re-run on every mix rather than cached. Fine for
/// one-shots; cache by recipe identity if long ambiences make a re-mix drag.
fn render_extension_clip(
    extensions: &ExtensionRegistries,
    source: &AudioAssetSource,
    clip: &Clip,
    sample_rate: u32,
) -> Result<SourceAudio, MixError> {
    let frames = usize::try_from(clip.duration_samples).map_err(|_| {
        MixError::new(
            MixErrorCode::TooLong,
            format!("clip {} is longer than this machine can render", clip.id),
        )
    })?;
    let (type_id, parameters) = match source {
        AudioAssetSource::Synth {
            type_id,
            parameters,
            ..
        } => (type_id, parameters),
        AudioAssetSource::Generated {
            generator_type,
            seed,
            parameters,
            ..
        } => {
            return render_generated_clip(
                extensions,
                generator_type,
                *seed,
                parameters,
                frames,
                sample_rate,
                clip,
            );
        }
        _ => unreachable!("only called for rendered assets"),
    };
    let type_id = ExtensionTypeId::new(type_id.clone()).map_err(|error| {
        MixError::new(
            MixErrorCode::SynthUnavailable,
            format!(
                "clip {} names an invalid synth type: {}",
                clip.id, error.message
            ),
        )
    })?;
    let mut synth = extensions
        .instantiate_synth(&type_id, parameters)
        .map_err(|error| {
            MixError::new(
                MixErrorCode::SynthUnavailable,
                format!("clip {} cannot be played: {}", clip.id, error.message),
            )
        })?;
    synth.prepare(sample_rate);
    synth.reset();

    let mut events = Vec::with_capacity(clip.notes.len() * 2);
    for note in &clip.notes {
        let start = usize::try_from(note.start_frame).unwrap_or(usize::MAX);
        events.push(NoteEvent::note_on(start, note.pitch_hz, note.velocity));
        events.push(NoteEvent::note_off(
            start.saturating_add(usize::try_from(note.duration_frames).unwrap_or(usize::MAX)),
            note.pitch_hz,
        ));
    }
    // Rendering applies events in order, so they have to be in order.
    events.sort_by_key(|event| event.frame_offset);

    let mut samples = vec![0.0_f32; frames];
    synth.render(&events, &mut samples);
    Ok(SourceAudio {
        sample_rate,
        channels: 1,
        samples: Arc::from(samples),
    })
}

/// Runs a generator for the length of the clip. The seed and parameters come
/// from the asset's provenance, so the same project always renders the same
/// audio without storing the samples.
#[allow(clippy::too_many_arguments)]
fn render_generated_clip(
    extensions: &ExtensionRegistries,
    generator_type: &str,
    seed: u64,
    parameters: &std::collections::BTreeMap<String, ParameterValue>,
    frames: usize,
    sample_rate: u32,
    clip: &Clip,
) -> Result<SourceAudio, MixError> {
    let type_id = ExtensionTypeId::new(generator_type).map_err(|error| {
        MixError::new(
            MixErrorCode::SynthUnavailable,
            format!(
                "clip {} names an invalid generator type: {}",
                clip.id, error.message
            ),
        )
    })?;
    let generator = extensions
        .instantiate_generator(&type_id, parameters)
        .map_err(|error| {
            MixError::new(
                MixErrorCode::SynthUnavailable,
                format!("clip {} cannot be generated: {}", clip.id, error.message),
            )
        })?;
    let samples = generator.generate_mono(seed, frames);
    Ok(SourceAudio {
        sample_rate,
        channels: 1,
        samples: Arc::from(samples),
    })
}

/// The tracks that should be heard: solo wins over mute, and with nothing
/// soloed every unmuted track plays.
fn audible_tracks(project: &Project) -> Vec<&Track> {
    let any_solo = project.tracks.iter().any(|track| flag(track, SOLO_KEY));
    project
        .tracks
        .iter()
        .filter(|track| {
            if any_solo {
                flag(track, SOLO_KEY)
            } else {
                !flag(track, MUTE_KEY)
            }
        })
        .collect()
}

fn flag(track: &Track, key: &str) -> bool {
    matches!(track.parameters.get(key), Some(ParameterValue::Bool(true)))
}

/// Reads a clip's gain in decibels, defaulting to unity.
#[must_use]
pub fn clip_gain_db(clip: &Clip) -> f64 {
    match clip.parameters.get(GAIN_DB_KEY) {
        Some(ParameterValue::Float(value)) => *value,
        _ => 0.0,
    }
}

/// Reads a clip's pan, clamped to the legal range and defaulting to centre.
#[must_use]
pub fn clip_pan(clip: &Clip) -> f64 {
    match clip.parameters.get(PAN_KEY) {
        Some(ParameterValue::Float(value)) => value.clamp(-1.0, 1.0),
        _ => 0.0,
    }
}

/// Reads a fade length in project frames, capped at the clip so a fade can
/// never run past the material it shapes.
#[must_use]
pub fn clip_fade(clip: &Clip, key: &str) -> u64 {
    let frames = match clip.parameters.get(key) {
        Some(ParameterValue::Integer(value)) => u64::try_from(*value).unwrap_or(0),
        Some(ParameterValue::Float(value)) if *value >= 0.0 => *value as u64,
        _ => 0,
    };
    frames.min(clip.duration_samples)
}

/// The fade envelope at `offset` frames into a clip: linear in, linear out,
/// unity in between. Fades that overlap simply multiply, which tapers a very
/// short clip rather than misbehaving.
fn fade_envelope(offset: u64, duration: u64, fade_in: u64, fade_out: u64) -> f32 {
    let mut gain = 1.0_f32;
    if fade_in > 0 && offset < fade_in {
        gain *= offset as f32 / fade_in as f32;
    }
    if fade_out > 0 {
        let remaining = duration.saturating_sub(offset);
        if remaining <= fade_out {
            gain *= remaining as f32 / fade_out as f32;
        }
    }
    gain
}

/// Square-root pan law, normalised so a centred clip is unity in both
/// channels — the behaviour a project with no pan at all has always had.
/// Hard panning therefore reaches +3 dB in the live channel rather than
/// dropping the centre by 3 dB, which would quietly re-level every project.
fn pan_gains(pan: f64) -> (f32, f32) {
    let pan = pan.clamp(-1.0, 1.0);
    ((1.0 - pan).sqrt() as f32, (1.0 + pan).sqrt() as f32)
}

fn render_clip(
    mix: &mut [f32],
    total_frames: usize,
    clip: &Clip,
    source: &SourceAudio,
    sample_rate: u32,
) {
    let source_channels = usize::from(source.channels);
    if source_channels == 0 || source.samples.is_empty() {
        return;
    }
    let source_frames = source.samples.len() / source_channels;
    // How far the read head moves through the source per project frame.
    let step = f64::from(source.sample_rate) / f64::from(sample_rate.max(1));
    let gain = 10_f32.powf(clip_gain_db(clip) as f32 / 20.0);
    let (left, right) = pan_gains(clip_pan(clip));
    let channel_gain = [gain * left, gain * right];
    let fade_in = clip_fade(clip, FADE_IN_KEY);
    let fade_out = clip_fade(clip, FADE_OUT_KEY);

    for offset in 0..clip.duration_samples {
        let Ok(destination) = usize::try_from(clip.start_sample.saturating_add(offset)) else {
            break;
        };
        if destination >= total_frames {
            break;
        }
        let read = clip.source_start_sample as f64 + offset as f64 * step;
        if read < 0.0 {
            continue;
        }
        let index = read.floor() as usize;
        if index >= source_frames {
            break;
        }
        // Linear interpolation between neighbouring source frames; the last
        // frame holds itself so the tail does not read past the buffer.
        let next = (index + 1).min(source_frames - 1);
        let blend = (read - read.floor()) as f32;
        let base = index * source_channels;
        let next_base = next * source_channels;

        let envelope = fade_envelope(offset, clip.duration_samples, fade_in, fade_out);
        for channel in 0..usize::from(MIX_CHANNELS) {
            let source_channel = channel % source_channels;
            let current = source.samples[base + source_channel];
            let upcoming = source.samples[next_base + source_channel];
            mix[destination * usize::from(MIX_CHANNELS) + channel] +=
                (current + (upcoming - current) * blend) * channel_gain[channel] * envelope;
        }
    }
}
