# muis

muis is the terminal emulator for Veldmuis: Rust backend, xterm.js
frontend, Tauri native window. It ships as the Veldmuis Arch system package
(the default).

The only packaged artifact is the Veldmuis Linux tarball. Windows has no
installer and no packaged binary: it runs from source in browser mode, either
on Node (`npm start`, no build step) or, where policy allows compiled
binaries, via the Rust host (`muis-server`). Flatpak, AppImage, and AUR builds
were removed to keep the pipeline focused; see `docs/deferred.md` before
restoring any of them.

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
node-host/           same browser host in JavaScript + the `npm start` launcher
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

Browser mode. The same frontend, served on loopback: no webview runtime, no
sidecar, nothing installed. Two interchangeable hosts speak one protocol, so
the frontend cannot tell them apart:

```sh
npm start                       # Node host: no compiler, no build step needed
npm run build --prefix src-ui   # Rust host: bundle is embedded at compile time
cargo run -p muis-server        # prints and opens http://127.0.0.1:<port>/?t=<token>
```

It writes the same config and session files as the native window, so a
session restored in one mode opens in the other. Flags: `--port N`,
`--no-open`, `--testmode`. Set `MUIS_HOME` (or, on Windows, drop a
`muis.portable` file next to the binary) to keep every path in one folder.
Differences from the native window: the folder picker is served by the
backend, notifications use the browser's Notification API, there is no
custom titlebar, and the tab is the window — closing it ends its ptys.

## Windows

Windows has no installer and no packaged binary. Two browser hosts cover it,
neither needing admin, WebView2, or the Windows SDK:

| Host | Needs | Use when |
|---|---|---|
| **Node host** (`npm start`) | Node 20+ | always — it starts no binary of its own |
| **Rust host** (`cargo build -p muis-server`) | rustup, GNU host | the machine allows locally built binaries |

Start with the Node host on a managed machine: the only executable it starts is
`node.exe`, which is installed and allowed, while a Rust build has to *start*
the binaries it just produced.

### Node host

```powershell
git clone https://github.com/bornmanr/muis
cd muis
npm start
```

`npm start` (or double-clicking `muis.cmd`) covers the setup that would
otherwise be manual:

- installs the host's dependencies once (`node-host\node_modules`);
- rebuilds `src-ui\dist` only when `src-ui` is newer than the last build;
- serves on <http://127.0.0.1:7800/>, so a bookmark keeps working, and reopens
  the browser instead of starting a second server when it is already up.

Flags pass through: `npm start -- --testmode`, `--no-open`, `--port 0`,
`--root D:\muis-dist`. State lives in `%APPDATA%\muis` and logs in
`%LOCALAPPDATA%\muis\logs`; set `MUIS_HOME`, or drop a `muis.portable` file in
`node-host\`, to keep everything in one deletable folder.

Notifications and tab labels work without shell support: the Codex hook points
at the host (`%USERPROFILE%\.codex\config.toml`):

```toml
notify = ["node", "C:\\Repos\\muis\\node-host\\muis-server.mjs", "--muis-notify", "--body", "done"]
```

Use `node` plus the script path, not `muis-notify.cmd`: Codex starts the hook
itself, without a shell, and Windows cannot start a `.cmd` that way.

### Rust host

For machines that allow compiled binaries. Per-user Rust, GNU host, no MSVC:

```powershell
# rustup-init.exe from https://rustup.rs ; per-user, no admin, no UAC
rustup-init.exe --default-host x86_64-pc-windows-gnu -y
rustup component add rust-mingw
rustup target add x86_64-pc-windows-gnu
```

`rust-mingw` ships the linker, `dlltool`, and the import libraries, but rustc
only finds `dlltool` on PATH, and `windows-sys` needs it (`raw-dylib`):

```powershell
$sysroot = rustc --print sysroot
$env:PATH = "$sysroot\lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained;$env:PATH"
dlltool --version    # expect: GNU dlltool ...
```

Then the bundle and the build — **server only**; a plain `cargo build` also
builds the Tauri window, whose Windows build script needs a resource compiler
the GNU toolchain does not ship:

```powershell
npm run build --prefix src-ui
cargo build --release -p muis-server --target x86_64-pc-windows-gnu
.\target\x86_64-pc-windows-gnu\release\muis-server.exe
```

The bundle is embedded at compile time, so a UI change needs both steps again;
`build.rs` re-embeds automatically once `dist` changes.

### Working on it

- Node host: `npm start` rebuilds the bundle itself; restart it for host
  changes. No build step, and devtools (F12) work.
- Rust host: rebuild with `cargo build -p muis-server` after a UI change.
- Tests: `cargo test --workspace`, `npm test --prefix src-ui`,
  `npm test --prefix node-host`.

### Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `npm start` fails while installing `node-pty` | Node too old, or npm blocked the install: the win32 prebuild is in the package, so `node -e "require('node-pty')"` shows whether it loads |
| Page says "built without a frontend bundle" | Node host: `src-ui\dist` is missing and the build was skipped; Rust host: `dist` was missing at compile time, or `MUIS_UI_DIST` has no `index.html` |
| Blank page | Wrong port or a stale tab: use the URL that was printed, then hard refresh |
| `cannot listen on …` | Port taken: `--port 0` lets the OS choose |
| `muis is already running: …` | Not an error; the launcher reopened the existing instance |
| Several `error: failed to run custom build command for …` blocks | You built the whole workspace; build `-p muis-server` only |
| `linker 'x86_64-w64-mingw32-gcc' not found` | `rustup component add rust-mingw` |
| `error calling dlltool 'dlltool.exe': program not found` | Run the `$env:PATH` line above in this shell |
| `error: failed to find tool. Is 'cl.exe' installed?` | MSVC host: add `--target x86_64-pc-windows-gnu` |
| Build fails on odd paths / files vanish in `target\` | OneDrive or antivirus: build from a plain folder such as `C:\muis` |
| No desktop toast when an agent finishes | Another instance already owns the notify pipe, or the agent's hook is not installed (see above) |
| Statusbar shows `⎇ —` | Not a git repo, or `git` is not on PATH (not an error) |

### If a Rust build says `Access is denied` (`os error 5`)

Windows refuses to *start* what cargo just built. That is machine policy, not
the code: `cargo`, `rustc`, and `node` run because they are installed and known,
while a freshly linked binary is neither. The Node host exists for exactly this
case — `npm start` needs no compiled binary at all.

To learn which rule is in play, run a hello-world in two folders:

```powershell
cargo new --bin "$env:TEMP\muis-probe"
cargo run --release --target x86_64-pc-windows-gnu --manifest-path "$env:TEMP\muis-probe\Cargo.toml"

cargo new --bin "C:\Repos\muis-probe"
cargo run --release --target x86_64-pc-windows-gnu --manifest-path "C:\Repos\muis-probe\Cargo.toml"
```

| Result | Rule | What works |
|---|---|---|
| Both run | transient: a scanner held the file | retry the build |
| `%TEMP%` runs, other folder denied | path-based (AppLocker) | build from an allowlisted folder, or use the Node host |
| Both denied | content-based (Defender ASR, WDAC, EDR) | only an IT exclusion gets past it; otherwise use the Node host |

### Filing a bug from that machine

`npm start -- --testmode` reproduces without touching a real layout, and
`MUIS_LOG=debug` raises verbosity. This collects the useful facts onto the
clipboard:

```powershell
& {
  "muis issue report"
  "date:  $(Get-Date -Format o)"
  "host:  node $(node --version 2>$null) / cargo $(cargo --version 2>$null)"
  "dist:  $(Test-Path .\src-ui\dist\index.html)"
  "node-pty: $(node -e 'require("node-pty");console.log("ok")' 2>$null)"
  ""
  Get-Content "$env:LOCALAPPDATA\muis\logs\muis-server.log" -Tail 40 -ErrorAction SilentlyContinue
} | Set-Clipboard
```

With `MUIS_HOME` or `muis.portable`, the log is under
`<that folder>\state\logs\muis-server.log`. In the browser, F12 →
`copy(JSON.stringify(window.__muisErrors))` dumps the errors the frontend
recorded, and `window.__muisDebug()` snapshots tabs and sessions.

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
- Browser pane: Ctrl+Shift+B (or the split-window button next to the
  gear) docks an HTML preview to the right of the terminal,
  Ctrl+Shift+U jumps to its address bar, and the × closes it (Escape with
  the pane focused also closes it). Drag the left edge to resize. Agents open it with `muis-agent open http://localhost:5173`;
  opencode exposes the same call as the `muis_open_url` tool, so a coding
  turn can show its own dev server or generated page without leaving muis.
  Pages that refuse framing get the ↗ button, which hands the URL to the
  system browser.

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
