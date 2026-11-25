//! OSC 133 hooks for the user's shell.
//!
//! muis already understands OSC 133 (command start/end, exit codes,
//! durations) and OSC 7 (cwd). Fish emits cwd itself; the command markers are
//! what this installs. Everything is opt-in, idempotent and reversible: an
//! installer that can quietly corrupt someone's `.zshrc` is not worth the
//! feature.
//!
//! zsh and fish get start + end markers. bash only gets the end marker: the
//! start marker needs a `DEBUG` trap, which fires inside functions and
//! pipelines and breaks real scripts. Shells without a start marker already
//! fall back to muis' keystroke capture for the command line.

use std::path::{Path, PathBuf};

pub const START_MARKER: &str = "# >>> muis shell integration >>>";
pub const END_MARKER: &str = "# <<< muis shell integration <<<";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Fish,
    Zsh,
    Bash,
}

impl Shell {
    pub fn parse(name: &str) -> Option<Self> {
        match name.rsplit('/').next().unwrap_or(name) {
            "fish" => Some(Self::Fish),
            "zsh" => Some(Self::Zsh),
            "bash" => Some(Self::Bash),
            _ => None,
        }
    }

    /// The rc file that gets the sourced block (fish uses conf.d instead).
    fn rc_file(self) -> &'static str {
        match self {
            Self::Fish => "",
            Self::Zsh => ".zshrc",
            Self::Bash => ".bashrc",
        }
    }
}

/// The shell script itself. Kept plain: no dependencies, no prompts, no
/// output, so a broken terminal is never a consequence of installing it.
pub fn snippet(shell: Shell) -> &'static str {
    match shell {
        Shell::Fish => {
            "# muis shell integration (managed by `muis shell-integration`)\n\
function __muis_cmd_start --on-event fish_preexec\n\
    printf '\\e]133;C\\a'\n\
end\n\
function __muis_cmd_end --on-event fish_postexec\n\
    printf '\\e]133;D;%s\\a' $status\n\
end\n"
        }
        Shell::Zsh => {
            "# muis shell integration (managed by `muis shell-integration`)\n\
__muis_cmd_end() {\n\
    printf '\\033]133;D;%s\\033\\\\' \"$?\"\n\
}\n\
__muis_cmd_start() {\n\
    printf '\\033]133;C\\033\\\\'\n\
}\n\
autoload -Uz add-zsh-hook\n\
add-zsh-hook precmd __muis_cmd_end\n\
add-zsh-hook preexec __muis_cmd_start\n"
        }
        Shell::Bash => {
            "# muis shell integration (managed by `muis shell-integration`)\n\
# bash gets the end marker only: a start marker needs a DEBUG trap, which\n\
# fires inside functions and pipelines. muis captures the command line from\n\
# keystrokes instead.\n\
__muis_cmd_end() {\n\
    local code=$?\n\
    printf '\\033]133;D;%s\\033\\\\' \"$code\"\n\
    return $code\n\
}\n\
case \"${PROMPT_COMMAND:-}\" in\n\
    *__muis_cmd_end*) ;;\n\
    *) PROMPT_COMMAND=\"__muis_cmd_end${PROMPT_COMMAND:+; $PROMPT_COMMAND}\" ;;\n\
esac\n"
        }
    }
}

/// Where the snippet lives: fish gets a conf.d file (auto-sourced), the
/// others get a file we source from their rc.
fn snippet_path(home: &Path, shell: Shell) -> PathBuf {
    match shell {
        Shell::Fish => home.join(".config/fish/conf.d/muis.fish"),
        Shell::Zsh => home.join(".config/muis/shell-integration.zsh"),
        Shell::Bash => home.join(".config/muis/shell-integration.bash"),
    }
}

/// The block appended to `.zshrc` / `.bashrc`.
fn rc_block(shell: Shell) -> String {
    let path = match shell {
        Shell::Zsh => "$HOME/.config/muis/shell-integration.zsh",
        Shell::Bash => "$HOME/.config/muis/shell-integration.bash",
        Shell::Fish => unreachable!("fish uses conf.d"),
    };
    format!("{START_MARKER}\n[ -f \"{path}\" ] && . \"{path}\"\n{END_MARKER}\n")
}

/// Install (or refresh) the integration. Returns the files it touched.
pub fn install(home: &Path, shell: Shell) -> Result<Vec<PathBuf>, String> {
    let snippet_file = snippet_path(home, shell);
    if let Some(dir) = snippet_file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&snippet_file, snippet(shell))
        .map_err(|e| format!("{}: {e}", snippet_file.display()))?;
    let mut touched = vec![snippet_file];

    if shell != Shell::Fish {
        let rc = home.join(shell.rc_file());
        let existing = std::fs::read_to_string(&rc).unwrap_or_default();
        let updated = replace_block(&existing, &rc_block(shell));
        if updated != existing {
            std::fs::write(&rc, updated).map_err(|e| format!("{}: {e}", rc.display()))?;
        }
        touched.push(rc);
    }
    Ok(touched)
}

/// Remove everything the installer wrote, restoring the rc file.
pub fn uninstall(home: &Path, shell: Shell) -> Result<Vec<PathBuf>, String> {
    let snippet_file = snippet_path(home, shell);
    let mut touched = Vec::new();
    if snippet_file.exists() {
        std::fs::remove_file(&snippet_file)
            .map_err(|e| format!("{}: {e}", snippet_file.display()))?;
        touched.push(snippet_file);
    }
    if shell != Shell::Fish {
        let rc = home.join(shell.rc_file());
        if let Ok(existing) = std::fs::read_to_string(&rc) {
            let updated = strip_block(&existing);
            if updated != existing {
                std::fs::write(&rc, updated).map_err(|e| format!("{}: {e}", rc.display()))?;
                touched.push(rc);
            }
        }
    }
    Ok(touched)
}

/// Replace an existing managed block, or append one. Never duplicates.
fn replace_block(text: &str, block: &str) -> String {
    let stripped = strip_block(text);
    let mut out = stripped;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(block);
    out
}

/// Remove every managed block, keeping the rest byte-for-byte.
fn strip_block(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for line in text.split_inclusive('\n') {
        if line.trim_end() == START_MARKER {
            inside = true;
            continue;
        }
        if inside && line.trim_end() == END_MARKER {
            inside = false;
            continue;
        }
        if !inside {
            out.push_str(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("muis-shell-int-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn snippet_emits_133_markers_per_shell() {
        assert!(snippet(Shell::Fish).contains("fish_preexec"));
        assert!(snippet(Shell::Fish).contains("133;C"));
        assert!(snippet(Shell::Zsh).contains("add-zsh-hook preexec"));
        // bash deliberately has no start marker.
        assert!(snippet(Shell::Bash).contains("133;D"));
        assert!(!snippet(Shell::Bash).contains("133;C"));
        assert!(snippet(Shell::Bash).contains("PROMPT_COMMAND"));
    }

    #[test]
    fn install_is_idempotent_and_uninstall_restores_the_rc() {
        let home = temp_home("zsh");
        let rc = home.join(".zshrc");
        std::fs::write(&rc, "export EDITOR=vim\n").unwrap();

        install(&home, Shell::Zsh).unwrap();
        let once = std::fs::read_to_string(&rc).unwrap();
        assert!(once.starts_with("export EDITOR=vim\n"));
        assert_eq!(once.matches(START_MARKER).count(), 1);

        install(&home, Shell::Zsh).unwrap();
        let twice = std::fs::read_to_string(&rc).unwrap();
        assert_eq!(twice.matches(START_MARKER).count(), 1);
        assert!(home.join(".config/muis/shell-integration.zsh").is_file());

        uninstall(&home, Shell::Zsh).unwrap();
        assert_eq!(std::fs::read_to_string(&rc).unwrap(), "export EDITOR=vim\n");
        assert!(!home.join(".config/muis/shell-integration.zsh").exists());
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn fish_writes_a_conf_d_file_and_removes_it() {
        let home = temp_home("fish");
        install(&home, Shell::Fish).unwrap();
        let file = home.join(".config/fish/conf.d/muis.fish");
        assert!(file.is_file());
        uninstall(&home, Shell::Fish).unwrap();
        assert!(!file.exists());
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn strip_block_keeps_untouched_text_byte_for_byte() {
        let text = "a\n# >>> muis shell integration >>>\nx\n# <<< muis shell integration <<<\nb\n";
        assert_eq!(strip_block(text), "a\nb\n");
        assert_eq!(strip_block("nothing\n"), "nothing\n");
    }
}
