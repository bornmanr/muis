//! End-to-end: run the real `muis-daemon` binary against a fake worker and
//! speak the wire protocol over a real unix socket.
#![cfg(unix)]

use muis_daemon::protocol::{Reply, Request};
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
    let path = dir.join("fake-worker.sh");
    std::fs::write(
        &path,
        "#!/bin/sh\nwhile IFS= read -r _line; do printf '{\"type\":\"spawned\",\"pty_id\":\"p1\"}\\n'; done\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// Each test gets its own directory: the harness runs them in parallel and a
/// shared path would let one test delete another's worker and socket.
static NEXT: AtomicUsize = AtomicUsize::new(0);

fn start_daemon() -> Daemon {
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("muis-daemon-test-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("daemon.sock");
    let worker = fake_worker(&dir);
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
