//! HTTP + WebSocket surface: static bundle, JSON RPC, and one pty socket.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

use crate::assets;
use crate::auth;
use crate::pool::{EventSink, WorkerLaunch, WorkerPool};
use crate::rpc::{self, Ctx};

#[derive(Clone)]
pub struct AppState {
    pub token: Arc<String>,
    pub worker: WorkerLaunch,
    /// CLI notifications and agent reports, fanned out to every open tab.
    pub events: broadcast::Sender<String>,
    pub test_mode: bool,
    pub port: u16,
    pub socket: String,
}

#[derive(Deserialize)]
struct TokenQuery {
    #[serde(default)]
    t: Option<String>,
}

/// A bound listener plus the port the OS actually gave us.
pub struct Bound {
    listener: tokio::net::TcpListener,
    pub port: u16,
}

pub async fn bind(addr: &str) -> Result<Bound, String> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("cannot listen on {addr}: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    Ok(Bound { listener, port })
}

pub async fn run(bound: Bound, state: AppState) -> Result<(), String> {
    axum::serve(bound.listener, app(state))
        .await
        .map_err(|e| e.to_string())
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/healthz", get(health))
        .route("/ws", get(ws_handler))
        .route("/rpc/{method}", post(rpc_handler))
        .fallback(static_handler)
        .with_state(state)
}

fn host_of(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
}

fn origin_of(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
}

/// A request from a page or process that has no business here.
fn guard(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    if !auth::host_allowed(host_of(headers)) {
        return Some((StatusCode::FORBIDDEN, "host not allowed").into_response());
    }
    if !auth::origin_allowed(origin_of(headers), state.port) {
        return Some((StatusCode::FORBIDDEN, "origin not allowed").into_response());
    }
    None
}

fn presented_token(query: &TokenQuery, headers: &HeaderMap) -> Option<String> {
    query.t.clone().or_else(|| {
        headers
            .get("x-muis-token")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    })
}

async fn health() -> &'static str {
    "ok"
}

async fn index(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(rejected) = guard(&state, &headers) {
        return rejected;
    }
    let html = assets::index_html(&state.token);
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        html,
    )
        .into_response()
}

async fn static_handler(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    if let Some(rejected) = guard(&state, &headers) {
        return rejected;
    }
    let path = uri.path().trim_start_matches('/');
    if path.is_empty() || path == "index.html" {
        return index(State(state), headers).await;
    }
    if path.contains("..") {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let Some(bytes) = assets::get(path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-store"
    };
    (
        [
            (header::CONTENT_TYPE, assets::content_type(path)),
            (header::CACHE_CONTROL, cache),
        ],
        bytes,
    )
        .into_response()
}

async fn rpc_handler(
    State(state): State<AppState>,
    AxumPath(method): AxumPath<String>,
    Query(query): Query<TokenQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(rejected) = guard(&state, &headers) {
        return rejected;
    }
    if !auth::token_matches(&state.token, presented_token(&query, &headers).as_deref()) {
        return (StatusCode::UNAUTHORIZED, "bad token").into_response();
    }
    let args: Value = if body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice(&body) {
            Ok(value) => value,
            Err(e) => {
                return (StatusCode::BAD_REQUEST, format!("invalid json: {e}")).into_response()
            }
        }
    };
    let ctx = Ctx {
        test_mode: state.test_mode,
    };
    match rpc::dispatch(ctx, &method, &args).await {
        Ok(value) => Json(value).into_response(),
        Err(message) => {
            log::warn!("rpc {method} failed: {message}");
            (StatusCode::INTERNAL_SERVER_ERROR, message).into_response()
        }
    }
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(query): Query<TokenQuery>,
    headers: HeaderMap,
) -> Response {
    if let Some(rejected) = guard(&state, &headers) {
        return rejected;
    }
    if !auth::token_matches(&state.token, presented_token(&query, &headers).as_deref()) {
        return (StatusCode::UNAUTHORIZED, "bad token").into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// One WebSocket carries one browser tab's sessions: it owns the worker pool,
/// so closing the tab kills exactly its own shells.
async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let (frames_tx, mut frames_rx) = mpsc::unbounded_channel::<String>();
    let sink: EventSink = Arc::new(move |event| {
        let line = match serde_json::to_string(&event) {
            Ok(line) => line,
            Err(e) => {
                log::error!("worker event serialize failed: {e}");
                return true;
            }
        };
        frames_tx.send(line).is_ok()
    });

    let pool = Arc::new(WorkerPool::new(
        sink,
        state.worker.clone(),
        state.socket.clone(),
    ));
    let mut events = state.events.subscribe();

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Err(message) = handle_frame(&pool, &text).await {
                            log::debug!("ws frame rejected: {message}");
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        log::debug!("ws error: {e}");
                        break;
                    }
                }
            }
            Some(line) = frames_rx.recv() => {
                if socket.send(Message::Text(line.into())).await.is_err() {
                    break;
                }
            }
            event = events.recv() => {
                if let Ok(line) = event {
                    if socket.send(Message::Text(line.into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    }

    log::info!("browser tab gone; stopping its workers");
    let pool_for_stop = Arc::clone(&pool);
    let _ = tokio::task::spawn_blocking(move || pool_for_stop.stop_all()).await;
}

#[derive(Deserialize)]
struct ClientFrame {
    op: String,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    line: String,
}

async fn handle_frame(pool: &Arc<WorkerPool>, text: &str) -> Result<(), String> {
    let frame: ClientFrame = serde_json::from_str(text).map_err(|e| e.to_string())?;
    match frame.op.as_str() {
        "spawn" => {
            let pool = Arc::clone(pool);
            let session = frame.session_id;
            tokio::task::spawn_blocking(move || pool.spawn_session(&session))
                .await
                .map_err(|e| e.to_string())??;
            Ok(())
        }
        "send" => pool.send_line(&frame.session_id, &frame.line),
        "stop" => {
            let pool = Arc::clone(pool);
            let session = frame.session_id;
            tokio::task::spawn_blocking(move || pool.stop_session(&session))
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        other => Err(format!("unknown op: {other}")),
    }
}

/// Bind `addr`, or let the OS pick when the port is 0.
pub fn socket_addr(host: &str, port: u16) -> Result<SocketAddr, String> {
    let host: std::net::IpAddr = host.parse().map_err(|_| format!("bad host: {host}"))?;
    Ok(SocketAddr::new(host, port))
}
