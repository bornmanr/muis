//! muis-server: the muis frontend in a browser, backed by the same pty
//! workers the native shell uses.
//!
//! ```text
//! muis-server [--port N] [--host ADDR] [--no-open] [--testmode]
//! ```
//!
//! The binary is self-contained: it embeds `src-ui/dist`, and it re-execs
//! itself with `--muis-worker` for each session, so a terminal works from a
//! folder of text and one compiled file — no installer, no webview runtime,
//! no sidecar binaries.
//!
//! Hidden modes (`--muis-worker`, `--muis-notify`, `--muis-agent`) are how the
//! opencode plugin and `muis-notify` reach the running terminal when there is
//! no `muis-notify` binary on PATH; the server exports `MUIS_BIN` into every
//! pty for exactly that.

mod assets;
mod auth;
mod logging;
mod opencode;
mod platform;
mod pool;
mod rpc;
mod serve;
mod store;

use std::net::Ipv4Addr;
use std::sync::Arc;

use serde_json::json;

const WORKER_ARG: &str = "--muis-worker";
const NOTIFY_ARG: &str = "--muis-notify";
const AGENT_ARG: &str = "--muis-agent";

const USAGE: &str = "\
usage: muis-server [--port N] [--host ADDR] [--no-open] [--testmode]

Serves the muis terminal UI to a browser on loopback and owns the ptys.
  --port N     listen port (default: ask the OS, so two copies never clash)
  --host ADDR  bind address (default 127.0.0.1; loopback only is supported)
  --no-open    do not launch a browser
  --testmode   fresh sessions, nothing persisted, separate notify socket
";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Options {
    host: String,
    port: u16,
    open_browser: bool,
    test_mode: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            host: Ipv4Addr::LOCALHOST.to_string(),
            port: 0,
            open_browser: true,
            test_mode: false,
        }
    }
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = |flag: &str| {
            iter.next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match arg.as_str() {
            "--port" => {
                options.port = value("--port")?
                    .parse()
                    .map_err(|_| "--port must be a number".to_string())?;
            }
            "--host" => options.host = value("--host")?,
            "--no-open" => options.open_browser = false,
            "--testmode" => options.test_mode = true,
            "-h" | "--help" => return Err(USAGE.to_string()),
            other => return Err(format!("unknown argument: {other}\n\n{USAGE}")),
        }
    }
    Ok(options)
}

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();

    // Hidden modes: this same binary is the worker, the notify client, and
    // the agent reporter, which is what makes the artifact one file.
    match raw.first().map(String::as_str) {
        Some(WORKER_ARG) => std::process::exit(muis_worker::run()),
        Some(NOTIFY_ARG) => std::process::exit(muis_notify::run_with(raw[1..].to_vec())),
        Some(AGENT_ARG) => std::process::exit(muis_agent::run_with(raw[1..].to_vec())),
        _ => {}
    }

    if let Some(path) = logging::init() {
        log::info!("muis-server starting; log file: {}", path.display());
    }

    let options = match parse_args(&raw) {
        Ok(options) => options,
        Err(message) => {
            println!("{message}");
            std::process::exit(if message.starts_with("usage:") { 0 } else { 2 });
        }
    };

    let token = Arc::new(auth::new_token());
    let socket = notify_socket_name(options.test_mode);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    if let Err(message) = runtime.block_on(start(options, token, socket)) {
        eprintln!("muis-server: {message}");
        std::process::exit(1);
    }
}

async fn start(options: Options, token: Arc<String>, socket: String) -> Result<(), String> {
    let addr = serve::socket_addr(&options.host, options.port)?;
    let bound = serve::bind(&addr.to_string()).await?;
    let port = bound.port;

    let worker = pool::WorkerLaunch::self_exe(vec![WORKER_ARG.to_string()])
        .ok_or("cannot locate this executable")?;

    // CLI notifications (`muis-notify`, `muis-agent`, the opencode plugin)
    // arrive on the per-user socket and fan out to every open browser tab.
    let (events, _) = tokio::sync::broadcast::channel::<String>(256);
    let notifier = events.clone();
    if let Err(e) = muis_core::notify::serve(&socket, move |request| {
        use muis_core::notify::ClientRequest;
        let (name, payload) = match request {
            ClientRequest::Notify(req) => ("muis-notify", serde_json::to_value(req)),
            ClientRequest::AgentSession(req) => ("muis-agent-session", serde_json::to_value(req)),
            ClientRequest::AgentActivity(req) => ("muis-agent-activity", serde_json::to_value(req)),
            ClientRequest::OpenUrl(req) => ("muis-open-url", serde_json::to_value(req)),
        };
        let Ok(payload) = payload else {
            return;
        };
        let _ = notifier.send(json!({ "event": name, "payload": payload }).to_string());
    }) {
        log::warn!("notification endpoint unavailable ({socket}): {e}");
    }

    let state = serve::AppState {
        token: Arc::clone(&token),
        worker,
        events,
        test_mode: options.test_mode,
        port,
        socket: socket.clone(),
    };

    let url = format!("http://127.0.0.1:{port}/?t={token}");
    println!("muis serve: {url}");
    println!("state:     {}", muis_core::paths::state_dir().display());
    if !assets::HAS_DIST {
        println!(
            "warning:    built without a frontend bundle (run: npm run build --prefix src-ui)"
        );
    }
    log::info!("serving on port {port}");

    if options.open_browser {
        if let Err(e) = platform::open_url(&url) {
            log::warn!("could not open a browser: {e}");
        }
    }

    serve::run(bound, state).await
}

/// Per-user notification socket. Test mode gets its own so a smoke run never
/// steals notifications from a real muis; `MUIS_SOCKET_NAME` overrides the
/// base name, matching the native shell.
fn notify_socket_name(test_mode: bool) -> String {
    let base = std::env::var("MUIS_SOCKET_NAME")
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(muis_core::notify::socket_name);
    if test_mode {
        format!("{base}-testmode")
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_are_loopback_and_os_assigned() {
        let options = parse_args(&args(&[])).unwrap();
        assert_eq!(options, Options::default());
        assert_eq!(options.host, "127.0.0.1");
        assert_eq!(options.port, 0);
        assert!(options.open_browser);
        assert!(!options.test_mode);
    }

    #[test]
    fn flags_parse_in_any_order() {
        let options = parse_args(&args(&[
            "--testmode",
            "--port",
            "9123",
            "--no-open",
            "--host",
            "127.0.0.1",
        ]))
        .unwrap();
        assert_eq!(options.port, 9123);
        assert!(!options.open_browser);
        assert!(options.test_mode);
    }

    #[test]
    fn bad_args_are_rejected() {
        assert!(parse_args(&args(&["--port"])).is_err());
        assert!(parse_args(&args(&["--port", "x"])).is_err());
        assert!(parse_args(&args(&["--wat"])).is_err());
        assert!(parse_args(&args(&["--help"])).is_err());
    }

    #[test]
    fn test_mode_uses_its_own_socket() {
        let base = muis_core::notify::socket_name();
        assert_eq!(notify_socket_name(false), base);
        assert_eq!(notify_socket_name(true), format!("{base}-testmode"));
    }

    #[test]
    fn socket_addr_requires_a_numeric_host() {
        assert!(serve::socket_addr("127.0.0.1", 0).is_ok());
        assert!(serve::socket_addr("localhost", 0).is_err());
    }
}
