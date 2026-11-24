//! `muis-daemon`: owns the worker pool so terminals outlive a window.
//!
//! Not wired into the shell yet. The protocol and lifecycle are exercised by
//! `tests/daemon.rs`; the next change makes the shell its first client.

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
    use muis_daemon::protocol::{Reply, Request};
    use muis_daemon::{default_socket, WorkerEnv, WorkerLaunch, WorkerPool};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    const USAGE: &str = "usage: muis-daemon [--socket PATH] [--worker PATH] [--idle-seconds N]";

    struct Args {
        socket: PathBuf,
        worker: PathBuf,
        idle: u64,
    }

    fn parse_args() -> Result<Args, String> {
        let mut socket = None;
        let mut worker = None;
        let mut idle = 900u64;
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
        })
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

        let clients: Arc<Mutex<Vec<UnixStream>>> = Arc::new(Mutex::new(Vec::new()));
        let client_count = Arc::new(AtomicUsize::new(0));
        let last_empty = Arc::new(Mutex::new(Instant::now()));

        let sink_clients = Arc::clone(&clients);
        let sink = Arc::new(move |event: muis_daemon::SessionEvent| {
            broadcast(&sink_clients, &Reply::event(event));
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
        start_idle_monitor(
            Arc::clone(&client_count),
            Arc::clone(&last_empty),
            args.idle,
        );

        for conn in listener.incoming() {
            let Ok(stream) = conn else { continue };
            client_count.fetch_add(1, Ordering::SeqCst);
            clients
                .lock()
                .unwrap()
                .push(stream.try_clone().expect("clone client"));
            let pool = Arc::clone(&pool);
            let clients = Arc::clone(&clients);
            let client_count = Arc::clone(&client_count);
            let last_empty = Arc::clone(&last_empty);
            std::thread::spawn(move || {
                serve_client(stream, pool, Arc::clone(&clients));
                client_count.fetch_sub(1, Ordering::SeqCst);
                *last_empty.lock().unwrap() = Instant::now();
            });
        }
    }

    fn serve_client(
        stream: UnixStream,
        pool: Arc<WorkerPool>,
        clients: Arc<Mutex<Vec<UnixStream>>>,
    ) {
        let reader = BufReader::new(stream.try_clone().expect("clone for reading"));
        for line in reader.lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            let reply = match serde_json::from_str::<Request>(&line) {
                Ok(Request::Spawn { session }) => pool
                    .spawn_session(&session)
                    .err()
                    .map(|message| Reply::Error { message }),
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
                    clients: clients.lock().unwrap().len(),
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
