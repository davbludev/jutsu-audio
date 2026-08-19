//! The listener side: the process that owns a project accepts requests here
//! and answers them from its own command engine.
//!
//! The server does the parts that are the same for every owner — binding
//! loopback, publishing the session file, checking the protocol version and
//! token, replaying duplicate request IDs — and hands whatever is left to the
//! owner as a [`SessionCall`]. It never touches a `Project`.

use std::collections::VecDeque;
use std::io::{self, BufReader};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::discovery::{PublishedSession, SessionDescriptor, new_token};
use crate::protocol::{
    RequestId, SESSION_PROTOCOL_VERSION, SessionError, SessionErrorCode, SessionRequest,
    SessionResponse, read_frame, write_frame,
};

/// How long a connection waits for the owner to answer before giving up. Well
/// beyond a UI frame, short enough that a wedged owner does not hang a script.
const OWNER_TIMEOUT: Duration = Duration::from_secs(5);

/// How many recent request IDs are remembered for replay.
const REPLAY_HISTORY: usize = 64;

/// A request waiting for the owner. Dropping it without answering closes the
/// connection with a `session_closed` error, so a client is never left hanging.
#[derive(Debug)]
pub struct SessionCall {
    request: SessionRequest,
    reply: SyncSender<SessionResponse>,
}

impl SessionCall {
    #[must_use]
    pub const fn request(&self) -> &SessionRequest {
        &self.request
    }

    /// Answers the caller. Consumes the call, so exactly one answer is possible.
    pub fn respond(self, response: SessionResponse) {
        let _ = self.reply.send(response);
    }
}

/// A live session: a bound loopback port, a published session file, and a queue
/// of calls for the owner to drain.
pub struct SessionServer {
    published: PublishedSession,
    calls: Receiver<SessionCall>,
    stop: Arc<AtomicBool>,
    port: u16,
    accept: Option<JoinHandle<()>>,
}

impl SessionServer {
    /// Binds loopback, publishes the session file for `project_path`, and
    /// starts accepting. `wake` is called whenever a new call is queued, so an
    /// owner that only runs on repaint can be nudged.
    pub fn start(
        project_path: impl AsRef<Path>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let project_path: PathBuf = project_path.as_ref().to_path_buf();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        let token = new_token();
        let published = SessionDescriptor::new(&project_path, port, token.clone()).publish()?;

        let (sender, calls) = sync_channel(0);
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared {
            token,
            calls: sender,
            wake: Box::new(wake),
            // ponytail: one mutex serializes every request against the replay
            // history. A single-writer service answers one request at a time
            // anyway; split it if a reader-only payload ever needs concurrency.
            replays: Mutex::new(VecDeque::new()),
        });

        let accept = std::thread::Builder::new()
            .name("jutsu-audio-session".into())
            .spawn({
                let stop = Arc::clone(&stop);
                move || accept_loop(&listener, &shared, &stop)
            })?;

        Ok(Self {
            published,
            calls,
            stop,
            port,
            accept: Some(accept),
        })
    }

    /// Takes the next queued call, if any. Never blocks.
    #[must_use]
    pub fn try_recv(&self) -> Option<SessionCall> {
        self.calls.try_recv().ok()
    }

    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    #[must_use]
    pub fn project_path(&self) -> &Path {
        &self.published.descriptor().project_path
    }

    #[must_use]
    pub fn token(&self) -> &str {
        &self.published.descriptor().token
    }
}

impl Drop for SessionServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Unblock the accept call so the thread notices the flag. A refused
        // connection here just means it already stopped.
        let _ = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port));
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

struct Shared {
    token: String,
    calls: SyncSender<SessionCall>,
    wake: Box<dyn Fn() + Send + Sync>,
    replays: Mutex<VecDeque<(RequestId, SessionResponse)>>,
}

fn accept_loop(listener: &TcpListener, shared: &Arc<Shared>, stop: &Arc<AtomicBool>) {
    for stream in listener.incoming() {
        if stop.load(Ordering::Acquire) {
            return;
        }
        let Ok(stream) = stream else { continue };
        let shared = Arc::clone(shared);
        // One thread per connection: a client holding a socket open must not
        // stop another from being served.
        let _ = std::thread::Builder::new()
            .name("jutsu-audio-session-conn".into())
            .spawn(move || serve(&stream, &shared));
    }
}

fn serve(stream: &TcpStream, shared: &Arc<Shared>) {
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);
    let mut writer = write_half;
    loop {
        match read_frame::<SessionRequest>(&mut reader) {
            Ok(Some(request)) => {
                let response = handle(request, shared);
                if write_frame(&mut writer, &response).is_err() {
                    return;
                }
            }
            // Clean hang-up.
            Ok(None) => return,
            Err(error) => {
                let response = SessionResponse::failed(
                    RequestId::from_uuid(uuid::Uuid::nil()),
                    SessionError::new(SessionErrorCode::MalformedRequest, error.message),
                );
                let _ = write_frame(&mut writer, &response);
                return;
            }
        }
    }
}

fn handle(request: SessionRequest, shared: &Arc<Shared>) -> SessionResponse {
    let request_id = request.request_id;
    if request.protocol_version != SESSION_PROTOCOL_VERSION {
        return SessionResponse::failed(
            request_id,
            SessionError::new(
                SessionErrorCode::UnsupportedProtocolVersion,
                format!(
                    "session protocol version {} is unsupported; expected {SESSION_PROTOCOL_VERSION}",
                    request.protocol_version
                ),
            ),
        );
    }
    if request.token != shared.token {
        return SessionResponse::failed(
            request_id,
            SessionError::new(
                SessionErrorCode::Unauthorized,
                "request token does not match this session",
            ),
        );
    }

    // Held across the dispatch: the replay entry must be visible before any
    // later request with the same ID can be admitted.
    let mut replays = shared
        .replays
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, response)) = replays.iter().find(|(id, _)| *id == request_id) {
        return replayed(response.clone());
    }

    let (reply, answer) = sync_channel(1);
    if shared.calls.send(SessionCall { request, reply }).is_err() {
        return closed(request_id);
    }
    (shared.wake)();

    let response = match answer.recv_timeout(OWNER_TIMEOUT) {
        Ok(response) => response,
        Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => closed(request_id),
    };
    replays.push_back((request_id, response.clone()));
    if replays.len() > REPLAY_HISTORY {
        replays.pop_front();
    }
    response
}

fn closed(request_id: RequestId) -> SessionResponse {
    SessionResponse::failed(
        request_id,
        SessionError::new(
            SessionErrorCode::SessionClosed,
            "the session owner did not answer",
        ),
    )
}

/// Marks a cached answer as a replay so a client can tell it apart from a
/// fresh application.
fn replayed(response: SessionResponse) -> SessionResponse {
    use crate::protocol::ResponsePayload;

    match response {
        SessionResponse::Ok {
            protocol_version,
            request_id,
            payload: ResponsePayload::Applied {
                revision, changes, ..
            },
        } => SessionResponse::Ok {
            protocol_version,
            request_id,
            payload: ResponsePayload::Applied {
                revision,
                changes,
                replayed: true,
            },
        },
        other => other,
    }
}
