//! Wire protocol between a muis client (the window, the browser host, a CLI)
//! and `muis-daemon`. One JSON object per line, both directions.
//!
//! The daemon owns the workers; a client only asks it to do things and
//! receives the same `SessionEvent` shape the in-process pool used to deliver
//! directly. Presence of `type` on both enums keeps the two directions
//! unambiguous on one connection.

use muis_core::ipc::WorkerToUi;
use serde::{Deserialize, Serialize};

/// What a connection is for. The window registers as `Ui`; everything else
/// (the control CLI, scripts) is a `Client`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Ui,
    Client,
}

/// Client → daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Announce what this connection is. The daemon forwards `ui` requests to
    /// the most recent `Ui` connection.
    Hello { role: Role },
    /// Start the worker for a session. Idempotent, like the pool it wraps.
    Spawn { session: String },
    /// Forward one `UiToWorker` JSON line to that session's worker.
    Send { session: String, line: String },
    /// Kill the session's worker.
    Stop { session: String },
    /// Ask what the daemon currently holds.
    Status,
    /// A client asking the window to do something the daemon cannot answer
    /// on its own (`capture`, `new-tab`, `focus`, ...). `id` is the client's
    /// own correlation id and is echoed back.
    Ui {
        id: u64,
        op: String,
        #[serde(default)]
        args: serde_json::Value,
    },
    /// The window's answer to a forwarded [`Reply::UiRequest`].
    UiReply {
        id: u64,
        ok: bool,
        #[serde(default)]
        result: serde_json::Value,
    },
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
    /// A client's `ui` request, forwarded to the attached window. `id` is the
    /// daemon's routing slug, not the caller's id.
    UiRequest {
        id: u64,
        op: String,
        #[serde(default)]
        args: serde_json::Value,
    },
    /// The window's answer, routed back to the caller with its own id.
    UiReply {
        id: u64,
        ok: bool,
        #[serde(default)]
        result: serde_json::Value,
    },
}

impl Reply {
    pub fn event(event: crate::SessionEvent) -> Self {
        Self::Event {
            session_id: event.session_id,
            frame: event.frame,
        }
    }
}
