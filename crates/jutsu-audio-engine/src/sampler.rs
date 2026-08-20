//! Playing notes from the project's own samples.
//!
//! A sampler is not an extension: what it plays is audio the project already
//! holds, so it renders here, where the loader that fetches assets lives. Given
//! the same zones and the same notes it produces the same samples, every run.

use jutsu_audio_model::{AssetId, ClipNote, SampleLoopMode, SamplerZone};

use crate::SourceAudio;
use crate::effects::{MixDiagnostic, MixDiagnosticCode};

/// One note being played by one zone.
struct Voice<'a> {
    zone: &'a SamplerZone,
    source: &'a SourceAudio,
    /// Where the read head is, in source frames.
    position: f64,
    /// How far it moves per project frame: pitch shift and rate conversion in
    /// one number.
    step: f64,
    velocity: f32,
    gain: f32,
    /// Frames until the note is released, and then until it is silent.
    remaining: u64,
    envelope: f32,
    releasing: bool,
}

/// Renders a sampler's notes into a mono buffer as long as the clip.
///
/// `max_voices` bounds how many notes sound at once: past the limit, a note is
/// dropped rather than an existing voice cut, because an offline render lays
/// each note down complete before the next one starts.
///
/// `load` fetches a zone's audio; a zone whose asset is missing is skipped with
/// a diagnostic rather than failing the mix, because losing one drum should not
/// lose the song.
#[allow(clippy::too_many_arguments)]
pub fn render(
    zones: &[SamplerZone],
    notes: &[ClipNote],
    frames: usize,
    sample_rate: u32,
    attack_ms: f64,
    release_ms: f64,
    max_voices: u32,
    entity_id: &str,
    load: &mut impl FnMut(AssetId) -> Result<SourceAudio, String>,
    diagnostics: &mut Vec<MixDiagnostic>,
) -> Vec<f32> {
    let mut output = vec![0.0_f32; frames];
    if zones.is_empty() || notes.is_empty() || frames == 0 {
        return output;
    }

    // Every zone's audio, fetched once. A zone whose asset cannot be read is
    // dropped here, so the render loop never has to think about it.
    let mut sources: Vec<(usize, SourceAudio)> = Vec::new();
    for (index, zone) in zones.iter().enumerate() {
        match load(zone.asset_id) {
            Ok(source) => sources.push((index, source)),
            Err(message) => diagnostics.push(MixDiagnostic {
                code: MixDiagnosticCode::EffectUnavailable,
                entity_id: entity_id.to_owned(),
                message: format!(
                    "sampler zone for asset {} cannot be read ({message}); it plays silence",
                    zone.asset_id
                ),
            }),
        }
    }
    if sources.is_empty() {
        return output;
    }

    let attack_frames = frames_of(attack_ms, sample_rate);
    let release_frames = frames_of(release_ms, sample_rate).max(1.0);
    let voice_limit = max_voices.max(1) as usize;

    // Notes are played one at a time into the shared buffer; the voice limit is
    // enforced by counting how many are sounding at each start.
    let mut starts: Vec<(usize, &ClipNote)> = notes.iter().enumerate().collect();
    starts.sort_by_key(|(_, note)| note.start_frame);

    let mut sounding: Vec<(u64, u64)> = Vec::new(); // (start, end) of live voices
    for (_, note) in starts {
        let Some(start) = usize::try_from(note.start_frame)
            .ok()
            .filter(|start| *start < frames)
        else {
            continue;
        };
        let end = note.start_frame.saturating_add(note.duration_frames);
        sounding.retain(|(_, voice_end)| *voice_end > note.start_frame);
        if sounding.len() >= voice_limit {
            // Notes are rendered one at a time into a finished buffer, so a
            // voice cannot be cut short after the fact. The limit therefore
            // keeps the notes that started first and drops the newcomer, which
            // is at least predictable — and identical every render.
            continue;
        }
        sounding.push((note.start_frame, end));

        let Some((index, source)) = sources
            .iter()
            .find(|(index, _)| zones[*index].covers(note.pitch_hz, note.velocity))
        else {
            continue;
        };
        let zone = &zones[*index];
        let mut voice = Voice {
            zone,
            source,
            position: 0.0,
            step: zone.playback_ratio(note.pitch_hz) * f64::from(source.sample_rate)
                / f64::from(sample_rate.max(1)),
            velocity: note.velocity.clamp(0.0, 1.0),
            gain: 10_f32.powf(zone.gain_db as f32 / 20.0),
            remaining: note.duration_frames,
            envelope: 0.0,
            releasing: false,
        };
        play(
            &mut output,
            start,
            &mut voice,
            attack_frames,
            release_frames,
        );
    }
    output
}

/// Renders one voice from `start` until it runs out of note, sample or buffer.
fn play(output: &mut [f32], start: usize, voice: &mut Voice, attack: f64, release: f64) {
    let channels = usize::from(voice.source.channels.max(1));
    let source_frames = voice.source.samples.len() / channels;
    if source_frames == 0 {
        return;
    }
    let attack_step = if attack < 1.0 {
        1.0
    } else {
        (1.0 / attack) as f32
    };
    let release_step = (1.0 / release) as f32;

    for slot in output.iter_mut().skip(start) {
        if voice.releasing && voice.envelope <= 0.0 {
            break;
        }
        if voice.remaining == 0 {
            voice.releasing = true;
        } else {
            voice.remaining -= 1;
        }

        let index = voice.position as usize;
        let index = match voice.zone.loop_mode {
            SampleLoopMode::OneShot => {
                if index >= source_frames {
                    break;
                }
                index
            }
            SampleLoopMode::Loop {
                start_frame,
                end_frame,
            } => {
                // A loop that is empty or backwards is treated as one-shot
                // rather than refused: the note still sounds.
                let start_frame = usize::try_from(start_frame).unwrap_or(0).min(source_frames);
                let end_frame = usize::try_from(end_frame)
                    .unwrap_or(source_frames)
                    .min(source_frames);
                if end_frame <= start_frame {
                    if index >= source_frames {
                        break;
                    }
                    index
                } else if index >= end_frame {
                    let span = end_frame - start_frame;
                    let wrapped = start_frame + (index - start_frame) % span;
                    voice.position = wrapped as f64 + voice.position.fract();
                    wrapped
                } else {
                    index
                }
            }
        };

        // Mono sum of whatever the source holds: a sampler voice is one signal,
        // and the strip decides where it sits.
        let base = index * channels;
        let mut sample = 0.0;
        for channel in 0..channels {
            sample += voice.source.samples[base + channel];
        }
        sample /= channels as f32;

        voice.envelope = if voice.releasing {
            (voice.envelope - release_step).max(0.0)
        } else {
            (voice.envelope + attack_step).min(1.0)
        };
        *slot += sample * voice.envelope * voice.velocity * voice.gain;
        voice.position += voice.step;
    }
}

fn frames_of(milliseconds: f64, sample_rate: u32) -> f64 {
    (milliseconds.max(0.0) / 1_000.0) * f64::from(sample_rate)
}
