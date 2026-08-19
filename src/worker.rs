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
use jutsu_audio_engine::{ExportEncoding, ExportRange, OfflineExporter, PlaybackSnapshot};
use jutsu_audio_model::{AssetId, AudioAssetSource, Project};
use jutsu_audio_project::{
    AssetManager, AudioMetadata, CachedWaveform, ImportMode, ImportStatus, ProjectStore,
};

/// One clip flattened into everything the mixdown needs, so the worker never
/// has to reach back into the project.
#[derive(Clone, Debug)]
pub struct MixClip {
    pub source: PathBuf,
    pub start_frame: u64,
    pub source_start_frame: u64,
    pub duration_frames: u64,
    pub gain_db: f64,
}

#[derive(Clone, Debug)]
pub struct MixRequest {
    pub id: u64,
    pub sample_rate: u32,
    pub clips: Vec<MixClip>,
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
    Export { snapshot: Arc<PlaybackSnapshot> },
    /// Load peaks for one asset, rebuilding the cache if it is missing.
    Waveform {
        asset_id: AssetId,
        project_path: PathBuf,
        source: PathBuf,
        fingerprint: String,
    },
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
    },
    /// An empty timeline: nothing to play, and that is not an error.
    MixdownEmpty {
        id: u64,
    },
    Saved {
        path: PathBuf,
        result: Result<(), String>,
    },
    Opened(Box<Result<(PathBuf, Project), String>>),
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
            result: ProjectStore::save(&path, &project).map_err(|error| error.message),
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
            Some(path) => JobResult::Opened(Box::new(
                ProjectStore::open(&path)
                    .map(|opened| (path, opened.project))
                    .map_err(|error| error.message),
            )),
            None => JobResult::Cancelled,
        },
        Job::Import {
            project,
            project_path,
        } => import(*project, project_path),
        Job::Export { snapshot } => match pick_export_destination() {
            Some(path) => JobResult::Exported(
                OfflineExporter::export_wav(
                    snapshot,
                    path,
                    ExportRange::full(),
                    ExportEncoding::Pcm16,
                )
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

// ─── mixdown ────────────────────────────────────────────────────────────────

/// Sums every clip into one interleaved stereo buffer at the project rate.
/// This is what Play and Export both consume, so the two cannot disagree.
fn mixdown(request: MixRequest, cache: &mut DecodeCache) -> JobResult {
    const CHANNELS: usize = 2;

    let total_frames = request
        .clips
        .iter()
        .map(|clip| clip.start_frame.saturating_add(clip.duration_frames))
        .max()
        .unwrap_or(0);
    if total_frames == 0 {
        return JobResult::MixdownEmpty { id: request.id };
    }
    let Ok(total_frames) = usize::try_from(total_frames) else {
        return JobResult::Mixdown {
            id: request.id,
            result: Err("timeline is longer than this machine can render".into()),
        };
    };

    let mut mix = vec![0.0_f32; total_frames * CHANNELS];
    for clip in &request.clips {
        let (metadata, samples) = match cache.get(&clip.source) {
            Ok(entry) => entry,
            Err(error) => {
                return JobResult::Mixdown {
                    id: request.id,
                    result: Err(error),
                };
            }
        };
        let source_channels = usize::from(metadata.channels);
        if source_channels == 0 || samples.is_empty() {
            continue;
        }
        let source_frames = samples.len() / source_channels;
        // How far the read head moves through the source per project frame.
        let step = f64::from(metadata.sample_rate) / f64::from(request.sample_rate.max(1));
        let gain = 10_f32.powf(clip.gain_db as f32 / 20.0);

        for offset in 0..clip.duration_frames {
            let Ok(destination) = usize::try_from(clip.start_frame + offset) else {
                break;
            };
            if destination >= total_frames {
                break;
            }
            let read = clip.source_start_frame as f64 + offset as f64 * step;
            let index = read.floor();
            if index < 0.0 {
                continue;
            }
            let index = index as usize;
            if index >= source_frames {
                break;
            }
            let next = (index + 1).min(source_frames - 1);
            let blend = (read - read.floor()) as f32;
            let base = index * source_channels;
            let next_base = next * source_channels;

            for channel in 0..CHANNELS {
                let source = channel % source_channels;
                let current = samples[base + source];
                let upcoming = samples[next_base + source];
                mix[destination * CHANNELS + channel] +=
                    (current + (upcoming - current) * blend) * gain;
            }
        }
    }

    JobResult::Mixdown {
        id: request.id,
        result: PlaybackSnapshot::new(request.sample_rate, CHANNELS as u16, Arc::from(mix))
            .map(Arc::new)
            .map_err(|error| error.message),
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
        AudioAssetSource::Generated { .. } => return None,
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

    fn render(request: MixRequest) -> Vec<f32> {
        let mut cache = DecodeCache::default();
        match mixdown(request, &mut cache) {
            JobResult::Mixdown { result, .. } => result.unwrap().samples().to_vec(),
            JobResult::MixdownEmpty { .. } => Vec::new(),
            _ => panic!("mixdown returns a mixdown result"),
        }
    }

    #[test]
    fn a_clip_lands_at_its_start_frame_and_fans_out_to_stereo() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("tone.wav");
        write_mono_wav(&source, 48_000, &[1.0, 0.5, -0.5, -1.0]);

        let mix = render(MixRequest {
            id: 1,
            sample_rate: 48_000,
            clips: vec![MixClip {
                source,
                start_frame: 2,
                source_start_frame: 0,
                duration_frames: 4,
                gain_db: 0.0,
            }],
        });

        // Two silent stereo frames, then the source in both channels.
        assert_eq!(
            mix,
            vec![
                0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.5, 0.5, -0.5, -0.5, -1.0, -1.0
            ]
        );
    }

    #[test]
    fn overlapping_clips_sum_and_gain_is_applied_per_clip() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("tone.wav");
        write_mono_wav(&source, 48_000, &[1.0, 1.0]);

        let quiet = MixClip {
            source: source.clone(),
            start_frame: 0,
            source_start_frame: 0,
            duration_frames: 2,
            // -6.0206 dB is exactly half amplitude.
            gain_db: -6.020_6,
        };
        let mix = render(MixRequest {
            id: 1,
            sample_rate: 48_000,
            clips: vec![
                quiet.clone(),
                MixClip {
                    start_frame: 1,
                    ..quiet
                },
            ],
        });

        assert_eq!(mix.len(), 6);
        for (index, expected) in [0.5, 0.5, 1.0, 1.0, 0.5, 0.5].into_iter().enumerate() {
            assert!(
                (mix[index] - expected).abs() < 1e-4,
                "sample {index}: expected {expected}, got {}",
                mix[index]
            );
        }
    }

    #[test]
    fn material_at_another_rate_is_resampled_onto_the_project_rate() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("half-rate.wav");
        // 24 kHz material in a 48 kHz project plays back over twice as many frames.
        write_mono_wav(&source, 24_000, &[0.0, 1.0]);

        let mix = render(MixRequest {
            id: 1,
            sample_rate: 48_000,
            clips: vec![MixClip {
                source,
                start_frame: 0,
                source_start_frame: 0,
                duration_frames: 3,
                gain_db: 0.0,
            }],
        });

        // Left channel only; the right mirrors it.
        let left: Vec<f32> = mix.iter().step_by(2).copied().collect();
        assert_eq!(left, vec![0.0, 0.5, 1.0]);
    }

    #[test]
    fn an_empty_timeline_reports_nothing_to_play_rather_than_an_error() {
        let mut cache = DecodeCache::default();
        assert!(matches!(
            mixdown(
                MixRequest {
                    id: 9,
                    sample_rate: 48_000,
                    clips: Vec::new(),
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
                    generator_type: "noise".into(),
                    algorithm_version: 1,
                    seed: 7,
                },
            )
            .is_none()
        );
    }
}
