//! Embedded frontend bundle plus the serve-mode bootstrap script.

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

/// Served when the binary was built without `src-ui/dist`. A release build
/// always embeds the real UI; this exists so `cargo test --workspace` works
/// on a machine with no Node installed.
const PLACEHOLDER: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>muis</title>
<style>body{background:#1b1d21;color:#cdd6e4;font:14px system-ui;padding:3rem;line-height:1.6}
code{background:#25282e;padding:.15rem .4rem;border-radius:4px}</style></head>
<body><h1>muis</h1>
<p>This binary was built without a frontend bundle.</p>
<p>Run <code>npm run build --prefix src-ui</code> and rebuild, or set
<code>MUIS_UI_DIST</code> to a built <code>dist</code> directory.</p>
</body></html>
"#;

/// Content type for a served asset, by extension.
pub fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "map" => "application/json",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// `index.html` with the serve-mode token injected, so the frontend knows to
/// talk HTTP/WebSocket instead of Tauri IPC. Serving this cross-origin is
/// harmless: the response is opaque without CORS, and the host allowlist
/// stops DNS-rebinding tricks.
pub fn index_html(token: &str) -> String {
    let html = get("index.html")
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .unwrap_or_else(|| PLACEHOLDER.to_string());
    let boot = format!("<script>window.__MUIS_SERVE__={{\"token\":\"{token}\"}};</script>");
    match html.rfind("</head>") {
        Some(idx) => {
            let mut out = String::with_capacity(html.len() + boot.len());
            out.push_str(&html[..idx]);
            out.push_str(&boot);
            out.push_str(&html[idx..]);
            out
        }
        None => format!("{boot}{html}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_types_cover_the_bundle() {
        assert!(content_type("assets/index-abc123.js").starts_with("text/javascript"));
        assert!(content_type("style.css").starts_with("text/css"));
        assert_eq!(content_type("noext"), "application/octet-stream");
    }

    #[test]
    fn index_injects_the_token_before_head_end() {
        let html = index_html("deadbeef");
        let boot = html.find("__MUIS_SERVE__").expect("token injected");
        let head = html.find("</head>").unwrap_or(usize::MAX);
        assert!(boot < head);
        assert!(html.contains(r#"{"token":"deadbeef"}"#));
    }
}
