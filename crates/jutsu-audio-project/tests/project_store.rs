use std::fs;

use jutsu_audio_model::CURRENT_PROJECT_SCHEMA_VERSION;
use jutsu_audio_project::{ProjectFileErrorCode, ProjectStore};
use serde_json::Value;
use tempfile::tempdir;

#[test]
fn create_save_and_reopen_round_trip() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("impact.jutsu-audio.json");

    let mut project = ProjectStore::create(&path, "Impact").unwrap();
    project.metadata.name = "Edited impact".into();
    ProjectStore::save(&path, &project).unwrap();
    let opened = ProjectStore::open(&path).unwrap();

    assert_eq!(opened.project, project);
    assert_eq!(opened.migrated_from, None);
    assert_eq!(opened.backup_path, None);
}

#[test]
fn corrupt_project_fails_without_changing_file() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("corrupt.jutsu-audio.json");
    let original = b"{ definitely not json";
    fs::write(&path, original).unwrap();

    let error = ProjectStore::open(&path).unwrap_err();

    assert_eq!(error.code, ProjectFileErrorCode::InvalidJson);
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn future_schema_fails_without_changing_file() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("future.jutsu-audio.json");
    let mut value: Value = serde_json::from_str(include_str!(
        "../../../fixtures/projects/v1/seeded-project.json"
    ))
    .unwrap();
    value["schema_version"] = Value::from(CURRENT_PROJECT_SCHEMA_VERSION + 1);
    let original = serde_json::to_vec_pretty(&value).unwrap();
    fs::write(&path, &original).unwrap();

    let error = ProjectStore::open(&path).unwrap_err();

    assert_eq!(error.code, ProjectFileErrorCode::UnsupportedSchemaVersion);
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn opening_v0_creates_exact_backup_and_migrates_atomically() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("legacy.jutsu-audio.json");
    let mut value: Value = serde_json::from_str(include_str!(
        "../../../fixtures/projects/v1/seeded-project.json"
    ))
    .unwrap();
    value["schema_version"] = Value::from(0);
    let original = format!("{}\n", serde_json::to_string_pretty(&value).unwrap());
    fs::write(&path, original.as_bytes()).unwrap();

    let opened = ProjectStore::open(&path).unwrap();

    assert_eq!(opened.migrated_from, Some(0));
    assert_eq!(
        opened.project.schema_version,
        CURRENT_PROJECT_SCHEMA_VERSION
    );
    let backup_path = opened.backup_path.unwrap();
    assert_eq!(fs::read_to_string(backup_path).unwrap(), original);
    let migrated: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(migrated["schema_version"], CURRENT_PROJECT_SCHEMA_VERSION);
}

#[test]
fn invalid_save_does_not_replace_existing_project() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("safe.jutsu-audio.json");
    let mut project = ProjectStore::create(&path, "Safe").unwrap();
    let original = fs::read(&path).unwrap();
    project.master_bus_id = jutsu_audio_model::BusId::new();

    let error = ProjectStore::save(&path, &project).unwrap_err();

    assert_eq!(error.code, ProjectFileErrorCode::InvalidProject);
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn save_rejects_absolute_asset_paths_without_replacing_project() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("portable.jutsu-audio.json");
    let mut project = ProjectStore::create(&path, "Portable").unwrap();
    let original = fs::read(&path).unwrap();
    project.assets.push(jutsu_audio_model::Asset {
        id: jutsu_audio_model::AssetId::new(),
        name: "External".into(),
        source: jutsu_audio_model::AudioAssetSource::File {
            path: std::env::temp_dir()
                .join("external.wav")
                .to_string_lossy()
                .into_owned(),
        },
    });

    let error = ProjectStore::save(&path, &project).unwrap_err();

    assert_eq!(error.code, ProjectFileErrorCode::InvalidAssetPath);
    assert_eq!(fs::read(&path).unwrap(), original);
}
