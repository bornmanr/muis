//! `muis-ctl`: one-shot control of a running muis, over the daemon socket.
//!
//! Deliberately small for now: session-level verbs that the daemon can answer
//! without a window (list, status, stop, raw send). UI verbs arrive when the
//! daemon can forward them to the attached shell.

use muis_daemon::default_socket;
use muis_daemon::protocol::{Reply, Request};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

const USAGE: &str = "\
usage: muis-ctl [--socket PATH] <command>

  ls                 sessions with a live worker, one per line
  status             the same list as JSON, with the attached client count
  stop <session>     kill that session's worker
  send <session> <line>
                     forward one UiToWorker JSON line to that worker

The socket defaults to the one muis uses ($XDG_RUNTIME_DIR/muis/daemon.sock).";

enum Command {
    Ls,
    Status,
    Stop(String),
    Send(String, String),
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<(Option<PathBuf>, Command), String> {
    let mut socket = None;
    let mut positional: Vec<String> = Vec::new();
    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => socket = Some(PathBuf::from(args.next().ok_or("--socket needs a path")?)),
            "-h" | "--help" => return Err(String::new()),
            other if other.starts_with('-') => return Err(format!("unknown argument: {other}")),
            other => positional.push(other.to_string()),
        }
    }
    let mut rest = positional.into_iter();
    let command = match rest.next().as_deref() {
        Some("ls") => Command::Ls,
        Some("status") => Command::Status,
        Some("stop") => Command::Stop(rest.next().ok_or("stop needs a session")?),
        Some("send") => {
            let session = rest.next().ok_or("send needs a session")?;
            let line = rest.next().ok_or("send needs a UiToWorker JSON line")?;
            Command::Send(session, line)
        }
        Some(other) => return Err(format!("unknown command: {other}")),
        None => return Err("missing command".into()),
    };
    if rest.next().is_some() {
        return Err("too many arguments".into());
    }
    Ok((socket, command))
}

fn main() {
    let (socket, command) = match parse_args(std::env::args().skip(1)) {
        Ok(parsed) => parsed,
        Err(e) if e.is_empty() => {
            println!("{USAGE}");
            return;
        }
        Err(e) => {
            eprintln!("muis-ctl: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    let socket = socket.unwrap_or_else(default_socket);
    match run(&socket, command) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("muis-ctl: {e} (socket {})", socket.display());
            std::process::exit(1);
        }
    }
}

fn run(socket: &PathBuf, command: Command) -> Result<(), String> {
    let mut stream = UnixStream::connect(socket).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let request = match &command {
        Command::Ls | Command::Status => Request::Status,
        Command::Stop(session) => Request::Stop {
            session: session.clone(),
        },
        Command::Send(session, line) => Request::Send {
            session: session.clone(),
            line: line.clone(),
        },
    };
    let text = serde_json::to_string(&request).map_err(|e| e.to_string())?;
    stream
        .write_all(format!("{text}\n").as_bytes())
        .and_then(|()| stream.flush())
        .map_err(|e| e.to_string())?;

    match &command {
        // Fire-and-forget verbs have no reply to wait for.
        Command::Stop(_) | Command::Send(..) => Ok(()),
        Command::Ls => {
            let reply = read_reply(stream)?;
            print_sessions(reply);
            Ok(())
        }
        Command::Status => {
            let reply = read_reply(stream)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&reply_json(reply)).unwrap_or_default()
            );
            Ok(())
        }
    }
}

fn read_reply(stream: UnixStream) -> Result<Reply, String> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    if line.trim().is_empty() {
        return Err("muis closed the connection".into());
    }
    serde_json::from_str(line.trim()).map_err(|e| e.to_string())
}

fn reply_json(reply: Reply) -> serde_json::Value {
    serde_json::to_value(reply).unwrap_or(serde_json::Value::Null)
}

fn print_sessions(reply: Reply) {
    match reply {
        Reply::Status { sessions, .. } => {
            for session in sessions {
                println!("{session}");
            }
        }
        Reply::Error { message } => eprintln!("muis-ctl: {message}"),
        other => eprintln!("muis-ctl: unexpected reply: {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(list: &[&str]) -> Result<(Option<PathBuf>, Command), String> {
        parse_args(list.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_the_session_verbs() {
        assert!(matches!(parse(&["ls"]).unwrap().1, Command::Ls));
        assert!(matches!(parse(&["status"]).unwrap().1, Command::Status));
        assert!(matches!(
            parse(&["stop", "s1"]).unwrap().1,
            Command::Stop(s)
            if s == "s1"
        ));
        assert!(matches!(
            parse(&["send", "s1", r#"{"type":"write"}"#]).unwrap().1,
            Command::Send(s, l) if s == "s1" && l.contains("write")
        ));
        assert_eq!(
            parse(&["--socket", "/tmp/x.sock", "ls"]).unwrap().0,
            Some(PathBuf::from("/tmp/x.sock"))
        );
    }

    #[test]
    fn rejects_missing_or_extra_arguments() {
        assert!(parse(&[]).is_err());
        assert!(parse(&["nope"]).is_err());
        assert!(parse(&["stop"]).is_err());
        assert!(parse(&["send", "s1"]).is_err());
        assert!(parse(&["ls", "extra"]).is_err());
        assert!(parse(&["--socket"]).is_err());
    }
}
