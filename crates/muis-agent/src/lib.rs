//! `muis-agent`: report the session an agent TUI currently has open, and ask
//! the terminal to show a URL in its browser pane.
//!
//! This is deliberately separate from `muis-notify`: an agent-session report
//! is state, not a notification, and the shell routes it to the tab's
//! resumable metadata instead of the toast/badge path. An open-url request is
//! the same kind of direct terminal control: it feeds the in-window preview
//! pane, not a toast.
//!
//! Wire format: one [`muis_core::notify::AgentSessionRequest`] or
//! [`muis_core::notify::OpenUrlRequest`] per connection, discriminated by
//! `kind`.
//!
//! ```text
//! usage: muis-agent session <ses_ID> [--dir DIR] [--tab ID] [--socket NAME]
//!        muis-agent clear [--tab ID] [--socket NAME]
//!        muis-agent open <URL> [--session ses_ID] [--tab ID] [--socket NAME]
//! ```

use muis_core::notify::{self, AgentSessionRequest, OpenUrlRequest};

const USAGE: &str = "\
usage: muis-agent session <ses_ID> [--dir DIR] [--tab ID] [--socket NAME]
       muis-agent clear [--tab ID] [--socket NAME]
       muis-agent open <URL> [--session ses_ID] [--tab ID] [--socket NAME]

session/clear report the agent session currently open in a muis tab, so the
tab can resume exactly that session after a restart. open shows an http(s)
URL in muis's browser pane next to the terminal. Defaults for --socket and
--tab come from the MUIS_SOCKET and MUIS_TAB_ID environment variables.";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Session(String),
    Clear,
    Open {
        url: String,
        session: Option<String>,
    },
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Cli {
    command: Option<Command>,
    directory: Option<String>,
    tab_id: Option<String>,
    socket: Option<String>,
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Cli, String> {
    let mut cli = Cli::default();
    let mut args = args;
    let mut command: Option<String> = None;
    let mut positional: Option<String> = None;
    let mut session: Option<String> = None;
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--dir" => cli.directory = Some(value()?),
            "--tab" => cli.tab_id = Some(value()?),
            "--socket" => cli.socket = Some(value()?),
            "--session" => session = Some(value()?),
            other if other.starts_with('-') => {
                return Err(format!("unknown argument: {other}"));
            }
            other => {
                if command.is_none() {
                    command = Some(other.to_string());
                } else if positional.is_none() {
                    positional = Some(other.to_string());
                } else {
                    return Err(format!("unexpected argument: {other}"));
                }
            }
        }
    }
    let saw_session = session.is_some();
    cli.command = Some(match command.as_deref() {
        Some("session") => {
            let id = positional.ok_or("session needs a session id")?;
            if !notify::valid_session_id(&id) {
                return Err(format!("invalid session id: {id}"));
            }
            Command::Session(id)
        }
        Some("clear") => {
            if positional.is_some() {
                return Err("clear takes no session id".to_string());
            }
            Command::Clear
        }
        Some("open") => {
            let url = positional.ok_or("open needs a URL")?;
            if !notify::valid_http_url(&url) {
                return Err(format!("open needs an http(s) URL: {url}"));
            }
            if let Some(id) = session.as_deref() {
                if !notify::valid_session_id(id) {
                    return Err(format!("invalid session id: {id}"));
                }
            }
            Command::Open { url, session }
        }
        Some(other) => return Err(format!("unknown command: {other}")),
        None => return Err("missing command".to_string()),
    });
    if saw_session && !matches!(cli.command, Some(Command::Open { .. })) {
        return Err("--session is only valid with open".to_string());
    }
    Ok(cli)
}

/// Standalone entrypoint: parse `std::env::args()` without the program name.
pub fn run() -> i32 {
    run_with(std::env::args().skip(1))
}

/// Run with an explicit argument list (without the program name). The
/// standalone binary passes `args().skip(1)`; the Windows shell passes the
/// arguments after its `--muis-agent` mode flag.
pub fn run_with(args: impl IntoIterator<Item = String>) -> i32 {
    let args: Vec<String> = args.into_iter().collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return 0;
    }
    let cli = match parse_args(args.into_iter()) {
        Ok(cli) => cli,
        Err(e) => {
            eprintln!("muis-agent: {e}\n{USAGE}");
            return 2;
        }
    };

    let socket = cli
        .socket
        .or_else(|| std::env::var("MUIS_SOCKET").ok())
        .unwrap_or_else(notify::socket_name);
    let tab = cli.tab_id.or_else(|| std::env::var("MUIS_TAB_ID").ok());

    let result = match cli.command {
        Some(Command::Session(id)) => {
            let req = AgentSessionRequest::new(Some(id), cli.directory, tab);
            notify::send_agent(&socket, &req)
        }
        Some(Command::Clear) => {
            let req = AgentSessionRequest::new(None, None, tab);
            notify::send_agent(&socket, &req)
        }
        Some(Command::Open { url, session }) => {
            // parse_args already rejected non-http(s); this keeps the
            // invariant local instead of trusting the caller.
            let Some(req) = OpenUrlRequest::new(&url, tab, session) else {
                eprintln!("muis-agent: open needs an http(s) URL: {url}");
                return 2;
            };
            notify::send_open_url(&socket, &req)
        }
        None => return 2,
    };

    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("muis-agent: could not reach muis (socket {socket}): {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> std::vec::IntoIter<String> {
        list.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn parses_session_with_optional_fields() {
        let cli = parse_args(args(&[
            "session",
            "ses_abc123",
            "--dir",
            "/tmp",
            "--tab",
            "t1",
            "--socket",
            "s",
        ]))
        .unwrap();
        assert_eq!(
            cli,
            Cli {
                command: Some(Command::Session("ses_abc123".into())),
                directory: Some("/tmp".into()),
                tab_id: Some("t1".into()),
                socket: Some("s".into()),
            }
        );
    }

    #[test]
    fn parses_clear() {
        assert_eq!(
            parse_args(args(&["clear"])).unwrap().command,
            Some(Command::Clear)
        );
        assert!(parse_args(args(&["clear", "ses_x"])).is_err());
    }

    #[test]
    fn parses_open_with_an_optional_session() {
        let cli = parse_args(args(&[
            "open",
            "http://localhost:5173/",
            "--session",
            "ses_abc123",
            "--tab",
            "t1",
        ]))
        .unwrap();
        assert_eq!(
            cli.command,
            Some(Command::Open {
                url: "http://localhost:5173/".into(),
                session: Some("ses_abc123".into()),
            })
        );

        let bare = parse_args(args(&["open", "https://example.com"])).unwrap();
        assert_eq!(
            bare.command,
            Some(Command::Open {
                url: "https://example.com".into(),
                session: None,
            })
        );
    }

    #[test]
    fn open_rejects_non_http_urls_and_stray_session() {
        assert!(parse_args(args(&["open"])).is_err());
        assert!(parse_args(args(&["open", "localhost:5173"])).is_err());
        assert!(parse_args(args(&["open", "file:///etc/passwd"])).is_err());
        assert!(parse_args(args(&["open", "javascript:alert(1)"])).is_err());
        assert!(parse_args(args(&["open", "http://x", "--session", "abc"])).is_err());
        assert!(parse_args(args(&["session", "ses_a", "--session", "ses_b"])).is_err());
    }

    #[test]
    fn rejects_bad_ids_and_unknown_flags() {
        assert!(parse_args(args(&["session", "abc"])).is_err());
        assert!(parse_args(args(&["session", "ses_"])).is_err());
        assert!(parse_args(args(&["session", "ses_a;rm"])).is_err());
        assert!(parse_args(args(&["--nope"])).is_err());
        assert!(parse_args(args(&[])).is_err());
        assert!(parse_args(args(&["wat"])).is_err());
    }
}
