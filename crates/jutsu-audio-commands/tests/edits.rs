//! Timeline editing primitives, and the edges they have to survive: cutting at
//! a clip boundary, rippling from frame zero, fades longer than the clip, and
//! overlaps.

use std::collections::BTreeMap;

use jutsu_audio_commands::edits::{
    self, DeleteMode, clamp_fades, crossfade, delete, duplicate, fade_in, fade_out, paste, slip,
    split,
};
use jutsu_audio_commands::{
    COMMAND_PROTOCOL_VERSION, CommandEnvelope, CommandHistory, CommandId, ProjectCommand,
    ProjectCommandEngine,
};
use jutsu_audio_model::{
    Asset, AssetId, AudioAssetSource, BusId, CURRENT_PROJECT_SCHEMA_VERSION, Clip, ClipId, Layer,
    LayerId, MixerBus, Project, ProjectId, ProjectMetadata, Track, TrackId,
};

struct Fixture {
    project: Project,
    track_id: TrackId,
    layer_id: LayerId,
    asset_id: AssetId,
}

fn fixture() -> Fixture {
    let bus_id = BusId::new();
    let asset = Asset {
        id: AssetId::new(),
        name: "Hit".into(),
        source: AudioAssetSource::File {
            path: "hit.wav".into(),
        },
    };
    let track_id = TrackId::new();
    let layer_id = LayerId::new();
    Fixture {
        asset_id: asset.id,
        project: Project {
            schema_version: CURRENT_PROJECT_SCHEMA_VERSION,
            id: ProjectId::new(),
            metadata: ProjectMetadata {
                name: "Edits".into(),
                properties: BTreeMap::new(),
            },
            assets: vec![asset],
            buses: vec![MixerBus {
                id: bus_id,
                name: "Master".into(),
                output_bus_id: None,
                parameters: BTreeMap::new(),
                effects: Vec::new(),
            }],
            master_bus_id: bus_id,
            markers: Vec::new(),
            loop_region: None,
            automation: Vec::new(),
            tempo: Vec::new(),
            tracks: vec![Track {
                id: track_id,
                name: "SFX".into(),
                output_bus_id: bus_id,
                parameters: BTreeMap::new(),
                layers: vec![Layer {
                    id: layer_id,
                    name: "Layer".into(),
                    clips: Vec::new(),
                }],
                effects: Vec::new(),
            }],
        },
        track_id,
        layer_id,
    }
}

impl Fixture {
    fn clip(&mut self, start: u64, duration: u64) -> ClipId {
        let clip = Clip {
            id: ClipId::new(),
            asset_id: self.asset_id,
            start_sample: start,
            source_start_sample: 0,
            duration_samples: duration,
            parameters: BTreeMap::new(),
            notes: Vec::new(),
        };
        let id = clip.id;
        self.project.tracks[0].layers[0].clips.push(clip);
        id
    }

    /// Applies a batch the way the editor does, and returns the project after.
    fn commit(&self, commands: Vec<ProjectCommand>) -> Project {
        let mut engine = ProjectCommandEngine::new(self.project.clone()).expect("engine");
        let mut history = CommandHistory::new();
        history
            .apply(
                &mut engine,
                CommandEnvelope {
                    protocol_version: COMMAND_PROTOCOL_VERSION,
                    command_id: CommandId::new(),
                    expected_revision: 0,
                    commands,
                },
            )
            .expect("batch applies");
        assert_eq!(engine.revision(), 1, "one batch is one revision to undo");
        engine.project().clone()
    }
}

fn clips(project: &Project) -> &[Clip] {
    &project.tracks[0].layers[0].clips
}

fn clip_of(project: &Project, clip_id: ClipId) -> &Clip {
    clips(project)
        .iter()
        .find(|clip| clip.id == clip_id)
        .expect("clip")
}

#[test]
fn splitting_keeps_the_source_running_across_the_cut() {
    let mut fixture = fixture();
    let clip_id = fixture.clip(100, 400);

    let after = fixture.commit(split(&fixture.project, clip_id, 250).expect("split"));
    assert_eq!(clips(&after).len(), 2);

    let head = clip_of(&after, clip_id);
    assert_eq!((head.start_sample, head.duration_samples), (100, 150));
    let tail = clips(&after)
        .iter()
        .find(|clip| clip.id != clip_id)
        .expect("tail");
    assert_eq!((tail.start_sample, tail.duration_samples), (250, 250));
    assert_eq!(
        tail.source_start_sample, 150,
        "the tail reads on from where the head stopped"
    );
}

#[test]
fn splitting_at_an_edge_is_refused_rather_than_making_an_empty_clip() {
    let mut fixture = fixture();
    let clip_id = fixture.clip(100, 400);

    for frame in [100, 500, 0, 900] {
        assert!(
            split(&fixture.project, clip_id, frame).is_err(),
            "frame {frame} is not inside the clip"
        );
    }
}

#[test]
fn ripple_delete_closes_the_gap_and_leaves_earlier_clips_alone() {
    let mut fixture = fixture();
    let first = fixture.clip(0, 100);
    let removed = fixture.clip(100, 100);
    let later = fixture.clip(200, 100);

    let after = fixture
        .commit(delete(&fixture.project, &[removed], DeleteMode::Ripple).expect("ripple delete"));
    assert_eq!(clips(&after).len(), 2);
    assert_eq!(clip_of(&after, first).start_sample, 0);
    assert_eq!(clip_of(&after, later).start_sample, 100);
}

#[test]
fn a_plain_delete_leaves_the_gap_where_the_clip_was() {
    let mut fixture = fixture();
    let removed = fixture.clip(100, 100);
    let later = fixture.clip(200, 100);

    let after =
        fixture.commit(delete(&fixture.project, &[removed], DeleteMode::Leave).expect("delete"));
    assert_eq!(clips(&after).len(), 1);
    assert_eq!(clip_of(&after, later).start_sample, 200);
}

#[test]
fn rippling_a_clip_at_frame_zero_pulls_the_rest_to_the_start() {
    let mut fixture = fixture();
    let removed = fixture.clip(0, 480);
    let later = fixture.clip(480, 100);

    let after =
        fixture.commit(delete(&fixture.project, &[removed], DeleteMode::Ripple).expect("delete"));
    assert_eq!(clip_of(&after, later).start_sample, 0);
}

#[test]
fn rippling_leaves_an_overlapping_clip_in_place_because_it_has_no_gap_to_close() {
    let mut fixture = fixture();
    let removed = fixture.clip(100, 200);
    let overlapping = fixture.clip(200, 200);

    let after =
        fixture.commit(delete(&fixture.project, &[removed], DeleteMode::Ripple).expect("delete"));
    assert_eq!(clip_of(&after, overlapping).start_sample, 200);
}

#[test]
fn duplicating_copies_the_clip_and_everything_that_shapes_its_sound() {
    let mut fixture = fixture();
    let clip_id = fixture.clip(0, 100);
    let with_fades =
        fixture.commit(edits::set_fades(&fixture.project, clip_id, 10, 20).expect("fades"));
    fixture.project = with_fades;

    let after = fixture.commit(duplicate(&fixture.project, &[clip_id], 500).expect("duplicate"));
    let copy = clips(&after)
        .iter()
        .find(|clip| clip.id != clip_id)
        .expect("copy");
    assert_eq!(copy.start_sample, 500);
    assert_eq!(copy.asset_id, fixture.asset_id);
    assert_eq!((fade_in(copy), fade_out(copy)), (10, 20));
}

#[test]
fn slipping_moves_the_material_and_never_reads_before_the_source_starts() {
    let mut fixture = fixture();
    let clip_id = fixture.clip(0, 100);

    let after = fixture.commit(slip(&fixture.project, &[clip_id], 40).expect("slip"));
    let slipped = clip_of(&after, clip_id);
    assert_eq!(slipped.source_start_sample, 40);
    assert_eq!(slipped.start_sample, 0, "the window itself does not move");

    fixture.project = after;
    let back = fixture.commit(slip(&fixture.project, &[clip_id], -400).expect("slip"));
    assert_eq!(clip_of(&back, clip_id).source_start_sample, 0);
}

#[test]
fn fades_are_clamped_so_they_always_fit_inside_the_clip() {
    assert_eq!(clamp_fades(100, 10, 20), (10, 20));
    assert_eq!(clamp_fades(100, 400, 0), (100, 0));
    assert_eq!(
        clamp_fades(100, 80, 40),
        (60, 40),
        "the longer fade gives way first"
    );
    assert_eq!(clamp_fades(100, 40, 80), (40, 60));
    assert_eq!(clamp_fades(0, 10, 10), (0, 0));
}

#[test]
fn a_crossfade_covers_the_whole_overlap_of_two_clips() {
    let mut fixture = fixture();
    let early = fixture.clip(0, 1_000);
    let late = fixture.clip(800, 1_000);

    let after = fixture.commit(crossfade(&fixture.project, early, late).expect("crossfade"));
    assert_eq!(fade_out(clip_of(&after, early)), 200);
    assert_eq!(fade_in(clip_of(&after, late)), 200);
}

#[test]
fn crossfading_clips_that_do_not_touch_is_refused() {
    let mut fixture = fixture();
    let first = fixture.clip(0, 100);
    let second = fixture.clip(500, 100);

    assert!(crossfade(&fixture.project, first, second).is_err());
}

#[test]
fn pasting_keeps_the_spacing_of_what_was_copied() {
    let mut fixture = fixture();
    let first = fixture.clip(1_000, 100);
    let second = fixture.clip(1_500, 100);
    let copied: Vec<Clip> = clips(&fixture.project).to_vec();
    let _ = (first, second);

    let after = fixture.commit(paste(&copied, fixture.track_id, fixture.layer_id, 0));
    let mut starts: Vec<u64> = clips(&after).iter().map(|clip| clip.start_sample).collect();
    starts.sort_unstable();
    assert_eq!(starts, vec![0, 500, 1_000, 1_500]);
}

#[test]
fn an_edit_naming_a_clip_that_is_gone_fails_before_anything_is_applied() {
    let fixture = fixture();
    let missing = ClipId::new();
    assert!(duplicate(&fixture.project, &[missing], 0).is_err());
    assert!(slip(&fixture.project, &[missing], 10).is_err());
    assert!(delete(&fixture.project, &[missing], DeleteMode::Ripple).is_err());
    assert!(split(&fixture.project, missing, 10).is_err());
}
