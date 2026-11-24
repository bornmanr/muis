//! Where the shell puts its workers: the daemon, or the in-process pool.
//!
//! The daemon is preferred so terminals survive closing the window. If it
//! cannot be reached or started (older install, missing sibling binary), the
//! shell falls back to the in-process pool from `muis-daemon`, which is the
//! pre-daemon behaviour. That fallback is what keeps a failed daemon from
//! taking the terminal with it.

use muis_daemon::protocol::{Reply, Request};
use muis_daemon::{default_socket, EventSink, SessionEvent, WorkerEnv, WorkerLaunch, WorkerPool};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The shell's worker backend. Same three operations either way, so the
/// Tauri commands do not care which one is live.
pub enum Backend {
    Daemon(DaemonClient),
    Local(WorkerPool),
}

impl Backend {
    /// Prefer the daemon; fall back to the in-process pool.
    pub fn connect(sink: EventSink, worker: Option<PathBuf>, env: WorkerEnv) -> Self {
        match DaemonClient::connect(&sink, worker.as_deref(), &env) {
            Some(client) => {
                log::info!("using muis-daemon for worker ownership");
                Self::Daemon(client)
            }
            None => {
                log::warn!("muis-daemon unavailable; workers stay in this process");
                Self::Local(WorkerPool::new(sink, worker.map(WorkerLaunch), env))
            }
        }
    }

    pub fn spawn_session(&self, session: &str) -> Result<(), String> {
        match self {
            Self::Daemon(client) => client.request(&Request::Spawn {
                session: session.to_string(),
            }),
            Self::Local(pool) => pool.spawn_session(session),
        }
    }

    pub fn send_line(&self, session: &str, line: &str) -> Result<(), String> {
        match self {
            Self::Daemon(client) => client.request(&Request::Send {
                session: session.to_string(),
                line: line.to_string(),
            }),
            Self::Local(pool) => pool.send_line(session, line),
        }
    }

    pub fn stop_session(&self, session: &str) {
        match self {
            Self::Daemon(client) => {
                let _ = client.request(&Request::Stop {
                    session: session.to_string(),
                });
            }
            Self::Local(pool) => pool.stop_session(session),
        }
    }
}

/// A connection to the running daemon. Requests are serialized by the mutex;
/// replies arrive on the reader thread and go straight to the event sink.
pub struct DaemonClient {
    stream: Mutex<UnixStream>,
    /// Session of the request in flight. A daemon-side failure has no
    /// session of its own, so it is reported against this one instead of
    /// disappearing into the log.
    pending: Arc<Mutex<Option<String>>>,
}

impl DaemonClient {
    fn connect(sink: &EventSink, worker: Option<&Path>, env: &WorkerEnv) -> Option<Self> {
        let socket = default_socket();
        if UnixStream::connect(&socket).is_err() {
            let worker = worker?;
            spawn_daemon(&socket, worker, env)?;
        }
        let stream = connect_with_retry(&socket)?;
        let reader = stream.try_clone().ok()?;
        let sink = Arc::clone(sink);
        let pending = Arc::new(Mutex::new(None::<String>));
        let pending_reader = Arc::clone(&pending);
        std::thread::spawn(move || {
            let pending = pending_reader;
            for line in BufReader::new(reader).lines().map_while(Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Reply>(&line) {
                    Ok(Reply::Event { session_id, frame }) => {
                        if !sink(SessionEvent { session_id, frame }) {
                            break;
                        }
                    }
                    Ok(Reply::Error { message }) => {
                        log::warn!("muis-daemon: {message}");
                        let session_id = pending.lock().unwrap().take().unwrap_or_default();
                        if !session_id.is_empty() {
                            sink(SessionEvent {
                                session_id,
                                frame: muis_core::ipc::WorkerToUi::Error {
                                    pty_id: String::new(),
                                    message,
                                },
                            });
                        }
                    }
                    Ok(Reply::Status { .. }) => {}
                    Err(e) => log::debug!("muis-daemon: undecodable reply: {e}"),
                }
            }
            log::warn!("muis-daemon connection ended");
        });
        Some(Self {
            stream: Mutex::new(stream),
            pending,
        })
    }

    fn request(&self, request: &Request) -> Result<(), String> {
        let session = match request {
            Request::Spawn { session } | Request::Send { session, .. } => Some(session.clone()),
            Request::Stop { session } => Some(session.clone()),
            Request::Status => None,
        };
        *self.pending.lock().unwrap() = session;
        let line = serde_json::to_string(request).map_err(|e| e.to_string())?;
        let mut stream = self.stream.lock().unwrap();
        stream
            .write_all(format!("{line}\n").as_bytes())
            .and_then(|()| stream.flush())
            .map_err(|e| format!("muis-daemon write failed: {e}"))
    }
}

/// Start the daemon next to the shell. `MUIS_SOCKET` rides along so every
/// pty the daemon spawns keeps reporting to the same notification socket.
fn spawn_daemon(socket: &Path, worker: &Path, env: &WorkerEnv) -> Option<()> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let daemon = find_sibling(dir, "muis-daemon")?;
    let mut command = std::process::Command::new(&daemon);
    command
        .arg("--socket")
        .arg(socket)
        .arg("--worker")
        .arg(worker)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    if let Some(notify) = &env.socket {
        command.env("MUIS_SOCKET", notify);
    }
    match command.spawn() {
        Ok(child) => {
            log::info!("started {} ({})", daemon.display(), child.id());
            Some(())
        }
        Err(e) => {
            log::warn!("cannot start {}: {e}", daemon.display());
            None
        }
    }
}

/// `muis-daemon`, or its triple-suffixed sidecar name.
fn find_sibling(dir: &Path, name: &str) -> Option<PathBuf> {
    let plain = dir.join(name);
    if plain.is_file() {
        return Some(plain);
    }
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|f| f.to_str())
                .is_some_and(|f| f.starts_with(&format!("{name}-")))
        })
}

fn connect_with_retry(socket: &Path) -> Option<UnixStream> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(stream) = UnixStream::connect(socket) {
            return Some(stream);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
