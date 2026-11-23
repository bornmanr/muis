# muis vs. the agent-tool field: parity review and decision deck

Status: **for review** (verdicts pending). Date: 2026-11-23.
Scope: muis feature inventory, competitive parity against the agent terminal /
agent console field, and a numbered decision deck to accept, reject or defer.
**Nothing here is approved for build.**

How to read: §2 is what muis is today. §3–§5 are the comparison and the
resulting gap/opportunity list. §6 is the only part that needs a decision —
36 items, each with one line of "what", "who has it", "why it fits", effort,
and a recommendation. §7 is what must stay out.

Effort scale: **S** ≤ 1 day · **M** 1–3 days · **L** 1–2 weeks · **XL** > 2 weeks.

---

## 1. muis' thesis (the yardstick)

From `README.md`, `ROADMAP.md`, `docs/deferred.md`:

- One terminal per tab. Splits are an explicit non-goal.
- The UI holds no PTY state: one `muis-worker` per session.
- Chrome follows the Breeze/Veldmuis target, not a generic terminal theme.
- Optional `veldmuis-muis` package; the app is a terminal, not an agent
  orchestrator.
- Windows is source-only through the browser hosts; no packaged artifact.

Every candidate below is judged against this: **does it make muis a better
terminal for agent work, or does it turn muis into a different product?**

---

## 2. What muis has today

| Area | Present | Source |
|---|---|---|
| Sessions | Folder-based workspaces in the sidebar, per-workspace visible tab, close/remove clamping | `src-ui/src/sessions.ts` |
| Tabs | Tabbar with done/waiting states, dirty-tab close confirm, cwd inheritance for new tabs | `sessions.ts`, `restore.ts` |
| Agent awareness | Busy/recent-output heuristic, done alerts (`unseen` → `seen`), window-title counts | `activity.ts`, `done.ts` |
| Notifications | OSC 9/99/777 + `muis-notify` CLI + command-finished policy; toast only when unfocused; dedupe; per-tab badge | `notify.ts`, `finish.ts`, `crates/muis-notify` |
| Terminal state | OSC 0/2 titles, OSC 7 cwd, OSC 133 last-command bar | `osc.ts`, `commandbar.ts` |
| Search | In-tab find, plus cross-tab search over **open** tabs' scrollback | `search.ts`, `searchall.ts` |
| Chrome | Command palette, settings, statusbar (session/tab, cwd, git branch, user, clock, shell), focus shortcut `Ctrl+Shift+F12` | `main.ts`, `commands.ts` |
| Browser pane | Right-docked http(s)-only preview, toolbar toggle, focus chord, `muis_open_url` tool | `browser.ts`, `crates/muis-shell` |
| Config | JSON `AppConfig`: font size/family, accent theme per workspace, scrollback cap, keybinding overrides, copy-on-select, middle-click paste, cursor, bell, statusbar segments | `config.ts` |
| Restore | Layout restore + opencode session resume, staggered restore pump | `restore.ts`, `agentSessions.ts` |
| Hosts | Tauri (WebKitGTK), browser server (Rust), Node host; shared IPC fixture | `crates/muis-server`, `node-host` |

Deliberately absent: split panes, detach/reattach, PTY persistence, agent
state beyond "done", worktree/branch automation, review UI, remote, mobile,
plugin API, scriptable browser.

---

## 3. The field (as of November 2026)

| Tool | Shape | Standout | Relation to muis |
|---|---|---|---|
| **herdr** | TUI multiplexer + server, agent-aware | Owns the terminals; agent `idle/working/blocked` from screen manifests or self-reports; sidebar rollups; detach; SSH machines; phone app; CLI/socket API; plugins | Direct host-layer competitor; runnable *inside* muis |
| **Hermes IDE** | Desktop console for any agent (BSL 1.1) | Task launcher → worktree+branch; attention inbox; per-turn git history + review desk; land/PR; checks; feature tracks; spend caps | Competes with muis' chrome + workflow layer |
| **Hermes Agent** (Nous) | Personal agent, MIT | Bounded curated memory (`MEMORY.md`/`USER.md`), auto skill creation, FTS5 session search, cron, messaging gateway, MCP, sandbox backends | Competes with **opencode**, not muis |
| **cmux** | Native macOS terminal on libghostty | Vertical tabs with branch/ports/notification text, notification rings, split panes, **scriptable in-app browser**, CLI/socket API, session restore incl. agent sessions, iOS companion | Closest single competitor to muis' concept |
| **Conductor** | Mac desktop, worktree-per-agent | Clean diff review + merge per agent; multiple agents in one workspace | Workflow-layer competitor; Mac-only |
| **Paseo** | Self-hosted daemon + desktop/web/iOS/Android/CLI | Daemon owns agents; clients are disposable; worktrees; plugins on the daemon; relay/Tailscale | Same detach thesis as herdr, broader clients |
| **T3 Code** | MIT desktop/web/mobile shell | Multi-agent surface, worktrees, hosted option | Detach + remote |
| **Vibe Kanban** | Local web kanban | Tasks as cards, agents move them; many CLIs | Orchestration, wrong shape for muis |
| **Claude Squad** | tmux TUI | Worktree per task, zero GUI, SSH-friendly | Proof that tmux+scripts is "good enough" for many |
| **tmux / Ghostty / WezTerm** | Baseline | Persistence, splits, remote, themes, images, shell integration | The floor muis is measured against |

---

## 4. Parity matrix

`✓` present · `~` partial · `–` absent

| Capability | muis | herdr | Hermes IDE | cmux | Conductor | Paseo |
|---|---|---|---|---|---|---|
| Native GUI window | ✓ | – (TUI/app) | ✓ | ✓ | ✓ | ✓ |
| Workspaces + tabs | ✓ | ✓ | ~ (sessions) | ✓ | ✓ | ✓ |
| Split panes | – (policy) | ✓ | ✓ | ✓ | ✓ | ✓ |
| Detach / agents survive close | – | ✓ | ✓ | ✓ | ✓ | ✓ |
| Scrollback restore after restart | – | ✓ | ~ | ✓ | ~ | ✓ |
| Agent state (working/blocked) | ~ done only | ✓ | ✓ | ~ notifications | ~ | ✓ |
| "Needs you" inbox | – | ✓ | ✓ | ~ rings | ~ | ✓ |
| Per-tab agent metadata | – | ✓ | ✓ | ✓ (branch/ports) | ✓ | ✓ |
| Worktree per task | – | – | ✓ | – (byo) | ✓ | ✓ |
| Diff / review UI | – | – | ✓ | – | ✓ | ✓ |
| Land / PR flow | – | – | ✓ | – | ✓ | ✓ |
| Browser pane | ✓ preview | – | – | ✓ scriptable | – | ✓ |
| Browser automation API | – | – | – | ✓ | – | ✓ |
| CLI / socket control | ~ notify + open-url | ✓ | – | ✓ | – | ✓ |
| Remote machines | – | ✓ | ~ (SSH) | ✓ | – | ✓ |
| Mobile client | – | ✓ | – | ✓ | – | ✓ |
| Plugins / extensions | – | ✓ | ~ | ~ skills | – | ✓ |
| Auto-update | – (package mgr) | ✓ | ✓ | ✓ | ✓ | ✓ |
| Memory / self-learning | – (agent's job) | – | – | – | – | – |

Read: muis trails on **persistence**, **agent state**, and **automation** —
and is at parity or ahead on **native chrome**, **browser pane**, and
**packaging for Veldmuis**.

---

## 5. Findings

### 5.1 The three real structural gaps

1. **Nothing survives the window.** Workers are children of the UI
   (`crates/muis-shell/src/bridge.rs` kills on exit). herdr, Paseo, cmux and
   Hermes IDE all separate "owns the process" from "shows the process". This
   is the only item on the list that cannot be faked with a hook.
2. **Agent state is one bit.** muis knows "done". The field knows
   `working / blocked / idle`, and "blocked" is the state that actually
   matters — it is the only one where the user's input is required.
3. **muis cannot be driven.** Every serious tool exposes a CLI or socket
   (herdr `agent wait`, cmux `send input`/`read screen`/`screenshot`,
   Paseo daemon API). muis exposes `muis-notify` and `muis_open_url` only,
   so agents cannot see or steer the terminal they live in.

### 5.2 Cool features worth stealing (by origin)

- **cmux**: vertical tab rows carrying git branch, working directory and
  listening ports; notification *rings*; a browser pane that is
  **scriptable** (navigate, DOM snapshot, click, type, console, network) so
  an agent can verify its own web changes. muis has the pane but not the API.
- **herdr**: state rollups (blocked bubbles to tab → workspace → sidebar);
  `agent explain` (why is this pane "working"?); direct attach to one agent.
- **Hermes IDE**: inbox ordered oldest-first with "jump to next waiting";
  per-turn checkpoints in git; risk flags on lockfiles/CI/secrets; a
  spike-free "land" with undo.
- **Hermes Agent**: bounded memory with hard caps and an approval gate for
  unprompted writes; session search as a first-class tool.
- **Conductor/Paseo**: worktree per task with a setup recipe; multi-client
  attach with a read-only spectator mode.
- **Warp/tmux**: command blocks (select/copy/re-run a command's output),
  OSC 9;4 progress in the tab, broadcast input, quake dropdown.
- **Modern terminals generally**: OSC 9;4, OSC 133 auto-install for
  bash/zsh/fish, kitty graphics, hints (keyboard-select a path/URL/hash).

### 5.3 Things muis already gets right (do not regress)

- One worker per session; UI holds no PTY state.
- Notification anti-corruption layer (OSC + CLI + finish policy, dedupe).
- http(s)-only browser pane; hostile terminal output never reaches the frame.
- Bounded, staggered restore (no thundering herd of agent tabs).
- Copy-on-select and middle-click paste already configurable.

---

## 5.4 Agent strategy (decided during review)

1. The generic seam is mandatory: agent state (D04), control socket (D12),
   MCP tools (D13). Any agent gets notifications and tools.
2. opencode is the **first-class tier**: auto-setup, route tracking, session
   resume, live metadata. Advertised as such, never assumed.
3. **No hard dependency.** `optdepends=('opencode: first-class agent
   integration')` plus an in-app setup action and a tested version range.
   An incompatible version refuses the plugin install and says so in the
   statusbar instead of half-working.
4. Resume is per-agent adapters, not opencode-only: `codex resume`,
   `claude --continue` / `--resume`. This removes most of the
   "half functionality" gap.
5. The UI never invents a state it does not have (no guessed badges for
   hookless agents).

---

## 6. Decision deck (36 items)

Verdict: **pending** → yes / no / defer. Recommendation is advisory only.

### Session and persistence

**D01 · Daemon-owned sessions (detach/reattach)** — herdr, Paseo, cmux, Hermes IDE
Close the window, agents keep running; reopen and reattach to the same PTYs.
Effort **XL** · Risk: touches PTY ownership, the worker pool and both hosts.
Why: the single largest gap; also fixes browser-mode "closing the tab ends its ptys".
Recommendation: **yes**, as its own project phase.

**D02 · `muis attach` + multi-client / read-only spectator** — herdr, Paseo, cmux
Open the same session in a second window or browser tab, and detach by
closing that client. Split in two: **2a** second client sharing a live session
(cheap once D01 exists — same UI, new attach path); **2b** a standalone TTY
client (`muis attach` inside a plain terminal) needs a VT renderer in Rust,
since muis' VT is xterm.js — defer it to when D26 exists.
Effort **2a S–M after D01 / 2b L** · Risk: input arbitration, resize
negotiation (solved by one writable client, others read-only).
Why: agents-as-workers only pays off if you can look from elsewhere; also the
fallback when the GUI is wedged.
Recommendation: **yes to 2a** (one writable + read-only spectators), defer 2b.

**D03 · Scrollback persistence + cross-session search** — cmux, tmux, Warp
Persist scrollback per tab to disk; extend `searchall.ts` to search closed
tabs and past sessions.
Effort **M** · Risk: disk growth, sensitive output on disk.
Why: today search only sees open tabs.
Recommendation: **yes** (with a retention cap and an off switch).

### Agent awareness

**D04 · Generic agent state protocol** — herdr, Hermes IDE, opencode plugins
`muis agent report --state working|blocked|done --session X` (CLI + OSC
escape + JSON on stdin), consumed by one state machine replacing today's
done-only `DoneTracker`.
Effort **S–M** · Risk: low; hooks are already installed for Codex/opencode.
Why: unlocks D05, D07, D09 with one small contract.
Recommendation: **yes** — highest value per line of code.

**D05 · "Needs you" inbox and jump-to-next-waiting** — herdr, Hermes IDE, cmux
One list ordered oldest-first of tabs that are `blocked` or `done`; a chord
jumps to the next one; blocked counts in the window title.
Effort **M** · Risk: low; reuses `DoneTracker` + tab switching.
Recommendation: **yes**.

**D06 · Screen-based blocked detection for hookless agents** — herdr
Read the live bottom buffer per pane, match known permission/question
prompts, fall back to idle.
Effort **L** · Risk: **high** — per-agent manifests, remote-updated rules,
false "idle" that automation could type into.
Why: works for agents that cannot report; herdr itself says self-reports
beat screen scraping, and muis only really needs opencode+claude+codex.
Recommendation: **no** (revisit only if D04 proves insufficient).

**D07 · Notification polish** — cmux (rings), OS taskbar/tray
Attention ring around the tab, click-notification-to-focus-that-tab, KDE
launcher badge for blocked/done counts.
Effort **S–M** · Risk: low.
Recommendation: **yes** (ring + click-to-focus; badge only if cheap).

**D08 · Away push (ntfy / Telegram / webhook)** — herdr, Hermes IDE
One short message when a tab goes `blocked` and the window has been
unfocused for N minutes; never includes prompts or output.
Effort **S** · Risk: secrets in config; must stay opt-in.
Recommendation: **defer** (nice, not load-bearing).

**D09 · Per-tab agent metadata** — herdr, Hermes IDE, cmux
From the same report: agent name, model, branch, working dir, listening
ports, turn count; show in the tab tooltip/statusbar.
Effort **M** · Risk: low; avoid cost estimation (needs transcripts).
Recommendation: **yes** once D04 exists.

### Browser pane

**D10 · Scriptable browser pane (agent eyes)** — cmux
Expose navigate / snapshot DOM / click / type / console / network over the
local socket or an MCP tool so an agent can verify its own web changes in
muis' existing pane.
Effort **L** · Risk: security (the pane becomes a remote-controlled browser);
keep http(s)-only, per-tab origin policy, explicit enable.
Why: this is cmux' most-praised feature and muis already has the pane.
Recommendation: **yes**, gated behind an opt-in setting.

**D11 · Browser pane basics** — cmux, Paseo
URL bar, back/forward/reload, multiple pages in the dock, devtools toggle
(native), remembered URL per workspace.
Effort **M** · Risk: low.
Recommendation: **yes** (URL bar + nav + per-workspace persistence; tabs later).

### Automation

**D12 · CLI + socket control API** — herdr, cmux, Paseo
`muis ls / new-tab / send / capture / screenshot` over a unix socket, plus
the JSON shapes the UI already uses.
Effort **L** · Risk: **high** (a socket that types into terminals is a
privilege boundary); loopback/unix-only, no network bind, caller uid check.
Why: prerequisite for D10, agent-driven workflows, and testing.
Recommendation: **yes**, minimal verbs only.

**D13 · MCP server for muis** — Paseo (daemon MCP), cmux (skills)
Thin MCP wrapper over D12 so opencode/claude can open a preview, read the
screen, or wait for a tab to finish.
Effort **S** (given D12) · Risk: same as D12.
Recommendation: **yes** (after D12).

### Terminal core

**D14 · OSC 9;4 progress + richer status** — Warp, modern terminals
In-tab progress bar for long-running commands and agents; taskbar progress
where the desktop supports it.
Effort **S** · Risk: low.
Recommendation: **yes**.

**D15 · Shell integration installer + command blocks** — Ghostty, Warp, tmux
Ship OSC 133 hooks for bash/zsh/fish (install command + docs) and render
command boundaries, so the last-command bar and block features work
everywhere instead of only with a hand-configured shell.
Effort **M** · Risk: touching users' rc files — must be opt-in and reversible.
Recommendation: **yes**.

**D16 · Command-block UX** — Warp, iTerm2
Select/copy a whole command's output, re-run it, per-command history menu.
Effort **M** (after D15) · Risk: xterm.js buffer bookkeeping.
Recommendation: **defer** (nice; not what the field is judged on).

**D17 · Link detection + keyboard hints** — kitty, iTerm2, WezTerm
Clickable URLs/paths/line-numbers/hashes, plus keyboard-select-and-copy.
Effort **M** · Risk: low.
Why: agent output is full of paths and hashes; this is daily friction.
Recommendation: **yes**.

**D18 · Paste safety** — Warp, modern terminals
Confirm/warn on multi-line or bracketed paste into an agent prompt; never
mangle newlines; optional "paste as one line".
Effort **S** · Risk: low. (Copy-on-select and middle-click paste already exist.)
Recommendation: **yes**.

**D19 · External config + theme packs + live reload** — Ghostty, WezTerm
A documented file (or config dir) for themes/fonts/keybindings that survives
upgrades and can be shared; reload without restart. `AppConfig` already
covers most fields — this exposes them.
Effort **M** · Risk: low.
Recommendation: **yes** (theme packs; live reload only for theme/font).

**D20 · Font ligatures + fallback chains** — Ghostty, kitty, WezTerm
Effort **S–M** · Risk: xterm.js WebGL renderer quirks.
Recommendation: **defer** (cosmetic; Veldmuis default font may not need it).

**D21 · Inline images (kitty graphics / sixel)** — Ghostty, kitty, WezTerm
Effort **L** · Risk: **high** in WebKitGTK + WebGL; big surface, small demand.
Recommendation: **no**.

### Workflow

**D22 · Worktree-per-tab launcher** — Conductor, Hermes IDE, Paseo, Claude Squad
"New tab in a worktree": pick base branch, create `feature/<slug>`, run setup
commands from a repo recipe, open the tab there.
Effort **M** · Risk: git edge cases (dirty tree, existing branch).
Why: the cheapest slice of the Conductor/Hermes value, and it composes with
the sessions model instead of replacing it.
Recommendation: **yes**.

**D23 · Task launcher + auto-named tabs** — Hermes IDE (⌘N), cmux
A prompt-first "new tab": `muis new --agent opencode "task"`, tab named from
the task or from the agent's first message.
Effort **S–M** · Risk: low.
Recommendation: **yes** (as a CLI flag first, UI later).

**D24 · Review / diff pane** — Conductor, Hermes IDE, Paseo
In-app diff and per-turn review for the tab's worktree.
Effort **XL** · Risk: becomes an IDE.
Recommendation: **no** — delegate to `lazygit`/`delta`/`gh` in a tab, or defer
to D22 + the user's own tools.

**D25 · Revisit split panes** — herdr, cmux, every terminal
Effort **L–XL** · Risk: contradicts a locked product decision and the
"one terminal surface visible" regression test.
Recommendation: **no** (keep the decision; herdr/cmux can run inside muis).

**D26 · Remote SSH workspaces** — herdr, cmux, Paseo
Open a workspace on a remote host, either muis-to-muis or attach to remote
tmux.
Effort **XL** · Risk: auth, key handling, latency, and a second persistence
story. Security-sensitive.
Recommendation: **defer** (until D01/D02 exist and local is solid).

**D27 · Mobile / web remote client** — Paseo, cmux iOS, herdr
Effort **XL** · Risk: a whole second product with its own auth and relay.
Recommendation: **no** (browser host already covers "another device on the
LAN"; anything more is a different company).

**D28 · Broadcast input to multiple tabs** — tmux `synchronize-panes`
Effort **S** · Risk: typing into several agents at once — a footgun with no
guard rail.
Recommendation: **no**.

**D29 · Quake / dropdown global hotkey** — Yakuake, Guake, iTerm2 hotkey window
Effort **M** · Risk: KWin/Wayland global shortcuts are compositor-specific;
Tauri global-shortcut support on Wayland is limited.
Recommendation: **defer** (valuable on KDE, but blocked on platform support).

**D30 · Tab management + workspace templates** — cmux, Ghostty, iTerm2
Drag-reorder, rename, pin, per-tab color; workspace templates with startup
commands/env per folder.
Effort **M** · Risk: low.
Recommendation: **yes** (reorder/rename/pin first; templates if cheap).

### Agent-layer (not muis)

**D31 · Memory + self-learning conventions** — Hermes Agent, opencode skills
Not a muis feature: split the global `AGENTS.md` into standing rules plus a
capped `MEMORY.md`/`USER.md` loaded via opencode's `instructions`, add a
`memory` skill that patches them, and a session-end discipline. Optionally
evaluate Hermes Agent on a scratch profile.
Effort **S** (dotfiles/config, outside this repo) · Risk: memory rot.
Recommendation: **yes**, in dotfiles — do not put it in muis.

### Runtime comfort (added on review)

**D32 · Terminal zoom chords** — every GUI terminal (Warp, iTerm2, GNOME
Terminal)
`Ctrl+=` / `Ctrl+-` / `Ctrl+0` resize the terminal text live (xterm
`fontSize` + `fit()`), persisted to `AppConfig.fontSize`; optional
`Ctrl+Shift+=` for whole-window scale (chrome + terminal) when reading an
agent TUI from across the room.
Effort **S** · Risk: low (xterm re-fit and WebGL atlas rebuild).
Why: `fontSize` already exists in config but has no runtime chord, and
zooming a TUI agent means zooming the terminal.
Recommendation: **yes**.

### Rendering and output (added on review)

**D33 · Viewer pane ("show file")** — cmux browser pane, Warp markdown
blocks, macOS Quick Look
Generalize the right dock from "web preview" into a local viewer:
`muis view <path>` renders images, PDFs, markdown, HTML, diffs and JSON;
`muis view --capture` renders a captured command block. Same pane, same D12
verbs, no VT changes and no inline-image protocols.
Effort **M** · Risk: low (WebKit already renders all of it; keep the path
scope to the workspace by default and never fetch remote content for a local
file).
Why: this covers 80% of what people want inline images for, plus the things
inline images cannot do (markdown, PDFs, diffs).
Recommendation: **yes**.

**D34 · Rendered markdown for agent output** — Warp, Hermes IDE agent view
Problem: agent messages arrive as raw text, so `**bold**` and `#` headers show
literally (observed in this review with opencode's TUI).
Two shapes: (a) nothing in muis — the agent's own UI should render its
markdown, and this is an opencode renderer issue; (b) an explicit
"render this block" action that sends a captured block through D33.
Effort **M** for (b) · Risk: guessing which text is markdown; must stay
explicit and never automatic.
Recommendation: **defer** unless (b) falls out of D33 for free.

### Clients and hosts (added on review)

**D35 · Browser client parity + app⇄tab handoff** — the Windows path, muis-server
Today the browser host differs in four ways: no custom titlebar, backend folder
picker, browser notifications, and the tab *is* the window. Make it a
first-class **secondary mode on Linux**: full chrome (hide in-page window
controls where a real frame exists, keep the Breeze titlebar), clipboard and
drag-drop parity, browser-aware chords (`Ctrl+W` must detach, never kill
sessions), a PWA manifest so it installs as a real window, and a one-click
handoff — "Open in browser" in the native titlebar serves loopback with a
one-time token and attaches the same session; "Open in app" returns.
Depends on D01 for the handoff (same session, two clients); before that it can
only hand off layout, not live PTYs.
Effort **M–L** · Risk: browser-reserved chords, clipboard permissions, and one
more parity surface to keep honest — mitigated by the existing host-parity
tests and the Selenium e2e suite.
Why: it makes Windows genuine parity (same client, no WebView), gives a LAN
second screen with no relay or SSH, and is the cheapest end-to-end proof that
the daemon model works.

### Orchestration (parked with Snor)

**D36 · Snor sends to other tabs** — parked with the Snor engine
An orchestrator (Snor) tells another tab to do something: "also run the
Veldmuis task in the veldmuis tab". The transport is D12's `send`, so the work
is policy: address by tab/workspace/agent name, mark the message in the target
tab as coming from Snor, require a delivery receipt (`capture` after send),
rate-limit, and confirm when the target tab is `blocked` (typing into an
approval prompt is the same footgun as D18).
Effort **M** after D12 · Risk: medium — one agent typing into another.
Recommendation: **defer** until the Snor engine exists.

---

## 7. Non-goals (would change what muis is)

- Split panes per tab (locked decision; the "exactly one terminal surface"
  regression test depends on it).
- Kanban boards, task CRMs, feature tracks, spend caps.
- Per-turn git checkpoints and an in-app review desk (Hermes IDE's core).
- Built-in agent: muis runs agents, it is not one.
- Cloud relay, accounts, telemetry, hosted anything.
- Plugin marketplace.

---

## 8. Agent-layer notes (for the record, not muis scope)

- **Memory**: Hermes' win is not storage (2.2k + 1.4k chars of markdown) but
  the loop — bounded caps, forced consolidation, dedupe, injection scanning,
  and an approval gate for unprompted writes. ~80% is reproducible in
  opencode with `instructions` + a capped `memory` skill.
- **Self-learning skills**: both use `SKILL.md` (`agentskills.io`), and
  opencode already reads `~/.agents/skills` — skills are portable, no lock-in.
- **Web**: opencode has `webfetch`/`websearch`; browser control comes from
  Playwright or chrome-devtools MCP. Parity with Hermes' bundled browser.
- **Desktop automation**: MCP either way (`computer-use-linux`: AT-SPI tree,
  Wayland/X11 input, compositor targeting). Hard on KDE Wayland regardless of
  agent; keep approvals on.

---

## 9. Review protocol

1. Walk the deck in order, one item at a time; each gets **yes / no / defer**.
2. A deferred item needs a trigger (e.g. "after D01") or it is a no.
3. Accepted items get a phase, not a date. Nothing is built from this doc
   until the deck is closed.
4. Verdicts are written back into §6 in the same PR.

Planned phases if the recommendations stand:

- **P1 (agent awareness, ~1 week):** D04 → D05 → D07 → D09 → D14 → D17 → D18
  → D32.
- **P2 (persistence, its own project):** D01 → D02 → D03.
- **P3 (control surface):** D12 → D10 → D13.
- **P4 (workflow niceties):** D22 → D23 → D30 → D15 → D19.
- **Never:** D06, D21, D24, D25, D27, D28.

---

## 10. Verdict ledger (filled during review)

| Item | Verdict | Constraints / notes |
|---|---|---|
| D01 | **yes** | PTY-centric, not agent-centric. Idle reaper with configurable timer; park idle/done tabs instead of keeping processes; byte-capped daemon ring buffer (not lines); parent-death signal on workers; start-up sweep by pid+start-time; `daemon status/stop`; one instance per `MUIS_HOME`. Must not assume "one agent process per tab" (future APC layer). Park only a shell at prompt or a known agent reporting done/idle; never park on recent input; `blocked` exempt by default; new `parked` tab state distinct from done/exited; resume resolver has both `resume` and `attach` verbs; agent state keyed by (tab, session id, state), never PID. |
| D02 | **yes (2a only)** | Second client = another muis window or browser tab sharing a live session; one writable, others read-only. Standalone TTY client (2b) deferred until remote exists. |
| D03 | **yes** | Byte caps, retention, per-workspace excludes, `--no-history` per tab, `muis history clear`. Plain text/JSONL so `rg` works without muis. |
| D04 | **yes** | CLI + injected env (`MUIS_SOCKET`, `MUIS_TAB_ID`) + OSC fallback; one state machine replaces `DoneTracker`; expiring `working`; no double toast with `finish.ts`. |
| D05 | **yes** | Own overlay; blocked first then oldest-first; jump-to-next chord; Enter clears `seen`. |
| D06 | **no** | No screen scraping. Revisit only if an agent we actually run cannot self-report. |
| D07 | **yes** | Attention ring + click-to-focus through `org.freedesktop.Notifications`; taskbar badge folded into D14. |
| D08 | **defer** | Trigger: opencode mobile stops covering away, or truly unattended runs. No self-hosted relay/SSH upkeep. |
| D09 | **yes** | Agent, model, branch, cwd, ports, state in tooltip + one compact strip. Ephemeral, never persisted. No cost estimates. |
| D10 | **yes (always available)** | No per-workspace toggle. Must keep: disposable profile by default, explicit opt-in "use my signed-in session" (marked in the pane), no cookie import, no auto-navigate to URLs from terminal output. Browser mode cannot isolate (iframe) → automation off there by default. |
| D11 | **yes (full)** | URL bar, nav, external-open, devtools/new-tab, remembered URL per workspace. |
| D12 | **yes (core + browser)** | JSON over `$XDG_RUNTIME_DIR` socket, 0600, peer-uid check, never TCP; reuses the 8 IPC frames; per-verb policy + audit log for `send`. |
| D13 | **yes (MCP + plugins)** | `muis mcp` over stdio wraps the D12 socket and is declared in both opencode and codex configs; the opencode server/TUI plugins stay for hooks, route tracking and session-id resolution. |
| D14 | **yes (tab + taskbar)** | Parse OSC 9;4 (clear/normal/error/indeterminate/paused); thin progress line in the tab; KDE launcher progress; same strip carries the state word. |
| D15 | **yes (installer)** | Idempotent marked snippets; fish `conf.d`, zsh/bash sourced file; `--uninstall`; tested against starship/oh-my-posh. |
| D16 | **defer** | Trigger: D15 shipped and the last-command bar proves insufficient. |
| D17 | **yes (links + hints)** | Conservative matcher; ctrl-click open, ctrl+shift-click copy; paths never executed; hints chord. |
| D18 | **yes (all three)** | Multi-line confirm when not in bracketed mode, paste-as-one-line, warn on paste while a blocked prompt is focused. |
| D19 | **yes (full)** | Documented stable keys, theme pack directory, live reload for theme/font only. |
| D20 | **yes (fallback only)** | Expose/document the font fallback stack. No ligature work (xterm.js has no native support). |
| D21 | **no** | No image protocols in the grid. Covered instead by D33. |
| D33 | **yes (viewer)** | `muis view <path>` + `--capture`; images, PDFs, markdown, HTML, diffs, JSON in the existing dock; workspace-scoped paths, no remote fetch for local files. |
| D34 | **defer** | Rendering agent markdown belongs to the agent's client (opencode TUI). Revisit only if a captured-block render falls out of D33 for free. |
| D22 | **yes** | UI action + `muis new --worktree`; defaults to `../.worktrees/<repo>-<slug>` and neutral `feature/` `fix/` `chore/` prefixes; setup recipe; refuse on dirty tree / existing branch instead of guessing. |
| D23 | **yes** | CLI + small input in the new-tab action; task text passed as argv, never interpolated into a shell string; per-agent prompt adapters. |
| D24 | **no** | No in-app diff/review. Delegate to lazygit/delta/gh in the tab; D33 renders patches. |

---

## 11. Open investigations (not muis scope)

- **Why bold/headings show as literal `**` in opencode.** Triage in order:
  same text in konsole with plain `opencode` (client vs terminal), then
  `NO_COLOR` / `TERM`, then which client is reading it (TUI vs mobile/web —
  separate renderers). Do not build a markdown renderer in muis to compensate;
  the muis-side answer is D33/D34.
| D25 | **no** | One terminal per tab stays. Side-by-side is covered by tabs, D02's second window and the D33 dock; herdr/cmux can run inside muis. |
| D26 | **no** | Local-only. Browser host covers LAN; opencode mobile covers away. No self-hosted relay or SSH upkeep. |
| D27 | **no** | D35's browser client covers the phone on the LAN; no separate app, no relay, no accounts. |
| D28 | **no** | No broadcast input. Scripted per-tab `send` (D12) covers legitimate cases; Snor orchestration is D36. |
