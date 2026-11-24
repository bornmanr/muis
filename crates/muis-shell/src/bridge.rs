//! Shell ↔ worker bridge.
//!
//! The pool lives in `muis-daemon`; this module only supplies the environment
//! every pty must inherit and re-exports the types the shell registers with
//! Tauri.

pub use muis_daemon::{EventSink, WorkerEnv, WorkerLaunch, WorkerPool, WORKER_EVENT};

use std::path::PathBuf;

/// `MUIS_AGENT_BIN` for the pty env when the sibling helper exists.
fn agent_bin() -> Option<PathBuf> {
    let path = std::env::current_exe().ok()?.parent()?.join("muis-agent");
    path.is_file().then_some(path)
}

/// Environment every worker inherits in the native shell.
pub fn env_for_shell() -> WorkerEnv {
    WorkerEnv {
        socket: Some(crate::notify_socket_name()),
        agent_bin: agent_bin(),
    }
}
