//! `muis-mcp`: the terminal as MCP tools.
//!
//! Stdio JSON-RPC, no SDK: `initialize`, `tools/list`, `tools/call`. Every
//! call becomes a `ui` request on the daemon socket, so what an agent can do
//! here is exactly what the control surface allows — no second policy.

#[cfg(not(unix))]
fn main() {
    eprintln!("muis-mcp needs unix sockets");
    std::process::exit(1);
}

#[cfg(unix)]
fn main() {
    imp::run();
}

/// Everything lives inside the unix gate: the transport is a unix socket, and
/// the Windows check compiles this crate.
#[cfg(unix)]
mod imp {
    use muis_daemon::default_socket;
    use muis_daemon::protocol::{Reply, Request};
    use serde_json::{json, Value};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::Mutex;
    use std::time::Duration;

    const PROTOCOL_VERSION: &str = "2024-11-05";

    /// One persistent connection; reconnected when a call finds it dead.
    static SOCKET: Mutex<Option<UnixStream>> = Mutex::new(None);

    pub fn run() {
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();
        for line in BufReader::new(stdin).lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            if let Some(response) = dispatch(&line, &mut |op, args| call(op, args)) {
                if writeln!(stdout, "{response}")
                    .and_then(|()| stdout.flush())
                    .is_err()
                {
                    break;
                }
            }
        }
    }

    /// Handle one JSON-RPC message. `call` runs a tool; injected so the protocol
    /// layer tests without a socket or a window.
    fn dispatch(
        line: &str,
        call: &mut dyn FnMut(&str, Value) -> Result<Value, String>,
    ) -> Option<String> {
        let message: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(e) => {
                return Some(error_reply(
                    Value::Null,
                    -32700,
                    &format!("parse error: {e}"),
                ));
            }
        };
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let method = message
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or_default();
        match method {
            "initialize" => Some(ok_reply(
                id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "muis", "version": env!("CARGO_PKG_VERSION") }
                }),
            )),
            // Notifications carry no id and get no response.
            "notifications/initialized" | "notifications/cancelled" => None,
            "tools/list" => Some(ok_reply(id, json!({ "tools": tools() }))),
            "tools/call" => {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                let name = params
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or_default();
                let args = params.get("arguments").cloned().unwrap_or(Value::Null);
                if !tools().iter().any(|t| t["name"] == name) {
                    return Some(error_reply(id, -32602, &format!("unknown tool: {name}")));
                }
                match call(name, args) {
                    Ok(value) => Some(ok_reply(
                        id,
                        json!({ "content": [{ "type": "text", "text": value_text(&value) }] }),
                    )),
                    Err(e) => Some(ok_reply(
                        id,
                        json!({
                            "content": [{ "type": "text", "text": e }],
                            "isError": true
                        }),
                    )),
                }
            }
            _ => Some(error_reply(
                id,
                -32601,
                &format!("unknown method: {method}"),
            )),
        }
    }

    fn ok_reply(id: Value, result: Value) -> String {
        json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
    }

    fn error_reply(id: Value, code: i64, message: &str) -> String {
        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
            .to_string()
    }

    /// Tool results are text; structured data is pretty JSON so the model can read
    /// it, and a plain string passes through unchanged.
    fn value_text(value: &Value) -> String {
        match value {
            Value::String(s) => s.clone(),
            other => serde_json::to_string_pretty(other).unwrap_or_default(),
        }
    }

    fn tools() -> Vec<Value> {
        vec![
            json!({
                "name": "muis_capture",
                "description": "Read the last lines of a muis terminal tab. Use it to see a build error or whether a dev server started, instead of asking the user.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "tab": { "type": "string", "description": "Tab id from muis_ls; defaults to the active tab." },
                        "lines": { "type": "number", "description": "How many lines, 1-2000 (default 200)." }
                    }
                }
            }),
            json!({
                "name": "muis_ls",
                "description": "List muis sessions and tabs with their cwd, active flag and agent state.",
                "inputSchema": { "type": "object", "properties": {} }
            }),
            json!({
                "name": "muis_focus",
                "description": "Bring a tab to the front and focus its terminal.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "tab": { "type": "string" } },
                    "required": ["tab"]
                }
            }),
            json!({
                "name": "muis_new_tab",
                "description": "Open a new terminal tab, optionally in a directory.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "cwd": { "type": "string" } }
                }
            }),
            json!({
                "name": "muis_view",
                "description": "Render a local file (image, PDF, markdown, diff) in muis's viewer pane.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "path": { "type": "string", "description": "Absolute path." } },
                    "required": ["path"]
                }
            }),
            json!({
                "name": "muis_open_url",
                "description": "Show a URL in muis's browser pane next to the terminal.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "url": { "type": "string", "description": "Absolute http(s) URL." } },
                    "required": ["url"]
                }
            }),
        ]
    }

    /// Map a tool onto its `ui` op.
    fn op_for(tool: &str) -> &'static str {
        match tool {
            "muis_capture" => "capture",
            "muis_ls" => "ls",
            "muis_focus" => "focus",
            "muis_new_tab" => "new-tab",
            "muis_view" => "view",
            "muis_open_url" => "open-url",
            _ => "ls",
        }
    }

    /// Run one tool against the daemon socket.
    fn call(tool: &str, args: Value) -> Result<Value, String> {
        let request = Request::Ui {
            id: 1,
            op: op_for(tool).to_string(),
            args,
        };
        let line = serde_json::to_string(&request).map_err(|e| e.to_string())?;

        let mut guard = SOCKET
            .lock()
            .map_err(|_| "socket lock poisoned".to_string())?;
        if guard.is_none() {
            let stream = UnixStream::connect(default_socket())
                .map_err(|e| format!("muis is not running ({e})"))?;
            stream
                .set_read_timeout(Some(Duration::from_secs(30)))
                .map_err(|e| e.to_string())?;
            *guard = Some(stream);
        }
        let stream = guard.as_mut().expect("stream connected");

        let attempt = stream
            .write_all(format!("{line}\n").as_bytes())
            .and_then(|()| stream.flush());
        if attempt.is_err() {
            // A stale socket: reconnect once, then give up.
            *guard = None;
            return Err("muis connection dropped; retry".to_string());
        }
        let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        let mut reply_line = String::new();
        reader
            .read_line(&mut reply_line)
            .map_err(|e| format!("no answer from muis: {e}"))?;
        match serde_json::from_str::<Reply>(reply_line.trim()) {
            Ok(Reply::UiReply {
                ok: true, result, ..
            }) => Ok(result),
            Ok(Reply::UiReply { result, .. }) => Err(value_text(&result)),
            Ok(Reply::Error { message }) => Err(message),
            Ok(other) => Err(format!("unexpected reply: {other:?}")),
            Err(e) => Err(format!("bad reply: {e}")),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn fake_call(tool: &str, _args: Value) -> Result<Value, String> {
            match tool {
                "muis_ls" => Ok(json!({ "sessions": [] })),
                "muis_capture" => Err("no window attached".to_string()),
                other => Err(format!("unhandled {other}")),
            }
        }

        fn send(line: &str) -> Value {
            let out = dispatch(line, &mut fake_call).expect("a response");
            serde_json::from_str(&out).unwrap()
        }

        #[test]
        fn initialize_reports_tools_capability() {
            let reply = send(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
            assert_eq!(reply["result"]["protocolVersion"], PROTOCOL_VERSION);
            assert!(reply["result"]["capabilities"]["tools"].is_object());
            assert_eq!(reply["result"]["serverInfo"]["name"], "muis");
        }

        #[test]
        fn tools_list_covers_the_control_surface() {
            let reply = send(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
            let names: Vec<&str> = reply["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["name"].as_str().unwrap())
                .collect();
            for expected in [
                "muis_capture",
                "muis_ls",
                "muis_focus",
                "muis_new_tab",
                "muis_view",
                "muis_open_url",
            ] {
                assert!(names.contains(&expected), "missing {expected}");
            }
        }

        #[test]
        fn tool_errors_become_text_results_not_protocol_errors() {
            let reply = send(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"muis_capture","arguments":{}}}"#,
            );
            assert_eq!(reply["result"]["isError"], true);
            assert_eq!(reply["result"]["content"][0]["text"], "no window attached");
        }

        #[test]
        fn unknown_tools_and_methods_are_protocol_errors() {
            let unknown_tool = send(
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
            );
            assert_eq!(unknown_tool["error"]["code"], -32602);
            let unknown_method = send(r#"{"jsonrpc":"2.0","id":5,"method":"nope"}"#);
            assert_eq!(unknown_method["error"]["code"], -32601);
        }

        #[test]
        fn notifications_get_no_response() {
            assert!(dispatch(
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                &mut fake_call
            )
            .is_none());
            assert!(dispatch("not json", &mut fake_call).is_some());
        }
    }
}
