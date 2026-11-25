//! `muis-daemon`: owns the worker pool so terminals outlive a window.
//!
//! It also fronts the control surface: session verbs it can answer itself,
//! and `ui` verbs forwarded to the attached window with the reply routed back
//! to the caller. One socket, one place to audit.

#[cfg(not(unix))]
fn main() {
    eprintln!("muis-daemon needs unix sockets");
    std::process::exit(1);
}

#[cfg(unix)]
fn main() {
    imp::run();
}

#[cfg(unix)]
mod imp {
    use muis_daemon::protocol::{Reply, Request, Role};
    use muis_daemon::{default_socket, WorkerEnv, WorkerLaunch, WorkerPool};
    use std::collections::{HashMap, VecDeque};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    const USAGE: &str = "usage: muis-daemon [--socket PATH] [--worker PATH] [--idle-seconds N]\n                     [--history-dir PATH] [--history-bytes N]";

    struct Args {
        socket: PathBuf,
        worker: PathBuf,
        idle: u64,
        history_dir: PathBuf,
        history_cap: u64,
    }

    fn parse_args() -> Result<Args, String> {
        let mut socket = None;
        let mut worker = None;
        let mut idle = 900u64;
        let mut history_dir = None;
        let mut history_cap = muis_daemon::history::DEFAULT_CAP;
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
            match arg.as_str() {
                "--socket" => socket = Some(PathBuf::from(value()?)),
                "--worker" => worker = Some(PathBuf::from(value()?)),
                "--idle-seconds" => {
                    idle = value()?
                        .parse()
                        .map_err(|_| "--idle-seconds needs a number")?
                }
                "--history-dir" => history_dir = Some(PathBuf::from(value()?)),
                "--history-bytes" => {
                    history_cap = value()?
                        .parse()
                        .map_err(|_| "--history-bytes needs a number")?
                }
                "-h" | "--help" => {
                    println!("{USAGE}");
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        let socket = socket.unwrap_or_else(default_socket);
        let worker = worker
            .or_else(|| {
                std::env::current_exe()
                    .ok()?
                    .parent()
                    .map(|dir| dir.join("muis-worker"))
            })
            .ok_or("cannot locate muis-worker next to this binary")?;
        Ok(Args {
            socket,
            worker,
            idle,
            history_dir: history_dir
                .unwrap_or_else(|| muis_core::paths::state_dir().join("history")),
            history_cap,
        })
    }

    /// Everything a client handler needs to reach the other clients.
    #[derive(Clone)]
    struct Shared {
        clients: Arc<Mutex<Vec<UnixStream>>>,
        /// Attached windows, oldest first. Several windows can share one
        /// daemon; a request goes to the newest, which is the one the user
        /// most recently opened. A window that reconnects replaces its own
        /// entry rather than stacking.
        ui: Arc<Mutex<Vec<(u64, UnixStream)>>>,
        /// Forwarded request slug -> (caller stream, caller's own id).
        pending: Arc<Mutex<HashMap<u64, (UnixStream, u64)>>>,
        next_slug: Arc<AtomicU64>,
        next_conn: Arc<AtomicU64>,
        /// Tail of each session's output, so a window that reconnects can
        /// rebuild the screen. Bytes only; the daemon never parses them.
        output: Arc<Mutex<HashMap<String, Tail>>>,
        /// Append-only history files, keyed by session.
        history: Arc<Mutex<HashMap<String, muis_daemon::history::HistoryFile>>>,
        history_dir: PathBuf,
        history_cap: u64,
    }

    /// Cap per session. Enough for a screen or three of history without
    /// letting a chatty agent pin memory in the daemon.
    const TAIL_BYTES: usize = 128 * 1024;

    #[derive(Default)]
    struct Tail {
        pty_id: String,
        bytes: VecDeque<u8>,
    }

    impl Tail {
        fn push(&mut self, pty_id: &str, data: &[u8]) {
            self.pty_id = pty_id.to_string();
            self.bytes.extend(data);
            while self.bytes.len() > TAIL_BYTES {
                self.bytes.pop_front();
            }
        }
    }

    pub fn run() {
        let args = match parse_args() {
            Ok(args) => args,
            Err(e) => {
                eprintln!("muis-daemon: {e}\n{USAGE}");
                std::process::exit(2);
            }
        };

        if let Some(dir) = args.socket.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // A leftover socket file is not an error; a live one is.
        if args.socket.exists() {
            if UnixStream::connect(&args.socket).is_ok() {
                eprintln!("muis-daemon: already running on {}", args.socket.display());
                std::process::exit(1);
            }
            let _ = std::fs::remove_file(&args.socket);
        }

        let listener = match UnixListener::bind(&args.socket) {
            Ok(listener) => listener,
            Err(e) => {
                eprintln!("muis-daemon: cannot bind {}: {e}", args.socket.display());
                std::process::exit(1);
            }
        };
        let _ = std::fs::set_permissions(&args.socket, std::fs::Permissions::from_mode(0o600));

        let shared = Shared {
            clients: Arc::new(Mutex::new(Vec::new())),
            ui: Arc::new(Mutex::new(Vec::new())),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_slug: Arc::new(AtomicU64::new(1)),
            next_conn: Arc::new(AtomicU64::new(1)),
            output: Arc::new(Mutex::new(HashMap::new())),
            history: Arc::new(Mutex::new(HashMap::new())),
            history_dir: args.history_dir.clone(),
            history_cap: args.history_cap,
        };

        let sink_shared = shared.clone();
        let sink = Arc::new(move |event: muis_daemon::SessionEvent| {
            if let muis_core::ipc::WorkerToUi::Output { pty_id, data_b64 } = &event.frame {
                if let Ok(bytes) = muis_core::ipc::b64_decode(data_b64) {
                    let mut output = sink_shared.output.lock().unwrap();
                    output
                        .entry(event.session_id.clone())
                        .or_default()
                        .push(pty_id, &bytes);
                    drop(output);
                    if sink_shared.history_cap > 0 {
                        let mut files = sink_shared.history.lock().unwrap();
                        let entry = files.entry(event.session_id.clone()).or_insert_with(|| {
                            muis_daemon::history::HistoryFile::open(
                                &sink_shared.history_dir,
                                &event.session_id,
                                sink_shared.history_cap,
                            )
                            .unwrap_or_else(|_| {
                                muis_daemon::history::HistoryFile::open(
                                    std::env::temp_dir().as_path(),
                                    "disabled",
                                    0,
                                )
                                .expect("temp history")
                            })
                        });
                        let _ = entry.append(&bytes);
                    }
                }
            }
            broadcast(&sink_shared.clients, &Reply::event(event));
            true
        });

        let env = WorkerEnv {
            socket: std::env::var("MUIS_SOCKET").ok(),
            agent_bin: args
                .worker
                .parent()
                .map(|dir| dir.join("muis-agent"))
                .filter(|path| path.is_file()),
        };
        let pool = Arc::new(WorkerPool::new(
            sink,
            Some(WorkerLaunch(args.worker.clone())),
            env,
        ));

        eprintln!("muis-daemon: listening on {}", args.socket.display());
        let client_count = Arc::new(AtomicUsize::new(0));
        let last_empty = Arc::new(Mutex::new(Instant::now()));
        start_idle_monitor(
            Arc::clone(&client_count),
            Arc::clone(&last_empty),
            args.idle,
        );

        for conn in listener.incoming() {
            let Ok(stream) = conn else { continue };
            let conn_id = shared.next_conn.fetch_add(1, Ordering::SeqCst);
            client_count.fetch_add(1, Ordering::SeqCst);
            shared
                .clients
                .lock()
                .unwrap()
                .push(stream.try_clone().expect("clone client"));
            let pool = Arc::clone(&pool);
            let shared = shared.clone();
            let client_count = Arc::clone(&client_count);
            let last_empty = Arc::clone(&last_empty);
            std::thread::spawn(move || {
                serve_client(stream, pool, shared.clone(), conn_id);
                {
                    // The window went away; drop its slot so requests go to a
                    // live window instead of a dead socket.
                    let mut windows = shared.ui.lock().unwrap();
                    let before = windows.len();
                    windows.retain(|(id, _)| *id != conn_id);
                    if windows.len() != before {
                        eprintln!("muis-daemon: window detached ({} alive)", windows.len());
                    }
                }
                client_count.fetch_sub(1, Ordering::SeqCst);
                *last_empty.lock().unwrap() = Instant::now();
            });
        }
    }

    fn serve_client(stream: UnixStream, pool: Arc<WorkerPool>, shared: Shared, conn_id: u64) {
        let reader = BufReader::new(stream.try_clone().expect("clone for reading"));
        for line in reader.lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            let reply = match serde_json::from_str::<Request>(&line) {
                Ok(Request::Hello { role }) => {
                    if role == Role::Ui {
                        let mut windows = shared.ui.lock().unwrap();
                        windows.retain(|(id, _)| *id != conn_id);
                        windows.push((conn_id, stream.try_clone().expect("clone ui")));
                        eprintln!("muis-daemon: window attached ({} alive)", windows.len());
                    }
                    // Acknowledge so the client knows it is registered before
                    // it relies on that (and tests never race the accept).
                    Some(Reply::Hello { role })
                }
                Ok(Request::Ui { id, op, args }) => {
                    let ui = shared
                        .ui
                        .lock()
                        .unwrap()
                        .last()
                        .and_then(|(_, s)| s.try_clone().ok());
                    match ui {
                        Some(ui) => {
                            let slug = shared.next_slug.fetch_add(1, Ordering::SeqCst);
                            shared
                                .pending
                                .lock()
                                .unwrap()
                                .insert(slug, (stream.try_clone().expect("clone caller"), id));
                            broadcast_one(&ui, &Reply::UiRequest { id: slug, op, args });
                            None
                        }
                        None => Some(Reply::Error {
                            message: "no window attached".to_string(),
                        }),
                    }
                }
                Ok(Request::UiReply { id, ok, result }) => {
                    if let Some((caller, client_id)) = shared.pending.lock().unwrap().remove(&id) {
                        broadcast_one(
                            &caller,
                            &Reply::UiReply {
                                id: client_id,
                                ok,
                                result,
                            },
                        );
                    }
                    None
                }
                Ok(Request::Spawn { session }) => match pool.spawn_session(&session) {
                    Ok(()) => {
                        // Replay what this session already printed, to this
                        // client only: a fresh window gets its screen back
                        // without disturbing anyone else.
                        let tail = shared.output.lock().unwrap();
                        if let Some(tail) = tail.get(&session) {
                            if !tail.bytes.is_empty() {
                                let data: Vec<u8> = tail.bytes.iter().copied().collect();
                                broadcast_one(
                                    &stream,
                                    &Reply::Event {
                                        session_id: session.clone(),
                                        frame: muis_core::ipc::WorkerToUi::Output {
                                            pty_id: tail.pty_id.clone(),
                                            data_b64: muis_core::ipc::b64_encode(&data),
                                        },
                                    },
                                );
                            }
                        }
                        None
                    }
                    Err(message) => Some(Reply::Error { message }),
                },
                Ok(Request::Send { session, line }) => pool
                    .send_line(&session, &line)
                    .err()
                    .map(|message| Reply::Error { message }),
                Ok(Request::Stop { session }) => {
                    pool.stop_session(&session);
                    None
                }
                Ok(Request::Status) => Some(Reply::Status {
                    sessions: pool.sessions(),
                    clients: shared.clients.lock().unwrap().len(),
                }),
                Err(e) => Some(Reply::Error {
                    message: format!("bad request: {e}"),
                }),
            };
            if let Some(reply) = reply {
                broadcast_one(&stream, &reply);
            }
        }
    }

    fn broadcast(clients: &Arc<Mutex<Vec<UnixStream>>>, reply: &Reply) {
        let Ok(line) = serde_json::to_string(reply) else {
            return;
        };
        let mut clients = clients.lock().unwrap();
        clients.retain_mut(|stream| {
            stream
                .write_all(format!("{line}\n").as_bytes())
                .and_then(|()| stream.flush())
                .is_ok()
        });
    }

    fn broadcast_one(stream: &UnixStream, reply: &Reply) {
        if let Ok(line) = serde_json::to_string(reply) {
            let mut stream = stream;
            let _ = stream.write_all(format!("{line}\n").as_bytes());
        }
    }

    /// Exit once nobody has been attached for `idle` seconds. `0` disables it:
    /// a daemon holding running agents should not be killed by a timer.
    fn start_idle_monitor(clients: Arc<AtomicUsize>, last_empty: Arc<Mutex<Instant>>, idle: u64) {
        if idle == 0 {
            return;
        }
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            if clients.load(Ordering::SeqCst) > 0 {
                *last_empty.lock().unwrap() = Instant::now();
                continue;
            }
            if last_empty.lock().unwrap().elapsed() >= Duration::from_secs(idle) {
                eprintln!("muis-daemon: idle for {idle}s, exiting");
                std::process::exit(0);
            }
        });
    }
}
