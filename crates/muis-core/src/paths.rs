use std::ffi::OsString;
use std::path::PathBuf;

/// Explicit override for every path muis writes: `MUIS_HOME`, or on Windows
/// a `muis.portable` marker file next to the executable (data lands in
/// `<exe-dir>/data`). This is the whole portable mode — no installer, no
/// registry, and deleting one folder resets muis.
pub fn home_override() -> Option<PathBuf> {
    let from_env = std::env::var_os("MUIS_HOME").filter(|value| !value.is_empty());
    #[cfg(windows)]
    let marker = portable_marker_dir();
    #[cfg(not(windows))]
    let marker: Option<PathBuf> = None;
    resolve_override(from_env, marker)
}

/// Pure step of [`home_override`], so the precedence is testable without
/// mutating the process environment.
fn resolve_override(env: Option<OsString>, marker: Option<PathBuf>) -> Option<PathBuf> {
    env.map(PathBuf::from).or(marker)
}

/// `<exe-dir>/data` when a `muis.portable` marker sits next to the binary.
#[cfg(windows)]
fn portable_marker_dir() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    dir.join("muis.portable")
        .is_file()
        .then(|| dir.join("data"))
}

/// Platform home directory: `%USERPROFILE%` on Windows, `$HOME` elsewhere.
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

/// Config directory. Linux follows XDG; Windows uses `%APPDATA%\muis` so the
/// same filenames work from a portable copy.
pub fn config_dir() -> PathBuf {
    if let Some(home) = home_override() {
        return home.join("config");
    }
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            if !appdata.is_empty() {
                return PathBuf::from(appdata).join("muis");
            }
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("muis");
            }
        }
    }
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("muis")
}

/// State directory (workspaces, tabs, agent resume state).
pub fn state_dir() -> PathBuf {
    if let Some(home) = home_override() {
        return home.join("state");
    }
    #[cfg(windows)]
    {
        return config_dir();
    }
    #[cfg(not(windows))]
    {
        if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("muis");
            }
        }
        home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".local")
            .join("share")
            .join("muis")
    }
}

/// Diagnostic log files live here. Windows uses `%LOCALAPPDATA%` (logs are
/// machine-local, not roaming); other platforms follow the state dir.
pub fn log_dir() -> PathBuf {
    if home_override().is_some() {
        return state_dir().join("logs");
    }
    #[cfg(windows)]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            if !local.is_empty() {
                return PathBuf::from(local).join("muis").join("logs");
            }
        }
    }
    state_dir().join("logs")
}

pub fn config_file() -> PathBuf {
    config_dir().join("muis.json")
}

/// Workspaces, tabs, and per-tab agent resume state.
pub fn sessions_file() -> PathBuf {
    state_dir().join("sessions.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirs_end_in_muis_and_nest() {
        assert!(config_dir().ends_with("muis"));
        assert!(log_dir().ends_with("logs"));
        assert_eq!(config_file(), config_dir().join("muis.json"));
        assert_eq!(sessions_file(), state_dir().join("sessions.json"));
    }

    #[test]
    fn explicit_home_wins_over_the_portable_marker() {
        let resolved = resolve_override(
            Some(OsString::from("/tmp/muis-home")),
            Some(PathBuf::from("/exe/data")),
        );
        assert_eq!(resolved, Some(PathBuf::from("/tmp/muis-home")));
    }

    #[test]
    fn portable_marker_is_the_fallback() {
        assert_eq!(
            resolve_override(None, Some(PathBuf::from("/exe/data"))),
            Some(PathBuf::from("/exe/data"))
        );
        assert_eq!(resolve_override(None, None), None);
    }
}
