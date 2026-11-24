//! Read-only lookups Snor may use on the current workspace.
//!
//! Nothing here can write, and every path is resolved inside the
//! workspace directory. The UI turns explicit slash commands into these
//! calls; the model never chooses a path or command on its own.

use std::path::{Path, PathBuf};

pub const MAX_DOC_BYTES: u64 = 64 * 1024;
pub const MAX_DOC_CHARS: usize = 20_000;
pub const MAX_GIT_CHARS: usize = 8_000;
pub const MAX_DOC_ENTRIES: usize = 200;

/// `git status --short --branch`, truncated so one huge status cannot
/// flood the context window.
pub fn git_status(cwd: &Path) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(["-C"])
        .arg(cwd)
        .args(["status", "--short", "--branch"])
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git status failed: {}", stderr.trim()));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(truncate_chars(text.trim_end(), MAX_GIT_CHARS))
}

/// Documentation files under `docs/` (or top-level Markdown when there
/// is no docs directory), relative to the workspace, sorted.
pub fn list_docs(cwd: &Path) -> Result<Vec<String>, String> {
    let root = match std::fs::canonicalize(cwd) {
        Ok(root) => root,
        Err(e) => return Err(format!("workspace not readable: {e}")),
    };
    let docs = root.join("docs");
    let base = if docs.is_dir() { docs } else { root.clone() };
    let mut out = Vec::new();
    collect_docs(&base, &root, 0, &mut out);
    out.sort();
    out.truncate(MAX_DOC_ENTRIES);
    Ok(out)
}

fn collect_docs(dir: &Path, root: &Path, depth: usize, out: &mut Vec<String>) {
    if depth > 3 || out.len() >= MAX_DOC_ENTRIES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect_docs(&path, root, depth + 1, out);
        } else if is_doc(&path) {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().into_owned());
            }
        }
    }
}

fn is_doc(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("md") | Some("txt") | Some("rst") | Some("jsonc") | Some("json")
    )
}

/// Read a UTF-8 text file inside the workspace. Rejects absolute paths,
/// `..` escapes, symlinks pointing outside, oversized files, and binary
/// content.
pub fn read_doc(cwd: &Path, relative: &str, max_bytes: u64) -> Result<String, String> {
    let relative = relative.trim();
    if relative.is_empty() {
        return Err("no file given".into());
    }
    let rel = Path::new(relative);
    if rel.is_absolute() {
        return Err("use a path relative to the workspace".into());
    }
    if rel
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("the path may not leave the workspace".into());
    }
    let root = std::fs::canonicalize(cwd).map_err(|e| format!("workspace not readable: {e}"))?;
    let joined = root.join(rel);
    let resolved = std::fs::canonicalize(&joined).map_err(|e| format!("cannot open {relative}: {e}"))?;
    if !resolved.starts_with(&root) {
        return Err("the path may not leave the workspace".into());
    }
    let meta = std::fs::metadata(&resolved).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    if meta.len() > max_bytes {
        return Err(format!(
            "file is {} bytes; the limit is {max_bytes}",
            meta.len()
        ));
    }
    let bytes = std::fs::read(&resolved).map_err(|e| e.to_string())?;
    let text = String::from_utf8(bytes).map_err(|_| "not a UTF-8 text file".to_string())?;
    Ok(truncate_chars(&text, MAX_DOC_CHARS))
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str("\n… (truncated)");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "muis-snor-tools-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/plan.md"), "# Plan\n").unwrap();
        std::fs::write(dir.join("README.md"), "# Readme\n").unwrap();
        std::fs::write(dir.join("docs/blob.bin"), [0xff, 0xfe, 0x00]).unwrap();
        std::fs::write(dir.join("outside.txt"), "secret\n").unwrap();
        dir
    }

    #[test]
    fn lists_docs_relative_and_sorted() {
        let dir = workspace("list");
        let docs = list_docs(&dir).unwrap();
        assert!(docs.contains(&"docs/plan.md".to_string()));
        assert!(!docs.iter().any(|d| d.ends_with(".bin")));
        let mut sorted = docs.clone();
        sorted.sort();
        assert_eq!(docs, sorted);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_text_inside_the_workspace() {
        let dir = workspace("read");
        let text = read_doc(&dir, "docs/plan.md", MAX_DOC_BYTES).unwrap();
        assert_eq!(text, "# Plan\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_escapes_absolute_paths_binaries_and_oversized_files() {
        let dir = workspace("reject");
        assert!(read_doc(&dir, "../outside.txt", MAX_DOC_BYTES).is_err());
        assert!(read_doc(&dir, "/etc/passwd", MAX_DOC_BYTES).is_err());
        assert!(read_doc(&dir, "docs/blob.bin", MAX_DOC_BYTES).is_err());
        assert!(read_doc(&dir, "docs/plan.md", 2).is_err());
        assert!(read_doc(&dir, "", MAX_DOC_BYTES).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn truncates_long_text_on_a_char_boundary() {
        let out = truncate_chars("aébc", 2);
        assert!(out.starts_with("aé"));
        assert!(out.ends_with("(truncated)"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_that_leave_the_workspace() {
        let dir = workspace("symlink");
        let link = dir.join("docs/link.md");
        std::os::unix::fs::symlink(dir.join("outside.txt"), &link).unwrap();
        // The link target resolves inside the workspace here, so this one
        // is allowed; the escape case points at a parent path outside.
        assert!(read_doc(&dir, "docs/link.md", MAX_DOC_BYTES).is_ok());
        let escape = dir.join("docs/escape.md");
        std::os::unix::fs::symlink("/etc/hostname", &escape).unwrap();
        assert!(read_doc(&dir, "docs/escape.md", MAX_DOC_BYTES).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_status_reports_non_repos_without_panicking() {
        let dir = workspace("git");
        let result = git_status(&dir);
        assert!(result.is_err() || result.as_deref().unwrap().contains("##"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
