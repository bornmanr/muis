//! Loopback auth: a random per-run token plus a host allowlist.
//!
//! The server binds 127.0.0.1, so the only reachable attackers are other
//! local processes and web pages the user is browsing. Pages cannot read a
//! cross-origin response, and the host allowlist blocks the DNS-rebinding
//! path that would make the request same-origin, so the token is defence in
//! depth rather than the only gate.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

/// 32 hex chars derived from clock, pid, and the stack address. Not a
/// cryptographic RNG, but unguessable from another process without reading
/// the URL that the server prints.
pub fn new_token() -> String {
    let mut hasher = DefaultHasher::new();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    nanos.hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    let local = 0u8;
    (&local as *const u8 as usize).hash(&mut hasher);
    let a = hasher.finish();
    let mut hasher = DefaultHasher::new();
    a.hash(&mut hasher);
    nanos.hash(&mut hasher);
    let b = hasher.finish();
    format!("{a:016x}{b:016x}")
}

/// Constant-time comparison, so a local brute force gains nothing from
/// response timing.
pub fn token_matches(expected: &str, given: Option<&str>) -> bool {
    let Some(given) = given else {
        return false;
    };
    if given.len() != expected.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in expected.bytes().zip(given.bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Host header must be a loopback name: the server is never a public
/// endpoint, and a foreign host means a rebound DNS name.
pub fn host_allowed(host: Option<&str>) -> bool {
    let Some(host) = host else {
        return true; // no Host header: not a browser request
    };
    let name = match host.rsplit_once(':') {
        // Keep IPv6 literals intact (`[::1]:8080`).
        Some((head, tail)) if tail.chars().all(|c| c.is_ascii_digit()) => head,
        _ => host,
    };
    matches!(name, "127.0.0.1" | "localhost" | "[::1]" | "::1")
}

/// Origin header, when present, must be this loopback server.
pub fn origin_allowed(origin: Option<&str>, port: u16) -> bool {
    let Some(origin) = origin else {
        return true; // same-origin navigations and non-browser clients
    };
    matches!(
        origin,
        o if o == format!("http://127.0.0.1:{port}") || o == format!("http://localhost:{port}")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_hex_and_unique() {
        let a = new_token();
        let b = new_token();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn token_comparison_rejects_wrong_and_missing() {
        assert!(token_matches("abc123", Some("abc123")));
        assert!(!token_matches("abc123", Some("abc124")));
        assert!(!token_matches("abc123", Some("abc12")));
        assert!(!token_matches("abc123", None));
    }

    #[test]
    fn only_loopback_hosts_are_allowed() {
        assert!(host_allowed(Some("127.0.0.1:8080")));
        assert!(host_allowed(Some("localhost:8080")));
        assert!(host_allowed(Some("[::1]:8080")));
        assert!(host_allowed(Some("127.0.0.1")));
        assert!(host_allowed(None));
        assert!(!host_allowed(Some("evil.example:8080")));
        // Rebinding: attacker name that resolves to loopback.
        assert!(!host_allowed(Some("muis.attacker.test:8080")));
    }

    #[test]
    fn origins_must_match_the_bound_port() {
        assert!(origin_allowed(Some("http://127.0.0.1:9000"), 9000));
        assert!(origin_allowed(Some("http://localhost:9000"), 9000));
        assert!(origin_allowed(None, 9000));
        assert!(!origin_allowed(Some("http://127.0.0.1:9001"), 9000));
        assert!(!origin_allowed(Some("http://evil.test"), 9000));
    }
}
