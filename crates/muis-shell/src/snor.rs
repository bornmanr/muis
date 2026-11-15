//! Snor: supervised local `llama-server` child and streaming chat.
//!
//! The UI never talks to llama.cpp directly. It calls these commands and
//! listens for `snor-*` events. One server is shared by the whole app;
//! requests are serialized (`--parallel 1`). See `docs/snor-plan.md`.
//!
//! The engine is optional: if `llama-server` is not installed or no model
//! is configured, the commands fail with a clear message and the panel
//! shows its unconfigured state. Nothing is downloaded or started at
//! launch; the child starts lazily on the first request.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};

pub const TOKEN_EVENT: &str = "snor-token";
pub const DONE_EVENT: &str = "snor-done";
pub const ERROR_EVENT: &str = "snor-error";
pub const STATUS_EVENT: &str = "snor-status";

const START_TIMEOUT: Duration = Duration::from_secs(30);
const HEALTH_POLL: Duration = Duration::from_millis(250);
const MAX_TOKENS: u32 = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SnorState {
    Stopped,
    Starting,
    Ready,
    Streaming,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnorStatus {
    pub state: SnorState,
    pub model: Option<String>,
    pub error: Option<String>,
    pub server: Option<String>,
}

impl Default for SnorStatus {
    fn default() -> Self {
        Self {
            state: SnorState::Stopped,
            model: None,
            error: None,
            server: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub system: Option<String>,
    pub temperature: Option<f64>,
}

#[derive(Clone, Serialize)]
struct TokenPayload {
    text: String,
}

#[derive(Clone, Serialize)]
struct ErrorPayload {
    message: String,
}

struct Inner {
    child: Option<Child>,
    port: Option<u16>,
    key: Option<String>,
    model: Option<String>,
    server: Option<String>,
    state: SnorState,
    error: Option<String>,
}

pub struct SnorEngine {
    inner: Mutex<Inner>,
    cancel: Arc<AtomicBool>,
    busy: AtomicBool,
    client: reqwest::blocking::Client,
}

impl Default for SnorEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl SnorEngine {
    pub fn new() -> Self {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        Self {
            inner: Mutex::new(Inner {
                child: None,
                port: None,
                key: None,
                model: None,
                server: None,
                state: SnorState::Stopped,
                error: None,
            }),
            cancel: Arc::new(AtomicBool::new(false)),
            busy: AtomicBool::new(false),
            client,
        }
    }

    pub fn status(&self) -> SnorStatus {
        let inner = self.inner.lock().unwrap();
        SnorStatus {
            state: inner.state,
            model: inner.model.clone(),
            error: inner.error.clone(),
            server: inner.server.clone(),
        }
    }

    /// Locate `llama-server`: an explicit path from settings wins, then
    /// `PATH`. `None` means the engine is not installed.
    pub fn find_server(explicit: Option<&str>) -> Option<PathBuf> {
        if let Some(path) = explicit.map(str::trim).filter(|p| !p.is_empty()) {
            let path = PathBuf::from(path);
            return path.is_file().then_some(path);
        }
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join("llama-server"))
            .find(|candidate| candidate.is_file())
    }

    /// Start the child if it is not already running. Idempotent for the
    /// same model; switching models restarts the server.
    pub fn start(
        &self,
        model: &str,
        ctx_size: u32,
        gpu_layers: i32,
        server_path: Option<&str>,
    ) -> Result<SnorStatus, String> {
        let model = model.trim();
        if model.is_empty() {
            return Err("choose a GGUF model in settings first".into());
        }
        let model_path = Path::new(model);
        if !model_path.is_file() {
            return Err(format!("model file not found: {model}"));
        }

        {
            let inner = self.inner.lock().unwrap();
            if inner.state == SnorState::Ready && inner.model.as_deref() == Some(model) {
                return Ok(status_of(&inner));
            }
        }
        self.stop();

        let server = Self::find_server(server_path)
            .ok_or("llama-server is not installed (install llama-cpp)")?;
        let port = free_port()?;
        let key = random_key()?;

        let child = Command::new(&server)
            .args(server_args(model_path, port, &key, ctx_size, gpu_layers))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", server.display()))?;

        {
            let mut inner = self.inner.lock().unwrap();
            inner.child = Some(child);
            inner.port = Some(port);
            inner.key = Some(key);
            inner.model = Some(model.to_string());
            inner.server = Some(server.to_string_lossy().into_owned());
            inner.state = SnorState::Starting;
            inner.error = None;
        }

        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if self.health(port) {
                let mut inner = self.inner.lock().unwrap();
                inner.state = SnorState::Ready;
                return Ok(status_of(&inner));
            }
            if let Some(status) = self.exited_early() {
                let message = format!("llama-server exited during start ({status})");
                let mut inner = self.inner.lock().unwrap();
                inner.state = SnorState::Error;
                inner.error = Some(message.clone());
                inner.child = None;
                return Err(message);
            }
            if Instant::now() >= deadline {
                self.stop();
                let message = "llama-server did not become ready in time".to_string();
                let mut inner = self.inner.lock().unwrap();
                inner.state = SnorState::Error;
                inner.error = Some(message.clone());
                return Err(message);
            }
            std::thread::sleep(HEALTH_POLL);
        }
    }

    fn health(&self, port: u16) -> bool {
        let url = format!("http://127.0.0.1:{port}/health");
        match self.client.get(url).send() {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    fn exited_early(&self) -> Option<String> {
        let mut inner = self.inner.lock().unwrap();
        let child = inner.child.as_mut()?;
        match child.try_wait() {
            Ok(Some(status)) => {
                inner.child = None;
                Some(status.to_string())
            }
            Ok(None) => None,
            Err(e) => {
                inner.child = None;
                Some(e.to_string())
            }
        }
    }

    pub fn stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        let mut inner = self.inner.lock().unwrap();
        if let Some(mut child) = inner.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        inner.port = None;
        inner.key = None;
        inner.state = SnorState::Stopped;
        inner.error = None;
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Stream one completion. Tokens arrive as `snor-token` events; the
    /// call returns as soon as the streaming thread is running.
    pub fn send(self: &Arc<Self>, app: AppHandle, req: ChatRequest) -> Result<(), String> {
        let (port, key) = {
            let inner = self.inner.lock().unwrap();
            if inner.state != SnorState::Ready {
                return Err(inner
                    .error
                    .clone()
                    .unwrap_or_else(|| "Snor is not running".to_string()));
            }
            (inner.port.ok_or("Snor has no port")?, inner.key.clone().ok_or("Snor has no key")?)
        };
        if self.busy.swap(true, Ordering::SeqCst) {
            return Err("Snor is already answering".into());
        }
        self.cancel.store(false, Ordering::SeqCst);
        {
            self.inner.lock().unwrap().state = SnorState::Streaming;
        }
        emit_status(&app, self.status());

        let engine = Arc::clone(self);
        std::thread::spawn(move || {
            let result = engine.stream(&app, port, &key, req);
            engine.busy.store(false, Ordering::SeqCst);
            {
                let mut inner = engine.inner.lock().unwrap();
                inner.state = match &result {
                    Ok(_) => SnorState::Ready,
                    Err(_) => SnorState::Error,
                };
                inner.error = result.as_ref().err().cloned();
            }
            match result {
                Ok(()) => {
                    let _ = app.emit(DONE_EVENT, ());
                }
                Err(message) => {
                    let _ = app.emit(ERROR_EVENT, ErrorPayload { message });
                }
            }
            emit_status(&app, engine.status());
        });
        Ok(())
    }

    fn stream(
        &self,
        app: &AppHandle,
        port: u16,
        key: &str,
        req: ChatRequest,
    ) -> Result<(), String> {
        self.stream_with(port, key, req, |text| {
            let _ = app.emit(TOKEN_EVENT, TokenPayload { text });
        })
    }

    fn stream_with(
        &self,
        port: u16,
        key: &str,
        req: ChatRequest,
        on_token: impl FnMut(String),
    ) -> Result<(), String> {
        let mut messages = Vec::with_capacity(req.messages.len() + 1);
        if let Some(system) = req.system.filter(|s| !s.trim().is_empty()) {
            messages.push(ChatMessage {
                role: "system".into(),
                content: system,
            });
        }
        messages.extend(req.messages);

        let url = format!("http://127.0.0.1:{port}/v1/chat/completions");
        let body = serde_json::json!({
            "model": "local",
            "messages": messages,
            "stream": true,
            "temperature": req.temperature.unwrap_or(0.2),
            "max_tokens": MAX_TOKENS,
        });
        let resp = self
            .client
            .post(url)
            .bearer_auth(key)
            .json(&body)
            .send()
            .map_err(|e| format!("cannot reach llama-server: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("llama-server returned {}", resp.status()));
        }

        let mut reader = BufReader::new(resp);
        pump_sse(&mut reader, &self.cancel, on_token)
    }
}

impl Drop for SnorEngine {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(mut child) = inner.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

fn status_of(inner: &Inner) -> SnorStatus {
    SnorStatus {
        state: inner.state,
        model: inner.model.clone(),
        error: inner.error.clone(),
        server: inner.server.clone(),
    }
}

fn emit_status(app: &AppHandle, status: SnorStatus) {
    let _ = app.emit(STATUS_EVENT, status);
}

/// llama-server invocation. One slot (`--parallel 1`); the context size is
/// per slot, so this also caps KV memory.
pub fn server_args(
    model: &Path,
    port: u16,
    key: &str,
    ctx_size: u32,
    gpu_layers: i32,
) -> Vec<String> {
    vec![
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string(),
        "--api-key".into(),
        key.into(),
        "--model".into(),
        model.to_string_lossy().into_owned(),
        "--ctx-size".into(),
        ctx_size.to_string(),
        "--parallel".into(),
        "1".into(),
        "--n-gpu-layers".into(),
        gpu_layers.to_string(),
    ]
}

/// Pull `choices[0].delta.content` out of one SSE `data:` line.
pub fn sse_delta(line: &str) -> Option<String> {
    let payload = line.strip_prefix("data:")?.trim();
    if payload.is_empty() || payload == "[DONE]" {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    value
        .get("choices")?
        .get(0)?
        .get("delta")?
        .get("content")?
        .as_str()
        .map(str::to_string)
}

/// Read an SSE response line by line until `[DONE]`, EOF, or cancel.
/// Split out from the request so the loop is testable without a server.
fn pump_sse<R: BufRead>(
    reader: &mut R,
    cancel: &AtomicBool,
    mut on_token: impl FnMut(String),
) -> Result<(), String> {
    let mut line = String::new();
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Ok(());
        }
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|e| format!("stream read failed: {e}"))?;
        if read == 0 {
            return Ok(());
        }
        let trimmed = line.trim();
        if trimmed == "data: [DONE]" {
            return Ok(());
        }
        if let Some(text) = sse_delta(trimmed) {
            if !text.is_empty() {
                on_token(text);
            }
        }
    }
}

fn free_port() -> Result<u16, String> {
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|e| format!("no free loopback port: {e}"))?;
    Ok(listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .port())
}

fn random_key() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| format!("cannot read /dev/urandom: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_args_bind_loopback_with_one_slot() {
        let args = server_args(Path::new("/m/model.gguf"), 1234, "abc", 8192, 99);
        let joined = args.join(" ");
        assert!(joined.contains("--host 127.0.0.1"));
        assert!(joined.contains("--port 1234"));
        assert!(joined.contains("--api-key abc"));
        assert!(joined.contains("--model /m/model.gguf"));
        assert!(joined.contains("--ctx-size 8192"));
        assert!(joined.contains("--parallel 1"));
        assert!(joined.contains("--n-gpu-layers 99"));
    }

    #[test]
    fn parses_sse_deltas_and_ignores_the_rest() {
        assert_eq!(
            sse_delta(r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#),
            Some("hi".into())
        );
        assert_eq!(
            sse_delta(r#"data:{"choices":[{"delta":{"content":"x"}}]}"#),
            Some("x".into())
        );
        assert_eq!(sse_delta("data: [DONE]"), None);
        assert_eq!(sse_delta("data: {\"choices\":[]}"), None);
        assert_eq!(sse_delta(": keep-alive"), None);
        assert_eq!(sse_delta("data: not json"), None);
    }

    #[test]
    fn pump_sse_assembles_tokens_until_done() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n",
            "\n",
            ": keep-alive\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n",
            "data: [DONE]\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"ignored\"}}]}\n",
        );
        let cancel = AtomicBool::new(false);
        let mut out = String::new();
        pump_sse(&mut std::io::Cursor::new(body), &cancel, |t| out.push_str(&t)).unwrap();
        assert_eq!(out, "Hello");
    }

    #[test]
    fn pump_sse_stops_when_cancelled() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n";
        let cancel = AtomicBool::new(true);
        let mut out = String::new();
        pump_sse(&mut std::io::Cursor::new(body), &cancel, |t| out.push_str(&t)).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn streams_tokens_from_a_local_compatible_server() {
        use std::io::{Read as _, Write as _};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let n = sock.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_lowercase();
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
                "data: [DONE]\n",
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(response.as_bytes()).unwrap();
            request
        });

        let engine = SnorEngine::new();
        let mut out = String::new();
        engine
            .stream_with(
                port,
                "secret",
                ChatRequest {
                    messages: vec![ChatMessage {
                        role: "user".into(),
                        content: "hi".into(),
                    }],
                    system: Some("be brief".into()),
                    temperature: Some(0.2),
                },
                |text| out.push_str(&text),
            )
            .unwrap();

        let request = server.join().unwrap();
        assert_eq!(out, "ok");
        assert!(request.contains("authorization: bearer secret"));
        assert!(request.contains("\"stream\":true"));
        assert!(request.contains("be brief"));
        assert!(request.contains("\"content\":\"hi\""));
    }

    #[test]
    fn random_key_is_hex_and_unique() {        let a = random_key().unwrap();
        let b = random_key().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn free_port_is_loopback_and_nonzero() {
        assert_ne!(free_port().unwrap(), 0);
    }

    #[test]
    fn start_rejects_missing_model_before_touching_the_server() {
        let engine = SnorEngine::new();
        let err = engine
            .start("/definitely/not/here.gguf", 8192, 0, None)
            .unwrap_err();
        assert!(err.contains("model file not found"));
        assert_eq!(engine.status().state, SnorState::Stopped);
    }

    #[test]
    fn find_server_prefers_an_explicit_existing_path() {
        let dir = std::env::temp_dir().join(format!("muis-snor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("llama-server");
        std::fs::write(&path, b"#!/bin/sh\n").unwrap();
        assert_eq!(
            SnorEngine::find_server(Some(&path.to_string_lossy())),
            Some(path.clone())
        );
        assert_eq!(SnorEngine::find_server(Some("/nope/llama-server")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
