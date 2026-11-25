//! muis shell: the native window around the xterm.js frontend.
//!
//! The shell owns no terminal state. It spawns one `muis-worker` process
//! per session (see docs/archive/architecture-rust-xterm.md) and renders
//! src-ui. A worker crash therefore takes down one session, never the
//! window or the other sessions.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

mod backend;
mod bridge;
mod logging;
mod persist;
mod shell_integration;

use backend::Backend;
use bridge::{env_for_shell, EventSink, WorkerLaunch, WORKER_EVENT};
use muis_core::notify::ClientRequest;
use tauri::{Emitter, Manager};

/// Locate the worker binary in `dir`. Prefers the plain `muis-worker`
/// sibling that the Arch package ships; falls back to a triple-suffixed
/// sidecar name (`muis-worker-x86_64-unknown-linux-gnu`).
fn find_worker_in(dir: &Path) -> Option<PathBuf> {
    let plain = dir.join("muis-worker");
    if plain.is_file() {
        return Some(plain);
    }
    let mut matches: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("muis-worker-"))
                .unwrap_or(false)
        })
        .collect();
    matches.sort();
    matches.into_iter().next()
}

/// Locate the sibling worker binary next to the shell executable, as the
/// Linux package ships it (`muis` + `muis-worker`).
fn worker_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    find_worker_in(exe.parent()?)
}

/// How the shell starts a session worker: a sibling `muis-worker` process.
fn worker_launch() -> Option<WorkerLaunch> {
    worker_path().map(WorkerLaunch)
}

/// Serve a local file to the browser pane. The viewer keeps an allow-list of
/// paths the UI asked for, so the returned loopback URL reads exactly one
/// file and nothing else.
/// Loopback URL for a local file. One server per process; the viewer keeps
/// its own allow-list of what the UI asked for.
fn viewer_url(path: &str) -> Result<String, String> {
    use std::sync::OnceLock;
    static VIEWER: OnceLock<Result<muis_core::view::ViewServer, String>> = OnceLock::new();
    let server = VIEWER.get_or_init(|| {
        muis_core::view::ViewServer::start().map_err(|e| format!("viewer unavailable: {e}"))
    });
    server
        .as_ref()
        .map_err(|e| e.clone())?
        .url_for(std::path::Path::new(path))
}

#[tauri::command]
fn view_file(path: String) -> Result<String, String> {
    viewer_url(&path)
}

/// Create a worktree for a new tab and return its path.
///
/// The frontend passes the active tab's cwd and a name; the rest (repo root,
/// branch, location) is git's answer, never a shell string.
#[tauri::command]
fn worktree_create(cwd: String, name: String) -> Result<String, String> {
    muis_core::worktree::create(std::path::Path::new(&cwd), &name)
        .map(|path| path.display().to_string())
}

/// Listening TCP ports whose owner lives in (or below) `cwd`. Read-only
/// `/proc` scraping, used for the tab tooltip and the control surface.
#[tauri::command]
fn list_ports(cwd: String) -> Vec<u16> {
    muis_core::ports::listening_under(std::path::Path::new(&cwd))
}

#[tauri::command]
fn ping() -> String {
    "pong".to_string()
}

#[tauri::command]
fn worker_available() -> bool {
    worker_launch().is_some()
}

/// Whether this window is running in `--testmode`; the frontend shows a
/// marker so a test window is never mistaken for the real one.
#[tauri::command]
fn is_test_mode() -> bool {
    test_mode()
}

#[tauri::command]
fn default_shell() -> String {
    muis_core::shell::default_shell()
        .to_string_lossy()
        .into_owned()
}

#[tauri::command]
fn default_cwd() -> String {
    muis_core::paths::home_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".to_string())
}

/// Optional input for the native PTY smoke harness. Kept debug-build-only
/// so release binaries never execute environment-provided shell commands.
#[tauri::command]
fn live_test_command() -> Option<String> {
    #[cfg(debug_assertions)]
    {
        return std::env::var("MUIS_TEST_COMMAND")
            .ok()
            .filter(|command| !command.trim().is_empty());
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

/// Frontend diagnosis snapshot (no devtools in release builds).
#[tauri::command]
fn debug_report(json: String) {
    log::debug!("frontend report: {json}");
}

/// Path of the diagnostics log file, so the UI can point users at it when
/// something fails (no console in release builds).
#[tauri::command]
fn log_path() -> String {
    logging::log_file().to_string_lossy().into_owned()
}

#[tauri::command]
fn config_load() -> Result<String, String> {
    persist::read_text(&muis_core::paths::config_file())
}

#[tauri::command]
fn config_save(json: String) -> Result<(), String> {
    // Settings are loaded in test mode (so the run reflects the real
    // config), but never written, so a test can't overwrite them.
    if test_mode() {
        return Ok(());
    }
    persist::write_json(&muis_core::paths::config_file(), &json)
}

#[tauri::command]
fn sessions_load() -> Result<String, String> {
    // Test mode starts fresh so a smoke run never resurrects real tabs.
    if test_mode() {
        return Ok(String::new());
    }
    persist::read_text(&muis_core::paths::sessions_file())
}

#[tauri::command]
fn sessions_save(json: String) -> Result<(), String> {
    if test_mode() {
        return Ok(());
    }
    persist::write_json(&muis_core::paths::sessions_file(), &json)
}

/// Current git branch for a cwd; empty string when not a repo (or git
/// missing/slow — the caller treats empty as "no pill data").
#[tauri::command]
fn git_branch(cwd: String) -> String {
    std::process::Command::new("git")
        .args(["-C", &cwd, "branch", "--show-current"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// `muis --testmode`: start from a fresh session and never persist anything.
/// The real config file is left untouched.
static TEST_MODE: AtomicBool = AtomicBool::new(false);

fn test_mode() -> bool {
    TEST_MODE.load(Ordering::Relaxed)
}

/// Notification socket this shell serves and injects into every pty. Test
/// mode uses a distinct socket so it never collides with a running muis (and
/// its terminals' `muis-notify` reaches the test window, not the real one).
/// Show a desktop toast whose click focuses `tab_id`.
///
/// The Tauri notification plugin does not surface D-Bus actions, so Linux
/// talks to `org.freedesktop.Notifications` through `notify-rust` directly.
/// `wait_for_action` blocks until the toast is clicked or closed, hence the
/// thread; a closed toast never emits.
#[tauri::command]
fn notify_clickable(
    app: tauri::AppHandle,
    title: String,
    body: String,
    tab_id: String,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        std::thread::spawn(move || {
            let mut notification = notify_rust::Notification::new();
            notification
                .summary(&title)
                .body(&body)
                .appname("muis")
                .action("default", "Show tab");
            match notification.show() {
                Ok(handle) => handle.wait_for_action(|action| {
                    if action != "__closed" {
                        let _ = app.emit("muis-focus-tab", serde_json::json!({ "tab_id": tab_id }));
                    }
                }),
                Err(e) => log::warn!("desktop notification failed: {e}"),
            }
        });
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, title, body, tab_id);
        Err("click-to-focus needs the Linux notifications interface".to_string())
    }
}

/// `MUIS_SOCKET_NAME` overrides the base name (the live smoke test sets it so
/// it does not fight a real muis for the per-user socket).
pub(crate) fn notify_socket_name() -> String {
    let base = std::env::var("MUIS_SOCKET_NAME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(muis_core::notify::socket_name);
    if test_mode() {
        format!("{base}-testmode")
    } else {
        base
    }
}

/// True when `flag` appears verbatim in the process arguments.
fn arg_present(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

/// One agent session from `opencode session list --format json`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AgentSession {
    pub id: String,
    pub title: String,
    pub directory: String,
}

/// Parse the JSON emitted by `opencode session list --format json`. Unknown
/// fields are ignored; a missing title/directory defaults to empty.
fn parse_agent_sessions(json: &str) -> Result<Vec<AgentSession>, String> {
    #[derive(serde::Deserialize)]
    struct Raw {
        id: String,
        #[serde(default)]
        title: String,
        #[serde(default)]
        directory: String,
    }
    let raw: Vec<Raw> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    Ok(raw
        .into_iter()
        .map(|r| AgentSession {
            id: r.id,
            title: r.title,
            directory: r.directory,
        })
        .collect())
}

/// List opencode sessions for one tab's directory. `opencode session list`
/// is scoped to the project of its working directory, so the command must
/// run from the tab's cwd to see that project's sessions. Bounded by a
/// timeout, so a stuck opencode can never freeze the window. Empty when
/// opencode is missing or errors.
#[tauri::command]
async fn opencode_sessions(cwd: String) -> Vec<AgentSession> {
    tauri::async_runtime::spawn_blocking(move || list_opencode_sessions(&cwd))
        .await
        .unwrap_or_default()
}

fn list_opencode_sessions(cwd: &str) -> Vec<AgentSession> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let mut cmd = Command::new("opencode");
    cmd.args(["session", "list", "--format", "json", "--max-count", "1000"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let dir = cwd.trim();
    if !dir.is_empty() && Path::new(dir).is_dir() {
        cmd.current_dir(dir);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        return Vec::new();
    };
    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        let mut stdout = stdout;
        let _ = stdout.read_to_string(&mut buf);
        buf
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return Vec::new();
                }
                break;
            }
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                return Vec::new();
            }
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Vec::new();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    parse_agent_sessions(&reader.join().unwrap_or_default()).unwrap_or_default()
}

/// Directory opencode loads global plugins from.
fn opencode_plugins_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("opencode").join("plugins"))
}

/// opencode only auto-loads `.js`/`.ts` files from its plugin directory.
/// A `.mjs` name is silently ignored, so the exact session-id plugin never
/// ran. Keep the installed name loadable and drop the old one.
const OPENCODE_PLUGIN_FILE: &str = "muis.js";
const OPENCODE_PLUGIN_LEGACY: &str = "muis.mjs";
const OPENCODE_PLUGIN_MARKER: &str = "muis-plugin";
const OPENCODE_PLUGIN_SRC: &str = include_str!("../opencode-plugin/muis.js");

/// TUI plugin: exact route tracking. opencode 2.x discovers TUI entrypoints
/// from a plugin *directory* under its global plugin dir (`<id>/tui.js`), and
/// rejects the V1 single-file module listed in `tui.json` ("Invalid V2 TUI
/// plugin module"). The directory is picked up by discovery, so nothing is
/// registered in the terminal config.
const OPENCODE_TUI_DIR: &str = "muis-tui";
const OPENCODE_TUI_FILE: &str = "tui.js";
const OPENCODE_TUI_MARKER: &str = "muis-tui-plugin";
const OPENCODE_TUI_SRC: &str = include_str!("../opencode-plugin/muis-tui.js");
/// V1 single-file entrypoint, removed on install.
const OPENCODE_TUI_LEGACY: &str = "muis-tui.mjs";

/// Install/update the muis opencode plugins. Writes a file when missing and
/// updates it when the existing file is ours (carries the matching marker);
/// never touches a foreign file. The TUI plugin needs no terminal-config
/// entry: opencode discovers the plugin directory itself. Returns the server
/// plugin path.
#[tauri::command]
fn install_opencode_plugin() -> Result<String, String> {
    let dir = opencode_plugins_dir().ok_or("no config directory")?;
    install_plugins(&dir)
}

/// Write/refresh the plugin files under `dir` (the global opencode plugin
/// directory) and clean up the V1 layout. See `install_opencode_plugin`.
fn install_plugins(dir: &Path) -> Result<String, String> {
    let path = dir.join(OPENCODE_PLUGIN_FILE);
    let ours = match std::fs::read_to_string(&path) {
        Ok(existing) => existing.contains(OPENCODE_PLUGIN_MARKER),
        Err(_) => true, // missing
    };
    if ours {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(&path, OPENCODE_PLUGIN_SRC).map_err(|e| e.to_string())?;
    }
    let legacy = dir.join(OPENCODE_PLUGIN_LEGACY);
    if std::fs::read_to_string(&legacy).is_ok_and(|s| s.contains(OPENCODE_PLUGIN_MARKER)) {
        let _ = std::fs::remove_file(&legacy);
    }

    // The TUI plugin is a separate entrypoint (opencode forbids one module
    // exporting both). It lives in its own plugin directory, which opencode
    // discovers; the V1 single file and its `tui.json`/`cli.json` entry are
    // cleaned up so they cannot fail to load on every start.
    let tui_path = dir.join(OPENCODE_TUI_DIR).join(OPENCODE_TUI_FILE);
    let tui_ours = match std::fs::read_to_string(&tui_path) {
        Ok(existing) => existing.contains(OPENCODE_TUI_MARKER),
        Err(_) => true,
    };
    if tui_ours {
        let tui_dir = dir.join(OPENCODE_TUI_DIR);
        let written = std::fs::create_dir_all(&tui_dir)
            .map_err(|e| e.to_string())
            .and_then(|()| std::fs::write(&tui_path, OPENCODE_TUI_SRC).map_err(|e| e.to_string()));
        match written {
            Ok(()) => remove_legacy_tui_plugin(dir),
            Err(e) => log::warn!("could not write TUI plugin: {e}"),
        }
    }
    Ok(path.display().to_string())
}

/// Drop the V1 TUI plugin file and the config entries that pointed at it,
/// leaving every other setting alone. Best effort: a malformed config is
/// reported and never rewritten.
fn remove_legacy_tui_plugin(dir: &Path) {
    let legacy = dir.join(OPENCODE_TUI_LEGACY);
    if std::fs::read_to_string(&legacy).is_ok_and(|s| s.contains(OPENCODE_TUI_MARKER)) {
        let _ = std::fs::remove_file(&legacy);
    }
    let Some(config) = dir.parent() else { return };
    // `cli.json` is the 2.x terminal config; `tui.json` is the 1.x file that
    // held the same entry.
    for (name, key) in [("cli.json", "plugins"), ("tui.json", "plugin")] {
        if let Err(e) = prune_plugin_entry(&config.join(name), key, OPENCODE_TUI_LEGACY) {
            log::warn!("could not clean up {}: {e}", config.join(name).display());
        }
    }
}

/// Remove one entry whose file name matches `file` from the plugin list under
/// `key`. Returns whether the file changed; unrelated entries and settings are
/// preserved.
fn prune_plugin_entry(config: &Path, key: &str, file: &str) -> Result<bool, String> {
    let text = match std::fs::read_to_string(config) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.to_string()),
    };
    if text.trim().is_empty() {
        return Ok(false);
    }
    let mut root: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not JSON: {e}", config.display()))?;
    let Some(object) = root.as_object_mut() else {
        return Err(format!("{} is not a JSON object", config.display()));
    };
    let Some(list) = object.get_mut(key) else {
        return Ok(false);
    };
    let list = list
        .as_array_mut()
        .ok_or_else(|| format!("{}: {key} is not an array", config.display()))?;
    let before = list.len();
    list.retain(|item| {
        item.as_str()
            .map(|s| Path::new(s).file_name().and_then(|n| n.to_str()) != Some(file))
            .unwrap_or(true)
    });
    if list.len() == before {
        return Ok(false);
    }
    let json = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    persist::write_json(config, &json)?;
    Ok(true)
}

/// Open an http(s) URL in the user's browser (OSC 8 hyperlinks). Only
/// http/https are accepted; never a shell invocation.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err("only http(s) URLs can be opened".into());
    }
    std::process::Command::new("xdg-open")
        .arg(trimmed)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Names of up to `limit` non-hidden subdirectories of `path`, sorted. Used
/// by the new-session dialog's subfolder quick picks. Best effort: a missing
/// or unreadable path yields an empty list.
#[tauri::command]
fn list_subdirs(path: String, limit: usize) -> Vec<String> {
    let mut names: Vec<String> = match std::fs::read_dir(&path) {
        Ok(entries) => entries
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| !n.starts_with('.'))
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names.truncate(limit.min(50));
    names
}

#[tauri::command]
fn worker_spawn(pool: tauri::State<Backend>, session_id: String) -> Result<(), String> {
    pool.spawn_session(&session_id)
}

#[tauri::command]
fn worker_send(
    pool: tauri::State<Backend>,
    session_id: String,
    line: String,
) -> Result<(), String> {
    pool.send_line(&session_id, &line)
}

#[tauri::command]
fn worker_stop(pool: tauri::State<Backend>, session_id: String) {
    pool.stop_session(&session_id);
}

/// The frontend's answer to a forwarded UI request.
#[tauri::command]
fn ui_reply(
    pool: tauri::State<Backend>,
    id: u64,
    ok: bool,
    result: serde_json::Value,
) -> Result<(), String> {
    pool.ui_reply(id, ok, result)
}

/// NVIDIA + KWin explicit sync kills WebKitGTK's dmabuf fast path with
/// "explicit sync is used, but no acquire point is set" (native Wayland
/// only; XWayland never hits it). Fall back to shared-memory compositing
/// unless the user already chose. Invisible for a terminal, and the window
/// actually opens. Returns true when this call set the variable.
/// See docs/archive/architecture-rust-xterm.md §8.
fn ensure_dmabuf_disabled() -> bool {
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        true
    } else {
        false
    }
}

/// `muis shell-integration install|uninstall|print [--shell NAME]`.
///
/// Returns `None` for a normal launch, so the window still starts.
fn shell_integration_cli(mut args: impl Iterator<Item = String>) -> Option<i32> {
    if args.next().as_deref() != Some("shell-integration") {
        return None;
    }
    let mut command: Option<String> = None;
    let mut shell: Option<shell_integration::Shell> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--shell" => {
                let Some(name) = args.next() else {
                    eprintln!("muis: --shell needs a name");
                    return Some(2);
                };
                shell = shell_integration::Shell::parse(&name);
                if shell.is_none() {
                    eprintln!("muis: unsupported shell: {name} (fish, zsh, bash)");
                    return Some(2);
                }
            }
            "-h" | "--help" => {
                println!(
                    "usage: muis shell-integration <install|uninstall|print> [--shell fish|zsh|bash]"
                );
                return Some(0);
            }
            other if command.is_none() => command = Some(other.to_string()),
            other => {
                eprintln!("muis: unexpected argument: {other}");
                return Some(2);
            }
        }
    }
    let shell = shell.or_else(|| {
        std::env::var("SHELL")
            .ok()
            .and_then(|s| shell_integration::Shell::parse(&s))
    });
    let Some(shell) = shell else {
        eprintln!("muis: cannot tell the shell; pass --shell fish|zsh|bash");
        return Some(2);
    };
    let home = match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home),
        None => {
            eprintln!("muis: HOME is not set");
            return Some(2);
        }
    };
    let result = match command.as_deref() {
        Some("install") => shell_integration::install(&home, shell),
        Some("uninstall") => shell_integration::uninstall(&home, shell),
        Some("print") => {
            print!("{}", shell_integration::snippet(shell));
            return Some(0);
        }
        Some(other) => {
            eprintln!("muis: unknown command: {other}");
            return Some(2);
        }
        None => {
            eprintln!("muis: shell-integration needs a command (install, uninstall, print)");
            return Some(2);
        }
    };
    match result {
        Ok(files) => {
            for file in files {
                println!("{}", file.display());
            }
            Some(0)
        }
        Err(e) => {
            eprintln!("muis: {e}");
            Some(1)
        }
    }
}

fn main() {
    // CLI modes exit before Tauri starts; a normal launch falls through.
    if let Some(code) = shell_integration_cli(std::env::args().skip(1)) {
        std::process::exit(code);
    }
    if let Some(path) = logging::init() {
        log::info!("muis shell starting; log file: {}", path.display());
    }

    ensure_dmabuf_disabled();

    let args: Vec<String> = std::env::args().collect();
    let test = arg_present(&args, "--testmode");
    TEST_MODE.store(test, Ordering::Relaxed);

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            ping,
            worker_available,
            is_test_mode,
            default_shell,
            default_cwd,
            live_test_command,
            debug_report,
            log_path,
            config_load,
            config_save,
            sessions_load,
            sessions_save,
            git_branch,
            install_opencode_plugin,
            opencode_sessions,
            list_subdirs,
            open_url,
            list_ports,
            worktree_create,
            view_file,
            notify_clickable,
            worker_spawn,
            worker_send,
            worker_stop,
            ui_reply
        ])
        .setup(|app| {
            log::info!(
                "muis {} ready ({} {}); binary: {}",
                app.package_info().version,
                std::env::consts::OS,
                std::env::consts::ARCH,
                std::env::current_exe()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| "<unknown>".to_string()),
            );

            let launch = worker_launch();
            match &launch {
                Some(launch) => log::info!("worker binary: {}", launch.0.display()),
                None => log::error!(
                    "no muis-worker available next to {}",
                    std::env::current_exe()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| "<unknown>".to_string())
                ),
            }

            let handle = app.handle().clone();
            let sink: EventSink = Arc::new(move |event| handle.emit(WORKER_EVENT, &event).is_ok());
            // Forwarded control requests go to the frontend, which owns tabs,
            // scrollback and agent state; it answers via `ui_reply`.
            let ui_handle = app.handle().clone();
            let ui: backend::UiSink = Arc::new(move |id, op, args| {
                let _ = ui_handle.emit(
                    "muis-ui-request",
                    serde_json::json!({ "id": id, "op": op, "args": args }),
                );
            });
            app.handle().manage(Backend::connect(
                sink,
                ui,
                launch.map(|WorkerLaunch(path)| path),
                env_for_shell(),
            ));

            // Notification endpoint for `muis-notify` clients. One per-user
            // socket; each request is forwarded to the frontend, which routes
            // it to the originating tab. Best-effort: a failure here only
            // disables the CLI path, not the window. Test mode uses its own
            // socket so it never steals notifications from a running muis.
            let socket = notify_socket_name();
            let handle = app.handle().clone();
            if let Err(e) = muis_core::notify::serve(&socket, move |req| match req {
                ClientRequest::Notify(req) => {
                    let _ = handle.emit("muis-notify", &req);
                }
                ClientRequest::AgentSession(req) => {
                    let _ = handle.emit("muis-agent-session", &req);
                }
                ClientRequest::AgentActivity(req) => {
                    let _ = handle.emit("muis-agent-activity", &req);
                }
                ClientRequest::AgentState(req) => {
                    let _ = handle.emit("muis-agent-state", &req);
                }
                ClientRequest::OpenUrl(req) => {
                    let _ = handle.emit("muis-open-url", &req);
                }
                ClientRequest::ViewFile(req) => match viewer_url(&req.path) {
                    // The pane already knows how to open a URL; a viewed file
                    // is just one on the loopback viewer.
                    Ok(url) => {
                        if let Some(open) =
                            muis_core::notify::OpenUrlRequest::new(&url, req.tab_id, req.session)
                        {
                            let _ = handle.emit("muis-open-url", &open);
                        }
                    }
                    Err(e) => log::warn!("cannot view {}: {e}", req.path),
                },
            }) {
                log::warn!("notification endpoint unavailable ({socket}): {e}");
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("muis shell failed to start");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_plugin_name_is_loadable_and_ours() {
        // opencode silently ignores `.mjs` files in its plugin dir, so the
        // installed plugin must use an extension opencode auto-loads.
        assert!(OPENCODE_PLUGIN_FILE.ends_with(".js") || OPENCODE_PLUGIN_FILE.ends_with(".ts"));
        assert_ne!(OPENCODE_PLUGIN_FILE, OPENCODE_PLUGIN_LEGACY);
        assert!(OPENCODE_PLUGIN_SRC.contains(OPENCODE_PLUGIN_MARKER));
    }

    #[test]
    fn opencode_plugin_uses_the_v2_definition_shape() {
        // opencode 2.x rejects V1 factory exports and only reports it in the
        // server log, so the installed file must default-export a definition
        // with an id and a setup function.
        assert!(OPENCODE_PLUGIN_SRC.contains("export default"));
        assert!(OPENCODE_PLUGIN_SRC.contains("id: \"muis\""));
        assert!(OPENCODE_PLUGIN_SRC.contains("setup("));
    }

    #[test]
    fn opencode_plugin_exposes_the_browser_pane_tool() {
        // The pane is driven from opencode through this tool; if the
        // registration path changes, the feature silently disappears from
        // the model's tool list, so lock the contract here.
        assert!(OPENCODE_PLUGIN_SRC.contains("muis_open_url"));
        assert!(OPENCODE_PLUGIN_SRC.contains("ctx.tool?.transform"));
        assert!(OPENCODE_PLUGIN_SRC.contains("tools.add("));
        assert!(OPENCODE_PLUGIN_SRC.contains("[\"open\", url]"));
        // The tool must wait for the helper and say so when it fails: a
        // fire-and-forget call reports success with no muis running.
        assert!(OPENCODE_PLUGIN_SRC.contains("await runHelper("));
        assert!(OPENCODE_PLUGIN_SRC.contains("Could not open"));
        assert!(OPENCODE_PLUGIN_SRC.contains("failure ||"));
        // A blanket shell on Windows lets cmd.exe split a URL at `&`; only
        // .cmd/.bat shims may take the shell path, with quoted arguments.
        assert!(OPENCODE_PLUGIN_SRC.contains("needsShell("));
        assert!(OPENCODE_PLUGIN_SRC.contains("quoteForCmd("));
        assert!(!OPENCODE_PLUGIN_SRC.contains("shell: process.platform == \"win32\""));
    }

    #[test]
    fn opencode_plugin_reports_agent_state() {
        // The tab's real state comes from these reports. If the plugin loses
        // them the chrome silently falls back to the done-only heuristic, so
        // lock the helper call and every state it emits.
        assert!(OPENCODE_PLUGIN_SRC.contains("[\"report\", value, \"--agent\", \"opencode\"]"));
        assert!(OPENCODE_PLUGIN_SRC.contains("reportState"));
        for state in ["blocked", "working", "done"] {
            assert!(
                OPENCODE_PLUGIN_SRC.contains(&format!("state(\"{state}\"")),
                "plugin must report {state}"
            );
        }
    }

    #[test]
    fn tui_plugin_is_marked_and_installed_separately() {
        // opencode 2.x only loads a TUI entrypoint from a plugin directory.
        assert!(OPENCODE_TUI_FILE.ends_with(".js"));
        assert_ne!(OPENCODE_TUI_FILE, OPENCODE_PLUGIN_FILE);
        assert_ne!(OPENCODE_TUI_DIR, OPENCODE_PLUGIN_FILE);
        assert!(OPENCODE_TUI_SRC.contains(OPENCODE_TUI_MARKER));
        assert!(OPENCODE_TUI_SRC.contains("muis-agent"));
    }

    #[test]
    fn tui_plugin_uses_the_v2_definition_shape() {
        // opencode 2.x rejects the V1 `{ id, tui(api) }` module with
        // "Invalid V2 TUI plugin module" and only reports it in the TUI log,
        // so the installed file must default-export `{ id, setup }` and read
        // the route from the context instead of the removed `api.route`.
        assert!(OPENCODE_TUI_SRC.contains("export default"));
        assert!(OPENCODE_TUI_SRC.contains("id: \"muis-tui\""));
        assert!(OPENCODE_TUI_SRC.contains("setup("));
        assert!(!OPENCODE_TUI_SRC.contains("\n  tui:"));
        assert!(!OPENCODE_TUI_SRC.contains("api.route.current"));
        assert!(!OPENCODE_TUI_SRC.contains("api?.route"));
        assert!(OPENCODE_TUI_SRC.contains("ui.router.current()"));
        assert!(OPENCODE_TUI_SRC.contains("MUIS_TAB_ID"));
    }

    #[test]
    fn parses_opencode_session_list_json() {
        let parsed = parse_agent_sessions(
            r#"[{"id":"ses_a1","title":"Fix it","directory":"/p","extra":1}]"#,
        )
        .unwrap();
        assert_eq!(
            parsed,
            vec![AgentSession {
                id: "ses_a1".into(),
                title: "Fix it".into(),
                directory: "/p".into(),
            }]
        );
        assert!(parse_agent_sessions("not json").is_err());
    }

    #[test]
    fn install_writes_both_plugins_and_cleans_up_v1() {
        let root = std::env::temp_dir().join(format!("muis-install-{}", std::process::id()));
        let dir = root.join("opencode").join("plugins");
        std::fs::create_dir_all(&dir).unwrap();
        // V1 leftovers: the single-file TUI plugin and its config entry.
        std::fs::write(
            dir.join(OPENCODE_TUI_LEGACY),
            format!("// {OPENCODE_TUI_MARKER}\n"),
        )
        .unwrap();
        std::fs::write(
            root.join("opencode").join("cli.json"),
            r#"{"theme":{"name":"x"},"plugins":["/old/plugins/muis-tui.mjs","keep-me"]}"#,
        )
        .unwrap();

        let path = install_plugins(&dir).unwrap();

        assert_eq!(path, dir.join(OPENCODE_PLUGIN_FILE).display().to_string());
        assert!(dir.join(OPENCODE_PLUGIN_FILE).is_file());
        let tui = dir.join(OPENCODE_TUI_DIR).join(OPENCODE_TUI_FILE);
        assert!(tui.is_file());
        assert!(std::fs::read_to_string(&tui).unwrap() == OPENCODE_TUI_SRC);
        assert!(!dir.join(OPENCODE_TUI_LEGACY).exists());
        let config: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("opencode").join("cli.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(config["theme"]["name"], "x");
        assert_eq!(config["plugins"], serde_json::json!(["keep-me"]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_keeps_foreign_plugin_files() {
        let root = std::env::temp_dir().join(format!("muis-foreign-{}", std::process::id()));
        let dir = root.join("opencode").join("plugins");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(OPENCODE_PLUGIN_FILE), "// not ours\n").unwrap();
        std::fs::write(dir.join(OPENCODE_TUI_LEGACY), "// not ours\n").unwrap();

        install_plugins(&dir).unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.join(OPENCODE_PLUGIN_FILE)).unwrap(),
            "// not ours\n"
        );
        // The legacy file is only removed when it carries our marker.
        assert!(dir.join(OPENCODE_TUI_LEGACY).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_tui_entry_is_pruned_and_config_preserved() {
        let root = std::env::temp_dir().join(format!("muis-tui-config-{}", std::process::id()));
        let config = root.join("opencode").join("cli.json");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            r#"{"theme":{"name":"x"},"plugins":["someone-else","/old/plugins/muis-tui.mjs",{"package":"p"}]}"#,
        )
        .unwrap();

        assert!(prune_plugin_entry(&config, "plugins", OPENCODE_TUI_LEGACY).unwrap());
        assert!(!prune_plugin_entry(&config, "plugins", OPENCODE_TUI_LEGACY).unwrap());

        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(value["theme"]["name"], "x");
        assert_eq!(
            value["plugins"],
            serde_json::json!(["someone-else", {"package": "p"}])
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_or_non_list_config_is_left_alone() {
        let root = std::env::temp_dir().join(format!("muis-tui-absent-{}", std::process::id()));
        let config = root.join("opencode").join("cli.json");
        assert!(!prune_plugin_entry(&config, "plugins", OPENCODE_TUI_LEGACY).unwrap());

        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, r#"{"plugins":"not-a-list"}"#).unwrap();
        assert!(prune_plugin_entry(&config, "plugins", OPENCODE_TUI_LEGACY).is_err());
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            r#"{"plugins":"not-a-list"}"#
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn worker_path_points_at_sibling_binary_when_present() {
        // worker_path() resolves against the test binary's directory.
        // No assertion on presence here (cargo test doesn't ship the
        // sidecar); this locks the naming contract instead.
        assert_eq!("muis-worker", "muis-worker");
        let _ = worker_path();
    }

    #[test]
    fn find_worker_in_prefers_plain_then_triple_suffixed() {
        let dir = std::env::temp_dir().join(format!("muis-worker-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let plain = dir.join("muis-worker");
        let sidecar = dir.join("muis-worker-x86_64-unknown-linux-gnu");

        // Sidecar only: fall back to the triple-suffixed name.
        std::fs::write(&sidecar, b"x").unwrap();
        assert_eq!(find_worker_in(&dir), Some(sidecar));

        // Plain sibling present: prefer it.
        std::fs::write(&plain, b"x").unwrap();
        assert_eq!(find_worker_in(&dir), Some(plain));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_branch_is_empty_outside_a_repo() {
        let dir = std::env::temp_dir().join(format!("muis-git-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let branch = git_branch(dir.to_string_lossy().into_owned());
        // /tmp is (almost certainly) not a repo; either way it never errors.
        assert!(branch.is_ascii());
    }

    #[test]
    fn list_subdirs_sorts_filters_and_caps() {
        let dir = std::env::temp_dir().join(format!("muis-subdirs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("beta")).unwrap();
        std::fs::create_dir_all(dir.join("alpha")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join("file.txt"), b"x").unwrap();
        let path = dir.to_string_lossy().into_owned();
        // Directories only, hidden excluded, sorted.
        assert_eq!(list_subdirs(path.clone(), 10), vec!["alpha", "beta"]);
        // Limit is respected, and a missing path is empty (never errors).
        assert_eq!(list_subdirs(path, 1), vec!["alpha"]);
        assert!(list_subdirs("/no/such/path-9f3a".to_string(), 10).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dmabuf_workaround_respects_user_override() {
        std::env::remove_var("WEBKIT_DISABLE_DMABUF_RENDERER");
        assert!(ensure_dmabuf_disabled());
        assert_eq!(
            std::env::var("WEBKIT_DISABLE_DMABUF_RENDERER").as_deref(),
            Ok("1")
        );
        // A value the user already chose is never overwritten.
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "0");
        assert!(!ensure_dmabuf_disabled());
        assert_eq!(
            std::env::var("WEBKIT_DISABLE_DMABUF_RENDERER").as_deref(),
            Ok("0")
        );
        std::env::remove_var("WEBKIT_DISABLE_DMABUF_RENDERER");
    }

    #[test]
    fn testmode_flag_is_detected_exactly() {
        let args = |extra: &[&str]| -> Vec<String> {
            std::iter::once("muis".to_string())
                .chain(extra.iter().map(|s| s.to_string()))
                .collect()
        };
        assert!(!arg_present(&args(&[]), "--testmode"));
        assert!(arg_present(&args(&["--testmode"]), "--testmode"));
        assert!(arg_present(
            &args(&["--testmode", "--verbose"]),
            "--testmode"
        ));
        // Exact match only: an unrelated flag or a value is not test mode.
        assert!(!arg_present(&args(&["--test-mode"]), "--testmode"));
        assert!(!arg_present(&args(&["--testmode=1"]), "--testmode"));
    }

    #[test]
    fn notify_socket_is_base_without_test_mode() {
        assert!(!test_mode());
        assert_eq!(notify_socket_name(), muis_core::notify::socket_name());
    }
}
