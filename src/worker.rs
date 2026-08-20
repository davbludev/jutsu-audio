//! Everything that touches a disk, a decoder, or a file dialog runs here, on a
//! single background thread. The UI thread only ever sends a [`Job`] and drains
//! [`JobResult`]s — it never blocks.
//!
//! ponytail: one worker thread, so a file dialog does hold up a queued autosave
//! until the user dismisses it. Split into an io thread and a dialog thread if
//! that ever shows up as a real stall.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread;

use eframe::egui;
use jutsu_audio_engine::{
    ExportEncoding, ExportRange, Meters, OfflineExporter, PlaybackSnapshot, SourceAudio,
    mix_project_metered,
};
use jutsu_audio_model::{AssetId, AudioAssetSource, Project};
use jutsu_audio_project::{
    AssetManager, AudioMetadata, CachedWaveform, ImportMode, ImportStatus, ProjectStore, autosave,
};

/// One request to turn the project into audio. The whole project travels, so
/// the summing rules live in one place — `jutsu-audio-engine` — rather than
/// being re-derived here and again in the CLI.
#[derive(Clone, Debug)]
pub struct MixRequest {
    pub id: u64,
    pub sample_rate: u32,
    pub project: Box<Project>,
    pub project_path: PathBuf,
}

pub enum Job {
    /// Render the whole timeline down to one playable snapshot.
    Mixdown(MixRequest),
    /// Silent autosave to a path the project already has.
    Save {
        path: PathBuf,
        project: Box<Project>,
    },
    /// Ask for a location, then save there.
    SaveAs { project: Box<Project> },
    /// Ask for a project file, then load it.
    Open,
    /// Ask for a WAV (and a project location first, if the project has none),
    /// then import it.
    Import {
        project: Box<Project>,
        project_path: Option<PathBuf>,
    },
    /// Ask for a destination, then write the current mix out.
    Export {
        snapshot: Arc<PlaybackSnapshot>,
        /// What to write: the whole timeline, or the loop the transport is
        /// repeating, so an exported loop is the loop that was heard.
        range: ExportRange,
    },
    /// Park unsaved work in the recovery sidecar.
    Autosave {
        path: PathBuf,
        project: Box<Project>,
    },
    /// Throw away parked work the user chose not to recover.
    DiscardAutosave { path: PathBuf },
    /// Load peaks for one asset, rebuilding the cache if it is missing.
    Waveform {
        asset_id: AssetId,
        project_path: PathBuf,
        source: PathBuf,
        fingerprint: String,
    },
}

#[derive(Debug)]
pub struct OpenOutcome {
    pub path: PathBuf,
    pub project: Project,
    /// Unsaved work found parked next to the project after a crash. Offered to
    /// the user; never applied on its own.
    pub recovered: Option<Project>,
}

#[derive(Debug)]
pub struct ImportOutcome {
    /// Set when the import also had to choose a project location first.
    pub project_path: PathBuf,
    pub status: ImportStatus,
    pub asset: Option<jutsu_audio_model::Asset>,
    /// True when a save location was picked as part of this import.
    pub saved_project: bool,
}

pub enum JobResult {
    Mixdown {
        id: u64,
        result: Result<Arc<PlaybackSnapshot>, String>,
        /// What each track and bus contributed, for the mixer's meters.
        meters: Box<Meters>,
        /// What the mix could not do as asked — a missing effect, say.
        diagnostics: Vec<String>,
    },
    /// An empty timeline: nothing to play, and that is not an error.
    MixdownEmpty {
        id: u64,
    },
    Saved {
        path: PathBuf,
        result: Result<(), String>,
    },
    Opened(Box<Result<OpenOutcome, String>>),
    /// An autosave write or discard finished. Only failures need reporting.
    Autosaved(Result<(), String>),
    Imported(Box<Result<ImportOutcome, String>>),
    Exported(Result<u64, String>),
    Waveform {
        asset_id: AssetId,
        result: Result<Arc<CachedWaveform>, String>,
    },
    /// The user dismissed a file dialog. Not an error, but the UI should stop
    /// showing the job as in flight.
    Cancelled,
}

pub struct Worker {
    jobs: mpsc::Sender<Job>,
    results: mpsc::Receiver<JobResult>,
}

impl Worker {
    #[must_use]
    pub fn spawn(context: egui::Context) -> Self {
        let (jobs, job_receiver) = mpsc::channel::<Job>();
        let (result_sender, results) = mpsc::channel::<JobResult>();
        thread::spawn(move || {
            let mut cache = DecodeCache::default();
            while let Ok(job) = job_receiver.recv() {
                let result = run(job, &mut cache);
                if result_sender.send(result).is_err() {
                    break;
                }
                // The UI thread may be idle; wake it so the result is drained.
                context.request_repaint();
            }
        });
        Self { jobs, results }
    }

    /// Queues a job. Returns false only if the worker thread has died, which
    /// the caller surfaces rather than ignores.
    pub fn send(&self, job: Job) -> bool {
        self.jobs.send(job).is_ok()
    }

    pub fn try_recv(&self) -> Option<JobResult> {
        self.results.try_recv().ok()
    }
}

fn run(job: Job, cache: &mut DecodeCache) -> JobResult {
    match job {
        Job::Mixdown(request) => mixdown(request, cache),
        Job::Save { path, project } => JobResult::Saved {
            result: ProjectStore::save(&path, &project)
                // The saved file now holds everything the sidecar did.
                .and_then(|()| autosave::discard(&path))
                .map_err(|error| error.message),
            path,
        },
        Job::SaveAs { project } => match pick_project_destination() {
            Some(path) => JobResult::Saved {
                result: ProjectStore::save(&path, &project).map_err(|error| error.message),
                path,
            },
            None => JobResult::Cancelled,
        },
        Job::Open => match pick_project_source() {
            Some(path) => JobResult::Opened(Box::new(open(path))),
            None => JobResult::Cancelled,
        },
        Job::Autosave { path, project } => {
            JobResult::Autosaved(autosave::write(&path, &project).map_err(|error| error.message))
        }
        Job::DiscardAutosave { path } => {
            JobResult::Autosaved(autosave::discard(&path).map_err(|error| error.message))
        }
        Job::Import {
            project,
            project_path,
        } => import(*project, project_path),
        Job::Export { snapshot, range } => match pick_export_destination() {
            Some(path) => JobResult::Exported(
                OfflineExporter::export_wav(snapshot, path, range, ExportEncoding::Pcm16)
                    .map(|report| report.frame_count)
                    .map_err(|error| error.message),
            ),
            None => JobResult::Cancelled,
        },
        Job::Waveform {
            asset_id,
            project_path,
            source,
            fingerprint,
        } => JobResult::Waveform {
            asset_id,
            result: AssetManager::load_waveform(&project_path, &fingerprint)
                .or_else(|_| AssetManager::rebuild_waveform(&project_path, &source, &fingerprint))
                .map(Arc::new)
                .map_err(|error| error.message),
        },
    }
}

/// Opens a project and looks for unsaved work parked beside it. A recovery
/// file that cannot be read is not fatal: the saved project still opens, and
/// the sidecar is simply not offered.
fn open(path: PathBuf) -> Result<OpenOutcome, String> {
    let opened = ProjectStore::open(&path).map_err(|error| error.message)?;
    let recovered = autosave::recover(&path)
        .ok()
        .flatten()
        .map(|recovered| recovered.project);
    Ok(OpenOutcome {
        path,
        project: opened.project,
        recovered,
    })
}

// ─── mixdown ────────────────────────────────────────────────────────────────

/// Renders the project through the shared mixdown, feeding it decoded sources
/// from the cache. Play and Export both consume the result, so the two cannot
/// disagree — and neither can the CLI, which calls the same function.
fn mixdown(request: MixRequest, cache: &mut DecodeCache) -> JobResult {
    let MixRequest {
        id,
        sample_rate,
        project,
        project_path,
    } = request;

    let mixed = mix_project_metered(
        &project,
        sample_rate,
        jutsu_audio::extensions::registries(),
        |asset_id| {
            let asset = project
                .assets
                .iter()
                .find(|asset| asset.id == asset_id)
                .ok_or_else(|| format!("asset {asset_id} is missing from the project"))?;
            let path = resolve_asset_path(&project_path, &asset.source)
                .ok_or_else(|| format!("asset {} has no file to read", asset.name))?;
            let (metadata, samples) = cache.get(&path)?;
            Ok(SourceAudio {
                sample_rate: metadata.sample_rate,
                channels: metadata.channels,
                samples,
            })
        },
    );

    match mixed {
        Ok(output) => {
            let diagnostics = output
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect();
            match output.snapshot {
                Some(snapshot) => JobResult::Mixdown {
                    id,
                    result: Ok(Arc::new(snapshot)),
                    meters: Box::new(output.meters),
                    diagnostics,
                },
                None => JobResult::MixdownEmpty { id },
            }
        }
        Err(error) => JobResult::Mixdown {
            id,
            result: Err(error.message),
            meters: Box::default(),
            diagnostics: Vec::new(),
        },
    }
}

// ─── import ─────────────────────────────────────────────────────────────────

fn import(project: Project, project_path: Option<PathBuf>) -> JobResult {
    // A copy-into-project import needs somewhere to copy to, so an unsaved
    // project has to choose a home first.
    let (project_path, saved_project) = match project_path {
        Some(path) => (path, false),
        None => {
            let Some(path) = pick_project_destination() else {
                return JobResult::Cancelled;
            };
            if let Err(error) = ProjectStore::save(&path, &project) {
                return JobResult::Imported(Box::new(Err(error.message)));
            }
            (path, true)
        }
    };
    let Some(source) = pick_wav_source() else {
        // The project location was still chosen and written; report that much
        // so the UI does not lose the path the user just picked.
        return if saved_project {
            JobResult::Imported(Box::new(Ok(ImportOutcome {
                project_path,
                status: ImportStatus::Prepared,
                asset: None,
                saved_project,
            })))
        } else {
            JobResult::Cancelled
        };
    };

    JobResult::Imported(Box::new(
        AssetManager::prepare_wav_import(
            &project,
            &project_path,
            &source,
            ImportMode::CopyIntoProject,
        )
        .map(|prepared| ImportOutcome {
            project_path,
            status: prepared.status,
            asset: prepared.asset,
            saved_project,
        })
        .map_err(|error| error.message),
    ))
}

// ─── decoded sample cache ───────────────────────────────────────────────────

/// Keeps recently decoded files in memory so re-mixing after a gain nudge is a
/// memcpy instead of a fresh WAV decode. Least-recently-used eviction against a
/// fixed sample budget.
#[derive(Default)]
struct DecodeCache {
    entries: VecDeque<(PathBuf, AudioMetadata, Arc<[f32]>)>,
    samples_held: usize,
}

/// 48M f32 ≈ 192 MB of decoded audio.
const CACHE_SAMPLE_BUDGET: usize = 48 << 20;

impl DecodeCache {
    fn get(&mut self, path: &Path) -> Result<(AudioMetadata, Arc<[f32]>), String> {
        if let Some(index) = self.entries.iter().position(|(key, ..)| key == path) {
            let entry = self.entries.remove(index).expect("index came from a scan");
            let hit = (entry.1.clone(), Arc::clone(&entry.2));
            self.entries.push_front(entry);
            return Ok(hit);
        }

        let (metadata, samples) =
            AssetManager::decode_wav_samples(path).map_err(|error| error.message)?;
        let samples: Arc<[f32]> = Arc::from(samples);
        self.samples_held += samples.len();
        self.entries
            .push_front((path.to_path_buf(), metadata.clone(), Arc::clone(&samples)));
        while self.samples_held > CACHE_SAMPLE_BUDGET && self.entries.len() > 1 {
            if let Some((.., evicted)) = self.entries.pop_back() {
                self.samples_held = self.samples_held.saturating_sub(evicted.len());
            }
        }
        Ok((metadata, samples))
    }
}

// ─── file dialogs ───────────────────────────────────────────────────────────
//
// `rfd`'s async dialogs are the portable way to open a picker off the main
// thread; blocking on the future here is fine because this thread exists to
// wait on slow things.

fn pick_project_source() -> Option<PathBuf> {
    pollster::block_on(
        rfd::AsyncFileDialog::new()
            .add_filter("Jutsu Audio project", &["json"])
            .set_title("Open project")
            .pick_file(),
    )
    .map(|handle| handle.path().to_path_buf())
}

fn pick_project_destination() -> Option<PathBuf> {
    pollster::block_on(
        rfd::AsyncFileDialog::new()
            .add_filter("Jutsu Audio project", &["json"])
            .set_file_name("project.jutsu-audio.json")
            .set_title("Save project")
            .save_file(),
    )
    .map(|handle| handle.path().to_path_buf())
}

fn pick_wav_source() -> Option<PathBuf> {
    pollster::block_on(
        rfd::AsyncFileDialog::new()
            .add_filter("WAV audio", &["wav"])
            .set_title("Import WAV")
            .pick_file(),
    )
    .map(|handle| handle.path().to_path_buf())
}

fn pick_export_destination() -> Option<PathBuf> {
    pollster::block_on(
        rfd::AsyncFileDialog::new()
            .add_filter("WAV audio", &["wav"])
            .set_file_name("jutsu-audio-export.wav")
            .set_title("Export mix")
            .save_file(),
    )
    .map(|handle| handle.path().to_path_buf())
}

/// Resolves a project-relative managed asset path against the project file.
#[must_use]
pub fn resolve_asset_path(project_path: &Path, source: &AudioAssetSource) -> Option<PathBuf> {
    let relative = match source {
        AudioAssetSource::ManagedFile { path, .. } | AudioAssetSource::File { path } => path,
        // Neither has a file to read: a generated asset is rendered, and a
        // synth is played from the notes on its clips.
        AudioAssetSource::Generated { .. }
        | AudioAssetSource::Synth { .. }
        | AudioAssetSource::Sampler { .. } => return None,
    };
    Some(
        project_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(relative),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_mono_wav(path: &Path, sample_rate: u32, samples: &[f32]) {
        let mut writer = hound::WavWriter::create(
            path,
            hound::WavSpec {
                channels: 1,
                sample_rate,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for sample in samples {
            writer.write_sample(*sample).unwrap();
        }
        writer.finalize().unwrap();
    }

    /// A one-clip project pointing at `source`, which is what the mixdown job
    /// is handed in the running editor.
    fn project_with_clip(project_path: &Path, source: &Path) -> Project {
        let mut project = ProjectStore::new_project("Mix");
        let asset = jutsu_audio_model::Asset {
            id: AssetId::new(),
            name: "Tone".into(),
            source: AudioAssetSource::File {
                path: pathdiff(project_path, source),
            },
        };
        let clip = jutsu_audio_model::Clip {
            id: jutsu_audio_model::ClipId::new(),
            asset_id: asset.id,
            start_sample: 2,
            source_start_sample: 0,
            duration_samples: 4,
            parameters: std::collections::BTreeMap::new(),
            notes: Vec::new(),
            pattern_id: None,
        };
        project.assets.push(asset);
        project.tracks[0].layers[0].clips.push(clip);
        project
    }

    /// Asset paths are stored relative to the project file.
    fn pathdiff(project_path: &Path, source: &Path) -> String {
        source
            .strip_prefix(project_path.parent().unwrap())
            .unwrap()
            .display()
            .to_string()
    }

    #[test]
    fn a_mix_job_decodes_through_the_cache_and_lands_the_clip_at_its_start_frame() {
        let directory = tempfile::tempdir().unwrap();
        let project_path = directory.path().join("mix.jutsu-audio.json");
        let source = directory.path().join("tone.wav");
        write_mono_wav(&source, 48_000, &[1.0, 0.5, -0.5, -1.0]);

        let mut cache = DecodeCache::default();
        let result = mixdown(
            MixRequest {
                id: 1,
                sample_rate: 48_000,
                project: Box::new(project_with_clip(&project_path, &source)),
                project_path,
            },
            &mut cache,
        );
        let JobResult::Mixdown { result, .. } = result else {
            panic!("a project with a clip mixes to audio");
        };
        assert_eq!(
            result.unwrap().samples().to_vec(),
            vec![
                0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.5, 0.5, -0.5, -0.5, -1.0, -1.0
            ]
        );
    }

    #[test]
    fn a_missing_source_is_reported_rather_than_rendered_as_silence() {
        let directory = tempfile::tempdir().unwrap();
        let project_path = directory.path().join("mix.jutsu-audio.json");
        let source = directory.path().join("never-written.wav");

        let mut cache = DecodeCache::default();
        let result = mixdown(
            MixRequest {
                id: 1,
                sample_rate: 48_000,
                project: Box::new(project_with_clip(&project_path, &source)),
                project_path,
            },
            &mut cache,
        );
        let JobResult::Mixdown { result, .. } = result else {
            panic!("a broken source is a mixdown failure, not an empty timeline");
        };
        assert!(result.is_err());
    }

    #[test]
    fn an_empty_timeline_reports_nothing_to_play_rather_than_an_error() {
        let mut cache = DecodeCache::default();
        assert!(matches!(
            mixdown(
                MixRequest {
                    id: 9,
                    sample_rate: 48_000,
                    project: Box::new(ProjectStore::new_project("Empty")),
                    project_path: PathBuf::from("empty.jutsu-audio.json"),
                },
                &mut cache,
            ),
            JobResult::MixdownEmpty { id: 9 }
        ));
    }

    #[test]
    fn decode_cache_evicts_the_least_recently_used_entry_first() {
        let mut cache = DecodeCache::default();
        // Fill past the budget with synthetic entries, oldest last.
        for index in 0..3 {
            let samples: Arc<[f32]> = Arc::from(vec![0.0; CACHE_SAMPLE_BUDGET / 2]);
            cache.samples_held += samples.len();
            cache.entries.push_front((
                PathBuf::from(format!("{index}.wav")),
                AudioMetadata {
                    sample_rate: 48_000,
                    channels: 1,
                    frame_count: samples.len() as u64,
                    bits_per_sample: 32,
                    sample_format: "float".into(),
                },
                samples,
            ));
        }
        while cache.samples_held > CACHE_SAMPLE_BUDGET && cache.entries.len() > 1 {
            if let Some((.., evicted)) = cache.entries.pop_back() {
                cache.samples_held = cache.samples_held.saturating_sub(evicted.len());
            }
        }

        let held: Vec<_> = cache
            .entries
            .iter()
            .map(|(path, ..)| path.to_string_lossy().into_owned())
            .collect();
        assert_eq!(held, vec!["2.wav".to_owned(), "1.wav".to_owned()]);
    }

    #[test]
    fn managed_assets_resolve_next_to_the_project_file() {
        let resolved = resolve_asset_path(
            Path::new("/projects/demo/demo.jutsu-audio.json"),
            &AudioAssetSource::ManagedFile {
                path: "assets/abc.wav".into(),
                fingerprint: "abc".into(),
                sample_rate: 48_000,
                channels: 2,
                frame_count: 10,
            },
        )
        .unwrap();
        assert!(resolved.ends_with("assets/abc.wav"), "got {resolved:?}");
    }

    #[test]
    fn generated_assets_have_no_file_to_resolve() {
        assert!(
            resolve_asset_path(
                Path::new("/projects/demo/demo.json"),
                &AudioAssetSource::Generated {
                    parameters: std::collections::BTreeMap::new(),
                    generator_type: "noise".into(),
                    algorithm_version: 1,
                    seed: 7,
                },
            )
            .is_none()
        );
    }
}
