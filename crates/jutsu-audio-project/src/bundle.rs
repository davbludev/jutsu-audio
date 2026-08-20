//! Packing a project up to go somewhere else, and putting it back together
//! when files have moved.
//!
//! A bundle is a directory: the project file, the audio it uses, and the preset
//! library beside it. Everything inside refers to everything else by relative
//! path, so the bundle opens the same on another machine, in another folder,
//! under another user name.
//!
//! Relinking is by content, not by name. A file that was renamed or moved is
//! found by its fingerprint, because that is what actually identifies the audio.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use jutsu_audio_model::{AssetId, AudioAssetSource, Project};

use crate::{AssetDiagnostic, AssetManager, ProjectFileError, ProjectStore, sha256_hex};

/// Where a bundle keeps its audio.
pub const BUNDLE_ASSETS_DIRECTORY: &str = "assets";
/// Where a bundle keeps the preset library that travelled with it.
pub const BUNDLE_PRESETS_DIRECTORY: &str = "presets";

/// What a bundling run produced, and what it could not.
#[derive(Clone, Debug, PartialEq)]
pub struct BundleReport {
    /// The project file inside the bundle.
    pub project_path: PathBuf,
    /// Assets copied in, by ID.
    pub copied_assets: Vec<AssetId>,
    /// Assets that could not be copied, with why. The bundle is still written:
    /// a project missing one sound is more use than no bundle at all.
    pub unresolved: Vec<AssetDiagnostic>,
    /// How many preset files travelled along.
    pub presets_copied: usize,
}

/// Packs a project and everything it uses into `destination`.
///
/// Every managed asset is copied under `assets/` and its path rewritten to
/// point there, so nothing in the bundle names a location outside it.
pub fn bundle(
    project_path: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> Result<BundleReport, ProjectFileError> {
    let project_path = project_path.as_ref();
    let destination = destination.as_ref();
    let source_directory = project_path.parent().unwrap_or_else(|| Path::new("."));
    let opened = ProjectStore::open(project_path)?;
    let mut project = opened.project;

    fs::create_dir_all(destination.join(BUNDLE_ASSETS_DIRECTORY))
        .map_err(|error| ProjectFileError::io(destination, "create bundle directory", error))?;

    let mut copied_assets = Vec::new();
    let mut unresolved = Vec::new();
    for asset in &mut project.assets {
        let AudioAssetSource::ManagedFile {
            path, fingerprint, ..
        } = &mut asset.source
        else {
            // Synths, samplers and generators carry no file of their own; a
            // sampler's zones name assets that are bundled in their own right.
            continue;
        };
        let from = source_directory.join(&*path);
        let name = Path::new(&*path).file_name().map_or_else(
            || format!("{}.wav", asset.id),
            |name| name.to_string_lossy().into_owned(),
        );
        // Prefixed with the asset ID so two samples of the same name cannot
        // collide inside the bundle.
        let bundled = format!("{BUNDLE_ASSETS_DIRECTORY}/{}-{name}", asset.id);
        let to = destination.join(&bundled);

        match fs::copy(&from, &to) {
            Ok(_) => {
                *path = bundled;
                copied_assets.push(asset.id);
            }
            Err(_) => unresolved.push(AssetDiagnostic {
                code: crate::AssetDiagnosticCode::MissingSource,
                asset_id: asset.id,
                path: from,
                message: format!(
                    "audio for '{}' could not be read, so it is not in the bundle",
                    asset.name
                ),
            }),
        }
        let _ = fingerprint;
    }

    let bundled_project = destination.join(
        project_path
            .file_name()
            .unwrap_or_else(|| "project.jutsu-audio.json".as_ref()),
    );
    ProjectStore::save(&bundled_project, &project)?;

    let presets_copied = copy_presets(
        &source_directory.join(BUNDLE_PRESETS_DIRECTORY),
        &destination.join(BUNDLE_PRESETS_DIRECTORY),
    );

    Ok(BundleReport {
        project_path: bundled_project,
        copied_assets,
        unresolved,
        presets_copied,
    })
}

/// Copies a preset library into the bundle, if there is one. Missing is fine —
/// most projects have no presets of their own.
fn copy_presets(from: &Path, to: &Path) -> usize {
    let Ok(kinds) = fs::read_dir(from) else {
        return 0;
    };
    let mut copied = 0;
    for kind in kinds.filter_map(Result::ok) {
        let Ok(entries) = fs::read_dir(kind.path()) else {
            continue;
        };
        let target = to.join(kind.file_name());
        if fs::create_dir_all(&target).is_err() {
            continue;
        }
        for entry in entries.filter_map(Result::ok) {
            if fs::copy(entry.path(), target.join(entry.file_name())).is_ok() {
                copied += 1;
            }
        }
    }
    copied
}

/// Every asset path in a project that names a location outside it.
///
/// A bundle with any of these is not portable, so this is what "opens on
/// another machine" is checked with.
#[must_use]
pub fn absolute_asset_paths(project: &Project) -> Vec<(AssetId, String)> {
    project
        .assets
        .iter()
        .filter_map(|asset| {
            let path = match &asset.source {
                AudioAssetSource::ManagedFile { path, .. } | AudioAssetSource::File { path } => {
                    path
                }
                _ => return None,
            };
            // Judged as text, not through this platform's rules: a path that
            // is absolute on the machine that wrote it is just as unportable
            // here, even where `Path::is_absolute` says otherwise.
            let rooted = path.starts_with('/') || path.starts_with('\\');
            let drive_letter = path.as_bytes().get(1).is_some_and(|byte| *byte == b':');
            (rooted || drive_letter || Path::new(path).is_absolute())
                .then(|| (asset.id, path.clone()))
        })
        .collect()
}

/// What a relink run found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RelinkReport {
    /// Assets whose file was found again, with where it was found.
    pub relinked: Vec<(AssetId, PathBuf)>,
    /// Assets still missing after the search.
    pub unresolved: Vec<AssetDiagnostic>,
}

/// Finds moved or renamed audio by fingerprint and rewrites the paths.
///
/// Only missing assets are searched for: an asset whose file is where it should
/// be is left alone, even if an identical copy turns up somewhere else.
pub fn relink(
    project: &Project,
    project_path: impl AsRef<Path>,
    search_paths: &[PathBuf],
) -> RelinkReport {
    let project_path = project_path.as_ref();
    let missing = AssetManager::verify_sources(project, project_path);
    if missing.is_empty() {
        return RelinkReport::default();
    }

    // One pass over the search paths, indexed by fingerprint: a search for ten
    // missing samples should not read the disk ten times.
    let wanted: BTreeMap<String, AssetId> = project
        .assets
        .iter()
        .filter(|asset| {
            missing
                .iter()
                .any(|diagnostic| diagnostic.asset_id == asset.id)
        })
        .filter_map(|asset| match &asset.source {
            AudioAssetSource::ManagedFile { fingerprint, .. } => {
                Some((fingerprint.clone(), asset.id))
            }
            _ => None,
        })
        .collect();

    let mut found: BTreeMap<AssetId, PathBuf> = BTreeMap::new();
    for root in search_paths {
        scan(root, &wanted, &mut found, 0);
    }

    let project_directory = project_path.parent().unwrap_or_else(|| Path::new("."));
    let relinked = found
        .into_iter()
        .map(|(asset_id, path)| {
            // Relative to the project when it can be, so the result is as
            // portable as what it replaced.
            let relative = path
                .strip_prefix(project_directory)
                .map_or_else(|_| path.clone(), Path::to_path_buf);
            (asset_id, relative)
        })
        .collect::<Vec<_>>();

    let unresolved = missing
        .into_iter()
        .filter(|diagnostic| {
            !relinked
                .iter()
                .any(|(asset_id, _)| *asset_id == diagnostic.asset_id)
        })
        .collect();

    RelinkReport {
        relinked,
        unresolved,
    }
}

/// How deep a search goes. Deep enough for a project folder, shallow enough not
/// to walk a whole drive by accident.
const MAXIMUM_SEARCH_DEPTH: usize = 8;

fn scan(
    directory: &Path,
    wanted: &BTreeMap<String, AssetId>,
    found: &mut BTreeMap<AssetId, PathBuf>,
    depth: usize,
) {
    if depth > MAXIMUM_SEARCH_DEPTH || found.len() == wanted.len() {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            scan(&path, wanted, found, depth + 1);
            continue;
        }
        // Only files that could be audio, and only until everything is found.
        if !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"))
        {
            continue;
        }
        let Ok(contents) = fs::read(&path) else {
            continue;
        };
        if let Some(asset_id) = wanted.get(&sha256_hex(&contents)) {
            found.entry(*asset_id).or_insert(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use jutsu_audio_model::Asset;

    use super::*;
    use crate::ImportMode;

    fn write_wav(path: &Path, value: i16) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("wav");
        for _ in 0..64 {
            writer.write_sample(value).expect("sample");
        }
        writer.finalize().expect("finalize");
    }

    /// A project with one imported sample, saved on disk.
    fn project_with_sample(directory: &Path, value: i16) -> (PathBuf, Project) {
        let path = directory.join("song.jutsu-audio.json");
        let source = directory.join("source.wav");
        write_wav(&source, value);

        let mut project = ProjectStore::new_project("Bundled");
        let prepared =
            AssetManager::prepare_wav_import(&project, &path, &source, ImportMode::CopyIntoProject)
                .expect("import");
        project
            .assets
            .push(prepared.asset.expect("a prepared asset"));
        ProjectStore::save(&path, &project).expect("save");
        (path, project)
    }

    #[test]
    fn a_bundle_holds_the_project_its_audio_and_nothing_absolute() {
        let directory = tempfile::tempdir().expect("temp dir");
        let (path, _) = project_with_sample(directory.path(), 1_000);
        let destination = directory.path().join("bundle");

        let report = bundle(&path, &destination).expect("bundle");
        assert_eq!(report.copied_assets.len(), 1);
        assert!(report.unresolved.is_empty());

        let bundled = ProjectStore::open(&report.project_path)
            .expect("open")
            .project;
        assert!(
            absolute_asset_paths(&bundled).is_empty(),
            "nothing in a bundle names a place outside it"
        );
        assert!(
            AssetManager::verify_sources(&bundled, &report.project_path).is_empty(),
            "and every sound it names is there"
        );
    }

    #[test]
    fn a_bundle_reports_audio_it_could_not_pack_and_is_written_anyway() {
        let directory = tempfile::tempdir().expect("temp dir");
        let (path, project) = project_with_sample(directory.path(), 1_000);

        // Delete the copy the import made, leaving the project naming it.
        let AudioAssetSource::ManagedFile { path: relative, .. } = &project.assets[0].source else {
            panic!("a managed import");
        };
        fs::remove_file(directory.path().join(relative)).expect("remove");

        let destination = directory.path().join("bundle");
        let report = bundle(&path, &destination).expect("bundle");
        assert_eq!(report.unresolved.len(), 1);
        assert!(
            report.project_path.exists(),
            "the bundle is still written: a project missing one sound beats no bundle"
        );
    }

    #[test]
    fn a_preset_library_travels_with_the_bundle() {
        let directory = tempfile::tempdir().expect("temp dir");
        let (path, _) = project_with_sample(directory.path(), 1_000);
        let presets = directory
            .path()
            .join(BUNDLE_PRESETS_DIRECTORY)
            .join("synths");
        fs::create_dir_all(&presets).expect("create");
        fs::write(presets.join("lead.json"), b"{}").expect("write");

        let destination = directory.path().join("bundle");
        let report = bundle(&path, &destination).expect("bundle");
        assert_eq!(report.presets_copied, 1);
        assert!(
            destination
                .join(BUNDLE_PRESETS_DIRECTORY)
                .join("synths/lead.json")
                .exists()
        );
    }

    #[test]
    fn a_moved_sample_is_found_again_by_its_fingerprint() {
        let directory = tempfile::tempdir().expect("temp dir");
        let (path, project) = project_with_sample(directory.path(), 1_000);
        let AudioAssetSource::ManagedFile { path: relative, .. } = &project.assets[0].source else {
            panic!("a managed import");
        };

        // Move the audio somewhere else, under a different name.
        let elsewhere = directory.path().join("moved");
        fs::create_dir_all(&elsewhere).expect("create");
        let moved = elsewhere.join("renamed.wav");
        fs::rename(directory.path().join(relative), &moved).expect("move");

        let report = relink(&project, &path, std::slice::from_ref(&elsewhere));
        assert_eq!(report.relinked.len(), 1, "found by content, not by name");
        assert!(report.unresolved.is_empty());
        assert!(report.relinked[0].1.ends_with("renamed.wav"));
    }

    #[test]
    fn a_sample_that_is_nowhere_stays_unresolved() {
        let directory = tempfile::tempdir().expect("temp dir");
        let (path, project) = project_with_sample(directory.path(), 1_000);
        let AudioAssetSource::ManagedFile { path: relative, .. } = &project.assets[0].source else {
            panic!("a managed import");
        };
        fs::remove_file(directory.path().join(relative)).expect("remove");

        let empty = directory.path().join("empty");
        fs::create_dir_all(&empty).expect("create");
        let report = relink(&project, &path, &[empty]);
        assert!(report.relinked.is_empty());
        assert_eq!(report.unresolved.len(), 1);
    }

    #[test]
    fn a_project_whose_audio_is_all_present_needs_no_relinking() {
        let directory = tempfile::tempdir().expect("temp dir");
        let (path, project) = project_with_sample(directory.path(), 1_000);
        assert_eq!(
            relink(&project, &path, &[directory.path().to_path_buf()]),
            RelinkReport::default()
        );
    }

    #[test]
    fn an_absolute_path_is_reported_as_the_portability_problem_it_is() {
        let mut project = ProjectStore::new_project("Portable");
        project.assets.push(Asset {
            id: AssetId::new(),
            name: "Outside".into(),
            source: AudioAssetSource::File {
                path: "/somewhere/else/hit.wav".into(),
            },
        });
        let _ = BTreeMap::<String, String>::new();

        let problems = absolute_asset_paths(&project);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].1.contains("somewhere"));
    }
}
