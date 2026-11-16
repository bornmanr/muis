//! opencode integration: the notification/agent plugin install and the
//! session list shown in the sidebar.
//!
//! The plugin sources are the same files the native shell ships
//! (`crates/muis-shell/opencode-plugin`), included here directly so the two
//! binaries can never install different plugin content.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::store;

/// One agent session from `opencode session list --format json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSession {
    pub id: String,
    pub title: String,
    pub directory: String,
}

const PLUGIN_FILE: &str = "muis.js";
const PLUGIN_LEGACY: &str = "muis.mjs";
const PLUGIN_MARKER: &str = "muis-plugin";
const PLUGIN_SRC: &str = include_str!("../../muis-shell/opencode-plugin/muis.js");

const TUI_DIR: &str = "muis-tui";
const TUI_FILE: &str = "tui.js";
const TUI_MARKER: &str = "muis-tui-plugin";
const TUI_SRC: &str = include_str!("../../muis-shell/opencode-plugin/muis-tui.js");
const TUI_LEGACY: &str = "muis-tui.mjs";

/// opencode's global plugin directory: `$XDG_CONFIG_HOME/opencode/plugins`
/// elsewhere, `%APPDATA%\opencode\plugins` on Windows.
pub fn plugins_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(|appdata| PathBuf::from(appdata)))
        .or_else(|| {
            std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".config"))
        })?;
    Some(base.join("opencode").join("plugins"))
}

/// Install/refresh the muis opencode plugins. Writes when missing and
/// updates when the existing file is ours; never touches a foreign file.
pub fn install_plugin() -> Result<String, String> {
    let dir = plugins_dir().ok_or("no config directory")?;
    install_plugins(&dir)
}

fn install_plugins(dir: &Path) -> Result<String, String> {
    let path = dir.join(PLUGIN_FILE);
    let ours = match std::fs::read_to_string(&path) {
        Ok(existing) => existing.contains(PLUGIN_MARKER),
        Err(_) => true,
    };
    if ours {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        std::fs::write(&path, PLUGIN_SRC).map_err(|e| e.to_string())?;
    }
    let legacy = dir.join(PLUGIN_LEGACY);
    if std::fs::read_to_string(&legacy).is_ok_and(|s| s.contains(PLUGIN_MARKER)) {
        let _ = std::fs::remove_file(&legacy);
    }

    // The TUI plugin is a separate entrypoint in its own directory; opencode
    // discovers the directory itself, so nothing is registered in config.
    let tui_path = dir.join(TUI_DIR).join(TUI_FILE);
    let tui_ours = match std::fs::read_to_string(&tui_path) {
        Ok(existing) => existing.contains(TUI_MARKER),
        Err(_) => true,
    };
    if tui_ours {
        let tui_dir = dir.join(TUI_DIR);
        let written =
            std::fs::create_dir_all(&tui_dir).and_then(|()| std::fs::write(&tui_path, TUI_SRC));
        match written {
            Ok(()) => remove_legacy_tui(dir),
            Err(e) => log::warn!("could not write TUI plugin: {e}"),
        }
    }
    Ok(path.display().to_string())
}

fn remove_legacy_tui(dir: &Path) {
    let legacy = dir.join(TUI_LEGACY);
    if std::fs::read_to_string(&legacy).is_ok_and(|s| s.contains(TUI_MARKER)) {
        let _ = std::fs::remove_file(&legacy);
    }
    let Some(config) = dir.parent() else {
        return;
    };
    for (name, key) in [("cli.json", "plugins"), ("tui.json", "plugin")] {
        if let Err(e) = prune_plugin_entry(&config.join(name), key, TUI_LEGACY) {
            log::warn!("could not clean up {}: {e}", config.join(name).display());
        }
    }
}

/// Remove one entry whose file name matches `file` from the plugin list under
/// `key`, preserving everything else.
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
    store::write_json(config, &json)?;
    Ok(true)
}

/// Sessions for one tab's directory. `opencode session list` is scoped to the
/// project of its working directory, so the command runs from the tab's cwd.
/// Bounded by a timeout so a stuck opencode can never freeze a request.
pub fn list_sessions(cwd: &str) -> Vec<AgentSession> {
    use std::io::Read;
    use std::process::Stdio;

    let mut cmd = crate::platform::command("opencode");
    cmd.args(["session", "list", "--format", "json", "--max-count", "1000"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let dir = cwd.trim();
    if !dir.is_empty() && Path::new(dir).is_dir() {
        cmd.current_dir(dir);
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
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
    parse_sessions(&reader.join().unwrap_or_default()).unwrap_or_default()
}

/// Parse `opencode session list --format json`; unknown fields are ignored.
pub fn parse_sessions(json: &str) -> Result<Vec<AgentSession>, String> {
    #[derive(Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_plugin_is_loadable_and_ours() {
        assert!(PLUGIN_FILE.ends_with(".js") || PLUGIN_FILE.ends_with(".ts"));
        assert_ne!(PLUGIN_FILE, PLUGIN_LEGACY);
        assert!(PLUGIN_SRC.contains(PLUGIN_MARKER));
        assert!(TUI_SRC.contains(TUI_MARKER));
    }

    #[test]
    fn sessions_parse_ignores_unknown_fields_and_defaults() {
        let list =
            parse_sessions(r#"[{"id":"ses_1","title":"t","directory":"/p","extra":1}]"#).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "ses_1");
        let bare = parse_sessions(r#"[{"id":"ses_2"}]"#).unwrap();
        assert_eq!(bare[0].title, "");
    }

    #[test]
    fn install_writes_ours_and_leaves_foreign_files_alone() {
        let dir = std::env::temp_dir().join(format!("muis-srv-plugin-{}", std::process::id()));
        let plugins = dir.join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        // Foreign file: must not be overwritten.
        std::fs::write(plugins.join(PLUGIN_FILE), "// someone else\n").unwrap();
        install_plugins(&plugins).unwrap();
        assert_eq!(
            std::fs::read_to_string(plugins.join(PLUGIN_FILE)).unwrap(),
            "// someone else\n"
        );
        // Ours (marker present): refreshed.
        std::fs::write(
            plugins.join(PLUGIN_FILE),
            format!("// {PLUGIN_MARKER} old\n"),
        )
        .unwrap();
        install_plugins(&plugins).unwrap();
        assert!(std::fs::read_to_string(plugins.join(PLUGIN_FILE))
            .unwrap()
            .contains("export default"));
        assert!(plugins.join(TUI_DIR).join(TUI_FILE).is_file());
        std::fs::remove_dir_all(&dir).ok();
    }
}
