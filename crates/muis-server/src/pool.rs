//! Server ↔ worker bridge.
//!
//! One `muis-worker` process per session, same model as the native shell: the
//! frontend's `UiToWorker` JSON lines go to worker stdin, worker stdout lines
//! come back through an [`EventSink`]. The difference is the launch: the
//! browser server re-execs *itself* with `--muis-worker`, so a single binary
//! is a complete terminal (no sidecar to find, stage, or sign).

use muis_core::ipc::{decode_worker, WorkerToUi};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize)]
pub struct SessionEvent {
    pub session_id: String,
    pub frame: WorkerToUi,
}

/// Delivers a worker event to the frontend. `false` stops the pump thread.
pub type EventSink = Arc<dyn Fn(SessionEvent) -> bool + Send + Sync + 'static>;

/// How to start a worker: this binary, plus the mode flag that selects the
/// embedded worker.
#[derive(Clone, Debug)]
pub struct WorkerLaunch {
    pub exe: PathBuf,
    pub args: Vec<String>,
}

impl WorkerLaunch {
    pub fn self_exe(args: Vec<String>) -> Option<Self> {
        let exe = std::env::current_exe().ok()?;
        Some(Self { exe, args })
    }
}

struct Running {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
}

/// Parse one worker stdout line into a frontend event.
pub fn parse_worker_line(session_id: &str, line: &str) -> Result<SessionEvent, String> {
    decode_worker(line)
        .map(|frame| SessionEvent {
            session_id: session_id.to_string(),
            frame,
        })
        .map_err(|e| e.to_string())
}

pub struct WorkerPool {
    sink: EventSink,
    workers: Mutex<HashMap<String, Arc<Running>>>,
    launch: WorkerLaunch,
    socket: String,
}

impl WorkerPool {
    pub fn new(sink: EventSink, launch: WorkerLaunch, socket: String) -> Self {
        Self {
            sink,
            workers: Mutex::new(HashMap::new()),
            launch,
            socket,
        }
    }

    fn dead_worker_event(session_id: &str) -> SessionEvent {
        SessionEvent {
            session_id: session_id.to_string(),
            frame: WorkerToUi::Error {
                pty_id: String::new(),
                message: "worker process ended".to_string(),
            },
        }
    }

    /// Start the worker for a session. Idempotent: already running is Ok.
    pub fn spawn_session(&self, session_id: &str) -> Result<(), String> {
        if self.workers.lock().unwrap().contains_key(session_id) {
            return Ok(());
        }

        let mut command = Command::new(&self.launch.exe);
        command
            .args(&self.launch.args)
            // muis-notify/muis-agent clients inside the pty find the server.
            .env("MUIS_SOCKET", &self.socket)
            // The opencode plugin spawns `$MUIS_BIN --muis-notify ...` when
            // there is no sidecar binary, which is the single-binary case.
            .env("MUIS_BIN", &self.launch.exe)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::platform::hide_console(&mut command);

        let mut child = command
            .spawn()
            .map_err(|e| format!("failed to start muis-worker: {e}"))?;
        let stdin = child.stdin.take().ok_or("worker stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("worker stdout unavailable")?;

        if let Some(stderr) = child.stderr.take() {
            let id = session_id.to_string();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if line.trim().is_empty() {
                        continue;
                    }
                    log::warn!("muis-worker[{id}] {line}");
                }
            });
        }

        let running = Arc::new(Running {
            stdin: Mutex::new(stdin),
            child: Mutex::new(child),
        });
        self.workers
            .lock()
            .unwrap()
            .insert(session_id.to_string(), Arc::clone(&running));

        let sink = Arc::clone(&self.sink);
        let id = session_id.to_string();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }
                match parse_worker_line(&id, &line) {
                    Ok(event) => {
                        if !sink(event) {
                            break;
                        }
                    }
                    Err(e) => log::debug!("muis-worker[{id}] undecodable frame: {e}"),
                }
            }
            log::warn!("muis-worker for session {id} ended");
            let _ = sink(Self::dead_worker_event(&id));
        });
        Ok(())
    }

    /// Forward one `UiToWorker` JSON line to the session's worker.
    pub fn send_line(&self, session_id: &str, line: &str) -> Result<(), String> {
        let workers = self.workers.lock().unwrap();
        let running = workers
            .get(session_id)
            .ok_or_else(|| format!("no worker for session {session_id}"))?;
        let mut stdin = running.stdin.lock().unwrap();
        stdin
            .write_all(line.as_bytes())
            .map_err(|e| format!("worker write failed: {e}"))?;
        if !line.ends_with('\n') {
            stdin
                .write_all(b"\n")
                .map_err(|e| format!("worker write failed: {e}"))?;
        }
        stdin
            .flush()
            .map_err(|e| format!("worker write failed: {e}"))
    }

    /// Kill one session's worker and forget it.
    pub fn stop_session(&self, session_id: &str) {
        if let Some(running) = self.workers.lock().unwrap().remove(session_id) {
            if let Ok(mut child) = running.child.lock() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    /// Tear down every worker: the browser tab that owned them is gone.
    pub fn stop_all(&self) {
        let ids: Vec<String> = self.workers.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.stop_session(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_worker_line_tags_session() {
        let e = parse_worker_line("s1", r#"{"type":"exited","pty_id":"t2","code":42}"#).unwrap();
        assert_eq!(e.session_id, "s1");
        assert_eq!(
            e.frame,
            WorkerToUi::Exited {
                pty_id: "t2".into(),
                code: Some(42),
            }
        );
    }

    #[test]
    fn parse_worker_line_rejects_garbage() {
        assert!(parse_worker_line("s1", "not json").is_err());
        assert!(parse_worker_line("s1", r#"{"type":"nope"}"#).is_err());
    }

    #[test]
    fn session_event_serializes_for_the_frontend() {
        let e = SessionEvent {
            session_id: "s1".into(),
            frame: WorkerToUi::Spawned {
                pty_id: "t1".into(),
            },
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["session_id"], "s1");
        assert_eq!(v["frame"]["type"], "spawned");
    }

    /// A worker that answers every line, so pool lifecycle is exercised
    /// without a pty. Uses `/bin/sh` for the fake worker, hence `cfg(unix)`.
    #[cfg(unix)]
    #[test]
    fn one_worker_per_session_and_stop_is_isolated() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::mpsc::channel;
        use std::time::Duration;

        let dir = std::env::temp_dir().join(format!("muis-srv-pool-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let worker = dir.join("fake-worker.sh");
        std::fs::write(
            &worker,
            "#!/bin/sh\nwhile IFS= read -r _line; do printf '{\"type\":\"spawned\",\"pty_id\":\"p1\"}\\n'; done\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&worker).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&worker, perms).unwrap();

        let (tx, rx) = channel::<SessionEvent>();
        let sink: EventSink = Arc::new(move |event| tx.send(event).is_ok());
        let pool = WorkerPool::new(
            sink,
            WorkerLaunch {
                exe: worker,
                args: Vec::new(),
            },
            "muis-test".to_string(),
        );

        pool.spawn_session("s1").unwrap();
        pool.spawn_session("s1").unwrap(); // idempotent
        pool.spawn_session("s2").unwrap();

        let write = r#"{"type":"write","pty_id":"p1","data":""}"#;
        pool.send_line("s1", write).unwrap();
        pool.send_line("s2", write).unwrap();

        let mut spawned: HashMap<String, usize> = HashMap::new();
        while spawned.values().sum::<usize>() < 2 {
            let event = rx
                .recv_timeout(Duration::from_secs(5))
                .expect("worker event");
            if matches!(event.frame, WorkerToUi::Spawned { .. }) {
                *spawned.entry(event.session_id).or_insert(0) += 1;
            }
        }
        assert_eq!(spawned.get("s1"), Some(&1));
        assert_eq!(spawned.get("s2"), Some(&1));

        pool.stop_session("s1");
        assert!(pool.send_line("s1", write).is_err());
        pool.send_line("s2", write).unwrap();

        pool.stop_all();
        assert!(pool.send_line("s2", write).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
