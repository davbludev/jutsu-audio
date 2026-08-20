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
use std::collections::{BTreeMap, BTreeSet};

use jutsu_audio_model::{
    AssetId, AudioAssetSource, AutomationLane, AutomationTarget, BusId, Clip, ParameterValue,
    Project, Track, TrackId,
};

use crate::effects::{ChainContext, ChainTiming, MixDiagnostic, MixDiagnosticCode, apply_chain};
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

/// Track and bus parameter read as level in decibels. Clips use the same key,
/// so one name means one thing everywhere.
pub const TRACK_GAIN_KEY: &str = GAIN_DB_KEY;
/// Track and bus parameter read as stereo position.
pub const TRACK_PAN_KEY: &str = PAN_KEY;

/// One decoded source, interleaved, at whatever rate it was stored in.
#[derive(Clone, Debug)]
pub struct SourceAudio {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Arc<[f32]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixErrorCode {
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

/// What the mix was, level by level: the loudest sample each track and bus
/// contributed. Static rather than live — it describes the rendered mix, which
/// is what a level check wants; the transport reports what is playing now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meters {
    pub tracks: BTreeMap<TrackId, f32>,
    pub buses: BTreeMap<BusId, f32>,
    pub master: f32,
}

/// A rendered mix and what it measured on the way through.
#[derive(Clone, Debug)]
pub struct MixOutput {
    /// `None` when nothing was audible, which is not a failure.
    pub snapshot: Option<PlaybackSnapshot>,
    pub meters: Meters,
    /// What the mix could not do as asked — a missing effect, a version that
    /// moved on — having done the next best thing.
    pub diagnostics: Vec<MixDiagnostic>,
    /// What the effect chains do to time, summed across the whole mix. Nothing
    /// is compensated here: an offline render lays everything on one timeline,
    /// and a caller aligning against live playback needs the numbers.
    pub timing: ChainTiming,
    /// One rendered buffer per track, in project order, when stems were asked
    /// for. Each is that track after its own inserts and its own fader, and
    /// before any bus touches it — which is what a game engine or another DAW
    /// expects a stem to be.
    pub stems: Vec<Stem>,
}

/// One track, rendered on its own.
#[derive(Clone, Debug)]
pub struct Stem {
    pub track_id: TrackId,
    pub name: String,
    /// Interleaved, the same rate and channel count as the master render.
    pub samples: Vec<f32>,
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
    load: impl FnMut(AssetId) -> Result<SourceAudio, String>,
) -> Result<Option<PlaybackSnapshot>, MixError> {
    mix_project_metered(project, sample_rate, extensions, load).map(|output| output.snapshot)
}

/// The same mix, with the levels each track and bus contributed.
pub fn mix_project_metered(
    project: &Project,
    sample_rate: u32,
    extensions: &ExtensionRegistries,
    load: impl FnMut(AssetId) -> Result<SourceAudio, String>,
) -> Result<MixOutput, MixError> {
    mix_inner(project, sample_rate, extensions, load, false)
}

/// The same mix again, keeping each track's own render.
///
/// One pass, not two: a stem has to be the same audio the master was built
/// from, and rendering twice would be two chances to disagree.
pub fn mix_project_stems(
    project: &Project,
    sample_rate: u32,
    extensions: &ExtensionRegistries,
    load: impl FnMut(AssetId) -> Result<SourceAudio, String>,
) -> Result<MixOutput, MixError> {
    mix_inner(project, sample_rate, extensions, load, true)
}

fn mix_inner(
    project: &Project,
    sample_rate: u32,
    extensions: &ExtensionRegistries,
    mut load: impl FnMut(AssetId) -> Result<SourceAudio, String>,
    collect_stems: bool,
) -> Result<MixOutput, MixError> {
    let audible = audible_tracks(project);
    let total_frames = timeline_frames(&audible)?;
    if total_frames == 0 {
        return Ok(MixOutput {
            snapshot: None,
            stems: Vec::new(),
            meters: Meters::default(),
            diagnostics: Vec::new(),
            timing: ChainTiming::default(),
        });
    }
    let samples = total_frames * usize::from(MIX_CHANNELS);

    // One buffer per bus, plus a scratch buffer per track. Routing is what
    // decides where a track lands, so the sum is built bus by bus rather than
    // straight into one master buffer.
    let mut bus_buffers: BTreeMap<BusId, Vec<f32>> = project
        .buses
        .iter()
        .map(|bus| (bus.id, vec![0.0_f32; samples]))
        .collect();
    let mut meters = Meters::default();
    let mut diagnostics = Vec::new();
    // Everything the chains need to follow the timeline: the lanes that write
    // to inserts, and the tempo some effects sync to. Collected once — the same
    // set applies to every strip, and filtering per insert is cheaper than
    // walking the project again for each.
    let tempo = project.tempo_map();
    let effect_lanes: Vec<&jutsu_audio_model::AutomationLane> = project
        .automation
        .iter()
        .filter(|lane| matches!(lane.target, AutomationTarget::Effect { .. }))
        .collect();
    // Anything named as a sidechain key is rendered first and kept, so the
    // strip that ducks under it has something to duck under. Ordinary tracks
    // are not kept: one buffer at a time is the whole point of the loop below.
    let keys: BTreeSet<TrackId> = project
        .tracks
        .iter()
        .flat_map(|track| &track.effects)
        .chain(project.buses.iter().flat_map(|bus| &bus.effects))
        .filter_map(|insert| insert.sidechain)
        .collect();
    let mut key_buffers: BTreeMap<TrackId, Vec<f32>> = BTreeMap::new();
    for track in audible.iter().filter(|track| keys.contains(&track.id)) {
        let mut buffer = vec![0.0_f32; samples];
        for clip in track.layers.iter().flat_map(|layer| &layer.clips) {
            render_one_clip(
                &mut buffer,
                total_frames,
                project,
                clip,
                extensions,
                sample_rate,
                &mut load,
                &mut diagnostics,
            )?;
        }
        // The key is what the track sounds like, fader and all: ducking under
        // a kick that has been turned down should duck less.
        apply_strip(
            &mut buffer,
            &track.parameters,
            &lanes_for(project, AutomationTarget::Track { track_id: track.id }),
        );
        key_buffers.insert(track.id, buffer);
    }

    // Impulse responses, loaded once each however many inserts want them, and
    // folded to mono: a convolver works on one channel at a time.
    let mut impulses: BTreeMap<AssetId, Vec<f32>> = BTreeMap::new();
    for insert in project
        .tracks
        .iter()
        .flat_map(|track| &track.effects)
        .chain(project.buses.iter().flat_map(|bus| &bus.effects))
    {
        let Some(jutsu_audio_model::ParameterValue::Text(named)) = insert
            .parameters
            .get(jutsu_audio_extensions::effects::convolution::IMPULSE_PARAMETER)
        else {
            continue;
        };
        let Ok(asset_id) = named.parse::<AssetId>() else {
            continue;
        };
        if impulses.contains_key(&asset_id) {
            continue;
        }
        let Ok(source) = load(asset_id) else {
            diagnostics.push(MixDiagnostic {
                code: MixDiagnosticCode::SourceUnreadable,
                entity_id: insert.id.to_string(),
                message: format!(
                    "impulse asset {asset_id} could not be read; the convolver passes its audio through"
                ),
            });
            continue;
        };
        let channels = usize::from(source.channels.max(1));
        let mono: Vec<f32> = source
            .samples
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect();
        // Resampling is the effect's business: it knows what rate it is
        // rendering at, and the impulse carries its own.
        let _ = source.sample_rate;
        impulses.insert(asset_id, mono);
    }

    let chain_context = ChainContext {
        lanes: &effect_lanes,
        tempo: &tempo,
        start_frame: 0,
        keys: &key_buffers,
        impulses: &impulses,
    };
    let mut track_buffer = vec![0.0_f32; samples];
    let mut master = None;
    let mut timing = ChainTiming::default();
    let mut stems = Vec::new();

    for track in &audible {
        track_buffer.fill(0.0);
        for clip in track.layers.iter().flat_map(|layer| &layer.clips) {
            render_one_clip(
                &mut track_buffer,
                total_frames,
                project,
                clip,
                extensions,
                sample_rate,
                &mut load,
                &mut diagnostics,
            )?;
        }
        // Inserts first, then the strip: a fader move should change how loud
        // the processed signal is, not how hard it hits the processing.
        let track_timing = apply_chain(
            &mut track_buffer,
            &track.effects,
            extensions,
            sample_rate,
            &chain_context,
            &mut diagnostics,
        );
        timing = combine(timing, track_timing);

        // A pre-fader send copies the signal as the effects left it, before the
        // fader has had a say. Only kept when something asks for one.
        let pre_fader = track
            .sends
            .iter()
            .any(|send| send.pre_fader)
            .then(|| track_buffer.clone());

        apply_strip(
            &mut track_buffer,
            &track.parameters,
            &lanes_for(project, AutomationTarget::Track { track_id: track.id }),
        );
        meters.tracks.insert(track.id, peak_of(&track_buffer));

        // Sends before the output, though the order does not matter: a send
        // adds a copy to its destination rather than diverting anything from
        // here. Every track is summed before any bus folds, so a send can name
        // any bus without the fold order having to know about it.
        for send in &track.sends {
            let source = if send.pre_fader {
                pre_fader.as_deref().unwrap_or(&track_buffer)
            } else {
                &track_buffer
            };
            let gain = 10_f32.powf(send.gain_db as f32 / 20.0);
            if let Some(bus) = bus_buffers.get_mut(&send.bus_id) {
                for (destination, sample) in bus.iter_mut().zip(source) {
                    *destination += sample * gain;
                }
            }
        }

        if collect_stems {
            stems.push(Stem {
                track_id: track.id,
                name: track.name.clone(),
                samples: track_buffer.clone(),
            });
        }

        if let Some(bus) = bus_buffers.get_mut(&track.output_bus_id) {
            add_into(bus, &track_buffer);
        }
    }

    // Buses fold outward from the leaves: a bus is only summed into its output
    // once everything feeding it has been.
    for bus_id in fold_order(project) {
        let Some(mut buffer) = bus_buffers.remove(&bus_id) else {
            continue;
        };
        let parameters = project
            .buses
            .iter()
            .find(|bus| bus.id == bus_id)
            .map(|bus| &bus.parameters);
        if let Some(effects) = project
            .buses
            .iter()
            .find(|bus| bus.id == bus_id)
            .map(|bus| &bus.effects)
        {
            let bus_timing = apply_chain(
                &mut buffer,
                effects,
                extensions,
                sample_rate,
                &chain_context,
                &mut diagnostics,
            );
            timing = combine(timing, bus_timing);
        }
        if let Some(parameters) = parameters {
            apply_strip(
                &mut buffer,
                parameters,
                &lanes_for(project, AutomationTarget::Bus { bus_id }),
            );
        }
        meters.buses.insert(bus_id, peak_of(&buffer));

        let output = project
            .buses
            .iter()
            .find(|bus| bus.id == bus_id)
            .and_then(|bus| bus.output_bus_id);
        match output.and_then(|output| bus_buffers.get_mut(&output)) {
            Some(target) => add_into(target, &buffer),
            None if bus_id == project.master_bus_id => {
                // What the master holds is the mix. Every other bus is still
                // folded and metered, so a mis-routed one can be seen.
                meters.master = peak_of(&buffer);
                master = Some(buffer);
            }
            // A bus routed nowhere: metered, and heard by nobody.
            None => {}
        }
    }

    let Some(master) = master else {
        // No master bus in the project. Validation will already have said so.
        return Ok(MixOutput {
            snapshot: None,
            stems,
            meters,
            diagnostics,
            timing,
        });
    };
    let snapshot = PlaybackSnapshot::new(sample_rate, MIX_CHANNELS, Arc::from(master))
        .map_err(MixError::from)?;
    Ok(MixOutput {
        snapshot: Some(snapshot),
        stems,
        meters,
        diagnostics,
        timing,
    })
}

/// How long the audible timeline is, in frames.
fn timeline_frames(tracks: &[&Track]) -> Result<usize, MixError> {
    let frames = tracks
        .iter()
        .flat_map(|track| &track.layers)
        .flat_map(|layer| &layer.clips)
        .map(|clip| clip.start_sample.saturating_add(clip.duration_samples))
        .max()
        .unwrap_or(0);
    usize::try_from(frames)
        .ok()
        .filter(|frames| frames.checked_mul(usize::from(MIX_CHANNELS)).is_some())
        .ok_or_else(|| {
            MixError::new(
                MixErrorCode::TooLong,
                "the timeline is longer than this machine can render",
            )
        })
}

/// Buses in the order they may be folded: everything that feeds a bus comes
/// before it. Validation has already rejected cycles, so this terminates.
fn fold_order(project: &Project) -> Vec<BusId> {
    let mut ordered: Vec<BusId> = Vec::with_capacity(project.buses.len());
    let mut depths: Vec<(BusId, usize)> = project
        .buses
        .iter()
        .map(|bus| (bus.id, depth_to_output(project, bus.id)))
        .collect();
    // Deepest first: a bus far from the master feeds one closer to it.
    depths.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    ordered.extend(depths.into_iter().map(|(bus_id, _)| bus_id));
    ordered
}

fn depth_to_output(project: &Project, start: BusId) -> usize {
    let mut depth = 0;
    let mut current = start;
    while let Some(next) = project
        .buses
        .iter()
        .find(|bus| bus.id == current)
        .and_then(|bus| bus.output_bus_id)
    {
        depth += 1;
        if depth > project.buses.len() {
            break;
        }
        current = next;
    }
    depth
}

/// Renders one clip into a buffer, from a file or from an extension.
#[allow(clippy::too_many_arguments)]
fn render_one_clip(
    buffer: &mut [f32],
    total_frames: usize,
    project: &Project,
    clip: &Clip,
    extensions: &ExtensionRegistries,
    sample_rate: u32,
    load: &mut impl FnMut(AssetId) -> Result<SourceAudio, String>,
    diagnostics: &mut Vec<MixDiagnostic>,
) -> Result<(), MixError> {
    match rendered_source(project, clip) {
        Some(source) => {
            // A clip playing a pattern has its notes resolved here, so the
            // synth sees one flat list however they were written.
            let notes = clip.resolved_notes(&project.patterns);
            match render_extension_clip(
                extensions,
                source,
                clip,
                &notes,
                sample_rate,
                load,
                diagnostics,
            ) {
                Ok(source) => {
                    // The rendered buffer *is* the clip, so it is read from its
                    // start rather than from the clip's source offset.
                    let mut placed = clip.clone();
                    placed.source_start_sample = 0;
                    render_clip(buffer, total_frames, &placed, &source, sample_rate);
                }
                // An extension this build does not have is the same kind of
                // problem as a sample it cannot read: one silent clip, named,
                // rather than a project that will not play at all.
                Err(error) => diagnostics.push(MixDiagnostic {
                    code: MixDiagnosticCode::ExtensionUnavailable,
                    entity_id: clip.id.to_string(),
                    message: error.message,
                }),
            }
        }
        None => {
            // A file that cannot be read isolates to its own clip: the rest of
            // the project still plays, and the diagnostic says which sound is
            // missing. One damaged sample should not silence a session.
            match load(clip.asset_id) {
                Ok(source) => render_clip(buffer, total_frames, clip, &source, sample_rate),
                Err(message) => diagnostics.push(MixDiagnostic {
                    code: MixDiagnosticCode::SourceUnreadable,
                    entity_id: clip.id.to_string(),
                    message: format!("clip {} plays silence: {message}", clip.id),
                }),
            }
        }
    }
    Ok(())
}

/// Latency adds up along a signal path; a tail is the longest one, not the sum.
const fn combine(left: ChainTiming, right: ChainTiming) -> ChainTiming {
    ChainTiming {
        latency_frames: left.latency_frames.saturating_add(right.latency_frames),
        tail_frames: if left.tail_frames > right.tail_frames {
            left.tail_frames
        } else {
            right.tail_frames
        },
    }
}

/// The lanes writing to one target, in project order.
fn lanes_for(project: &Project, target: AutomationTarget) -> Vec<&AutomationLane> {
    project
        .automation
        .iter()
        .filter(|lane| lane.target == target)
        .collect()
}

/// A channel strip: level and stereo position, from the same parameter keys a
/// clip uses. Muted strips are already excluded from the sum.
///
/// Automation is evaluated per frame. A fader move is a curve, not a step, so
/// evaluating per block would stair-step it back into the clicks the crossfade
/// exists to avoid.
fn apply_strip(
    buffer: &mut [f32],
    parameters: &BTreeMap<String, ParameterValue>,
    lanes: &[&AutomationLane],
) {
    let static_gain_db = match parameters.get(GAIN_DB_KEY) {
        Some(ParameterValue::Float(value)) => *value,
        _ => 0.0,
    };
    let static_pan = match parameters.get(PAN_KEY) {
        Some(ParameterValue::Float(value)) => value.clamp(-1.0, 1.0),
        _ => 0.0,
    };
    let gain_lane = lanes
        .iter()
        .find(|lane| lane.parameter == GAIN_DB_KEY)
        .copied();
    let pan_lane = lanes.iter().find(|lane| lane.parameter == PAN_KEY).copied();

    if gain_lane.is_none()
        && pan_lane.is_none()
        && static_gain_db.abs() < f64::EPSILON
        && static_pan.abs() < f64::EPSILON
    {
        return;
    }

    let channels = usize::from(MIX_CHANNELS);
    for frame in 0..buffer.len() / channels {
        // A lane replaces the stored value wherever it has one; where it has
        // none — an empty lane — the stored value still stands.
        let gain_db = gain_lane
            .and_then(|lane| lane.value_at(frame as u64))
            .unwrap_or(static_gain_db);
        let pan = pan_lane
            .and_then(|lane| lane.value_at(frame as u64))
            .unwrap_or(static_pan)
            .clamp(-1.0, 1.0);
        let gain = 10_f32.powf(gain_db as f32 / 20.0);
        let (left, right) = pan_gains(pan);
        let channel_gain = [gain * left, gain * right];
        for channel in 0..channels {
            buffer[frame * channels + channel] *= channel_gain[channel];
        }
    }
}

fn add_into(target: &mut [f32], source: &[f32]) {
    for (slot, sample) in target.iter_mut().zip(source) {
        *slot += sample;
    }
}

fn peak_of(buffer: &[f32]) -> f32 {
    buffer
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
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
        AudioAssetSource::Synth { .. }
            | AudioAssetSource::Generated { .. }
            | AudioAssetSource::Sampler { .. }
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
#[allow(clippy::too_many_arguments)]
fn render_extension_clip(
    extensions: &ExtensionRegistries,
    source: &AudioAssetSource,
    clip: &Clip,
    notes: &[jutsu_audio_model::ClipNote],
    sample_rate: u32,
    load: &mut impl FnMut(AssetId) -> Result<SourceAudio, String>,
    diagnostics: &mut Vec<MixDiagnostic>,
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
        AudioAssetSource::Sampler {
            zones,
            attack_ms,
            release_ms,
            max_voices,
        } => {
            let samples = crate::sampler::render(
                zones,
                notes,
                frames,
                sample_rate,
                *attack_ms,
                *release_ms,
                *max_voices,
                &clip.id.to_string(),
                load,
                diagnostics,
            );
            return Ok(SourceAudio {
                sample_rate,
                channels: 1,
                samples: Arc::from(samples),
            });
        }
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

    let mut events = Vec::with_capacity(notes.len() * 2);
    for note in notes {
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
    parameters: &BTreeMap<String, ParameterValue>,
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
