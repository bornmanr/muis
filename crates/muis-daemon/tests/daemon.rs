//! End-to-end: run the real `muis-daemon` binary against a fake worker and
//! speak the wire protocol over a real unix socket.
#![cfg(unix)]

use muis_daemon::protocol::{Reply, Request, Role};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

struct Daemon {
    child: Child,
    socket: PathBuf,
    dir: PathBuf,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fake_worker(dir: &Path) -> PathBuf {
    write_worker(
        dir,
        "#!/bin/sh
while IFS= read -r _line; do printf '{\"type\":\"spawned\",\"pty_id\":\"p1\"}
'; done
",
    )
}

/// Write an executable shell script and return its path.
fn write_worker(dir: &Path, script: &str) -> PathBuf {
    let path = dir.join("fake-worker.sh");
    std::fs::write(&path, script).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// Each test gets its own directory: the harness runs them in parallel and a
/// shared path would let one test delete another's worker and socket.
static NEXT: AtomicUsize = AtomicUsize::new(0);

fn start_daemon() -> Daemon {
    start_daemon_with(
        "#!/bin/sh\nwhile IFS= read -r _line; do printf '{\"type\":\"spawned\",\"pty_id\":\"p1\"}\\n'; done\n",
    )
}

/// Same daemon, a worker that answers differently; used by the replay test.
fn start_daemon_with(script: &str) -> Daemon {
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("muis-daemon-test-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("daemon.sock");
    let worker = write_worker(&dir, script);
    let child = Command::new(env!("CARGO_BIN_EXE_muis-daemon"))
        .arg("--socket")
        .arg(&socket)
        .arg("--worker")
        .arg(&worker)
        // A test daemon must never exit under us.
        .args(["--idle-seconds", "0"])
        .env_remove("MUIS_SOCKET")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn muis-daemon");
    let daemon = Daemon {
        child,
        socket: socket.clone(),
        dir,
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while UnixStream::connect(&socket).is_err() {
        assert!(
            Instant::now() < deadline,
            "daemon never bound {}",
            socket.display()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    daemon
}

fn connect(socket: &Path) -> (BufReader<UnixStream>, UnixStream) {
    let stream = UnixStream::connect(socket).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    (BufReader::new(stream.try_clone().unwrap()), stream)
}

fn send(stream: &mut UnixStream, request: &Request) {
    let line = serde_json::to_string(request).unwrap();
    stream.write_all(format!("{line}\n").as_bytes()).unwrap();
    stream.flush().unwrap();
}

fn read_reply(reader: &mut BufReader<UnixStream>) -> Reply {
    let mut line = String::new();
    reader.read_line(&mut line).expect("read reply");
    assert!(!line.is_empty(), "daemon closed the connection");
    serde_json::from_str(line.trim()).unwrap_or_else(|e| panic!("bad reply {line:?}: {e}"))
}

#[test]
fn spawn_send_and_status_roundtrip() {
    let daemon = start_daemon();
    let (mut reader, mut stream) = connect(&daemon.socket);

    // Spawning only starts the process; the worker speaks when it is asked
    // to, exactly like the in-process pool.
    send(
        &mut stream,
        &Request::Spawn {
            session: "s1".into(),
        },
    );
    send(
        &mut stream,
        &Request::Send {
            session: "s1".into(),
            line: r#"{"type":"spawn","pty_id":"p1","shell":"/bin/sh","cwd":"/tmp","cols":80,"rows":24}"#
                .into(),
        },
    );
    match read_reply(&mut reader) {
        Reply::Event { session_id, frame } => {
            assert_eq!(session_id, "s1");
            assert!(matches!(frame, muis_core::ipc::WorkerToUi::Spawned { .. }));
        }
        other => panic!("unexpected reply: {other:?}"),
    }

    // A forwarded line reaches the worker and its answer comes back through
    // the same branch as real pty output.
    send(
        &mut stream,
        &Request::Send {
            session: "s1".into(),
            line: r#"{"type":"write","pty_id":"p1","data_b64":""}"#.into(),
        },
    );
    let reply = read_reply(&mut reader);
    match reply {
        Reply::Event { session_id, .. } => assert_eq!(session_id, "s1"),
        other => panic!("unexpected reply: {other:?}"),
    }

    send(&mut stream, &Request::Status);
    match read_reply(&mut reader) {
        Reply::Status { sessions, clients } => {
            assert_eq!(sessions, vec!["s1".to_string()]);
            assert_eq!(clients, 1);
        }
        other => panic!("unexpected reply: {other:?}"),
    }
}

#[test]
fn a_bad_request_answers_with_an_error_and_keeps_the_connection() {
    let daemon = start_daemon();
    let (mut reader, mut stream) = connect(&daemon.socket);

    stream.write_all(b"not json\n").unwrap();
    stream.flush().unwrap();
    match read_reply(&mut reader) {
        Reply::Error { message } => assert!(message.starts_with("bad request")),
        other => panic!("unexpected reply: {other:?}"),
    }
    // The socket still works after a malformed line.
    send(&mut stream, &Request::Status);
    assert!(matches!(read_reply(&mut reader), Reply::Status { .. }));
}

#[test]
fn send_without_a_spawn_reports_an_error() {
    let daemon = start_daemon();
    let (mut reader, mut stream) = connect(&daemon.socket);
    send(
        &mut stream,
        &Request::Send {
            session: "missing".into(),
            line: r#"{"type":"write","pty_id":"p1","data_b64":""}"#.into(),
        },
    );
    match read_reply(&mut reader) {
        Reply::Error { message } => assert!(message.contains("no worker for session"), "{message}"),
        other => panic!("unexpected reply: {other:?}"),
    }
}

/// The control CLI speaks the same socket; it should see the daemon's
/// sessions and exit cleanly when there are none.
#[test]
fn ctl_lists_live_sessions() {
    let daemon = start_daemon();
    let empty = Command::new(env!("CARGO_BIN_EXE_muis-ctl"))
        .arg("--socket")
        .arg(&daemon.socket)
        .arg("ls")
        .output()
        .expect("run muis-ctl");
    assert!(empty.status.success(), "muis-ctl ls failed");
    assert_eq!(String::from_utf8_lossy(&empty.stdout).trim(), "");

    // Register a session through the protocol, then ask the CLI.
    let (mut reader, mut stream) = connect(&daemon.socket);
    send(
        &mut stream,
        &Request::Spawn {
            session: "s1".into(),
        },
    );
    send(&mut stream, &Request::Status);
    assert!(matches!(read_reply(&mut reader), Reply::Status { .. }));

    let listed = Command::new(env!("CARGO_BIN_EXE_muis-ctl"))
        .arg("--socket")
        .arg(&daemon.socket)
        .arg("ls")
        .output()
        .expect("run muis-ctl");
    assert!(listed.status.success());
    assert_eq!(String::from_utf8_lossy(&listed.stdout).trim(), "s1");
}

#[test]
fn ctl_reports_a_missing_daemon() {
    let dir = std::env::temp_dir().join(format!("muis-ctl-none-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_muis-ctl"))
        .arg("--socket")
        .arg(dir.join("absent.sock"))
        .arg("ls")
        .output()
        .expect("run muis-ctl");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("muis-ctl:"));
}

/// The window registers once; a client's `ui` request is forwarded to it and
/// the answer routed back to the caller with the caller's own id.
#[test]
fn ui_requests_are_forwarded_and_replies_routed() {
    let daemon = start_daemon();
    let (mut ui_reader, mut ui_stream) = connect(&daemon.socket);
    send(&mut ui_stream, &Request::Hello { role: Role::Ui });
    assert!(matches!(
        read_reply(&mut ui_reader),
        Reply::Hello { role: Role::Ui }
    ));

    let (mut caller_reader, mut caller) = connect(&daemon.socket);
    send(
        &mut caller,
        &Request::Ui {
            id: 7,
            op: "ls".into(),
            args: serde_json::json!({}),
        },
    );

    let slug = match read_reply(&mut ui_reader) {
        Reply::UiRequest { id, op, .. } => {
            assert_eq!(op, "ls");
            id
        }
        other => panic!("window should see the forwarded request, got {other:?}"),
    };
    send(
        &mut ui_stream,
        &Request::UiReply {
            id: slug,
            ok: true,
            result: serde_json::json!(["t1"]),
        },
    );
    match read_reply(&mut caller_reader) {
        Reply::UiReply { id, ok, result } => {
            assert_eq!(id, 7, "the caller's id survives the round trip");
            assert!(ok);
            assert_eq!(result[0], "t1");
        }
        other => panic!("caller should get the answer, got {other:?}"),
    }
}

#[test]
fn ui_request_without_a_window_answers_an_error() {
    let daemon = start_daemon();
    let (mut reader, mut stream) = connect(&daemon.socket);
    send(
        &mut stream,
        &Request::Ui {
            id: 1,
            op: "ls".into(),
            args: serde_json::Value::Null,
        },
    );
    match read_reply(&mut reader) {
        Reply::Error { message } => assert!(message.contains("no window attached"), "{message}"),
        other => panic!("unexpected reply: {other:?}"),
    }
}

/// A window that reconnects gets the session's recent output before anything
/// else, without spawning a second worker.
#[test]
fn reattaching_replays_buffered_output() {
    let daemon = start_daemon_with(
        "#!/bin/sh\nwhile IFS= read -r _line; do \
         printf '{\"type\":\"spawned\",\"pty_id\":\"p1\"}\n'; \
         printf '{\"type\":\"output\",\"pty_id\":\"p1\",\"data_b64\":\"aGk=\"}\n'; done\n",
    );

    let (mut reader, mut stream) = connect(&daemon.socket);
    send(
        &mut stream,
        &Request::Spawn {
            session: "s1".into(),
        },
    );
    send(
        &mut stream,
        &Request::Send {
            session: "s1".into(),
            line: r#"{"type":"write","pty_id":"p1","data_b64":""}"#.into(),
        },
    );
    assert!(matches!(
        read_reply(&mut reader),
        Reply::Event {
            frame: muis_core::ipc::WorkerToUi::Spawned { .. },
            ..
        }
    ));
    match read_reply(&mut reader) {
        Reply::Event {
            frame: muis_core::ipc::WorkerToUi::Output { data_b64, .. },
            ..
        } => assert_eq!(data_b64, "aGk="),
        other => panic!("expected the worker's output, got {other:?}"),
    }

    // A second window attaches: the tail comes back to it only.
    let (mut reader2, mut stream2) = connect(&daemon.socket);
    send(
        &mut stream2,
        &Request::Spawn {
            session: "s1".into(),
        },
    );
    match read_reply(&mut reader2) {
        Reply::Event {
            session_id,
            frame: muis_core::ipc::WorkerToUi::Output { data_b64, .. },
        } => {
            assert_eq!(session_id, "s1");
            assert_eq!(data_b64, "aGk=", "the buffered bytes are replayed");
        }
        other => panic!("expected a replay, got {other:?}"),
    }
}
