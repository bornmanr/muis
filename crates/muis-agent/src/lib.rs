//! `muis-agent`: report the session an agent TUI currently has open.
//!
//! This is deliberately separate from `muis-notify`: an agent-session report
//! is state, not a notification, and the shell routes it to the tab's
//! resumable metadata instead of the toast/badge path.
//!
//! Wire format: one [`muis_core::notify::AgentSessionRequest`] per
//! connection, carrying `kind: "agent-session"`, the session id (or no
//! session for a clear), an optional directory, and the originating tab.
//!
//! ```text
//! usage: muis-agent session <ses_ID> [--dir DIR] [--todo-done N]
//!                                [--todo-total N] [--tab ID] [--socket NAME]
//!        muis-agent clear [--tab ID] [--socket NAME]
//! ```

use muis_core::notify::{self, AgentSessionRequest};

const USAGE: &str = "\
usage: muis-agent session <ses_ID> [--dir DIR] [--todo-done N] [--todo-total N]
                            [--tab ID] [--socket NAME]
       muis-agent clear [--tab ID] [--socket NAME]

Reports the agent session currently open in a muis tab, so the tab can
resume exactly that session after a restart. Defaults for --socket and
--tab come from the MUIS_SOCKET and MUIS_TAB_ID environment variables.";

#[derive(Debug, Default, PartialEq, Eq)]
struct Cli {
    session: Option<Option<String>>,
    directory: Option<String>,
    tab_id: Option<String>,
    socket: Option<String>,
    todo_done: Option<u32>,
    todo_total: Option<u32>,
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Cli, String> {
    let mut cli = Cli::default();
    let mut args = args;
    let mut command: Option<String> = None;
    let mut positional: Option<String> = None;
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--dir" => cli.directory = Some(value()?),
            "--tab" => cli.tab_id = Some(value()?),
            "--socket" => cli.socket = Some(value()?),
            "--todo-done" => {
                cli.todo_done = Some(value()?.parse().map_err(|_| "todo-done needs a number")?)
            }
            "--todo-total" => {
                cli.todo_total = Some(value()?.parse().map_err(|_| "todo-total needs a number")?)
            }
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
    if let (Some(done), Some(total)) = (cli.todo_done, cli.todo_total) {
        if done > total {
            return Err("todo-done cannot exceed todo-total".into());
        }
    } else if cli.todo_done.is_some() || cli.todo_total.is_some() {
        return Err("todo progress needs both --todo-done and --todo-total".into());
    }
    match command.as_deref() {
        Some("session") => {
            let id = positional.ok_or("session needs a session id")?;
            if !notify::valid_session_id(&id) {
                return Err(format!("invalid session id: {id}"));
            }
            cli.session = Some(Some(id));
        }
        Some("clear") => {
            if positional.is_some() {
                return Err("clear takes no session id".to_string());
            }
            if cli.todo_done.is_some() {
                return Err("clear takes no todo progress".to_string());
            }
            cli.session = Some(None);
        }
        Some(other) => return Err(format!("unknown command: {other}")),
        None => return Err("missing command".to_string()),
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
    let mut req = AgentSessionRequest::new(
        cli.session.unwrap_or(None),
        cli.directory,
        cli.tab_id.or_else(|| std::env::var("MUIS_TAB_ID").ok()),
    );
    if let (Some(done), Some(total)) = (cli.todo_done, cli.todo_total) {
        req = req.with_todos(done, total);
    }

    match notify::send_agent(&socket, &req) {
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
                session: Some(Some("ses_abc123".into())),
                directory: Some("/tmp".into()),
                tab_id: Some("t1".into()),
                socket: Some("s".into()),
                todo_done: None,
                todo_total: None,
            }
        );
    }

    #[test]
    fn parses_todo_progress_and_validates_it() {
        let cli = parse_args(args(&[
            "session",
            "ses_abc123",
            "--todo-done",
            "2",
            "--todo-total",
            "5",
        ]))
        .unwrap();
        assert_eq!(cli.todo_done, Some(2));
        assert_eq!(cli.todo_total, Some(5));
        assert!(parse_args(args(&["session", "ses_abc123", "--todo-done", "6", "--todo-total", "5"])).is_err());
        assert!(parse_args(args(&["session", "ses_abc123", "--todo-done", "1"])).is_err());
        assert!(parse_args(args(&["session", "ses_abc123", "--todo-done", "x", "--todo-total", "1"])).is_err());
        assert!(parse_args(args(&["clear", "--todo-done", "1", "--todo-total", "1"])).is_err());
    }

    #[test]
    fn parses_clear() {
        assert_eq!(parse_args(args(&["clear"])).unwrap().session, Some(None));
        assert!(parse_args(args(&["clear", "ses_x"])).is_err());
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
