//! On-disk scrollback: what a session printed, kept after the process is gone.
//!
//! The daemon appends each session's output to `<state>/muis/history/<session>.log`
//! and rotates at a byte cap, so a chatty agent cannot fill the disk. Search
//! strips ANSI sequences first: colour codes split words, and a grep that
//! cannot match `error` because it is `\e[31merror\e[0m` is useless.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Default per-session cap: 8 MiB, one rotated file, so 16 MiB worst case.
pub const DEFAULT_CAP: u64 = 8 * 1024 * 1024;

pub struct HistoryFile {
    session: String,
    dir: PathBuf,
    cap: u64,
    file: std::fs::File,
    written: u64,
}

impl HistoryFile {
    pub fn open(dir: &Path, session: &str, cap: u64) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("{}.log", safe_name(session)));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            session: session.to_string(),
            dir: dir.to_path_buf(),
            cap,
            file,
            written,
        })
    }

    pub fn append(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if self.cap == 0 {
            return Ok(());
        }
        if self.written + bytes.len() as u64 > self.cap {
            self.rotate()?;
        }
        self.file.write_all(bytes)?;
        self.written += bytes.len() as u64;
        Ok(())
    }

    /// Keep one previous generation: `<session>.log.1`, then start fresh.
    fn rotate(&mut self) -> std::io::Result<()> {
        let path = self.dir.join(format!("{}.log", safe_name(&self.session)));
        let _ = std::fs::rename(&path, path.with_extension("log.1"));
        self.file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)?;
        self.written = 0;
        Ok(())
    }
}

fn safe_name(session: &str) -> String {
    session
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

/// Strip CSI/OSC sequences. Deliberately small: enough for search, not a
/// terminal.
pub fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                // CSI: parameters until a final byte in @..~
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                // OSC: until BEL or ST
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {
                chars.next();
            }
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub session: String,
    pub line: usize,
    pub text: String,
}

/// Case-insensitive substring search across the history directory.
pub fn search(dir: &Path, query: &str, limit: usize) -> Vec<Hit> {
    let query = query.trim().to_lowercase();
    if query.is_empty() || limit == 0 {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("log")
                || p.to_string_lossy().ends_with(".log.1")
        })
        .collect();
    files.sort();
    let mut hits = Vec::new();
    for path in files {
        let session = path
            .file_name()
            .map(|n| {
                n.to_string_lossy()
                    .trim_end_matches(".log")
                    .trim_end_matches(".1")
                    .to_string()
            })
            .unwrap_or_default();
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (i, raw) in text.lines().enumerate() {
            let line = strip_ansi(raw);
            if line.to_lowercase().contains(&query) {
                hits.push(Hit {
                    session: session.clone(),
                    line: i + 1,
                    text: line.trim_end().to_string(),
                });
                if hits.len() >= limit {
                    return hits;
                }
            }
        }
    }
    hits
}

/// Render hits as `session:line: text` for the CLI.
pub fn render(hits: &[Hit]) -> String {
    let mut out = String::new();
    for hit in hits {
        let _ = writeln!(out, "{}:{}: {}", hit.session, hit.line, hit.text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("muis-history-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn appends_and_searches_without_ansi_noise() {
        let dir = temp_dir("search");
        let mut file = HistoryFile::open(&dir, "s1", DEFAULT_CAP).unwrap();
        file.append(b"all good\n\x1b[31merror\x1b[0m: boom\n")
            .unwrap();

        let hits = search(&dir, "ERROR", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session, "s1");
        assert_eq!(hits[0].text, "error: boom");
        assert_eq!(render(&hits), "s1:2: error: boom\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rotates_at_the_cap_and_keeps_one_generation() {
        let dir = temp_dir("rotate");
        let mut file = HistoryFile::open(&dir, "s1", 8).unwrap();
        file.append(b"12345678").unwrap();
        file.append(b"abcdefgh").unwrap();
        let current = std::fs::read_to_string(dir.join("s1.log")).unwrap();
        let previous = std::fs::read_to_string(dir.join("s1.log.1")).unwrap();
        assert_eq!(current, "abcdefgh");
        assert_eq!(previous, "12345678");
        assert_eq!(
            search(&dir, "1234", 5).len(),
            1,
            "rotated file is searched too"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cap_zero_disables_writing() {
        let dir = temp_dir("off");
        let mut file = HistoryFile::open(&dir, "s1", 0).unwrap();
        file.append(b"nothing").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("s1.log")).unwrap(), "");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn session_names_cannot_escape_the_directory() {
        assert_eq!(safe_name("../../etc/passwd"), "______etc_passwd");
    }

    #[test]
    fn strips_csi_and_osc() {
        assert_eq!(strip_ansi("\x1b[1;32mok\x1b[0m"), "ok");
        assert_eq!(strip_ansi("a\x1b]0;title\x07b"), "ab");
        assert_eq!(strip_ansi("plain"), "plain");
    }
}
