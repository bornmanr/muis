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
//!        muis-agent report <working|blocked|done|idle> [--agent NAME]
//!                   [--model NAME] [--session ses_ID] [--tab ID]
//! ```

use muis_core::notify::{self, AgentSessionRequest, AgentStateRequest, OpenUrlRequest};

const USAGE: &str = "\
usage: muis-agent session <ses_ID> [--dir DIR] [--tab ID] [--socket NAME]
       muis-agent clear [--tab ID] [--socket NAME]
       muis-agent open <URL> [--session ses_ID] [--tab ID] [--socket NAME]
       muis-agent report <working|blocked|done|idle> [--agent NAME]
                  [--model NAME] [--session ses_ID] [--tab ID] [--socket NAME]

session/clear report the agent session currently open in a muis tab, so the
tab can resume exactly that session after a restart. open shows an http(s)
URL in muis's browser pane next to the terminal. report says what the agent
is doing right now; `blocked` is what puts a tab in the needs-you queue.
Defaults for --socket and --tab come from the MUIS_SOCKET and MUIS_TAB_ID
environment variables.";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Session(String),
    Clear,
    Open {
        url: String,
    },
    Report {
        state: String,
        agent: Option<String>,
        model: Option<String>,
    },
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Cli {
    command: Option<Command>,
    directory: Option<String>,
    tab_id: Option<String>,
    socket: Option<String>,
    session: Option<String>,
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Cli, String> {
    let mut cli = Cli::default();
    let mut args = args;
    let mut command: Option<String> = None;
    let mut positional: Option<String> = None;
    let mut agent: Option<String> = None;
    let mut model: Option<String> = None;
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--dir" => cli.directory = Some(value()?),
            "--tab" => cli.tab_id = Some(value()?),
            "--socket" => cli.socket = Some(value()?),
            "--session" => cli.session = Some(value()?),
            "--agent" => agent = Some(value()?),
            "--model" => model = Some(value()?),
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
    let saw_session = cli.session.is_some();
    let saw_agent = agent.is_some() || model.is_some();
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
            if let Some(id) = cli.session.as_deref() {
                if !notify::valid_session_id(id) {
                    return Err(format!("invalid session id: {id}"));
                }
            }
            Command::Open { url }
        }
        Some("report") => {
            let state = positional.ok_or("report needs a state")?;
            if !notify::valid_agent_state(&state) {
                return Err(format!(
                    "unknown state: {state} (expected one of {})",
                    notify::AGENT_STATES.join(", ")
                ));
            }
            if let Some(id) = cli.session.as_deref() {
                if !notify::valid_session_id(id) {
                    return Err(format!("invalid session id: {id}"));
                }
            }
            Command::Report {
                state,
                agent,
                model,
            }
        }
        Some(other) => return Err(format!("unknown command: {other}")),
        None => return Err("missing command".to_string()),
    });
    let command_allows_session = matches!(
        cli.command,
        Some(Command::Open { .. }) | Some(Command::Report { .. })
    );
    let command_allows_agent = matches!(cli.command, Some(Command::Report { .. }));
    if saw_session && !command_allows_session {
        return Err("--session is only valid with open or report".to_string());
    }
    if saw_agent && !command_allows_agent {
        return Err("--agent/--model are only valid with report".to_string());
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
        Some(Command::Open { url }) => {
            // parse_args already rejected non-http(s); this keeps the
            // invariant local instead of trusting the caller.
            let Some(req) = OpenUrlRequest::new(&url, tab, cli.session) else {
                eprintln!("muis-agent: open needs an http(s) URL: {url}");
                return 2;
            };
            notify::send_open_url(&socket, &req)
        }
        Some(Command::Report {
            state,
            agent,
            model,
        }) => {
            let Some(req) = AgentStateRequest::new(&state, tab, cli.session, agent, model) else {
                eprintln!("muis-agent: unknown state: {state}");
                return 2;
            };
            notify::send_agent_state(&socket, &req)
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
                session: None,
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
            })
        );
        assert_eq!(cli.session.as_deref(), Some("ses_abc123"));

        let bare = parse_args(args(&["open", "https://example.com"])).unwrap();
        assert_eq!(
            bare.command,
            Some(Command::Open {
                url: "https://example.com".into(),
            })
        );
        assert_eq!(bare.session, None);
    }

    #[test]
    fn parses_report_with_labels_and_routing() {
        let cli = parse_args(args(&[
            "report",
            "blocked",
            "--agent",
            "opencode",
            "--model",
            "opus",
            "--session",
            "ses_abc123",
            "--tab",
            "t1",
        ]))
        .unwrap();
        assert_eq!(
            cli.command,
            Some(Command::Report {
                state: "blocked".into(),
                agent: Some("opencode".into()),
                model: Some("opus".into()),
            })
        );
        assert_eq!(cli.session.as_deref(), Some("ses_abc123"));

        // The state alone is enough: the tab and socket come from the env.
        let bare = parse_args(args(&["report", "done"])).unwrap();
        assert_eq!(
            bare.command,
            Some(Command::Report {
                state: "done".into(),
                agent: None,
                model: None,
            })
        );
    }

    #[test]
    fn report_rejects_unknown_states_and_misplaced_flags() {
        assert!(parse_args(args(&["report"])).is_err());
        assert!(parse_args(args(&["report", "running"])).is_err());
        assert!(parse_args(args(&["report", "DONE"])).is_err());
        assert!(parse_args(args(&["report", "done", "extra"])).is_err());
        assert!(parse_args(args(&["report", "done", "--session", "abc"])).is_err());
        // Labels belong to report, not to the other commands.
        assert!(parse_args(args(&["session", "ses_a", "--agent", "x"])).is_err());
        assert!(parse_args(args(&["clear", "--model", "x"])).is_err());
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
