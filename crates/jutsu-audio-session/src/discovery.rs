//! How a client finds the process that currently owns a project.
//!
//! The owner publishes a small sidecar file next to the project file. Anyone
//! who can read that file is on the same machine and in the same user account,
//! which is exactly the trust boundary the token stands for.

use std::fs;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::protocol::SESSION_PROTOCOL_VERSION;

/// Sidecar suffix. Appended to the whole project file name, so
/// `song.jutsu-audio.json` publishes `song.jutsu-audio.json.session`.
pub const SESSION_FILE_SUFFIX: &str = ".session";

/// Where the owner of `project_path` publishes its endpoint.
#[must_use]
pub fn session_file_path(project_path: impl AsRef<Path>) -> PathBuf {
    sidecar_path(project_path.as_ref(), SESSION_FILE_SUFFIX)
}

/// Builds a sidecar path by appending to the full file name rather than
/// replacing an extension — project files carry two dots already.
pub(crate) fn sidecar_path(project_path: &Path, suffix: &str) -> PathBuf {
    let mut name = project_path
        .file_name()
        .unwrap_or_else(|| "project".as_ref())
        .to_os_string();
    name.push(suffix);
    project_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(name)
}

/// What the owner advertises about itself.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SessionDescriptor {
    pub protocol_version: u32,
    /// Loopback port. The listener never binds a routable interface.
    pub port: u16,
    /// Shared secret every request must echo back.
    pub token: String,
    pub project_path: PathBuf,
    /// Owner process ID. Diagnostics only — liveness is decided by whether the
    /// port still answers, which needs no platform-specific process probing.
    pub process_id: u32,
}

impl SessionDescriptor {
    #[must_use]
    pub fn new(project_path: impl Into<PathBuf>, port: u16, token: impl Into<String>) -> Self {
        Self {
            protocol_version: SESSION_PROTOCOL_VERSION,
            port,
            token: token.into(),
            project_path: project_path.into(),
            process_id: std::process::id(),
        }
    }

    /// The loopback address a client should dial.
    #[must_use]
    pub const fn address(&self) -> SocketAddr {
        SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), self.port)
    }

    /// Reads the descriptor published for `project_path`. A missing, truncated
    /// or unreadable file simply means "no live session".
    #[must_use]
    pub fn read(project_path: impl AsRef<Path>) -> Option<Self> {
        let contents = fs::read(session_file_path(project_path)).ok()?;
        serde_json::from_slice(&contents).ok()
    }

    /// Publishes this descriptor. The returned guard removes the file when the
    /// owner drops it, so a clean exit leaves no stale session behind.
    pub fn publish(self) -> io::Result<PublishedSession> {
        let path = session_file_path(&self.project_path);
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let mut encoded = serde_json::to_vec_pretty(&self)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        encoded.push(b'\n');
        fs::write(&path, &encoded)?;
        Ok(PublishedSession {
            path,
            descriptor: self,
        })
    }
}

/// Removes a session file whose owner is gone. Safe to call when the file was
/// already removed; only a real removal failure is reported.
pub fn clear_session_file(project_path: impl AsRef<Path>) -> io::Result<()> {
    match fs::remove_file(session_file_path(project_path)) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// A fresh shared secret for one session.
#[must_use]
pub fn new_token() -> String {
    Uuid::new_v4().to_string()
}

/// Owns the published session file for as long as the session runs.
#[derive(Debug)]
pub struct PublishedSession {
    path: PathBuf,
    descriptor: SessionDescriptor,
}

impl PublishedSession {
    #[must_use]
    pub const fn descriptor(&self) -> &SessionDescriptor {
        &self.descriptor
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PublishedSession {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sidecar_sits_beside_the_project_and_keeps_its_whole_name() {
        let path = session_file_path(Path::new("/projects/song.jutsu-audio.json"));
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("song.jutsu-audio.json.session")
        );
    }

    #[test]
    fn publishing_writes_a_descriptor_a_client_can_read_back() {
        let directory = tempfile::tempdir().expect("temp dir");
        let project = directory.path().join("song.jutsu-audio.json");
        let published = SessionDescriptor::new(&project, 40_000, "secret")
            .publish()
            .expect("publish");

        let read = SessionDescriptor::read(&project).expect("descriptor");
        assert_eq!(read.port, 40_000);
        assert_eq!(read.token, "secret");
        assert_eq!(read.protocol_version, SESSION_PROTOCOL_VERSION);
        assert_eq!(read.address().port(), 40_000);
        assert!(read.address().ip().is_loopback());

        drop(published);
        assert!(
            SessionDescriptor::read(&project).is_none(),
            "a clean exit leaves no session file"
        );
    }

    #[test]
    fn an_unreadable_session_file_reads_as_no_session() {
        let directory = tempfile::tempdir().expect("temp dir");
        let project = directory.path().join("song.jutsu-audio.json");
        fs::write(session_file_path(&project), b"{ truncated").expect("write");
        assert!(SessionDescriptor::read(&project).is_none());
        clear_session_file(&project).expect("clear");
        clear_session_file(&project).expect("clearing twice is not an error");
    }
}
