//! Running an effect chain over a strip.
//!
//! An insert that cannot be instantiated — the extension is gone, or its state
//! version has moved on — does not fail the mix. It passes its audio through
//! and reports why, because losing an afternoon's work to a missing plug-in is
//! worse than hearing the mix without it.

use std::collections::BTreeMap;

use jutsu_audio_extensions::{ExtensionRegistries, ExtensionTypeId};
use jutsu_audio_model::{
    AssetId, AutomationLane, AutomationTarget, EffectInsert, ParameterValue, TempoMap, TrackId,
};

use jutsu_audio_extensions::effects::convolution::IMPULSE_PARAMETER;

use crate::MIX_CHANNELS;

/// How many frames a chain renders between parameter updates.
///
/// Small enough that a sweep is heard as a sweep — 1024 frames is 21
/// milliseconds at 48 kHz, well under the ear's resolution for a filter move —
/// and large enough that the per-block work stays in the noise. A per-frame
/// update would be smoother in theory and would cost a coefficient
/// recalculation forty-eight thousand times a second in practice.
const AUTOMATION_BLOCK: usize = 1_024;

/// What a chain needs to know about time: which lanes write to the inserts in
/// it, where the buffer starts on the project timeline, and the tempo, which
/// some effects ask for by name.
pub struct ChainContext<'a> {
    pub lanes: &'a [&'a AutomationLane],
    pub tempo: &'a TempoMap,
    pub start_frame: u64,
    /// The rendered audio of every track something listens to, interleaved the
    /// same way the buffer being processed is. Only tracks named as a key are
    /// in here: rendering the rest twice would be work nobody asked for.
    pub keys: &'a BTreeMap<TrackId, Vec<f32>>,
    /// The impulse responses inserts asked for, mono, at the render's rate.
    ///
    /// Extensions do not read files, so anything that needs audio to work with
    /// gets it from here — loaded once by the mix, however many inserts and
    /// channels end up sharing it.
    pub impulses: &'a BTreeMap<AssetId, Vec<f32>>,
}

impl ChainContext<'_> {
    /// The lanes writing to one insert. Usually none, which is the case worth
    /// keeping cheap.
    fn lanes_for(&self, insert: &EffectInsert) -> Vec<&AutomationLane> {
        self.lanes
            .iter()
            .copied()
            .filter(|lane| match lane.target {
                AutomationTarget::Effect { effect_id } => effect_id == insert.id,
                _ => false,
            })
            .collect()
    }
}

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
    context: &ChainContext<'_>,
    diagnostics: &mut Vec<MixDiagnostic>,
) -> ChainTiming {
    let mut timing = ChainTiming::default();
    let channels = usize::from(MIX_CHANNELS);
    let frames = buffer.len() / channels.max(1);
    if frames == 0 {
        return timing;
    }

    // Scratch buffers reused across the whole chain: one channel in, one
    // channel out, so a wet/dry blend has something to blend against. Sized for
    // a block rather than the render, because the chain now walks the buffer.
    let mut dry = vec![0.0_f32; AUTOMATION_BLOCK];
    let mut wet = vec![0.0_f32; AUTOMATION_BLOCK];
    let mut key = vec![0.0_f32; AUTOMATION_BLOCK];

    for insert in inserts {
        if !insert.enabled {
            continue;
        }
        let Some(mut instances) = instantiate(insert, extensions, sample_rate, diagnostics) else {
            continue;
        };
        // An insert naming an impulse gets it before it renders anything. One
        // that names one the project has lost simply renders without: an effect
        // whose asset has gone should not silence the strip it is on.
        if let Some(samples) = impulse_for(insert, context) {
            for instance in &mut instances {
                instance.set_impulse(samples, sample_rate);
            }
        }
        timing.latency_frames = timing
            .latency_frames
            .saturating_add(instances[0].latency_frames());
        timing.tail_frames = timing.tail_frames.max(instances[0].tail_frames());

        let lanes = context.lanes_for(insert);
        let mix = insert.wet.clamp(0.0, 1.0) as f32;
        // A key that names a track this mix does not have is silently absent
        // rather than an error: the diagnostic belongs to validation, and a
        // mix that refused to render over it would be the worse answer.
        let keyed = insert
            .sidechain
            .and_then(|track_id| context.keys.get(&track_id));

        for (channel, instance) in instances.iter_mut().enumerate() {
            // An effect that wants stereo width needs to know which side it is
            // on; one that does not ignores this.
            instance.set_channel(u16::try_from(channel).unwrap_or(0));

            let mut start = 0;
            while start < frames {
                let length = AUTOMATION_BLOCK.min(frames - start);
                let frame = context.start_frame.saturating_add(start as u64);

                // Tempo first, so an effect that syncs to it sees the tempo in
                // force for the block it is about to render rather than the one
                // it was built with.
                instance.set_parameter("tempo_bpm", context.tempo.at(frame).beats_per_minute);
                for lane in &lanes {
                    if let Some(value) = lane.value_at(frame) {
                        instance.set_parameter(&lane.parameter, value);
                    }
                }

                let dry = &mut dry[..length];
                let wet = &mut wet[..length];
                for (offset, slot) in dry.iter_mut().enumerate() {
                    *slot = buffer[(start + offset) * channels + channel];
                }
                wet.copy_from_slice(dry);
                match keyed {
                    Some(source) => {
                        let key = &mut key[..length];
                        for (offset, slot) in key.iter_mut().enumerate() {
                            let index = (start + offset) * channels + channel;
                            *slot = source.get(index).copied().unwrap_or(0.0);
                        }
                        instance.process_with_key(wet, key);
                    }
                    None => instance.process(wet),
                }
                for offset in 0..length {
                    buffer[(start + offset) * channels + channel] =
                        dry[offset] * (1.0 - mix) + wet[offset] * mix;
                }
                start += length;
            }
        }
    }
    timing
}

/// The impulse an insert asked for, if the mix loaded one.
fn impulse_for<'a>(insert: &EffectInsert, context: &'a ChainContext<'_>) -> Option<&'a [f32]> {
    let ParameterValue::Text(named) = insert.parameters.get(IMPULSE_PARAMETER)? else {
        return None;
    };
    let asset_id: AssetId = named.parse().ok()?;
    context.impulses.get(&asset_id).map(Vec::as_slice)
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
