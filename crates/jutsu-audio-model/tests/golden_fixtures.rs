use jutsu_audio_model::{AudioAssetSource, CURRENT_PROJECT_SCHEMA_VERSION, Project};

const SEEDED_PROJECT_V1: &str = include_str!("../../../fixtures/projects/v1/seeded-project.json");

#[test]
fn seeded_v1_fixture_is_valid_and_byte_reproducible() {
    let project: Project = serde_json::from_str(SEEDED_PROJECT_V1).unwrap();

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
    assert_eq!(encoded, SEEDED_PROJECT_V1.replace("\r\n", "\n"));
}
