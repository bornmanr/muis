//! Worktree-per-tab support: where a new worktree goes and what it is called.
//!
//! The convention is the one the repository already uses by hand:
//! `<parent>/.worktrees/<repo>-<slug>` on a neutral `feature/<slug>` branch.
//! Everything is plain `git` invocations with argv (never a shell string), so
//! a worktree name cannot turn into a command.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Neutral prefixes only: `main` stays a read-only integration target.
pub const BRANCH_PREFIX: &str = "feature/";

/// Lowercase, dash-separated, 1-48 characters. `None` when nothing is left.
pub fn slugify(input: &str) -> Option<String> {
    let mut out = String::new();
    let mut dash = false;
    for ch in input.trim().chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() || trimmed.len() > 48 {
        return None;
    }
    Some(trimmed.to_string())
}

pub fn branch_for(slug: &str) -> String {
    format!("{BRANCH_PREFIX}{slug}")
}

/// `<parent>/.worktrees/<repo>-<slug>`, or `None` for a filesystem root.
pub fn worktree_path(repo: &Path, slug: &str) -> Option<PathBuf> {
    let name = repo.file_name()?.to_string_lossy();
    Some(
        repo.parent()?
            .join(".worktrees")
            .join(format!("{name}-{slug}")),
    )
}

/// Create the worktree and return its path. Refuses anything ambiguous: an
/// existing directory, an existing branch, or a name that slugs to nothing.
pub fn create(cwd: &Path, name: &str) -> Result<PathBuf, String> {
    let slug = slugify(name).ok_or("use letters or digits in the worktree name")?;
    let repo = PathBuf::from(git(cwd, &["rev-parse", "--show-toplevel"])?.trim());
    let path =
        worktree_path(&repo, &slug).ok_or("cannot place a worktree next to this repository")?;
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    let branch = branch_for(&slug);
    if Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return Err(format!("branch {branch} already exists"));
    }
    let base = git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let base = base.trim();
    if base.is_empty() || base == "HEAD" {
        return Err("the repository has no branch checked out".to_string());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let status = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["worktree", "add", "-b", &branch])
        .arg(&path)
        .arg(base)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("git worktree add failed for {branch}"));
    }
    Ok(path)
}

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("muis-wt-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(dir.join("README.md"), "hi\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        dir
    }

    #[test]
    fn slugs_are_safe_and_stable() {
        assert_eq!(
            slugify("Add cursor pagination").unwrap(),
            "add-cursor-pagination"
        );
        assert_eq!(slugify("  Fix   the  thing!! ").unwrap(), "fix-the-thing");
        assert_eq!(slugify("miXed/CASE"), Some("mixed-case".to_string()));
        assert!(slugify("   ").is_none());
        assert!(slugify("!!!").is_none());
        assert!(slugify(&"a".repeat(60)).is_none());
        assert_eq!(branch_for("x"), "feature/x");
    }

    #[test]
    fn worktrees_live_in_the_sibling_convention() {
        let path = worktree_path(Path::new("/home/user/Projects/muis"), "feat-x").unwrap();
        assert_eq!(
            path,
            PathBuf::from("/home/user/Projects/.worktrees/muis-feat-x")
        );
        assert!(worktree_path(Path::new("/"), "x").is_none());
    }

    #[test]
    fn create_makes_a_branch_and_refuses_a_second_time() {
        let repo = temp_repo("create");
        // `create` is given a cwd inside the repo, like a tab would be.
        let sub = repo.join("crates");
        std::fs::create_dir_all(&sub).unwrap();
        let path = create(&sub, "Try me once").unwrap();
        assert!(path.is_dir(), "worktree directory exists");
        assert!(
            path.join("README.md").is_file(),
            "worktree has the base tree"
        );

        let branches = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["branch", "--list", "feature/try-me-once"])
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&branches.stdout).contains("feature/try-me-once"));

        assert!(
            create(&sub, "Try me once").is_err(),
            "refuses the duplicate"
        );
        assert!(create(&sub, "   ").is_err(), "refuses a blank name");

        let _ = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["worktree", "remove", "--force"])
            .arg(&path)
            .status();
        std::fs::remove_dir_all(&repo).ok();
    }
}
