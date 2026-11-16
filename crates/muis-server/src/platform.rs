//! Small per-platform shims the browser server needs.

use std::process::Command;

/// No console window pops up for a child of the (windowless) browser server.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Build a command for a console tool. On Windows everything goes through
/// `cmd /C`: npm-installed tools (`opencode`, `git` from a shim) are `.cmd`
/// files, which `CreateProcess` refuses to start directly.
pub fn command(program: &str) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", program]);
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    }
    #[cfg(not(windows))]
    {
        Command::new(program)
    }
}

/// Keep a spawned child from flashing a console window (no-op elsewhere).
pub fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

/// Open an http(s) URL in the user's browser. Only http/https are accepted;
/// never a shell invocation.
pub fn open_url(url: &str) -> Result<(), String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err("only http(s) URLs can be opened".into());
    }
    #[cfg(windows)]
    {
        // `start` takes the window title as its first argument.
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "start", "", trimmed]);
        hide_console(&mut cmd);
        cmd.spawn().map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(trimmed)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(trimmed)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_url_rejects_non_http_schemes() {
        assert!(open_url("file:///etc/passwd").is_err());
        assert!(open_url("javascript:alert(1)").is_err());
        assert!(open_url("").is_err());
    }
}
