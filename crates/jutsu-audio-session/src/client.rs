//! The dialling side: attach to whichever process owns a project, or find out
//! that nobody does.

use std::io::BufReader;
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use crate::discovery::{SessionDescriptor, clear_session_file};
use crate::protocol::{
    RequestPayload, SESSION_PROTOCOL_VERSION, SessionRequest, SessionResponse, read_frame,
    write_frame,
};

/// A session that does not answer within this is treated as dead. Loopback
/// connects in microseconds when anyone is listening.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientErrorCode {
    /// A session owns the project but speaks a different protocol version.
    IncompatibleSession,
    /// The connection failed or ended mid-exchange.
    Transport,
}

#[derive(Debug)]
pub struct ClientError {
    pub code: ClientErrorCode,
    pub message: String,
}

impl ClientError {
    fn transport(message: impl Into<String>) -> Self {
        Self {
            code: ClientErrorCode::Transport,
            message: message.into(),
        }
    }
}

/// One connection to a session owner. Requests are answered in order.
pub struct SessionClient {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    token: String,
}

impl SessionClient {
    /// Connects to the session owning `project_path`.
    ///
    /// `Ok(None)` means nobody owns it — either no session file, or one whose
    /// port no longer answers, in which case the stale file is removed so the
    /// caller can take the offline write lock instead.
    pub fn attach(project_path: impl AsRef<Path>) -> Result<Option<Self>, ClientError> {
        let project_path = project_path.as_ref();
        let Some(descriptor) = SessionDescriptor::read(project_path) else {
            return Ok(None);
        };
        if descriptor.protocol_version != SESSION_PROTOCOL_VERSION {
            return Err(ClientError {
                code: ClientErrorCode::IncompatibleSession,
                message: format!(
                    "the open session speaks protocol version {}; this build speaks {SESSION_PROTOCOL_VERSION}",
                    descriptor.protocol_version
                ),
            });
        }
        let Ok(stream) = TcpStream::connect_timeout(&descriptor.address(), CONNECT_TIMEOUT) else {
            // The owner is gone. Clean up after it rather than refusing to work.
            let _ = clear_session_file(project_path);
            return Ok(None);
        };
        let reader = stream
            .try_clone()
            .map_err(|error| ClientError::transport(error.to_string()))?;
        Ok(Some(Self {
            reader: BufReader::new(reader),
            writer: stream,
            token: descriptor.token,
        }))
    }

    /// Sends one payload under a fresh request ID and waits for its answer.
    pub fn request(&mut self, payload: RequestPayload) -> Result<SessionResponse, ClientError> {
        let request = SessionRequest::new(self.token.clone(), payload);
        self.send(&request)
    }

    /// Sends a request verbatim. Re-sending one that was already answered
    /// returns the original answer rather than applying it twice.
    pub fn send(&mut self, request: &SessionRequest) -> Result<SessionResponse, ClientError> {
        write_frame(&mut self.writer, request)
            .map_err(|error| ClientError::transport(error.message))?;
        match read_frame::<SessionResponse>(&mut self.reader) {
            Ok(Some(response)) => Ok(response),
            Ok(None) => Err(ClientError::transport(
                "the session closed before answering",
            )),
            Err(error) => Err(ClientError::transport(error.message)),
        }
    }

    /// The session's shared secret, for building requests by hand.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }
}
