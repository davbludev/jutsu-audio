//! Single-writer session layer: one process owns a project at a time, and
//! everyone else either talks to that owner or waits for the write lock.
//!
//! - [`protocol`] is the wire contract (newline-delimited JSON, versioned).
//! - [`discovery`] is how a client finds the owner, and how a dead owner is
//!   detected: its endpoint stops answering.
//! - [`lock`] is the exclusive write lock used when no session is live.
//!
//! The contract is written up in
//! `docs/design/jutsu-audio-session-protocol-v1.md`.

pub mod discovery;
pub mod lock;
pub mod protocol;

pub use discovery::{PublishedSession, SessionDescriptor, clear_session_file, new_token};
pub use lock::{LockError, LockErrorCode, ProjectLock};
pub use protocol::{
    RequestId, RequestPayload, ResponsePayload, SESSION_PROTOCOL_VERSION, SessionError,
    SessionErrorCode, SessionRequest, SessionResponse, TransportAction,
};
