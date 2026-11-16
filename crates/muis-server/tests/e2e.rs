//! End-to-end: the real `muis-server` binary, its embedded bundle, a real
//! WebSocket, and a real pty spawned by the worker mode of the same binary.
//!
//! Unix-only because it writes to `/bin/sh`; the code under test is shared.

#![cfg(unix)]

use std::process::Stdio;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio_tungstenite::tungstenite::Message;

struct Server {
    child: Child,
    port: u16,
    token: String,
}

impl Server {
    async fn start() -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_muis-server"))
            .args(["--no-open", "--testmode", "--port", "0"])
            .env(
                "MUIS_SOCKET_NAME",
                format!("muis-e2e-{}", std::process::id()),
            )
            .env(
                "MUIS_HOME",
                std::env::temp_dir().join(format!("muis-e2e-home-{}", std::process::id())),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn muis-server");

        let stdout = child.stdout.take().expect("stdout");
        let mut lines = BufReader::new(stdout).lines();
        let banner = tokio::time::timeout(Duration::from_secs(20), lines.next_line())
            .await
            .expect("server printed its url")
            .expect("read banner")
            .expect("banner line");

        // Keep draining so the child never blocks on a full pipe.
        tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });

        let (port, token) = parse_banner(&banner).unwrap_or_else(|| panic!("bad banner: {banner}"));
        Server { child, port, token }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

fn parse_banner(line: &str) -> Option<(u16, String)> {
    let rest = line.strip_prefix("muis serve: http://127.0.0.1:")?;
    let (port, rest) = rest.split_once("/?t=")?;
    Some((port.parse().ok()?, rest.trim().to_string()))
}

/// Minimal HTTP/1.1 request: no client library in the test dependency tree.
async fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

#[tokio::test]
async fn browser_mode_serves_the_ui_and_runs_a_shell() {
    let server = Server::start().await;

    // The bundle is served with the serve-mode bootstrap injected.
    let (status, index) = http(server.port, "GET", "/", None).await;
    assert_eq!(status, 200);
    assert!(index.contains("__MUIS_SERVE__"), "index: {index:.200}");
    assert!(index.contains(&server.token));

    // Commands answer, and a wrong token is refused.
    let (status, body) = http(
        server.port,
        "POST",
        &format!("/rpc/ping?t={}", server.token),
        Some("{}"),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body.trim(), "\"pong\"");
    let (status, _) = http(server.port, "POST", "/rpc/ping?t=wrong", Some("{}")).await;
    assert_eq!(status, 401);

    // A real pty through the WebSocket, spawned by `muis-server --muis-worker`.
    let url = format!("ws://127.0.0.1:{}/ws?t={}", server.port, server.token);
    let (mut socket, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("ws connect");

    let spawn = serde_json::json!({
        "op": "spawn",
        "session_id": "s1",
    });
    socket
        .send(Message::Text(spawn.to_string().into()))
        .await
        .unwrap();

    // The spawn frame carries no shell, so drive the tab the way the UI does:
    // a `spawn` UiToWorker line with the shell and size.
    let ui_spawn = serde_json::json!({
        "type": "spawn",
        "pty_id": "t1",
        "shell": "/bin/sh",
        "cwd": "/tmp",
        "cols": 80,
        "rows": 24,
    });
    socket
        .send(Message::Text(
            serde_json::json!({ "op": "send", "session_id": "s1", "line": ui_spawn.to_string() })
                .to_string()
                .into(),
        ))
        .await
        .unwrap();

    let input = serde_json::json!({
        "type": "write",
        "pty_id": "t1",
        "data_b64": base64("printf 'muis-e2e-ok\\n'\n"),
    });
    socket
        .send(Message::Text(
            serde_json::json!({ "op": "send", "session_id": "s1", "line": input.to_string() })
                .to_string()
                .into(),
        ))
        .await
        .unwrap();

    let mut seen = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "no shell output; saw: {seen}");
        let message = tokio::time::timeout(remaining, socket.next())
            .await
            .expect("frame before timeout")
            .expect("socket open")
            .expect("decodable frame");
        let Message::Text(text) = message else {
            continue;
        };
        let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
        if let Some(data) = frame
            .get("frame")
            .and_then(|f| f.get("data_b64"))
            .and_then(|d| d.as_str())
        {
            let bytes = decode_b64(data);
            seen.push_str(&String::from_utf8_lossy(&bytes));
            if seen.contains("muis-e2e-ok") {
                break;
            }
        }
    }
    assert!(seen.contains("muis-e2e-ok"), "shell output: {seen}");
}

fn base64(text: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(text.as_bytes())
}

fn decode_b64(text: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .unwrap_or_default()
}
