# muis

muis is the terminal emulator for Veldmuis: Rust backend, xterm.js
frontend, Tauri native window. It ships as the Veldmuis Arch system package
(the default).

The only packaged artifact is the Veldmuis Linux tarball. Windows has no
installer and no packaged binary: it runs from source in browser mode
(`muis-server`, below). Flatpak, AppImage, and AUR builds were removed to
keep the pipeline focused; see `docs/deferred.md` before restoring any of
them.

Chrome follows `../wezterm-web/index.html` (Breeze titlebar, sessions
sidebar, tabbar, statusbar). `design/mock.html` is superseded. VT
emulation is xterm.js, not ours.

## Layout

```text
crates/muis-core/    session model, config, shell probe, UI/worker IPC (no UI deps)
crates/muis-worker/  muis-worker binary: one process per session, pty via portable-pty
crates/muis-notify/  muis-notify binary: send a notification to the running window
crates/muis-shell/   muis binary: Tauri 2 window (WebKitGTK)
crates/muis-server/  muis-server binary: the same UI in a browser (HTTP + WS)
src-ui/              TypeScript + xterm.js frontend (Vite)
tests/fixtures/      shared UI/worker IPC contract both sides test against
docs/                architecture notes, target mockup
design/mock.html     superseded Muis Dark mock (kept for history)
```

## Build

Needs Rust (rustup) and Node. From the repo root:

```sh
cargo build
cargo test --workspace
```

UI:

```sh
cd src-ui
npm install
npm test        # vitest: theme lock, session model, IPC fixture
npm run build   # tsc + vite -> dist/ (embedded by the Tauri shell)
```

Dev window (needs system WebKit):

```sh
cargo run -p muis-shell
```

Browser mode (`muis-server`). The same frontend, served on loopback by a
self-contained binary: it embeds `src-ui/dist` and re-execs itself as the pty
worker, so there is no webview runtime, no sidecar, and nothing to install.

```sh
npm run build --prefix src-ui   # baked into the binary at compile time
cargo run -p muis-server        # prints and opens http://127.0.0.1:<port>/?t=<token>
```

It writes the same config and session files as the native window, so a
session restored in one mode opens in the other. Flags: `--port N`,
`--no-open`, `--testmode`. Set `MUIS_HOME` (or, on Windows, drop a
`muis.portable` file next to the binary) to keep every path in one folder.
Differences from the native window: the folder picker is served by the
backend, notifications use the browser's Notification API, there is no
custom titlebar, and the tab is the window — closing it ends its ptys.

## Windows (from source)

Windows has no installer and no packaged executable. It runs the same UI in a
browser, served by `muis-server`, which you compile on the machine itself:
nothing is installed, nothing needs admin, and there is no WebView2, Visual
Studio, or Windows SDK dependency. A locally built binary also carries no
Mark-of-the-Web, so it starts without a SmartScreen prompt.

### 1. Toolchain (once)

Rust per-user with the GNU host, so no MSVC toolchain is involved:

```powershell
# rustup-init.exe from https://rustup.rs ; per-user, no admin, no UAC
rustup-init.exe --default-host x86_64-pc-windows-gnu -y
rustup component add rust-mingw
rustup target add x86_64-pc-windows-gnu
```

Already have Rust on the MSVC host? Add the GNU toolchain instead of
reinstalling, then prefix `cargo` with `+stable-x86_64-pc-windows-gnu`:

```powershell
rustup toolchain install stable-x86_64-pc-windows-gnu
rustup component add rust-mingw --toolchain stable-x86_64-pc-windows-gnu
```

Either way, open a new terminal afterwards so PATH applies.

### 2. Source (once)

```powershell
git clone https://github.com/bornmanr/muis
cd muis
```

Without git: download the source zip from the repo page and extract it. Source
files carry nothing for a policy to block, and no later step downloads an
executable.

### 3. Frontend bundle (once)

With Node:

```powershell
npm ci --prefix src-ui
npm run build --prefix src-ui
```

Without Node: copy the `src-ui\dist` folder from a machine that has it (it is
gitignored, so a clone never contains it), or point the build at the copy:

```powershell
$env:MUIS_UI_DIST = "D:\muis-dist"
```

### 4. Build

```powershell
cargo build --release -p muis-server --target x86_64-pc-windows-gnu
```

Build the **server only**. A plain `cargo build` also builds the Tauri window,
whose Windows build script needs a resource compiler the GNU toolchain does not
ship; that is the most common failure here, and Windows does not need the Tauri
window.

### 5. Run

```powershell
.\target\x86_64-pc-windows-gnu\release\muis-server.exe
```

It prints `muis serve: http://127.0.0.1:<port>/?t=<token>` and opens your
browser. It binds loopback only, so there is no firewall prompt.

### Keep it in one folder

One directory, one executable, one marker file — no install step:

```powershell
$app = "$env:USERPROFILE\muis"
New-Item -ItemType Directory -Force $app | Out-Null
Copy-Item .\target\x86_64-pc-windows-gnu\release\muis-server.exe $app
New-Item -ItemType File $app\muis.portable
& "$app\muis-server.exe" --port 7800
```

`muis.portable` (or `MUIS_HOME`) keeps config, sessions, and logs in
`$app\data`, so deleting that folder resets muis completely. Without it, state
lives in `%APPDATA%\muis` and logs in `%LOCALAPPDATA%\muis\logs`.

Bookmark `http://127.0.0.1:7800/`: the token is injected on every page load, so
a stale `?t=` in the bookmark still works. For a window without browser chrome:

```powershell
Start-Process msedge "--app=http://127.0.0.1:7800/"
```

| Flag | Effect |
|---|---|
| `--port N` | fixed port (default: the OS picks one, so two copies never clash) |
| `--no-open` | do not launch a browser |
| `--testmode` | fresh sessions, nothing persisted, separate notify socket |
| `--help` | usage |

### Working on it

The UI is embedded at compile time, so a UI change needs both steps and a
server restart:

```powershell
npm run build --prefix src-ui
cargo build --release -p muis-server --target x86_64-pc-windows-gnu
```

`build.rs` watches `src-ui/dist` and re-embeds it, so `cargo run -p muis-server
--target x86_64-pc-windows-gnu` alone picks up new frontend code. Rust-only
changes need nothing else. Browser mode has devtools (F12), unlike the native
window.

### Troubleshooting

| Symptom | Cause and fix |
|---|---|
| Several `error: failed to run custom build command for …` blocks | You built the whole workspace; build `-p muis-server` only |
| `linker 'x86_64-w64-mingw32-gcc' not found` | Missing runtime libs: `rustup component add rust-mingw` |
| `error: failed to find tool. Is 'cl.exe' installed?` | MSVC host: add `--target x86_64-pc-windows-gnu` to the build |
| Page says "built without a frontend bundle" | `src-ui\dist` was missing at compile time: build or copy it, then rebuild |
| Blank page | Wrong port or stale tab: use the URL the server printed, then hard-refresh |
| `cannot listen on …` | Port taken: drop `--port` and let the OS choose |
| Build fails on odd paths / files vanish in `target\` | OneDrive or antivirus: build from a plain folder such as `C:\muis` |
| No desktop toast when an agent finishes | Another `muis-server` already owns the notify pipe; close it |
| Statusbar shows `⎇ —` | Not a git repo, or `git` is not on PATH (not an error) |

### Filing a bug from that machine

`--testmode` reproduces without touching a real layout, and `MUIS_LOG=debug`
raises log verbosity. This collects everything into the clipboard for an issue:

```powershell
& {
  "muis-server issue report"
  "date:  $(Get-Date -Format o)"
  "rust:  $(rustc --version)"
  "cargo: $(cargo --version)"
  "node:  $(node --version 2>$null)"
  "dist:  $(Test-Path .\src-ui\dist\index.html)"
  ""
  Get-Content "$env:LOCALAPPDATA\muis\logs\muis-server.log" -Tail 40 -ErrorAction SilentlyContinue
} | Set-Clipboard
```

With `muis.portable` the log is `<exe folder>\data\state\logs\muis-server.log`.
In the browser, F12 → `copy(JSON.stringify(window.__muisErrors))` dumps the
errors the frontend recorded, and `window.__muisDebug()` snapshots tabs and
sessions.

## Use

- Ctrl+T new tab, × button closes (confirms when a process is busy),
  Ctrl+Shift+F12 focuses the terminal, Ctrl+Shift+F search, Ctrl+, settings.
- Sessions and tabs are restored on the next launch from
  `$XDG_STATE_HOME/muis/sessions.json` (default `~/.local/share/muis`).
  Settings persist in `~/.config/muis/muis.json`. Tabs that were running
  opencode reopen with the saved session; plain shells start clean because
  scrollback is not saved.
- Tabs track `cd` via OSC 7; fish on Veldmuis needs no setup.
- Quitting with busy tabs asks first.
- `muis --testmode` opens a throwaway window: it starts from a fresh
  session and never writes settings, so the real config is left untouched.
  A `test mode` badge marks the window.
- Task notifications: terminal agents (Codex, Claude Code) are detected
  from their OSC 9/777/99 escapes; anything with a hook command can call
  `muis-notify --body "done"` (targets the current tab via `MUIS_TAB_ID`).
  Off-screen tabs badge, and the OS toast fires while muis is unfocused.
  See `docs/archive/notifications.md`.

## Tests

- `cargo test --workspace`: config load/save, shell probe, IPC roundtrips
  + shared fixture, and a live `muis-worker` stdio roundtrip (real pty,
  echo, exit code 42).
- `npm test` in `src-ui`: theme values locked to the target HTML, session
  store behavior, same IPC fixture from the TS side.
- `cargo test -p muis-server`: serves the embedded bundle, answers the RPC
  surface, and drives a real pty over the WebSocket end to end.
- Selenium preview interaction tests (headless Chrome): build the UI,
  start `python3 -m http.server 4173 --directory src-ui/dist`, then run
  `MUIS_URL=http://127.0.0.1:4173 npm test --prefix tests/e2e`.
- Native no-desktop smoke test (real Tauri + WebKit + PTY): install
  Xvfb, xauth, and ImageMagick, then run:

  ```sh
  npm ci --prefix src-ui
  npm run build --prefix src-ui
  cargo build --locked -p muis-shell -p muis-worker
  npm ci --prefix tests/e2e
  npm run test:live:x11 --prefix tests/e2e
  ```

  A debug-only test hook sends a unique command through the UI/worker
  bridge; the shell writes a marker file proving it reached the real PTY,
  and the test writes a screenshot artifact under
  `tests/e2e/artifacts/live-x11/`.
- CI runs all three layers: Rust/PTY, Selenium browser interactions,
  and native Tauri PTY smoke under Xvfb. The Xvfb test uses the same
  terminal-focus shortcut, not monitor coordinates.

## Shell integration (cwd + titles)

Fish on Veldmuis already reports cwd (OSC 7) and titles (OSC 0), so
tabs track `cd` with no setup. Other shells need a prompt hook, e.g.
bash:

```sh
muis_osc7() { printf '\e]7;file://%s%s\e\\' "$HOSTNAME" "$PWD"; }
PROMPT_COMMAND="muis_osc7${PROMPT_COMMAND:+; $PROMPT_COMMAND}"
```
