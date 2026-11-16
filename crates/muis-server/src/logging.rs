//! Diagnostic log next to the state dir, mirroring the native shell's log so
//! a browser-mode bug report has somewhere to point.

use std::path::PathBuf;

use simplelog::{Config, LevelFilter, WriteLogger};

/// Log file path (whether or not logging could be started).
pub fn log_file() -> PathBuf {
    muis_core::paths::log_dir().join("muis-server.log")
}

/// Start writing to the log file. Returns the path on success; a failure
/// only means no log, never a failed start.
pub fn init() -> Option<PathBuf> {
    let path = log_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    let level = match std::env::var("MUIS_LOG")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "trace" => LevelFilter::Trace,
        "debug" => LevelFilter::Debug,
        "warn" => LevelFilter::Warn,
        "error" => LevelFilter::Error,
        "off" => LevelFilter::Off,
        _ => LevelFilter::Info,
    };
    WriteLogger::init(level, Config::default(), file).ok()?;
    Some(path)
}
