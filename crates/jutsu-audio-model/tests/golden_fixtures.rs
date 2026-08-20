//! The golden projects, one per schema version.
//!
//! The current-version fixture is the byte-for-byte contract: serialising it
//! again has to produce the same file, or something in the model changed shape
//! without anyone deciding to. The older ones are what migration replays, and
//! they are never edited — a fixture that gets updated to keep a test passing
//! is no longer evidence of anything.

use jutsu_audio_model::{AudioAssetSource, CURRENT_PROJECT_SCHEMA_VERSION, Project};

const SEEDED_PROJECT_V1: &str = include_str!("../../../fixtures/projects/v1/seeded-project.json");
const SEEDED_PROJECT_V2: &str = include_str!("../../../fixtures/projects/v2/seeded-project.json");

#[test]
fn the_current_fixture_is_valid_and_byte_reproducible() {
    let project: Project = serde_json::from_str(SEEDED_PROJECT_V2).unwrap();

    assert_eq!(project.schema_version, CURRENT_PROJECT_SCHEMA_VERSION);
    assert!(project.validate().is_empty());
    assert!(matches!(
        project.assets[0].source,
        AudioAssetSource::Generated {
            algorithm_version: 1,
            seed: 0x4a55_5453_5541_5544,
            ..
        }
    ));

    let encoded = format!("{}\n", serde_json::to_string_pretty(&project).unwrap());
    assert_eq!(encoded, SEEDED_PROJECT_V2.replace("\r\n", "\n"));
}

/// The older document still parses. Everything version 2 added is optional, so
/// what a version 1 file describes is unchanged by the version it declares —
/// which is exactly why its migration is a stamp.
#[test]
fn the_previous_fixture_still_describes_the_same_project() {
    let old: Project = serde_json::from_str(SEEDED_PROJECT_V1).unwrap();
    let current: Project = serde_json::from_str(SEEDED_PROJECT_V2).unwrap();

    assert_eq!(old.schema_version, 1);
    assert_eq!(current.schema_version, CURRENT_PROJECT_SCHEMA_VERSION);
    assert_eq!(old.tracks, current.tracks);
    assert_eq!(old.assets, current.assets);
    assert_eq!(old.buses, current.buses);
    assert_eq!(old.automation, current.automation);
}
