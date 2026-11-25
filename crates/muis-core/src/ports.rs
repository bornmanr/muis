//! Listening TCP ports, attributed to the tabs that own them.
//!
//! Read-only `/proc` scraping: `/proc/net/tcp{,6}` gives the sockets and their
//! inodes, `/proc/<pid>/fd` maps an inode back to a process, and the process
//! cwd decides which tab it belongs to. Nothing here touches a network: the
//! kernel already knows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One `LISTEN` socket from `/proc/net/tcp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Listener {
    pub port: u16,
    pub inode: u64,
}

/// Parse `/proc/net/tcp` or `tcp6` content, keeping `LISTEN` rows only.
///
/// Header line, then columns: `sl local_address rem_address st ... inode`.
/// The state for a listening socket is `0A`.
pub fn parse_listeners(text: &str) -> Vec<Listener> {
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 10 || cols[3] != "0A" {
            continue;
        }
        let Some((_, port)) = cols[1].rsplit_once(':') else {
            continue;
        };
        let (Ok(port), Ok(inode)) = (u16::from_str_radix(port, 16), cols[9].parse::<u64>()) else {
            continue;
        };
        if inode > 0 {
            out.push(Listener { port, inode });
        }
    }
    out.sort_by_key(|l| (l.port, l.inode));
    out.dedup();
    out
}

/// Inode -> pid for every process that holds a socket.
pub fn socket_owners(proc_root: &Path) -> HashMap<u64, u32> {
    let mut owners = HashMap::new();
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return owners;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            let Ok(target) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let Some(rest) = target.to_str().and_then(|t| t.strip_prefix("socket:[")) else {
                continue;
            };
            let Some(inode) = rest.strip_suffix(']').and_then(|n| n.parse::<u64>().ok()) else {
                continue;
            };
            owners.insert(inode, pid);
        }
    }
    owners
}

/// Listening ports belonging to `cwd` or anything below it. `cwd_of` is
/// injected so the attribution is testable without real processes.
pub fn ports_belonging(
    cwd: &Path,
    listeners: &[Listener],
    owners: &HashMap<u64, u32>,
    cwd_of: impl Fn(u32) -> Option<PathBuf>,
) -> Vec<u16> {
    let base = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut ports: Vec<u16> = listeners
        .iter()
        .filter(|l| {
            owners
                .get(&l.inode)
                .and_then(|pid| cwd_of(*pid))
                .is_some_and(|p| p.starts_with(&base))
        })
        .map(|l| l.port)
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// Same, reading the real process table.
pub fn ports_for(cwd: &Path, listeners: &[Listener], owners: &HashMap<u64, u32>) -> Vec<u16> {
    ports_belonging(cwd, listeners, owners, process_cwd)
}

fn process_cwd(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// The live answer for one tab.
pub fn listening_under(cwd: &Path) -> Vec<u16> {
    let mut listeners = Vec::new();
    for file in ["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(text) = std::fs::read_to_string(file) {
            listeners.extend(parse_listeners(&text));
        }
    }
    ports_for(cwd, &listeners, &socket_owners(Path::new("/proc")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:0CEA 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 111 1
   1: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 222 1
   2: 0100007F:1F90 0100007F:D431 01 00000000:00000000 00:00000000 00000000  1000        0 333 1
   3: 00000000:0000 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 0 1
";

    #[test]
    fn keeps_only_listening_sockets_with_an_inode() {
        let listeners = parse_listeners(SAMPLE);
        assert_eq!(
            listeners,
            vec![
                Listener {
                    port: 22,
                    inode: 222
                },
                Listener {
                    port: 3306,
                    inode: 111
                },
            ]
        );
    }

    #[test]
    fn attributes_a_port_to_the_process_cwd_below_the_tab() {
        let listeners = parse_listeners(SAMPLE);
        let owners = HashMap::from([(111u64, 42u32), (222, 43)]);
        let project = std::env::temp_dir().join("muis-ports-project");
        let inside = project.join("web");
        let outside = std::env::temp_dir().join("muis-ports-elsewhere");
        let cwd_of = |pid: u32| match pid {
            42 => Some(inside.clone()),
            43 => Some(outside.clone()),
            _ => None,
        };
        // Only the socket owned from inside the tab's directory counts.
        assert_eq!(
            ports_belonging(&project, &listeners, &owners, cwd_of),
            vec![3306]
        );
        assert!(ports_belonging(&outside, &listeners, &owners, |_| None).is_empty());
    }

    #[test]
    fn ignores_malformed_lines() {
        assert!(parse_listeners("garbage\n 0: nope\n").is_empty());
        assert!(parse_listeners("").is_empty());
    }
}
