//! `POST /rpc/<command>`: the browser-mode equivalent of the native shell's
//! `#[tauri::command]` surface. Argument names are accepted in either the
//! camelCase the frontend uses or the snake_case Rust uses, so the same call
//! sites work on both backends.

use std::path::PathBuf;

use serde::Serialize;
use serde_json::{json, Value};

use crate::{opencode, platform, store};

/// What the dispatcher needs to know about the running server.
#[derive(Clone, Copy)]
pub struct Ctx {
    pub test_mode: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub dir: bool,
}

/// First matching argument, so `{cwd}` and `{cwd}`-style spellings both work.
fn arg<'a>(args: &'a Value, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|name| args.get(*name))
}

fn arg_str<'a>(args: &'a Value, names: &[&str]) -> Option<&'a str> {
    arg(args, names).and_then(Value::as_str)
}

fn arg_usize(args: &Value, names: &[&str]) -> Option<usize> {
    arg(args, names).and_then(Value::as_u64).map(|n| n as usize)
}

/// Dispatch one command. Errors become HTTP 500 with the message as the body,
/// which is exactly what a rejected Tauri `invoke` looks like to the UI.
pub async fn dispatch(ctx: Ctx, method: &str, args: &Value) -> Result<Value, String> {
    match method {
        "ping" => Ok(json!("pong")),
        "worker_available" => Ok(json!(true)),
        "is_test_mode" => Ok(json!(ctx.test_mode)),
        "default_shell" => Ok(json!(muis_core::shell::default_shell()
            .to_string_lossy()
            .into_owned())),
        "default_cwd" => Ok(json!(muis_core::paths::home_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".to_string()))),
        "live_test_command" => Ok(json!(live_test_command())),
        "log_path" => Ok(json!(crate::logging::log_file().to_string_lossy())),

        "debug_report" => {
            if let Some(report) = arg_str(args, &["json"]) {
                log::debug!("frontend report: {report}");
            }
            Ok(Value::Null)
        }

        "theme_packs" => Ok(muis_core::themes::packs_json(
            &muis_core::paths::config_dir().join("muis").join("themes"),
        )),

        "search_history" => {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let limit = args
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(20)
                .clamp(1, 200) as usize;
            let dir = muis_core::paths::state_dir().join("history");
            let hits = muis_daemon::history::search(&dir, query, limit);
            Ok(serde_json::json!(hits
                .iter()
                .map(|hit| serde_json::json!({
                    "session": hit.session,
                    "line": hit.line,
                    "text": hit.text,
                }))
                .collect::<Vec<_>>()))
        }

        "view_file" => {
            use std::sync::OnceLock;
            static VIEWER: OnceLock<Result<muis_core::view::ViewServer, String>> = OnceLock::new();
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or("view_file needs a path")?;
            let viewer = VIEWER
                .get_or_init(|| {
                    muis_core::view::ViewServer::start()
                        .map_err(|e| format!("viewer unavailable: {e}"))
                })
                .as_ref()
                .map_err(|e| e.clone())?;
            viewer
                .url_for(std::path::Path::new(path))
                .map(serde_json::Value::String)
        }

        "list_ports" => {
            let cwd = args.get("cwd").and_then(|v| v.as_str()).unwrap_or("/");
            Ok(serde_json::json!(muis_core::ports::listening_under(
                std::path::Path::new(cwd)
            )))
        }

        "worktree_create" => {
            let cwd = args
                .get("cwd")
                .and_then(|v| v.as_str())
                .ok_or("worktree_create needs a cwd")?;
            let name = args
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or("worktree_create needs a name")?;
            muis_core::worktree::create(std::path::Path::new(cwd), name)
                .map(|path| serde_json::Value::String(path.display().to_string()))
        }

        "config_load" => Ok(json!(store::read_text(&muis_core::paths::config_file())?)),
        "config_save" => {
            let json = arg_str(args, &["json"]).ok_or("config_save needs json")?;
            if !ctx.test_mode {
                store::write_json(&muis_core::paths::config_file(), json)?;
            }
            Ok(Value::Null)
        }
        "sessions_load" => {
            if ctx.test_mode {
                return Ok(json!(""));
            }
            Ok(json!(store::read_text(&muis_core::paths::sessions_file())?))
        }
        "sessions_save" => {
            let json = arg_str(args, &["json"]).ok_or("sessions_save needs json")?;
            if !ctx.test_mode {
                store::write_json(&muis_core::paths::sessions_file(), json)?;
            }
            Ok(Value::Null)
        }

        "git_branch" => {
            let cwd = arg_str(args, &["cwd"]).unwrap_or_default().to_string();
            let branch = tokio::task::spawn_blocking(move || git_branch(&cwd))
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!(branch))
        }

        "opencode_sessions" => {
            let cwd = arg_str(args, &["cwd"]).unwrap_or_default().to_string();
            let sessions = tokio::task::spawn_blocking(move || opencode::list_sessions(&cwd))
                .await
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(sessions).map_err(|e| e.to_string())?)
        }

        "install_opencode_plugin" => {
            let path = tokio::task::spawn_blocking(opencode::install_plugin)
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e)?;
            Ok(json!(path))
        }

        "list_subdirs" => {
            let path = arg_str(args, &["path"]).unwrap_or_default().to_string();
            let limit = arg_usize(args, &["limit"]).unwrap_or(50);
            Ok(json!(list_subdirs(&path, limit)))
        }
        "list_dir" => {
            let path = arg_str(args, &["path"]).unwrap_or_default().to_string();
            Ok(serde_json::to_value(list_dir(&path)).map_err(|e| e.to_string())?)
        }

        "open_url" => {
            let url = arg_str(args, &["url"]).ok_or("open_url needs url")?;
            platform::open_url(url)?;
            Ok(Value::Null)
        }

        // Worker traffic is a WebSocket concern, not RPC.
        "worker_spawn" | "worker_send" | "worker_stop" => {
            Err("worker commands are served over /ws in browser mode".to_string())
        }

        other => Err(format!("unknown command: {other}")),
    }
}

/// Debug builds may hand the pty one command through the environment (the
/// native Xvfb harness); release builds never execute environment input.
fn live_test_command() -> Option<String> {
    #[cfg(debug_assertions)]
    {
        std::env::var("MUIS_TEST_COMMAND")
            .ok()
            .filter(|command| !command.trim().is_empty())
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

/// Current git branch for a directory; empty when not a repo (or git is
/// missing/slow — the caller treats empty as "no pill data").
fn git_branch(cwd: &str) -> String {
    let dir = cwd.trim();
    let mut cmd = platform::command("git");
    cmd.args(["branch", "--show-current"]);
    if !dir.is_empty() && std::path::Path::new(dir).is_dir() {
        cmd.current_dir(dir);
    }
    cmd.output()
        .ok()
        .and_then(|out| {
            if out.status.success() {
                String::from_utf8(out.stdout).ok()
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Names of up to `limit` non-hidden subdirectories of `path`, sorted.
fn list_subdirs(path: &str, limit: usize) -> Vec<String> {
    let mut names: Vec<String> = match std::fs::read_dir(path) {
        Ok(entries) => entries
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| !n.starts_with('.') && !n.starts_with('$'))
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort_by_key(|name| name.to_lowercase());
    names.truncate(limit.min(50));
    names
}

/// Directory listing for the browser-mode folder picker: directories only,
/// hidden and system folders skipped, sorted by name.
fn list_dir(path: &str) -> Vec<DirEntry> {
    let base = if path.trim().is_empty() {
        return Vec::new();
    } else {
        PathBuf::from(path)
    };
    let Ok(entries) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut out: Vec<DirEntry> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            if name.starts_with('.') || name.starts_with('$') {
                return None;
            }
            Some(DirEntry {
                path: e.path().to_string_lossy().into_owned(),
                name,
                dir: true,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.truncate(500);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Ctx {
        Ctx { test_mode: false }
    }

    #[tokio::test]
    async fn ping_and_shell_probe_answer() {
        assert_eq!(
            dispatch(ctx(), "ping", &json!({})).await.unwrap(),
            json!("pong")
        );
        let shell = dispatch(ctx(), "default_shell", &json!({})).await.unwrap();
        assert!(shell.as_str().is_some_and(|s| !s.is_empty()));
    }

    #[tokio::test]
    async fn unknown_command_is_an_error() {
        assert!(dispatch(ctx(), "nope", &json!({})).await.is_err());
    }

    #[tokio::test]
    async fn saving_sessions_rejects_garbage_json() {
        let err = dispatch(ctx(), "sessions_save", &json!({"json": "not json"}))
            .await
            .unwrap_err();
        assert!(err.contains("invalid json"));
    }

    #[test]
    fn list_dir_returns_directories_sorted_and_unhidden() {
        let root = std::env::temp_dir().join(format!("muis-srv-listdir-{}", std::process::id()));
        std::fs::create_dir_all(root.join("beta")).unwrap();
        std::fs::create_dir_all(root.join("Alpha")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        std::fs::write(root.join("file.txt"), "x").unwrap();

        let names: Vec<String> = list_dir(&root.to_string_lossy())
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, vec!["Alpha".to_string(), "beta".to_string()]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn subdirs_are_capped_and_skip_hidden() {
        let root = std::env::temp_dir().join(format!("muis-srv-subdirs-{}", std::process::id()));
        std::fs::create_dir_all(root.join("one")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let names = list_subdirs(&root.to_string_lossy(), 10);
        assert_eq!(names, vec!["one".to_string()]);
        std::fs::remove_dir_all(&root).ok();
    }
}
