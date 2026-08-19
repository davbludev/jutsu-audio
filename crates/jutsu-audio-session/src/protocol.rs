//! The wire contract between a live editor session and its clients.
//!
//! One JSON object per line in each direction, so a client can be written in
//! any language with a socket and a JSON parser. Framing is deliberately not
//! length-prefixed: newline-delimited JSON stays greppable in a capture.

use std::fmt;
use std::io::{BufRead, Write};

use jutsu_audio_commands::{ChangeEvent, ProjectCommand};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Version of the session wire contract. Bumped whenever an existing field
/// changes meaning; additive optional fields do not bump it.
pub const SESSION_PROTOCOL_VERSION: u32 = 1;

/// Caller-generated correlation ID. The server answers with the same ID, and
/// replaying a request ID returns the first response rather than re-applying.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RequestId(Uuid);

impl RequestId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    /// The raw UUID, so an owner can correlate a request with the command
    /// envelope it produced.
    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// One request from a client to the session owner.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SessionRequest {
    pub protocol_version: u32,
    /// Copied from the session file. Proves the client can read a file only the
    /// owning user account can read — the whole authentication boundary.
    pub token: String,
    pub request_id: RequestId,
    pub payload: RequestPayload,
}

impl SessionRequest {
    #[must_use]
    pub fn new(token: impl Into<String>, payload: RequestPayload) -> Self {
        Self {
            protocol_version: SESSION_PROTOCOL_VERSION,
            token: token.into(),
            request_id: RequestId::new(),
            payload,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RequestPayload {
    /// Read-only: who owns the project and where its revision stands.
    Status,
    /// Apply a command batch through the owner's command engine.
    Apply {
        /// `None` means "apply against whatever the current revision is".
        /// A number is an optimistic-concurrency precondition.
        #[serde(default)]
        expected_revision: Option<u64>,
        commands: Vec<ProjectCommand>,
    },
    /// Drive the owner's transport.
    Transport {
        action: TransportAction,
        #[serde(default)]
        position_frames: u64,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportAction {
    Play,
    Pause,
    Stop,
    Seek,
}

/// One response from the session owner. `status` discriminates, so a client can
/// branch before deserializing the rest.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SessionResponse {
    Ok {
        protocol_version: u32,
        request_id: RequestId,
        payload: ResponsePayload,
    },
    Error {
        protocol_version: u32,
        request_id: RequestId,
        error: SessionError,
    },
}

impl SessionResponse {
    #[must_use]
    pub const fn ok(request_id: RequestId, payload: ResponsePayload) -> Self {
        Self::Ok {
            protocol_version: SESSION_PROTOCOL_VERSION,
            request_id,
            payload,
        }
    }

    #[must_use]
    pub const fn failed(request_id: RequestId, error: SessionError) -> Self {
        Self::Error {
            protocol_version: SESSION_PROTOCOL_VERSION,
            request_id,
            error,
        }
    }

    #[must_use]
    pub const fn request_id(&self) -> RequestId {
        match self {
            Self::Ok { request_id, .. } | Self::Error { request_id, .. } => *request_id,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponsePayload {
    Status {
        project_path: Option<String>,
        project_name: String,
        revision: u64,
        unsaved: bool,
    },
    Applied {
        revision: u64,
        changes: Vec<ChangeEvent>,
        /// True when this response was replayed from the idempotency cache
        /// instead of applied again.
        replayed: bool,
    },
    TransportAck {
        action: TransportAction,
        position_frames: u64,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionErrorCode {
    UnsupportedProtocolVersion,
    Unauthorized,
    MalformedRequest,
    RevisionConflict,
    CommandFailed,
    SessionClosed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SessionError {
    pub code: SessionErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_revision: Option<u64>,
}

impl SessionError {
    #[must_use]
    pub fn new(code: SessionErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            expected_revision: None,
            actual_revision: None,
        }
    }
}

/// Framing errors are separate from protocol errors: a broken pipe is not
/// something the peer can answer.
#[derive(Debug)]
pub struct FrameError {
    pub message: String,
}

impl fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl From<std::io::Error> for FrameError {
    fn from(error: std::io::Error) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

/// Writes one JSON value followed by a newline and flushes it. Callers must
/// flush per message; the peer is blocked on the newline.
pub fn write_frame<T: Serialize>(writer: &mut impl Write, value: &T) -> Result<(), FrameError> {
    let mut encoded = serde_json::to_vec(value).map_err(|error| FrameError {
        message: format!("frame cannot be serialized: {error}"),
    })?;
    encoded.push(b'\n');
    writer.write_all(&encoded)?;
    writer.flush()?;
    Ok(())
}

/// Reads one newline-delimited JSON value. `Ok(None)` means the peer hung up
/// cleanly between frames.
pub fn read_frame<T: for<'de> Deserialize<'de>>(
    reader: &mut impl BufRead,
) -> Result<Option<T>, FrameError> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    if line.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&line)
        .map(Some)
        .map_err(|error| FrameError {
            message: format!("frame is not valid session JSON: {error}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_survives_a_round_trip_through_the_wire_format() {
        let request = SessionRequest::new("token", RequestPayload::Status);
        let mut buffer = Vec::new();
        write_frame(&mut buffer, &request).expect("write");
        assert!(buffer.ends_with(b"\n"), "frames are newline delimited");

        let mut reader = std::io::BufReader::new(buffer.as_slice());
        let decoded: SessionRequest = read_frame(&mut reader).expect("read").expect("one frame");
        assert_eq!(decoded, request);
    }

    #[test]
    fn a_closed_stream_reads_as_no_frame_rather_than_an_error() {
        let mut reader = std::io::BufReader::new(&b""[..]);
        let decoded: Option<SessionRequest> = read_frame(&mut reader).expect("read");
        assert!(decoded.is_none());
    }

    #[test]
    fn garbage_on_the_wire_is_reported_as_a_frame_error() {
        let mut reader = std::io::BufReader::new(&b"not json\n"[..]);
        let decoded: Result<Option<SessionRequest>, _> = read_frame(&mut reader);
        assert!(decoded.is_err());
    }
}
