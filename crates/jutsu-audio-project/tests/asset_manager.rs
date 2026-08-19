use std::fs;

use hound::{SampleFormat, WavSpec, WavWriter};
use jutsu_audio_model::AudioAssetSource;
use jutsu_audio_project::{
    AssetDiagnosticCode, AssetManager, ImportMode, ImportStatus, ProjectStore,
};
use tempfile::tempdir;

fn write_pcm16(path: &std::path::Path, samples: &[i16]) {
    let mut writer = WavWriter::create(
        path,
        WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        },
    )
    .unwrap();
    for sample in samples {
        writer.write_sample(*sample).unwrap();
    }
    writer.finalize().unwrap();
}

#[test]
fn imports_pcm_wav_into_managed_assets_and_caches_waveform() {
    let directory = tempdir().unwrap();
    let project_path = directory.path().join("sfx.jutsu-audio.json");
    let source_path = directory.path().join("source.wav");
    write_pcm16(&source_path, &[i16::MIN, 0, i16::MAX, 0]);
    let project = ProjectStore::new_project("SFX");

    let prepared = AssetManager::prepare_wav_import(
        &project,
        &project_path,
        &source_path,
        ImportMode::CopyIntoProject,
    )
    .unwrap();

    assert_eq!(prepared.status, ImportStatus::Prepared);
    assert_eq!(prepared.metadata.sample_rate, 48_000);
    assert_eq!(prepared.metadata.channels, 1);
    assert_eq!(prepared.metadata.frame_count, 4);
    assert_eq!(prepared.waveform.peaks.len(), 1);
    assert!(prepared.waveform.peaks[0].minimum <= -0.99);
    assert!(prepared.waveform.peaks[0].maximum >= 0.99);
    assert!(prepared.cache_path.exists());
    match &prepared.asset.unwrap().source {
        AudioAssetSource::ManagedFile {
            path, fingerprint, ..
        } => {
            assert!(path.starts_with("assets/"));
            assert!(directory.path().join(path).exists());
            assert_eq!(fingerprint.len(), 64);
        }
        source => panic!("unexpected source: {source:?}"),
    }
}

#[test]
fn duplicate_fingerprint_returns_existing_asset_without_copy() {
    let directory = tempdir().unwrap();
    let project_path = directory.path().join("sfx.jutsu-audio.json");
    let source_path = directory.path().join("source.wav");
    write_pcm16(&source_path, &[1, 2, 3]);
    let mut project = ProjectStore::new_project("SFX");
    let first = AssetManager::prepare_wav_import(
        &project,
        &project_path,
        &source_path,
        ImportMode::CopyIntoProject,
    )
    .unwrap();
    let first_asset = first.asset.unwrap();
    let first_id = first_asset.id;
    project.assets.push(first_asset);

    let duplicate = AssetManager::prepare_wav_import(
        &project,
        &project_path,
        &source_path,
        ImportMode::CopyIntoProject,
    )
    .unwrap();

    assert_eq!(duplicate.status, ImportStatus::Duplicate(first_id));
    assert!(duplicate.asset.is_none());
}

#[test]
fn local_reference_remains_project_relative() {
    let directory = tempdir().unwrap();
    let project_path = directory.path().join("sfx.jutsu-audio.json");
    let source_directory = directory.path().join("source");
    fs::create_dir(&source_directory).unwrap();
    let source_path = source_directory.join("local.wav");
    write_pcm16(&source_path, &[0]);
    let project = ProjectStore::new_project("SFX");

    let prepared = AssetManager::prepare_wav_import(
        &project,
        &project_path,
        &source_path,
        ImportMode::ReferenceInPlace,
    )
    .unwrap();

    match prepared.asset.unwrap().source {
        AudioAssetSource::ManagedFile { path, .. } => assert_eq!(path, "source/local.wav"),
        source => panic!("unexpected source: {source:?}"),
    }
}

#[test]
fn missing_and_changed_managed_sources_return_structured_diagnostics() {
    let directory = tempdir().unwrap();
    let project_path = directory.path().join("sfx.jutsu-audio.json");
    let source_path = directory.path().join("source.wav");
    write_pcm16(&source_path, &[1, 2, 3]);
    let mut project = ProjectStore::new_project("SFX");
    let prepared = AssetManager::prepare_wav_import(
        &project,
        &project_path,
        &source_path,
        ImportMode::CopyIntoProject,
    )
    .unwrap();
    let asset = prepared.asset.unwrap();
    let managed_path = match &asset.source {
        AudioAssetSource::ManagedFile { path, .. } => directory.path().join(path),
        source => panic!("unexpected source: {source:?}"),
    };
    project.assets.push(asset);

    fs::write(&managed_path, b"changed").unwrap();
    let changed = AssetManager::verify_sources(&project, &project_path);
    assert_eq!(changed[0].code, AssetDiagnosticCode::ChangedSource);

    fs::remove_file(&managed_path).unwrap();
    let missing = AssetManager::verify_sources(&project, &project_path);
    assert_eq!(missing[0].code, AssetDiagnosticCode::MissingSource);
}

#[test]
fn cached_waveform_is_readable_back_and_rebuildable_after_deletion() {
    let directory = tempdir().unwrap();
    let project_path = directory.path().join("sfx.jutsu-audio.json");
    let source_path = directory.path().join("source.wav");
    write_pcm16(&source_path, &[i16::MIN, 0, i16::MAX, 0]);
    let project = ProjectStore::new_project("SFX");

    let prepared = AssetManager::prepare_wav_import(
        &project,
        &project_path,
        &source_path,
        ImportMode::CopyIntoProject,
    )
    .unwrap();
    let AudioAssetSource::ManagedFile { fingerprint, .. } =
        &prepared.asset.as_ref().unwrap().source
    else {
        panic!("import produces a managed file");
    };

    let loaded = AssetManager::load_waveform(&project_path, fingerprint).unwrap();
    assert_eq!(loaded, prepared.waveform);

    let cache_path = AssetManager::waveform_cache_path(&project_path, fingerprint);
    fs::remove_file(&cache_path).unwrap();
    assert!(AssetManager::load_waveform(&project_path, fingerprint).is_err());

    let rebuilt = AssetManager::rebuild_waveform(&project_path, &source_path, fingerprint).unwrap();
    assert_eq!(rebuilt, prepared.waveform);
    assert!(cache_path.exists());
}
