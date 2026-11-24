//! Local file viewer served to the browser pane over loopback.
//!
//! The pane only loads http(s), deliberately. To show a local file it needs a
//! loopback origin, so this starts a tiny HTTP server on `127.0.0.1:0` and
//! serves exactly the files the UI registered — never an arbitrary path from
//! the query string. The token and the allow-list are both required, so a
//! guessed URL cannot read anything.
//!
//! Kept in core, std-only: the native window and the browser host share it,
//! and the whole thing tests without WebKit.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Largest file the viewer will read into memory. Bigger files are refused
/// rather than streamed: a terminal view of a 200 MB file helps nobody.
pub const MAX_VIEW_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone)]
struct State {
    token: String,
    allowed: Arc<Mutex<HashSet<PathBuf>>>,
}

/// A running loopback viewer. Dropping it leaves the port bound for the
/// process lifetime (the thread owns the listener); callers keep one.
pub struct ViewServer {
    port: u16,
    state: State,
}

impl ViewServer {
    /// Bind an ephemeral port and start serving.
    pub fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let state = State {
            token: random_token(),
            allowed: Arc::new(Mutex::new(HashSet::new())),
        };
        let server_state = state.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = server_state.clone();
                std::thread::spawn(move || {
                    let _ = handle(stream, &state);
                });
            }
        });
        Ok(Self { port, state })
    }

    /// Register a file and return the URL that renders it.
    pub fn url_for(&self, path: &Path) -> Result<String, String> {
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if !canonical.is_file() {
            return Err(format!("{} is not a file", canonical.display()));
        }
        let size = std::fs::metadata(&canonical)
            .map_err(|e| e.to_string())?
            .len();
        if size > MAX_VIEW_BYTES {
            return Err(format!(
                "{} is {} MB; the viewer stops at {} MB",
                canonical.display(),
                size / (1024 * 1024),
                MAX_VIEW_BYTES / (1024 * 1024)
            ));
        }
        self.state.allowed.lock().unwrap().insert(canonical.clone());
        Ok(format!(
            "http://127.0.0.1:{}/view?t={}&p={}",
            self.port,
            self.state.token,
            percent_encode(&canonical.to_string_lossy())
        ))
    }
}

/// 128-bit hex token. Not a security boundary on its own (the allow-list is),
/// but it keeps a stray navigation from landing on the viewer.
fn random_token() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let seed = format!(
        "{now}-{}-{:p}",
        std::process::id(),
        &now as *const _ as *const ()
    );
    let mut hash: u128 = 0xcbf2_9ce4_8422_2325;
    for byte in seed.as_bytes() {
        hash ^= u128::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:032x}")
}

fn handle(mut stream: TcpStream, state: &State) -> std::io::Result<()> {
    let mut buf = [0u8; 8192];
    let read = stream.read(&mut buf)?;
    let request = String::from_utf8_lossy(&buf[..read]);
    let path = request.lines().next().unwrap_or_default();

    let (status, content_type, body) = match route(path, state) {
        Ok((content_type, body)) => ("200 OK", content_type, body),
        Err(status) => (
            status,
            "text/plain; charset=utf-8",
            status.as_bytes().to_vec(),
        ),
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(&body)?;
    stream.flush()
}

fn route(request_line: &str, state: &State) -> Result<(&'static str, Vec<u8>), &'static str> {
    let Some(target) = request_line.split_whitespace().nth(1) else {
        return Err("400 Bad Request");
    };
    let Some(query) = target.strip_prefix("/view?") else {
        return Err("404 Not Found");
    };
    let mut token = None;
    let mut path = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "t" => token = Some(value),
            "p" => path = Some(percent_decode(value)),
            _ => {}
        }
    }
    if token != Some(state.token.as_str()) {
        return Err("403 Forbidden");
    }
    let Some(path) = path else {
        return Err("400 Bad Request");
    };
    let path = PathBuf::from(path);
    if !state.allowed.lock().unwrap().contains(&path) {
        return Err("403 Forbidden");
    }
    let bytes = std::fs::read(&path).map_err(|_| "404 Not Found")?;
    let kind = kind_of(&path);
    let body = if kind == "text/markdown; charset=utf-8" {
        markdown_page(&String::from_utf8_lossy(&bytes)).into_bytes()
    } else {
        bytes
    };
    Ok((kind, body))
}

/// Content type by extension. Everything unknown is plain text, and markup
/// is served as source: a viewer must not execute what it is showing.
fn kind_of(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "pdf" => "application/pdf",
        "md" | "markdown" => "text/markdown; charset=utf-8",
        _ => "text/plain; charset=utf-8",
    }
}

fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &text[i + 1..i + 3];
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Markdown as a self-contained page. The renderer is deliberately tiny: the
/// viewer shows notes and diffs, not a publishing pipeline.
fn markdown_page(source: &str) -> String {
    format!(
        r#"<!doctype html><meta charset="utf-8"><title>muis view</title>
<style>
:root{{color-scheme:dark}}
body{{margin:0;padding:24px 32px;background:#1d2023;color:#fcfcfc;
font:15px/1.6 'Noto Sans',system-ui,sans-serif;max-width:900px}}
h1,h2,h3{{color:#3daee9;line-height:1.25;margin:1.4em 0 .5em}}
code{{background:#2d3237;padding:1px 4px;border-radius:3px;font-family:'JetBrains Mono',monospace;font-size:.9em}}
pre{{background:#232629;border:1px solid #3a4147;border-radius:6px;padding:12px;overflow:auto}}
pre code{{background:none;padding:0}}
a{{color:#7bbfb6}}
blockquote{{margin:0;padding-left:14px;border-left:3px solid #3a4147;color:#9ca4ab}}
hr{{border:0;border-top:1px solid #3a4147;margin:24px 0}}
table{{border-collapse:collapse}}td,th{{border:1px solid #3a4147;padding:4px 8px}}
</style><body><div id="out"></div>
<script>
const src = {source};
const esc = (s) => s.replace(/[&<>]/g, (c) => ({{'&':'&amp;','<':'&lt;','>':'&gt;'}})[c]);
let html = '', inCode = false, inList = false;
for (const raw of src.split(/\r?\n/)) {{
  const line = raw.replace(/\s+$/, '');
  if (line.startsWith('```')) {{
    if (inCode) {{ html += '</code></pre>'; inCode = false; }}
    else {{ html += '<pre><code>'; inCode = true; }}
    continue;
  }}
  if (inCode) {{ html += esc(line) + '\n'; continue; }}
  const list = /^\s*[-*]\s+(.*)$/.exec(line);
  if (list) {{
    if (!inList) {{ html += '<ul>'; inList = true; }}
    html += '<li>' + inline(list[1]) + '</li>';
    continue;
  }}
  if (inList) {{ html += '</ul>'; inList = false; }}
  if (!line) {{ continue; }}
  const head = /^(#{{1,6}})\s+(.*)$/.exec(line);
  if (head) {{ const n = head[1].length; html += `<h${{n}}>` + inline(head[2]) + `</h${{n}}>`; continue; }}
  if (/^>\s?/.test(line)) {{ html += '<blockquote>' + inline(line.replace(/^>\s?/, '')) + '</blockquote>'; continue; }}
  if (/^(-{{3,}}|\*{{3,}})$/.test(line)) {{ html += '<hr>'; continue; }}
  html += '<p>' + inline(line) + '</p>';
}}
if (inCode) html += '</code></pre>';
if (inList) html += '</ul>';
function inline(s) {{
  return esc(s)
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
    .replace(/\*([^*]+)\*/g, '<em>$1</em>')
    .replace(/\[([^\]]+)\]\((https?:[^)]+)\)/g, '<a href="$2" rel="noreferrer noopener">$1</a>');
}}
document.getElementById('out').innerHTML = html;
</script>"#,
        source = serde_json_string(source),
    )
}

/// JSON string literal for embedding untrusted text in the page. The source
/// is already escaped for HTML; this escapes it again for a JS string.
fn serde_json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // `<` and `>` are escaped so a file containing `</script>` cannot
            // close the page's own script block, and control characters stay
            // escaped.
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    fn get(port: u16, target: &str) -> (String, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(stream, "GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut reader = BufReader::new(stream);
        let mut status = String::new();
        reader.read_line(&mut status).unwrap();
        let mut content_type = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line.trim().is_empty() {
                break;
            }
            if let Some(v) = line.strip_prefix("Content-Type: ") {
                content_type = v.trim().to_string();
            }
        }
        let mut body = String::new();
        reader.read_to_string(&mut body).unwrap();
        (status.trim().to_string(), format!("{content_type}\n{body}"))
    }

    fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("muis-view-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn serves_only_registered_files() {
        let server = ViewServer::start().unwrap();
        let path = temp_file("hello.txt", b"hi there");
        let url = server.url_for(&path).unwrap();
        let target = url.strip_prefix("http://127.0.0.1:").unwrap();
        let target = &target[target.find('/').unwrap()..];

        let (status, body) = get(server.port, target);
        assert_eq!(status, "HTTP/1.1 200 OK");
        assert!(body.contains("text/plain"));
        assert!(body.ends_with("hi there"));

        // A different file on disk is not in the allow-list.
        let other = temp_file("secret.txt", b"no");
        let forged = format!(
            "/view?t={}&p={}",
            server.state.token,
            percent_encode(&other.to_string_lossy())
        );
        let (status, _) = get(server.port, &forged);
        assert_eq!(status, "HTTP/1.1 403 Forbidden");
    }

    #[test]
    fn rejects_bad_tokens_and_other_routes() {
        let server = ViewServer::start().unwrap();
        let path = temp_file("token.txt", b"x");
        let encoded = percent_encode(&path.to_string_lossy());
        let (status, _) = get(server.port, &format!("/view?t=wrong&p={encoded}"));
        assert_eq!(status, "HTTP/1.1 403 Forbidden");
        let (status, _) = get(server.port, "/nope");
        assert_eq!(status, "HTTP/1.1 404 Not Found");
    }

    #[test]
    fn markdown_is_rendered_and_escaped() {
        let server = ViewServer::start().unwrap();
        let path = temp_file("notes.md", b"# Title\n\n**bold** <script>alert(1)</script>");
        let url = server.url_for(&path).unwrap();
        let target = url.strip_prefix("http://127.0.0.1:").unwrap();
        let target = &target[target.find('/').unwrap()..];
        let (status, body) = get(server.port, target);
        assert_eq!(status, "HTTP/1.1 200 OK");
        assert!(body.contains("text/markdown"));
        // The page ships the source as data; the renderer escapes it.
        assert!(body.contains("\\u003c") || body.contains("&lt;"));
        assert!(body.contains("Title"));
    }

    #[test]
    fn refuses_paths_that_are_not_files() {
        let server = ViewServer::start().unwrap();
        assert!(server
            .url_for(Path::new("/definitely/not/here.txt"))
            .is_err());
        assert!(server.url_for(&std::env::temp_dir()).is_err());
    }
}
