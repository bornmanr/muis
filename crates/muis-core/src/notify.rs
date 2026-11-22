//! Local client endpoint for `muis-notify` and `muis-agent`.
//!
//! A running muis shell listens on a per-user local socket; each client
//! sends one JSON request per connection. This is how hook-only tools —
//! OpenCode, Aider, Claude `Notification` hooks, Codex `notify` — reach the
//! terminal, the same shape as `wezterm cli` or `cmux notify`.
//!
//! Notifications and agent-session reports share the transport but not the
//! payload: a `kind` discriminator keeps `muis-agent` out of the
//! notification path. Requests without `kind` are notifications, so old
//! `muis-notify` clients keep working unchanged.
//!
//! The socket uses the abstract namespace via `interprocess`, so there is no
//! stale file to clean up.

use std::io::{self, BufRead, BufReader, Write};

use interprocess::local_socket::{prelude::*, GenericNamespaced, ListenerOptions, Name, Stream};
use serde::{Deserialize, Serialize};

pub const AGENT_KIND: &str = "agent-session";
pub const ACTIVITY_KIND: &str = "agent-activity";
pub const OPEN_KIND: &str = "open-url";
pub const ACTIVITY_RUNNING: &str = "running";

/// One notification request from a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub body: String,
    /// Originating tab, injected into ptys as `MUIS_TAB_ID`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    /// 0 low, 1 normal, 2 critical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub urgency: Option<u8>,
    /// opencode session the notification is about. The server-side plugin
    /// runs in a shared opencode server, so its `MUIS_TAB_ID` belongs to
    /// whichever tab started that server; the session id is the only
    /// reliable route back to the tab that is actually waiting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// What the agent in a tab is doing now. `running` means it picked the work
/// back up — the user replied — so the tab's waiting mark can go without a
/// new alert being raised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityRequest {
    pub kind: String,
    /// Only `running` today; an unknown state is ignored by the shell.
    pub state: String,
    /// Originating tab, injected into ptys as `MUIS_TAB_ID`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    /// opencode session that resumed, routed like `NotifyRequest::session`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

impl ActivityRequest {
    pub fn running(tab_id: Option<String>, session: Option<String>) -> Self {
        Self {
            kind: ACTIVITY_KIND.to_string(),
            state: ACTIVITY_RUNNING.to_string(),
            tab_id,
            session,
        }
    }
}

/// The terminal's currently open agent session. `session: None` means the
/// agent is no longer showing a session (for example OpenCode's home screen)
/// and the tab must stop treating itself as resumable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionRequest {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    /// Originating tab, injected into ptys as `MUIS_TAB_ID`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
}

impl AgentSessionRequest {
    pub fn new(session: Option<String>, directory: Option<String>, tab_id: Option<String>) -> Self {
        Self {
            kind: AGENT_KIND.to_string(),
            session,
            directory,
            tab_id,
        }
    }
}

/// Ask the terminal to show a URL in its browser pane. The pane is part of
/// the window, so an agent can put a local preview next to the terminal it
/// is driving instead of stealing focus with a system browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenUrlRequest {
    pub kind: String,
    pub url: String,
    /// Originating tab, injected into ptys as `MUIS_TAB_ID`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    /// opencode session the preview belongs to, routed like
    /// `NotifyRequest::session`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

impl OpenUrlRequest {
    /// Build a request, or `None` when the URL is not http(s). Callers never
    /// get to hand the frontend a `javascript:` or `file:` URL.
    pub fn new(url: &str, tab_id: Option<String>, session: Option<String>) -> Option<Self> {
        if !valid_http_url(url) {
            return None;
        }
        Some(Self {
            kind: OPEN_KIND.to_string(),
            url: url.trim().to_string(),
            tab_id,
            session,
        })
    }
}

/// Every request a local client may send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientRequest {
    Notify(NotifyRequest),
    AgentSession(AgentSessionRequest),
    AgentActivity(ActivityRequest),
    OpenUrl(OpenUrlRequest),
}

/// True for http(s) URLs with a non-empty host and no control characters.
/// Deliberately scheme-only: the frontend and the OS opener both get the
/// same guarantee, and nothing here can turn into a shell invocation.
pub fn valid_http_url(url: &str) -> bool {
    let url = url.trim();
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    let Some(rest) = rest else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.is_empty() && !host.starts_with(':')
}

/// True for the only session ids muis will ever write to a shell.
pub fn valid_session_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("ses_") else {
        return false;
    };
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Per-user abstract-namespace socket name.
pub fn socket_name() -> String {
    let user = std::env::var("USER").unwrap_or_else(|_| "default".to_string());
    format!("muis-notify-{user}")
}

fn socket_address(socket: &str) -> io::Result<Name<'_>> {
    socket.to_ns_name::<GenericNamespaced>()
}

/// Bind the socket and serve requests on a background thread, calling
/// `on_request` for each decoded request. The bind is synchronous, so a
/// client can connect as soon as this returns.
pub fn serve<F>(socket: &str, on_request: F) -> io::Result<()>
where
    F: Fn(ClientRequest) + Send + 'static,
{
    let listener = ListenerOptions::new()
        .name(socket_address(socket)?)
        .try_overwrite(true)
        .create_sync()?;
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else { continue };
            let mut reader = BufReader::new(conn);
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            if let Some(req) = decode_client(line.trim()) {
                on_request(req);
            }
        }
    });
    Ok(())
}

/// Decode one wire line. Agent reports are recognized by `kind`; everything
/// else is a notification (including old clients that never sent `kind`).
pub fn decode_client(line: &str) -> Option<ClientRequest> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    match value.get("kind").and_then(|v| v.as_str()) {
        Some(AGENT_KIND) => serde_json::from_value(value)
            .ok()
            .map(ClientRequest::AgentSession),
        Some(ACTIVITY_KIND) => serde_json::from_value(value)
            .ok()
            .map(ClientRequest::AgentActivity),
        Some(OPEN_KIND) => serde_json::from_value(value)
            .ok()
            .map(ClientRequest::OpenUrl),
        _ => serde_json::from_value(value).ok().map(ClientRequest::Notify),
    }
}

fn send_value(socket: &str, value: &serde_json::Value) -> io::Result<()> {
    let mut conn = Stream::connect(socket_address(socket)?)?;
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    conn.write_all(line.as_bytes())?;
    conn.flush()
}

/// Send one notification request to a running muis.
pub fn send(socket: &str, req: &NotifyRequest) -> io::Result<()> {
    send_value(socket, &serde_json::to_value(req)?)
}

/// Send one agent-session report to a running muis.
pub fn send_agent(socket: &str, req: &AgentSessionRequest) -> io::Result<()> {
    send_value(socket, &serde_json::to_value(req)?)
}

/// Send one agent-activity report to a running muis.
pub fn send_activity(socket: &str, req: &ActivityRequest) -> io::Result<()> {
    send_value(socket, &serde_json::to_value(req)?)
}

/// Send one open-url request to a running muis.
pub fn send_open_url(socket: &str, req: &OpenUrlRequest) -> io::Result<()> {
    send_value(socket, &serde_json::to_value(req)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn request_json_roundtrips_and_optional_fields_may_be_omitted() {
        let req = NotifyRequest {
            title: Some("T".into()),
            body: "b".into(),
            tab_id: Some("t1".into()),
            urgency: Some(2),
            session: Some("ses_abc".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(serde_json::from_str::<NotifyRequest>(&json).unwrap(), req);

        let min: NotifyRequest = serde_json::from_str(r#"{"body":"hi"}"#).unwrap();
        assert_eq!(
            min,
            NotifyRequest {
                title: None,
                body: "hi".into(),
                tab_id: None,
                urgency: None,
                session: None,
            }
        );
    }

    #[test]
    fn socket_name_is_per_user() {
        assert!(socket_name().starts_with("muis-notify-"));
    }

    #[test]
    fn agent_request_roundtrips_with_and_without_a_session() {
        let req = AgentSessionRequest::new(
            Some("ses_abc123".into()),
            Some("/tmp".into()),
            Some("t1".into()),
        );
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("agent-session"));
        assert_eq!(decode_client(&json), Some(ClientRequest::AgentSession(req)));

        let clear = AgentSessionRequest::new(None, None, Some("t1".into()));
        assert_eq!(
            decode_client(&serde_json::to_string(&clear).unwrap()),
            Some(ClientRequest::AgentSession(clear))
        );
    }

    #[test]
    fn legacy_notify_and_agent_do_not_cross_decode() {
        let notify = NotifyRequest {
            title: None,
            body: "hello".into(),
            tab_id: Some("t1".into()),
            urgency: None,
            session: None,
        };
        assert_eq!(
            decode_client(&serde_json::to_string(&notify).unwrap()),
            Some(ClientRequest::Notify(notify))
        );
        assert!(decode_client("not json").is_none());
    }

    #[test]
    fn session_ids_are_strictly_ses_alphanumeric() {
        assert!(valid_session_id("ses_abc123"));
        assert!(!valid_session_id("ses_"));
        assert!(!valid_session_id("abc123"));
        assert!(!valid_session_id("ses_ab-c"));
        assert!(!valid_session_id("ses_ab c"));
        assert!(!valid_session_id("ses_ab\nrm -rf /"));
    }

    #[test]
    fn serve_and_send_roundtrip() {
        let socket = format!("muis-notify-test-{}", std::process::id());
        let (tx, rx) = mpsc::channel();
        serve(&socket, move |req| {
            let _ = tx.send(req);
        })
        .unwrap();

        let req = NotifyRequest {
            title: None,
            body: "hello".into(),
            tab_id: Some("tab-1".into()),
            urgency: None,
            session: None,
        };
        send(&socket, &req).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            ClientRequest::Notify(req)
        );

        let agent = AgentSessionRequest::new(Some("ses_x1".into()), None, Some("tab-1".into()));
        send_agent(&socket, &agent).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            ClientRequest::AgentSession(agent)
        );

        let running = ActivityRequest::running(Some("tab-1".into()), None);
        send_activity(&socket, &running).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            ClientRequest::AgentActivity(running)
        );

        let open = OpenUrlRequest::new(
            "http://127.0.0.1:5173/",
            Some("tab-1".into()),
            Some("ses_x1".into()),
        )
        .unwrap();
        send_open_url(&socket, &open).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            ClientRequest::OpenUrl(open)
        );
    }

    #[test]
    fn open_url_requests_decode_by_kind_and_keep_their_route() {
        let req = OpenUrlRequest::new(
            " https://localhost:8443/preview?a=1 ",
            Some("t3".into()),
            Some("ses_open".into()),
        )
        .unwrap();
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("open-url"));
        assert_eq!(
            decode_client(&json),
            Some(ClientRequest::OpenUrl(req.clone()))
        );
        // A bare URL still decodes; the route is optional.
        assert_eq!(
            decode_client(r#"{"kind":"open-url","url":"http://localhost:3000"}"#),
            Some(ClientRequest::OpenUrl(
                OpenUrlRequest::new("http://localhost:3000", None, None).unwrap()
            ))
        );
    }

    #[test]
    fn open_url_only_accepts_http_and_https() {
        assert!(valid_http_url("http://localhost:5173"));
        assert!(valid_http_url("https://example.com/a/b?c=d#e"));
        assert!(valid_http_url("  http://127.0.0.1:8000  "));

        assert!(!valid_http_url(""));
        assert!(!valid_http_url("localhost:5173"));
        assert!(!valid_http_url("file:///etc/passwd"));
        assert!(!valid_http_url("javascript:alert(1)"));
        assert!(!valid_http_url("data:text/html,<h1>x</h1>"));
        assert!(!valid_http_url("http://"));
        assert!(!valid_http_url("http:///path"));
        assert!(!valid_http_url("http://localhost:5173/\nrm -rf /"));

        // The constructor refuses what the checker refuses.
        assert!(OpenUrlRequest::new("file:///etc/passwd", None, None).is_none());
    }

    #[test]
    fn a_notification_may_carry_a_session_route() {
        // The opencode server plugin cannot trust MUIS_TAB_ID, so the session
        // travels with the notification and must survive the wire.
        let req: NotifyRequest = serde_json::from_str(
            r#"{"body":"done","tab_id":"t9","session":"ses_routed"}"#,
        )
        .unwrap();
        assert_eq!(req.session.as_deref(), Some("ses_routed"));
        assert_eq!(
            decode_client(&serde_json::to_string(&req).unwrap()),
            Some(ClientRequest::Notify(req))
        );
    }

    #[test]
    fn activity_reports_decode_by_kind_not_as_notifications() {
        let running = ActivityRequest::running(Some("t2".into()), Some("ses_run".into()));
        let json = serde_json::to_string(&running).unwrap();
        assert!(json.contains("agent-activity"));
        assert_eq!(
            decode_client(&json),
            Some(ClientRequest::AgentActivity(running))
        );

        // A running report without a tab is still decodable; the shell drops
        // it because it cannot say which tab resumed.
        let bare = ActivityRequest::running(None, None);
        assert_eq!(
            decode_client(r#"{"kind":"agent-activity","state":"running"}"#),
            Some(ClientRequest::AgentActivity(bare))
        );
    }
}
