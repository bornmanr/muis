//! Wire protocol between a muis client (the window, the browser host, a CLI)
//! and `muis-daemon`. One JSON object per line, both directions.
//!
//! The daemon owns the workers; a client only asks it to do things and
//! receives the same `SessionEvent` shape the in-process pool used to deliver
//! directly. Presence of `type` on both enums keeps the two directions
//! unambiguous on one connection.

use muis_core::ipc::WorkerToUi;
use serde::{Deserialize, Serialize};

/// Client → daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Start the worker for a session. Idempotent, like the pool it wraps.
    Spawn { session: String },
    /// Forward one `UiToWorker` JSON line to that session's worker.
    Send { session: String, line: String },
    /// Kill the session's worker.
    Stop { session: String },
    /// Ask what the daemon currently holds.
    Status,
}

/// Daemon → client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    /// One worker frame, exactly as the frontend used to receive it.
    Event {
        session_id: String,
        frame: WorkerToUi,
    },
    Status {
        sessions: Vec<String>,
        clients: usize,
    },
    /// A request that could not be carried out.
    Error { message: String },
}

impl Reply {
    pub fn event(event: crate::SessionEvent) -> Self {
        Self::Event {
            session_id: event.session_id,
            frame: event.frame,
        }
    }
}
