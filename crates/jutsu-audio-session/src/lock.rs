//! Exclusive write lock for editing a project with no live session.
//!
//! Only holders of this lock may write the project file. The GUI takes it for
//! the lifetime of the session; an offline CLI edit takes it for the length of
//! one command batch.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::discovery::sidecar_path;

/// Sidecar suffix for the write lock.
pub const LOCK_FILE_SUFFIX: &str = ".lock";

/// A lock this old is assumed to belong to a process that died without
/// cleaning up. Well above any single command batch.
// ponytail: age-based staleness, not process liveness. A batch that legitimately
// runs longer than this would be broken into; move to a per-platform process
// probe if that ever becomes reachable.
pub const STALE_AFTER: Duration = Duration::from_secs(60);

/// How often `acquire_within` retries while waiting for a busy lock.
const RETRY_INTERVAL: Duration = Duration::from_millis(25);

#[must_use]
pub fn lock_file_path(project_path: impl AsRef<Path>) -> PathBuf {
    sidecar_path(project_path.as_ref(), LOCK_FILE_SUFFIX)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockErrorCode {
    /// Another live process holds the lock.
    Busy,
    /// The lock file itself could not be read or written.
    Io,
}

#[derive(Debug)]
pub struct LockError {
    pub code: LockErrorCode,
    pub path: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct LockRecord {
    process_id: u32,
    /// Written by the holder rather than read from the file system, so a copied
    /// or restored project directory cannot look freshly locked.
    acquired_unix_millis: u128,
}

/// Held for as long as the owner may write the project. Dropping releases it.
#[derive(Debug)]
pub struct ProjectLock {
    path: PathBuf,
}

impl ProjectLock {
    /// Takes the lock or fails immediately.
    pub fn acquire(project_path: impl AsRef<Path>) -> Result<Self, LockError> {
        Self::acquire_within(project_path, Duration::ZERO)
    }

    /// Takes the lock, waiting up to `wait` for a busy holder to release it.
    /// A lock older than [`STALE_AFTER`] is broken rather than waited on.
    pub fn acquire_within(
        project_path: impl AsRef<Path>,
        wait: Duration,
    ) -> Result<Self, LockError> {
        let path = lock_file_path(project_path);
        let deadline = SystemTime::now() + wait;
        loop {
            match try_create(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if is_stale(&path) {
                        // Break it and try once more; a live holder that races
                        // us here simply wins the next `try_create`.
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    if SystemTime::now() >= deadline {
                        return Err(LockError {
                            code: LockErrorCode::Busy,
                            path,
                            message: "another process is editing this project".into(),
                        });
                    }
                    std::thread::sleep(RETRY_INTERVAL);
                }
                Err(error) => {
                    return Err(LockError {
                        code: LockErrorCode::Io,
                        message: format!("failed to take project lock: {error}"),
                        path,
                    });
                }
            }
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ProjectLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn try_create(path: &Path) -> io::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let record = LockRecord {
        process_id: std::process::id(),
        acquired_unix_millis: unix_millis(SystemTime::now()),
    };
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let encoded = serde_json::to_vec(&record)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    file.write_all(&encoded)?;
    file.flush()
}

/// A lock file we cannot parse is treated as stale: it can only come from a
/// crash mid-write, and refusing forever would strand the project.
fn is_stale(path: &Path) -> bool {
    let Ok(contents) = fs::read(path) else {
        return false;
    };
    let Ok(record) = serde_json::from_slice::<LockRecord>(&contents) else {
        return true;
    };
    let now = unix_millis(SystemTime::now());
    now.saturating_sub(record.acquired_unix_millis) > STALE_AFTER.as_millis()
}

fn unix_millis(time: SystemTime) -> u128 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_writer_is_refused_while_the_first_holds_the_lock() {
        let directory = tempfile::tempdir().expect("temp dir");
        let project = directory.path().join("song.jutsu-audio.json");

        let held = ProjectLock::acquire(&project).expect("first lock");
        let refused = ProjectLock::acquire(&project).expect_err("second lock");
        assert_eq!(refused.code, LockErrorCode::Busy);

        drop(held);
        ProjectLock::acquire(&project).expect("lock is free again");
    }

    #[test]
    fn a_lock_left_behind_by_a_dead_process_is_broken_rather_than_honoured() {
        let directory = tempfile::tempdir().expect("temp dir");
        let project = directory.path().join("song.jutsu-audio.json");
        fs::write(
            lock_file_path(&project),
            serde_json::to_vec(&LockRecord {
                process_id: 1,
                acquired_unix_millis: 0,
            })
            .expect("encode"),
        )
        .expect("write stale lock");

        ProjectLock::acquire(&project).expect("stale lock is broken");
    }

    #[test]
    fn a_corrupt_lock_file_does_not_strand_the_project() {
        let directory = tempfile::tempdir().expect("temp dir");
        let project = directory.path().join("song.jutsu-audio.json");
        fs::write(lock_file_path(&project), b"{ truncated").expect("write");

        ProjectLock::acquire(&project).expect("corrupt lock is broken");
    }
}
