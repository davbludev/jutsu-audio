//! Running an effect chain over a strip.
//!
//! An insert that cannot be instantiated — the extension is gone, or its state
//! version has moved on — does not fail the mix. It passes its audio through
//! and reports why, because losing an afternoon's work to a missing plug-in is
//! worse than hearing the mix without it.

use jutsu_audio_extensions::{ExtensionRegistries, ExtensionTypeId};
use jutsu_audio_model::EffectInsert;

use crate::MIX_CHANNELS;

/// Why an insert is not doing what the project says it should.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixDiagnosticCode {
    /// The extension is not registered in this build.
    EffectUnavailable,
    /// The extension exists, but the project's state version is not the one it
    /// declares. The insert is played anyway, and this says so.
    EffectVersionMismatch,
    /// The extension refused the stored parameters.
    EffectParametersRejected,
    /// A clip's audio could not be read or decoded. The clip plays silence and
    /// everything else carries on.
    SourceUnreadable,
    /// A clip names a synth, sampler or generator this build cannot provide.
    /// Same treatment: that clip is silent, the rest of the mix plays.
    ExtensionUnavailable,
}

/// One thing the mix could not do as asked, and what it did instead.
#[derive(Clone, Debug, PartialEq)]
pub struct MixDiagnostic {
    pub code: MixDiagnosticCode,
    /// The insert this is about.
    pub entity_id: String,
    pub message: String,
}

/// What a chain does to time. Reported rather than compensated: an offline
/// render lays everything on one timeline, and a caller that needs to align
/// against live playback needs the numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ChainTiming {
    pub latency_frames: u32,
    pub tail_frames: u32,
}

/// Runs every enabled insert over an interleaved stereo buffer.
///
/// Each channel gets its own instance of each effect, so a filter's state
/// cannot leak from left to right.
pub fn apply_chain(
    buffer: &mut [f32],
    inserts: &[EffectInsert],
    extensions: &ExtensionRegistries,
    sample_rate: u32,
    diagnostics: &mut Vec<MixDiagnostic>,
) -> ChainTiming {
    let mut timing = ChainTiming::default();
    let channels = usize::from(MIX_CHANNELS);
    let frames = buffer.len() / channels.max(1);
    if frames == 0 {
        return timing;
    }

    // Scratch buffers reused across the whole chain: one channel in, one
    // channel out, so a wet/dry blend has something to blend against.
    let mut dry = vec![0.0_f32; frames];
    let mut wet = vec![0.0_f32; frames];

    for insert in inserts {
        if !insert.enabled {
            continue;
        }
        let Some(mut instances) = instantiate(insert, extensions, sample_rate, diagnostics) else {
            continue;
        };
        timing.latency_frames = timing
            .latency_frames
            .saturating_add(instances[0].latency_frames());
        timing.tail_frames = timing.tail_frames.max(instances[0].tail_frames());

        let mix = insert.wet.clamp(0.0, 1.0) as f32;
        for (channel, instance) in instances.iter_mut().enumerate() {
            for frame in 0..frames {
                dry[frame] = buffer[frame * channels + channel];
            }
            wet.copy_from_slice(&dry);
            instance.process(&mut wet);
            for frame in 0..frames {
                buffer[frame * channels + channel] = dry[frame] * (1.0 - mix) + wet[frame] * mix;
            }
        }
    }
    timing
}

/// One prepared instance per channel, or `None` with a diagnostic saying why.
fn instantiate(
    insert: &EffectInsert,
    extensions: &ExtensionRegistries,
    sample_rate: u32,
    diagnostics: &mut Vec<MixDiagnostic>,
) -> Option<Vec<Box<dyn jutsu_audio_extensions::Effect>>> {
    let Ok(type_id) = ExtensionTypeId::new(insert.type_id.clone()) else {
        diagnostics.push(MixDiagnostic {
            code: MixDiagnosticCode::EffectUnavailable,
            entity_id: insert.id.to_string(),
            message: format!("'{}' is not a valid extension ID", insert.type_id),
        });
        return None;
    };
    let Some(descriptor) = extensions.effect_descriptor(&type_id) else {
        diagnostics.push(MixDiagnostic {
            code: MixDiagnosticCode::EffectUnavailable,
            entity_id: insert.id.to_string(),
            message: format!(
                "effect '{}' is not registered in this build; its audio passes through unchanged",
                insert.type_id
            ),
        });
        return None;
    };
    if descriptor.state_version != insert.state_version {
        // Played anyway: an older state is usually still meaningful, and
        // refusing to play it would lose the mix rather than a nuance.
        diagnostics.push(MixDiagnostic {
            code: MixDiagnosticCode::EffectVersionMismatch,
            entity_id: insert.id.to_string(),
            message: format!(
                "effect '{}' was saved at state version {} and this build has {}; playing it as saved",
                insert.type_id, insert.state_version, descriptor.state_version
            ),
        });
    }

    let mut instances = Vec::with_capacity(usize::from(MIX_CHANNELS));
    for _ in 0..MIX_CHANNELS {
        match extensions.instantiate_effect(&type_id, &insert.parameters) {
            Ok(mut instance) => {
                instance.prepare(sample_rate);
                instance.reset();
                instances.push(instance);
            }
            Err(error) => {
                diagnostics.push(MixDiagnostic {
                    code: MixDiagnosticCode::EffectParametersRejected,
                    entity_id: insert.id.to_string(),
                    message: format!(
                        "effect '{}' refused its stored parameters ({}); its audio passes through unchanged",
                        insert.type_id, error.message
                    ),
                });
                return None;
            }
        }
    }
    Some(instances)
}
