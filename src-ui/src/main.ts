import { Terminal, type ILinkHandler } from "xterm";
import "xterm/css/xterm.css";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { WebglAddon } from "@xterm/addon-webgl";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { SessionStore, newTabCwd, type Tab, type Workspace } from "./sessions";
import { xtermTheme, colorFor, sessionColorMap, applyTheme, themeNames, MUIS_THEME } from "./theme";
import { defaultConfig, configFromJSON, clampScrollback, bellAction, effectiveFontSize, segmentVisible, themeForWorkspace, type AppConfig } from "./config";
import { clampIndex, filterCommands, type Command } from "./commands";
import { QueryHistory } from "./history";
import { WorkerClient, type Transport } from "./worker";
import { SearchController } from "./search";
import { SidePanelRegistry } from "./panels";
import { newActivityState, isTabBusy, anyTabBusy, forgetTab } from "./activity";
import { OscParser, type NotifyEvent } from "./osc";
import { CommandTracker, tabLabel as tabLabelOf } from "./commandbar";
import { shouldToastOnFinish } from "./finish";
import { collectMatches, type SearchHit, type SearchScope, type SearchableTab } from "./searchall";
import { resolveShortcut } from "./shortcuts";
import { DoneTracker, sidebarAttention, tabAttentionLabel, type Attention } from "./done";
import { NotifyRouter, notifyEventFromCli, notifyEventFromCommand, type CliNotify } from "./notify";
import {
  isInjectedResume,
  tabForReport,
  applyAgentReport,
  isOpencodeTitle,
  opencodeResume,
  opencodeTitleFromLabel,
  resolveRestoreSession,
  restoreQueue,
  shouldClearAgentOnCommandEnd,
  type AgentSessionInfo,
  type RestoreEntry,
} from "./agentSessions";
import { RestorePump } from "./restore";
import { expandHome, shortPath as shortPathOf } from "./paths";
import { clockText } from "./clock";
import { shouldFocusTerminal } from "./focus";
import { clampPreviewWidth, defaultPreviewWidth, normalizeUrl } from "./browser";
import muisIcon from "./assets/muis.png";
import type { UiToWorker, WorkerToUi } from "./ipc";
import {
  IS_SERVE,
  WsTransport,
  notifyDesktop,
  onServeEvent,
  pickDirectoryServe,
  rpc,
} from "./serve";
import "./style.css";

/**
 * muis frontend — a live mirror of ../wezterm-web/index.html.
 *
 * Chrome uses the mock's exact elements/classes (.titlebar, .sidebar,
 * .session, .tabbar, .tab, .statusbar, .search). Each tab owns one
 * xterm.js surface backed by a pty in the session's muis-worker.
 * Outside Tauri (plain browser dev) tabs echo.
 */

const IN_TAURI = "__TAURI_INTERNALS__" in window;

/**
 * Whether a real backend is present: the Tauri window and muis-server both
 * own ptys, only the transport differs (`shellCall` vs HTTP + WebSocket).
 */
const BACKEND = IN_TAURI || IS_SERVE;

/** Run one shell command on whichever backend is present. */
function shellCall<T>(name: string, args: Record<string, unknown> = {}): Promise<T> {
  return IS_SERVE ? rpc<T>(name, args) : invoke<T>(name, args);
}

/** Set once at init from the shell; true for `muis --testmode`. */
let testModeActive = false;

function reportDebugStage(stage: string, detail: unknown = undefined): void {
  if (!BACKEND) return;
  void shellCall("debug_report", {
    json: JSON.stringify({ stage, detail }),
  }).catch(() => {});
}

if (BACKEND) {
  reportDebugStage("frontend-module-loaded");
  window.addEventListener("error", (event) => {
    reportDebugStage("frontend-error", String(event.message));
  });
  window.addEventListener("unhandledrejection", (event) => {
    reportDebugStage("frontend-unhandled-rejection", String(event.reason));
  });
}

/** Whether the muis window has keyboard focus; gates desktop notifications. */
let windowFocused = !IN_TAURI ? document.hasFocus() : true;
if (IN_TAURI) {
  const win = getCurrentWindow();
  void win
    .isFocused()
    .then((f) => {
      windowFocused = f;
    })
    .catch(() => {});
  void win
    .onFocusChanged(({ payload }) => {
      windowFocused = payload;
    })
    .catch(() => {});
} else {
  window.addEventListener("focus", () => {
    windowFocused = true;
  });
  window.addEventListener("blur", () => {
    windowFocused = false;
  });
}

/** Best-effort OS toast; no-op outside Tauri or without permission. */
async function sendDesktopNotification(title: string, body: string): Promise<void> {
  if (IS_SERVE) {
    notifyDesktop(title, body);
    return;
  }
  if (!IN_TAURI) return;
  try {
    const { isPermissionGranted, requestPermission, sendNotification } = await import(
      "@tauri-apps/plugin-notification"
    );
    let granted = await isPermissionGranted();
    if (!granted) granted = (await requestPermission()) === "granted";
    if (granted) sendNotification({ title, body });
  } catch {
    /* notifications are best-effort */
  }
}

class TauriTransport implements Transport {
  private handler: ((sessionId: string, frame: WorkerToUi) => void) | null = null;

  constructor() {
    // A rejected listen() means worker output can never arrive — record
    // it where debug_report() will pick it up (no devtools in release).
    void listen<{ session_id: string; frame: WorkerToUi }>("muis-worker-event", (e) => {
      this.handler?.(e.payload.session_id, e.payload.frame);
    }).catch((e: unknown) => {
      const w = window as unknown as { __muisErrors?: string[] };
      w.__muisErrors ??= [];
      w.__muisErrors.push(`event-listen failed: ${String(e)}`);
    });
  }

  async spawn(sessionId: string): Promise<void> {
    await invoke("worker_spawn", { sessionId });
  }

  async send(sessionId: string, msg: UiToWorker): Promise<void> {
    await invoke("worker_send", { sessionId, line: JSON.stringify(msg) });
  }

  async stop(sessionId: string): Promise<void> {
    await invoke("worker_stop", { sessionId });
  }

  onEvent(cb: (sessionId: string, frame: WorkerToUi) => void): void {
    this.handler = cb;
  }
}

class EchoTransport implements Transport {
  private handler: ((sessionId: string, frame: WorkerToUi) => void) | null = null;

  async spawn(): Promise<void> {}
  async stop(): Promise<void> {}
  onEvent(cb: (sessionId: string, frame: WorkerToUi) => void): void {
    this.handler = cb;
  }
  async send(sessionId: string, msg: UiToWorker): Promise<void> {
    if (msg.type === "write" && this.handler) {
      this.handler(sessionId, { type: "output", pty_id: msg.pty_id, data_b64: msg.data_b64 });
    }
  }
}

let store = new SessionStore();
const client = new WorkerClient(
  IN_TAURI ? new TauriTransport() : IS_SERVE ? new WsTransport() : new EchoTransport(),
);

/** A `muis-agent-activity` report routed from the shell. */
interface CliAgentActivity {
  kind?: string;
  state?: string;
  tab_id?: string | null;
  session?: string | null;
}

/** A `muis-agent session` report routed from the shell. */
interface CliAgentSession {
  kind?: string;
  session?: string | null;
  directory?: string | null;
  tab_id?: string | null;
}

/** A `muis-agent open` request forwarded by the shell. */
interface CliOpenUrl {
  kind?: string;
  url: string;
  tab_id?: string | null;
  session?: string | null;
}

/** Tabs whose restore launch already ran this session. */
const agentLaunched = new Set<string>();
/** Resume line injected per tab, so the shell's cmd-start for it is ignored. */
const restoreCommands = new Map<string, string>();
/** opencode's session list is per directory; fetch once per launch. */
const opencodeSessionsCache = new Map<string, Promise<AgentSessionInfo[]>>();
let saveTimer: number | undefined;
let lastSaved = "";
/** Tabs that have produced any pty output, and callbacks waiting for it. */
const sawOutput = new Set<string>();
const outputWaiters = new Map<string, () => void>();

function scheduleSave(): void {
  if (!BACKEND || testModeActive) return;
  window.clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => {
    void saveNow();
  }, 400);
}

async function saveNow(): Promise<void> {
  if (!BACKEND || testModeActive) return;
  const json = store.toJSON();
  if (json === lastSaved) return;
  lastSaved = json;
  try {
    await shellCall("sessions_save", { json });
  } catch {
    /* best effort: a lost save must never break the terminal */
  }
}

// `muis-notify` clients reach the shell over a local socket; the shell
// forwards each request as an event. Browser mode gets the same events over
// the page's WebSocket, so only the subscription differs.
function subscribe<T>(event: string, handler: (payload: T) => void): void {
  if (IS_SERVE) {
    onServeEvent(event, (payload) => handler(payload as T));
    return;
  }
  if (!IN_TAURI) return;
  void listen<T>(event, (e) => handler(e.payload)).catch((e: unknown) => {
    const w = window as unknown as { __muisErrors?: string[] };
    w.__muisErrors ??= [];
    w.__muisErrors.push(`${event}-listen failed: ${String(e)}`);
  });
}

subscribe<CliNotify>("muis-notify", handleNotifyRequest);
subscribe<CliAgentSession>("muis-agent-session", handleAgentSession);
subscribe<CliAgentActivity>("muis-agent-activity", handleAgentActivity);
subscribe<CliOpenUrl>("muis-open-url", handleOpenUrl);

/** View options; loaded from disk in init, edited in the settings dialog. */
let cfg: AppConfig = defaultConfig();

/** Pty liveness for dirty-tab / quit confirmations. */
const activity = newActivityState();

/** One OSC observer per tab (sequences split across frames reassemble). */
const oscParsers = new Map<string, OscParser>();

/**
 * Reserved extension slot (Snor assistant and future panels). Panels
 * dock right of the terminal; nothing is registered yet, so the slot
 * stays invisible until panel code lands.
 */
export const sidePanels = new SidePanelRegistry();

let shellCache: string | null = null;
let liveTestCommandCache: Promise<string | null> | null = null;
const liveTestCommandSent = new Set<string>();

/** Debug-build-only PTY input requested by the native Xvfb harness. */
function liveTestCommand(): Promise<string | null> {
  if (!IN_TAURI) return Promise.resolve(null);
  liveTestCommandCache ??= invoke<string | null>("live_test_command").catch(() => null);
  return liveTestCommandCache;
}

async function defaultShell(): Promise<string> {
  if (shellCache) return shellCache;
  try {
    shellCache = await shellCall<string>("default_shell");
  } catch {
    shellCache = "/bin/sh";
  }
  return shellCache;
}

function shellBaseName(): string {
  const shell = shellCache ?? "";
  const parts = shell.replace(/\\/g, "/").split("/");
  return parts[parts.length - 1] || "shell";
}

interface TabView {
  term: Terminal;
  fit: FitAddon;
  search: SearchAddon;
  box: HTMLElement;
  /** Refits when the surface resizes (layout settling, window, sidebar). */
  ro?: ResizeObserver;
}

const views = new Map<string, TabView>();

/** Scrollback search follows the visible tab. */
function activeSearch() {
  const ws = store.currentWorkspace();
  const active = ws?.tabs[ws.active];
  const view = active ? views.get(active.id) : undefined;
  return view?.search;
}
const searcher = new SearchController(activeSearch);

/** Real home directory, set once from the backend (`default_cwd`). */
let homeDir = "";

function shortPath(p: string): string {
  return shortPathOf(p, homeDir);
}

const enc = new TextEncoder();

/* ---------------- static skeleton (mock .window) ---------------- */

const app = document.getElementById("app")!;

const titlebar = document.createElement("div");
titlebar.className = "titlebar";
const tbLeft = document.createElement("div");
tbLeft.className = "tb-left";
if (IN_TAURI) tbLeft.setAttribute("data-tauri-drag-region", "");
const appIcon = document.createElement("div");
appIcon.className = "app-icon";
const appIconImg = document.createElement("img");
appIconImg.src = muisIcon;
appIconImg.alt = "";
appIcon.append(appIconImg);
const appTitle = document.createElement("div");
appTitle.className = "app-title";
appTitle.textContent = "muis";
const testBadge = el("span", "test-badge", "test mode");
testBadge.style.display = "none";
tbLeft.append(appIcon, appTitle, testBadge);

const tbRight = document.createElement("div");
tbRight.className = "tb-right";
const searchBox = document.createElement("div");
searchBox.className = "search";
const searchGlyph = document.createElement("span");
searchGlyph.className = "search-glyph";
searchGlyph.textContent = "⌕";
const titleSearch = document.createElement("input");
titleSearch.id = "titleSearch";
titleSearch.placeholder = "search";
titleSearch.autocomplete = "off";
titleSearch.spellcheck = false;
const searchHint = document.createElement("span");
searchHint.className = "kbd";
searchHint.textContent = "ctrl shift f";
const searchResults = document.createElement("div");
searchResults.id = "searchResults";
searchResults.className = "results";
searchResults.style.display = "none";
const caseBtn = document.createElement("button");
caseBtn.className = "search-toggle";
caseBtn.textContent = "Aa";
caseBtn.title = "Match case";
const regexBtn = document.createElement("button");
regexBtn.className = "search-toggle";
regexBtn.textContent = ".*";
regexBtn.title = "Regular expression";
searchBox.append(searchGlyph, titleSearch, caseBtn, regexBtn, searchHint, searchResults);
let searchCase = false;
let searchRegex = false;
const searchHistory = new QueryHistory();
let historyIndex = -1;
caseBtn.addEventListener("click", () => {
  searchCase = !searchCase;
  caseBtn.classList.toggle("on", searchCase);
  renderSearchResults();
});
regexBtn.addEventListener("click", () => {
  searchRegex = !searchRegex;
  regexBtn.classList.toggle("on", searchRegex);
  renderSearchResults();
});
function winButton(kind: "min" | "max" | "close"): HTMLDivElement {
  const btn = document.createElement("div");
  btn.className = kind === "close" ? "win-btn close" : "win-btn";
  const icon = document.createElement("span");
  icon.className = "win-btn-icon";
  const path =
    kind === "min"
      ? "M4 6l4 4 4-4"
      : kind === "max"
        ? "M4 10l4-4 4 4"
        : "M4.5 4.5l7 7M11.5 4.5l-7 7";
  icon.innerHTML = `<svg viewBox="0 0 16 16" aria-hidden="true"><path d="${path}"/></svg>`;
  btn.append(icon);
  return btn;
}
const winControls = document.createElement("div");
winControls.className = "win-controls";
const winMin = winButton("min");
winMin.title = "Minimize";
const winMax = winButton("max");
winMax.title = "Maximize / restore";
const winClose = winButton("close");
winClose.title = "Close";
winControls.append(winMin, winMax, winClose);
tbRight.append(searchBox, winControls);
titlebar.append(tbLeft, tbRight);

const mainRow = document.createElement("div");
mainRow.className = "main";
const sidebar = document.createElement("div");
sidebar.className = "sidebar";
const center = document.createElement("div");
center.className = "center";
const tabbar = document.createElement("div");
tabbar.className = "tabbar";
tabbar.id = "tabbar";
const tabScroller = document.createElement("div");
tabScroller.className = "tab-scroll";
const newTabBtn = el("button", "newtab", "+");
newTabBtn.title = "New terminal";
newTabBtn.addEventListener("click", () => {
  const ws = store.currentWorkspace();
  if (ws) addShellTab(ws);
});
const gearBtn = el("button", "newtab settings", "⚙") as HTMLButtonElement;
gearBtn.title = "Settings (ctrl + ,)";
gearBtn.addEventListener("click", () => openSettings());
const paneBtn = document.createElement("button");
paneBtn.className = "newtab pane-toggle";
paneBtn.title = "Browser pane (ctrl+shift+b)";
paneBtn.setAttribute("aria-label", "Browser pane");
paneBtn.append(paneIcon());
paneBtn.addEventListener("click", () => togglePreview());
tabbar.append(newTabBtn, tabScroller, paneBtn, gearBtn);
const termWrap = document.createElement("div");
termWrap.className = "term-wrap";

const statusbar = document.createElement("div");
statusbar.className = "statusbar";
center.append(tabbar, termWrap, statusbar);

// Browser pane: docked to the right of the terminal, empty until a URL is
// opened by the user or by an agent (`muis-agent open`). It is a plain
// iframe, so the pane works identically in the native shell and in
// `muis serve`; sites that refuse framing get the "open externally" button.
const previewSlot = document.createElement("div");
previewSlot.className = "preview";
previewSlot.style.display = "none";
const previewGrip = el("div", "preview-grip");
previewGrip.title = "Drag to resize, double-click to reset";
const previewHead = document.createElement("div");
previewHead.className = "preview-head";
const previewReload = el("button", "preview-btn", "↻") as HTMLButtonElement;
previewReload.title = "Reload";
const previewUrlInput = document.createElement("input");
previewUrlInput.className = "preview-url";
previewUrlInput.type = "text";
previewUrlInput.spellcheck = false;
previewUrlInput.placeholder = "http://localhost:5173";
previewUrlInput.setAttribute("aria-label", "Browser URL");
const previewExternal = el("button", "preview-btn", "↗") as HTMLButtonElement;
previewExternal.title = "Open in the system browser";
const previewClose = el("button", "preview-btn", "×") as HTMLButtonElement;
previewClose.title = "Close browser pane (ctrl+shift+b)";
previewHead.append(previewReload, previewUrlInput, previewExternal, previewClose);
const previewBody = document.createElement("div");
previewBody.className = "preview-body";
const previewFrame = document.createElement("iframe");
previewFrame.className = "preview-frame";
previewFrame.title = "Browser preview";
previewFrame.setAttribute(
  "sandbox",
  "allow-scripts allow-same-origin allow-forms allow-popups allow-modals allow-downloads",
);
previewFrame.setAttribute("referrerpolicy", "no-referrer");
const previewEmpty = el("div", "preview-empty");
previewEmpty.textContent =
  "No page yet. Type a URL above, or let an agent open one with muis_open_url.";
previewBody.append(previewEmpty, previewFrame);
previewSlot.append(previewGrip, previewHead, previewBody);
mainRow.append(sidebar, center, previewSlot);

const panelSlot = document.createElement("div");
panelSlot.className = "sidepanel";
panelSlot.style.display = "none";

app.append(titlebar, mainRow, panelSlot);

/* ---------------- titlebar: search + window controls ---------------- */

titleSearch.addEventListener("input", () => {
  historyIndex = -1;
  renderSearchResults();
});
titleSearch.addEventListener("keydown", (e) => {
  e.stopPropagation();
  if (e.key === "Enter") {
    e.preventDefault();
    const first = searchResults.querySelector<HTMLElement>(".r-item");
    if (first) first.click();
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    const list = searchHistory.list();
    if (list.length) {
      historyIndex = Math.min(historyIndex + 1, list.length - 1);
      titleSearch.value = list[historyIndex];
      renderSearchResults();
    }
  } else if (e.key === "ArrowDown") {
    e.preventDefault();
    const list = searchHistory.list();
    if (historyIndex > 0) {
      historyIndex -= 1;
      titleSearch.value = list[historyIndex];
    } else {
      historyIndex = -1;
      titleSearch.value = "";
    }
    renderSearchResults();
  } else if (e.key === "Escape") {
    e.preventDefault();
    titleSearch.value = "";
    hideSearchResults();
    titleSearch.blur();
  }
});

async function winControl(action: "min" | "max" | "close"): Promise<void> {
  if (!IN_TAURI) return;
  try {
    const win = getCurrentWindow();
    if (action === "min") await win.minimize();
    else if (action === "max") await win.toggleMaximize();
    else await win.close();
  } catch {
    /* capability missing — controls stay decorative */
  }
}
winMin.addEventListener("click", () => void winControl("min"));
winMax.addEventListener("click", () => void winControl("max"));
winClose.addEventListener("click", () => void winControl("close"));

/* ---------------- frameless resize handles ---------------- */

type ResizeEdgeDirection =
  | "East"
  | "North"
  | "NorthEast"
  | "NorthWest"
  | "South"
  | "SouthEast"
  | "SouthWest"
  | "West";

const RESIZE_EDGES: ReadonlyArray<[string, ResizeEdgeDirection]> = [
  ["n", "North"],
  ["s", "South"],
  ["e", "East"],
  ["w", "West"],
  ["ne", "NorthEast"],
  ["nw", "NorthWest"],
  ["se", "SouthEast"],
  ["sw", "SouthWest"],
];

if (IN_TAURI) {
  const layer = document.createElement("div");
  layer.className = "resize-layer";
  for (const [edge, dir] of RESIZE_EDGES) {
    const handle = document.createElement("div");
    handle.className = `resize-edge resize-${edge}`;
    handle.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      void getCurrentWindow().startResizeDragging(dir).catch(() => {});
    });
    layer.append(handle);
  }
  document.body.append(layer);
}

/* ---------------- terminals ---------------- */

/** Size the shown surface once layout settles, retrying a few frames. */
function fitShown(sessionId: string, tab: Tab, view: TabView): void {
  const w = window as unknown as { __muisErrors?: string[] };
  let tries = 0;
  const attempt = () => {
    try {
      const dims = view.fit.proposeDimensions();
      if (dims && dims.cols >= 2 && dims.rows >= 2) {
        if (dims.cols !== view.term.cols || dims.rows !== view.term.rows) {
          view.term.resize(dims.cols, dims.rows);
        }
        if (BACKEND) {
          void client.resize(sessionId, tab.id, view.term.cols, view.term.rows).catch(() => {});
        }
        return;
      }
    } catch (e) {
      w.__muisErrors?.push(`fit failed: ${String(e)}`);
      return;
    }
    if (++tries < 30) {
      requestAnimationFrame(attempt);
    } else {
      w.__muisErrors?.push("fit gave up: surface never measurable");
    }
  };
  requestAnimationFrame(attempt);
}

let audioCtx: AudioContext | null = null;
/** Short, quiet beep for the audible bell. */
function beep(): void {
  try {
    audioCtx = audioCtx ?? new AudioContext();
    const osc = audioCtx.createOscillator();
    const gain = audioCtx.createGain();
    osc.frequency.value = 800;
    gain.gain.value = 0.04;
    osc.connect(gain);
    gain.connect(audioCtx.destination);
    osc.start();
    osc.stop(audioCtx.currentTime + 0.08);
  } catch {
    /* no audio device */
  }
}

function ensureView(sessionId: string, tab: Tab): TabView {
  let view = views.get(tab.id);
  if (view) {
    if (!view.box.isConnected) termWrap.append(view.box);
    return view;
  }

  const box = document.createElement("div");
  box.className = "tabbox";
  const head = document.createElement("div");
  head.className = "pane-head";
  const surface = document.createElement("div");
  surface.className = "tab-surface";
  box.append(head, surface);
  termWrap.append(box);

  const term = new Terminal({
    theme: xtermTheme(cfg.theme),
    fontFamily: cfg.fontFamily ?? MUIS_THEME.termFont,
    fontSize: effectiveFontSize(cfg),
    cursorStyle: cfg.cursorStyle,
    cursorBlink: cfg.cursorBlink,
    scrollback: cfg.scrollback,
  });
  const search = new SearchAddon();
  term.loadAddon(search);
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(surface);

  // OSC 8 hyperlinks: activate opens http(s) URLs in the browser.
  term.options.linkHandler = {
    activate: (_event: MouseEvent, text: string) => {
      if (!/^https?:\/\//.test(text)) return;
      if (BACKEND) void shellCall("open_url", { url: text }).catch(() => {});
      else window.open(text, "_blank", "noopener");
    },
    hover: () => {},
    leave: () => {},
  } as ILinkHandler;

  // Bell: no-op, a brief visual flash, an audible beep, or both.
  term.onBell(() => {
    const { visual, audible } = bellAction(cfg.bell);
    if (visual) {
      surface.classList.add("bell-flash");
      window.setTimeout(() => surface.classList.remove("bell-flash"), 140);
    }
    if (audible) beep();
  });

  // Clipboard modes (opt-in via config).
  surface.addEventListener("mouseup", () => {
    if (!cfg.copyOnSelect) return;
    const sel = term.getSelection();
    if (sel) void navigator.clipboard?.writeText(sel).catch(() => {});
  });
  surface.addEventListener("auxclick", (e) => {
    if (e.button !== 1 || !cfg.middleClickPaste) return;
    e.preventDefault();
    void (async () => {
      try {
        const text = await navigator.clipboard.readText();
        if (!text) return;
        if (BACKEND) void client.write(sessionId, tab.id, enc.encode(text)).catch(() => {});
        else term.write(text);
      } catch {
        /* clipboard read blocked */
      }
    })();
  });

  // Prefer the GPU renderer, fall back to xterm's built-in canvas renderer
  // if WebGL is unavailable or the context is lost (some WebKitGTK/NVIDIA
  // stacks). Canvas is correct, just slower for large scrollback.
  try {
    const webgl = new WebglAddon();
    webgl.onContextLoss(() => webgl.dispose());
    term.loadAddon(webgl);
  } catch {
    /* canvas renderer stays active */
  }

  if (BACKEND) {
    activity.spawnedAt.set(tab.id, Date.now());
    void (async () => {
      try {
        const shell = await defaultShell();
        renderCommandHead(tab.id);
        const testCommand = await liveTestCommand();
        await client.spawnTab(
          sessionId,
          tab.id,
          shell,
          tab.cwd,
          80,
          24,
          (data) => {
            activity.lastOutputAt.set(tab.id, Date.now());
            sawOutput.add(tab.id);
            outputWaiters.get(tab.id)?.();
            term.write(data);
            observeOsc(sessionId, tab, data);
            armIdleFallback(tab.id);
            if (testCommand && !liveTestCommandSent.has(tab.id)) {
              liveTestCommandSent.add(tab.id);
              // Wait until the shell has emitted its first prompt/banner
              // before exercising the exact UI -> worker -> PTY input path.
              window.setTimeout(() => {
                trackInput(tab.id, `${testCommand}\n`, false);
                void client
                  .write(sessionId, tab.id, enc.encode(`${testCommand}\n`))
                  .catch(() => {});
              }, 1000);
            }
          },
          (code) => {
            activity.exited.add(tab.id);
            term.writeln(`\r\n[process exited${code === null ? "" : ` (${code})`}]`);
            if (!isTabVisible(tab.id)) {
              doneTabs.mark(tab.id);
            }
            renderTabs();
            renderSessions();
            updateWindowTitle();
            // Deferred so the view's own exit callback finishes first.
            window.setTimeout(() => dropSessionIfLastTab(sessionId, tab.id), 0);
          },
        );
        // The pty exists now. The fit that ran when the tab was created may
        // have raced the spawn and been dropped by the worker, leaving the
        // pty at the default size; size it to the real surface here too.
        const spawned = views.get(tab.id);
        if (spawned && spawned.box.classList.contains("active")) {
          fitShown(sessionId, tab, spawned);
        }
      } catch (e) {
        term.writeln(`\r\n[failed to spawn pty: ${String(e)}]`);
      }
    })();
    term.onData((data) => {
      trackInput(tab.id, data);
      void client.write(sessionId, tab.id, enc.encode(data));
    });
  } else {
    term.writeln("browser preview — local echo only; run in the Tauri window for real ptys.");
    term.onData((data) => {
      trackInput(tab.id, data);
      term.write(data);
    });
  }

  view = { term, fit, search, box };
  views.set(tab.id, view);
  renderCommandHead(tab.id);

  // Refit whenever the surface actually changes size. This catches the
  // hidden->shown transition (xterm measures 0 while display:none, so the
  // first fit can be stale) and window/sidebar resizes, which a single
  // one-shot fit misses — the cause of a TUI rendering at the wrong size
  // and not filling the pane.
  const self = view;
  view.ro = new ResizeObserver(() => {
    if (!self.box.classList.contains("active")) return;
    requestAnimationFrame(() => {
      if (self.box.classList.contains("active")) fitShown(sessionId, tab, self);
    });
  });
  view.ro.observe(surface);

  return view;
}

/** Shell-reported cwd/title (OSC 7 / OSC 0,2) and command markers (OSC 133). */
function observeOsc(sessionId: string, tab: Tab, data: Uint8Array): void {
  let parser = oscParsers.get(tab.id);
  if (!parser) {
    parser = new OscParser();
    oscParsers.set(tab.id, parser);
  }
  let changed = false;
  for (const ev of parser.push(data)) {
    const wsNow = store.workspaces.find((w) => w.id === sessionId);
    const tabNow = wsNow?.tabs.find((t) => t.id === tab.id);
    if (!tabNow) continue;
    if (ev.type === "cwd" && tabNow.cwd !== ev.path) {
      tabNow.cwd = ev.path;
      changed = true;
    } else if (ev.type === "title" && ev.title) {
      // Fish reports its abbreviated cwd as title. Pinned (manual) tabs
      // keep their names. opencode prefixes its session title with "OC |";
      // remember that separately so a manual rename never hides the session.
      if (isOpencodeTitle(ev.title)) {
        const agentTitle = opencodeTitleFromLabel(ev.title);
        if (agentTitle && tabNow.agentTitle !== agentTitle) {
          tabNow.agentTitle = agentTitle;
          changed = true;
        }
      }
      if (!tabNow.manual && tabNow.title !== ev.title) {
        tabNow.title = ev.title;
        changed = true;
      }
    } else if (ev.type === "cmd-start") {
      // Prefer the shell-reported command line, then captured keystrokes,
      // then the on-screen line (input that bypassed the keyboard).
      const st = commandTracker.state(tabNow.id);
      const cmd = ev.cmd || st.input.trim() || currentInputLine(tabNow.id);
      commandTracker.onCmdStart(tabNow.id, cmd);
      reportDebugStage("cmd-start", { tab: tabNow.id, cmd });
      // A new command means the previous completion is handled — unless the
      // tab is owned by an agent. There the mark belongs to the agent and only
      // a reply (`muis-notify --running`) or closing the tab may drop it: the
      // resume line muis types during restore, and the prompt text some shells
      // report as a command start, must not.
      if (isInjectedResume(restoreCommands.get(tabNow.id), cmd)) {
        restoreCommands.delete(tabNow.id);
      } else if (doneTabs.has(tabNow.id) && !tabNow.agentSession) {
        doneTabs.clear(tabNow.id);
        reportDebugStage("attention-cleared", { tab: tabNow.id, reason: "command" });
        changed = true;
      }
      renderCommandHead(tabNow.id);
    } else if (ev.type === "cmd-end") {
      finishCommand(tabNow.id, ev.exit);
    } else if (ev.type === "notify") {
      // Terminal agents report completion with OSC 9/777/99 (or the
      // muis-notify CLI). Badge the tab when the user was not looking at
      // it, and raise a desktop toast only while the window is unfocused.
      if (applyNotification(tabNow, ev)) changed = true;
    }
  }
  if (changed) {
    renderTabs();
    renderSessions();
    renderStatusbar();
    updateWindowTitle();
    scheduleSave();
  }
}

/** Drop a tab's saved scrollback lines (CSI 3 J; keeps the live screen). */
function clearScrollback(tabId: string): void {
  const view = views.get(tabId);
  if (view) view.term.write("\x1b[3J");
}

function closeTab(sessionId: string, tabId: string): void {
  const owner = store.workspaces.find((w) => w.id === sessionId);
  const tab = owner?.tabs.find((t) => t.id === tabId);
  if (tab && isTabBusy(activity, tabId, Date.now())) {
    if (!window.confirm(`Close "${tab.title}"? A process is still running.`)) return;
  }
  const view = views.get(tabId);
  if (view) {
    if (BACKEND) {
      void client.kill(sessionId, tabId).catch(() => {});
    }
    view.ro?.disconnect(); view.term.dispose();
    view.box.remove();
    views.delete(tabId);
    oscParsers.delete(tabId);
    forgetTab(activity, tabId);
    forgetTabState(tabId);
  }
  const wsIndex = store.workspaces.findIndex((w) => w.id === sessionId);
  const ws = store.workspaces[wsIndex];
  if (ws) {
    const tabIndex = ws.tabs.findIndex((t) => t.id === tabId);
    if (tabIndex >= 0) store.closeTab(wsIndex, tabIndex);
  }
  renderAll();
  // Closing a tab must never resurrect it on the next launch.
  void saveNow();
}

function closeSession(w: Workspace): void {
  const busy = w.tabs.some((t) => isTabBusy(activity, t.id, Date.now()));
  if (busy && !window.confirm(`Close session "${w.name}"? A process is still running.`)) return;
  for (const t of w.tabs) {
    // Background sessions have no view yet; still tear down their pty.
    if (BACKEND) {
      void client.kill(w.id, t.id).catch(() => {});
    }
    const view = views.get(t.id);
    if (view) {
      view.ro?.disconnect(); view.term.dispose();
      view.box.remove();
      views.delete(t.id);
    }
    oscParsers.delete(t.id);
    forgetTab(activity, t.id);
    forgetTabState(t.id);
  }
  if (BACKEND) void client.stopSession(w.id).catch(() => {});
  const idx = store.workspaces.findIndex((x) => x.id === w.id);
  if (idx >= 0) store.removeWorkspace(idx);
  renderAll();
  void saveNow();
}

/**
 * A pty exited. If it was the session's only tab, tear the session down
 * immediately (no grace period) so a dead shell does not leave an empty
 * session behind. Background sessions are handled the same as the active
 * one.
 */
function dropSessionIfLastTab(sessionId: string, tabId: string): void {
  const ws = store.workspaces.find((w) => w.id === sessionId);
  if (!ws) return;
  const isLast = ws.tabs.length === 1 && ws.tabs[0]?.id === tabId;
  if (!isLast) return;
  if (BACKEND) {
    void client.stopSession(sessionId).catch(() => {});
  }
  const view = views.get(tabId);
  if (view) {
    view.ro?.disconnect(); view.term.dispose();
    view.box.remove();
    views.delete(tabId);
  }
  oscParsers.delete(tabId);
  forgetTab(activity, tabId);
  forgetTabState(tabId);
  const idx = store.workspaces.findIndex((w) => w.id === sessionId);
  if (idx >= 0) store.removeWorkspace(idx);
  renderAll();
  // A dead session must not come back on the next launch.
  void saveNow();
}

/* ---------------- chrome render ---------------- */

function el(tag: string, cls: string, text?: string): HTMLElement {
  const e = document.createElement(tag);
  e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

/**
 * Toolbar icon for the browser pane: a window split into terminal and page.
 * Drawn rather than typed so it renders the same in every font fallback.
 */
function paneIcon(): SVGSVGElement {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("width", "16");
  svg.setAttribute("height", "16");
  svg.setAttribute("aria-hidden", "true");
  const frame = document.createElementNS(ns, "rect");
  frame.setAttribute("x", "1.75");
  frame.setAttribute("y", "2.75");
  frame.setAttribute("width", "12.5");
  frame.setAttribute("height", "10.5");
  frame.setAttribute("rx", "2");
  const split = document.createElementNS(ns, "path");
  split.setAttribute("d", "M9.25 2.75v10.5");
  svg.append(frame, split);
  return svg;
}

let appliedTheme: string | null = null;
/** Apply the current workspace's accent theme (chrome + terminals). */
function applyWorkspaceTheme(): void {
  const name = themeForWorkspace(cfg, store.currentWorkspace()?.name ?? "");
  if (name === appliedTheme) return;
  appliedTheme = name;
  applyTheme(name);
  const xt = xtermTheme(name);
  for (const [, view] of views) view.term.options.theme = xt;
}

function renderAll(): void {
  const ws = store.currentWorkspace();
  applyWorkspaceTheme();
  // A session with no tabs is a valid state: it shows only the + tab
  // button and no terminal, and is kept until the user closes the session
  // or exits its last process. New sessions open a tab in addSession.
  // Looking at a tab acknowledges its completion without dropping it: the
  // tab stays marked until the user works in it (input or next command).
  const active = ws?.tabs[ws.active];
  if (active) doneTabs.see(active.id);
  updateWindowTitle();
  sidebar.style.display = cfg.showSessions ? "" : "none";
  renderSessions();
  renderTabs();
  renderTerms(ws);
  renderStatusbar();
  renderPanelSlot();
  scheduleSave();
}

function renderSessions(): void {
  sidebar.innerHTML = "";
  const label = el("div", "side-label");
  label.append(el("span", "", "Sessions"));
  sidebar.append(label);
  const list = el("div", "");
  list.style.display = "flex";
  list.style.flexDirection = "column";
  list.style.gap = "6px";
  list.style.overflowY = "auto";
  const colors = sessionColorMap(store.workspaces.map((w) => w.name));
  store.workspaces.forEach((w, i) => {
    list.append(sessionElement(w, i === store.current, colors.get(w.name) ?? colorFor(w.name)));
  });
  sidebar.append(list);
  const footer = el("div", "side-footer");
  const addBtn = el("button", "btn", "＋ session");
  addBtn.style.flex = "1";
  addBtn.addEventListener("click", () => {
    openSessionDialog("new");
  });
  footer.append(addBtn);
  sidebar.append(footer);
}

/**
 * Directory chooser for a new session. Inside Tauri the Browse button opens
 * the native folder picker; the browser preview types the path directly, so
 * the Selenium suite and plain `vite dev` work without the native picker.
 */
async function pickDirectory(start: string): Promise<string | null> {
  // A page cannot open a native dialog, so the server lists folders.
  if (IS_SERVE) return pickDirectoryServe(start);
  if (!IN_TAURI) return null;
  try {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ directory: true, multiple: false, defaultPath: start });
    if (typeof picked === "string") return picked;
    if (Array.isArray(picked) && typeof picked[0] === "string") return picked[0];
  } catch {
    /* fall back to the typed path */
  }
  return null;
}

/* ---------------- new / rename session dialog ---------------- */

const sessionOverlay = el("div", "session-overlay");
sessionOverlay.style.display = "none";
const sessionBox = el("div", "settings-box session-box");
const sessionHeader = el("div", "settings-header");
const sessionTitle = el("h2", "", "New session");
const sessionClose = el("button", "settings-close", "✕") as HTMLButtonElement;
sessionClose.type = "button";
sessionClose.title = "Close";
sessionHeader.append(sessionTitle, sessionClose);

const sessionBody = el("div", "settings-body");
const sessionName = document.createElement("input");
sessionName.type = "text";
sessionName.placeholder = "session name";
sessionName.autocomplete = "off";
sessionName.spellcheck = false;
const sessionDir = document.createElement("input");
sessionDir.type = "text";
sessionDir.placeholder = "~";
sessionDir.autocomplete = "off";
sessionDir.spellcheck = false;
const sessionBrowse = el("button", "btn", "Browse…") as HTMLButtonElement;
sessionBrowse.type = "button";
if (!BACKEND) sessionBrowse.style.display = "none";
const dirWrap = el("span", "dir-pick");
dirWrap.append(sessionDir, sessionBrowse);
const sessionQuick = el("div", "quick-picks");
const sessionSubs = el("div", "quick-picks session-subs");
sessionSubs.style.display = "none";
const sessionNameRow = settingsRow("Name", sessionName);
const sessionDirRow = settingsRow("Directory", dirWrap);
const sessionError = el("div", "session-error");
sessionError.style.display = "none";
sessionBody.append(sessionNameRow, sessionDirRow, sessionQuick, sessionSubs, sessionError);

const sessionButtons = el("div", "settings-buttons");
const sessionCancel = el("button", "", "Cancel");
const sessionSubmit = el("button", "primary", "Create");
sessionButtons.append(sessionCancel, sessionSubmit);

sessionBox.append(sessionHeader, sessionBody, sessionButtons);
sessionOverlay.append(sessionBox);
document.body.append(sessionOverlay);

let sessionRenameTarget: Workspace | null = null;
/** True once the user edits the name, so picking a folder stops renaming it. */
let sessionNameEdited = false;

/** Last path segment, e.g. /home/x/Projects -> Projects. */
function sessionDirName(dir: string): string {
  const trimmed = dir.replace(/\/+$/, "");
  if (!trimmed) return "";
  return trimmed.slice(trimmed.lastIndexOf("/") + 1) || trimmed;
}

/** Point the directory field at `dir`; derive the name from it unless edited. */
function applyPickedDir(dir: string): void {
  sessionDir.value = dir;
  if (!sessionNameEdited) {
    const name = sessionDirName(dir);
    if (name) sessionName.value = name;
  }
  void renderSessionSubdirs(dir);
}

let sessionSubsToken = 0;

/** List up to ten subfolders of `dir` as one-click pills (Tauri only). */
async function renderSessionSubdirs(dir: string): Promise<void> {
  const token = ++sessionSubsToken;
  const clear = (): void => {
    sessionSubs.replaceChildren();
    sessionSubs.style.display = "none";
  };
  if (!BACKEND || !dir) {
    clear();
    return;
  }
  let names: string[] = [];
  try {
    names = await shellCall<string[]>("list_subdirs", { path: dir, limit: 10 });
  } catch {
    names = [];
  }
  if (token !== sessionSubsToken) return; // a newer pick superseded this one
  if (names.length === 0) {
    clear();
    return;
  }
  const parent = dir.replace(/\/+$/, "");
  sessionSubs.replaceChildren(el("span", "quick-label", "Subfolders"));
  for (const name of names) {
    const b = el("button", "quick-pick", name) as HTMLButtonElement;
    b.type = "button";
    b.addEventListener("click", () => applyPickedDir(`${parent}/${name}`));
    sessionSubs.append(b);
  }
  sessionSubs.style.display = "";
}

function renderSessionQuickPicks(): void {
  sessionQuick.replaceChildren();
  const choices: [string, string][] = [];
  if (homeDir) {
    choices.push(["Home", homeDir]);
    choices.push(["Downloads", `${homeDir}/Downloads`]);
    choices.push(["Projects", `${homeDir}/Projects`]);
  } else {
    choices.push(["Home", "~"]);
  }
  const cur = store.currentWorkspace();
  if (cur && cur.dir && !choices.some(([, v]) => v === cur.dir)) {
    choices.push(["Current session", cur.dir]);
  }
  for (const [label, value] of choices) {
    const b = el("button", "quick-pick", label) as HTMLButtonElement;
    b.type = "button";
    b.title = value;
    b.addEventListener("click", () => applyPickedDir(value));
    sessionQuick.append(b);
  }
}

function openSessionDialog(mode: "new" | "rename", target?: Workspace): void {
  sessionRenameTarget = mode === "rename" ? target ?? null : null;
  sessionError.style.display = "none";
  const isNew = mode === "new";
  sessionTitle.textContent = isNew ? "New session" : "Rename session";
  sessionSubmit.textContent = isNew ? "Create" : "Save";
  sessionDirRow.style.display = isNew ? "" : "none";
  sessionQuick.style.display = isNew ? "" : "none";
  if (isNew) {
    sessionNameEdited = false;
    sessionName.value = `session-${store.workspaces.length + 1}`;
    sessionDir.value = store.currentWorkspace()?.dir || homeDir || "~";
    renderSessionQuickPicks();
  } else {
    sessionName.value = target ? target.name : "";
  }
  sessionOverlay.style.display = "flex";
  sessionName.focus();
  sessionName.select();
}

function closeSessionDialog(): void {
  sessionOverlay.style.display = "none";
  sessionRenameTarget = null;
}

function submitSessionDialog(): void {
  const name = sessionName.value.trim();
  if (!name) {
    sessionError.textContent = "Enter a name.";
    sessionError.style.display = "";
    return;
  }
  let created = false;
  if (sessionRenameTarget) {
    sessionRenameTarget.name = name;
  } else {
    const dirRaw = sessionDir.value.trim();
    if (!dirRaw) {
      sessionError.textContent = "Enter a directory.";
      sessionError.style.display = "";
      return;
    }
    const dir = expandHome(dirRaw, homeDir);
    const idx = store.addWorkspace(name, dir);
    store.switch(idx);
    // A new session opens a terminal; an emptied session later stays empty.
    store.newTab("Terminal 1", dir);
    created = true;
  }
  closeSessionDialog();
  renderAll();
  if (created) window.setTimeout(focusActiveTerminal, 0);
}

sessionClose.addEventListener("click", closeSessionDialog);
sessionCancel.addEventListener("click", closeSessionDialog);
sessionSubmit.addEventListener("click", submitSessionDialog);
sessionBrowse.addEventListener("click", () => {
  void pickDirectory(sessionDir.value.trim() || homeDir).then((picked) => {
    if (picked) applyPickedDir(picked);
  });
});
sessionName.addEventListener("input", () => {
  sessionNameEdited = true;
});
sessionOverlay.addEventListener("click", (e) => {
  if (e.target === sessionOverlay) closeSessionDialog();
});
for (const input of [sessionName, sessionDir]) {
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      submitSessionDialog();
    } else if (e.key === "Escape") {
      e.preventDefault();
      closeSessionDialog();
    }
  });
}

/** Stable per-session glyph (mock shows an icon per session). */
const SESSION_GLYPHS = ["◈", "⬢", "⬣", "✦", "⬔", "❖", "◆", "●"];
function sessionGlyph(name: string): string {
  let h = 0;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0;
  return SESSION_GLYPHS[h % SESSION_GLYPHS.length];
}

function renameSession(w: Workspace): void {
  openSessionDialog("rename", w);
}

/* ---------------- drag reorder (tabs + sessions) ---------------- */

let dragKind: "session" | "tab" | null = null;
let dragFrom = -1;
let dragTabId: string | null = null;

/** Move a dragged tab into another session (append); switch to the target. */
function moveTabToSession(tabId: string, toSessionId: string): void {
  const fromWsIndex = store.workspaces.findIndex((w) => w.tabs.some((t) => t.id === tabId));
  if (fromWsIndex < 0) return;
  const toWsIndex = store.workspaces.findIndex((w) => w.id === toSessionId);
  if (toWsIndex < 0 || toWsIndex === fromWsIndex) return;
  const tabIndex = store.workspaces[fromWsIndex].tabs.findIndex((t) => t.id === tabId);
  if (store.moveTabToWorkspace(fromWsIndex, tabIndex, toWsIndex)) renderAll();
}

function clearDropMarks(): void {
  document
    .querySelectorAll(".drop-before, .drop-after")
    .forEach((elm) => elm.classList.remove("drop-before", "drop-after"));
}

/** Final index for a drop near `idx`; reorder only, never detach. */
function dropIndex(idx: number, before: boolean): number {
  if (before) return dragFrom < idx ? idx - 1 : idx;
  return dragFrom < idx ? idx : idx + 1;
}

function attachReorder(
  d: HTMLElement,
  kind: "session" | "tab",
  idx: number,
  axis: "x" | "y",
  apply: (from: number, to: number) => boolean,
): void {
  const beforeAt = (e: DragEvent): boolean => {
    const rect = d.getBoundingClientRect();
    return axis === "y"
      ? e.clientY < rect.top + rect.height / 2
      : e.clientX < rect.left + rect.width / 2;
  };
  d.draggable = true;
  d.addEventListener("dragstart", (e) => {
    dragKind = kind;
    dragFrom = idx;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", String(idx));
    }
    d.classList.add("dragging");
  });
  d.addEventListener("dragend", () => {
    dragKind = null;
    dragFrom = -1;
    d.classList.remove("dragging");
    clearDropMarks();
  });
  d.addEventListener("dragover", (e) => {
    if (dragKind !== kind) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    const before = beforeAt(e);
    clearDropMarks();
    d.classList.add(before ? "drop-before" : "drop-after");
  });
  d.addEventListener("drop", (e) => {
    if (dragKind !== kind) return;
    e.preventDefault();
    if (idx === dragFrom) return; // dropped on itself
    const to = dropIndex(idx, beforeAt(e));
    const from = dragFrom;
    clearDropMarks();
    dragKind = null;
    dragFrom = -1;
    if (to !== from) apply(from, to);
  });
}

/** Put keyboard focus on the current workspace's active terminal, if any. */
function focusActiveTerminal(): void {
  const ws = store.currentWorkspace();
  const tab = ws ? ws.tabs[ws.active] : undefined;
  if (tab) views.get(tab.id)?.term.focus();
}

/** Focus the active terminal after a tab/session selection click. */
function focusActiveTerminalOnSelect(): void {
  if (
    shouldFocusTerminal({
      dragging: dragKind !== null,
      searchFocused: searchBox.contains(document.activeElement),
    })
  ) {
    focusActiveTerminal();
  }
}

function sessionElement(w: Workspace, selected: boolean, col: string): HTMLElement {
  const idx = store.workspaces.indexOf(w);
  // A session with no tabs has no work open; grey it out so the active
  // sessions stand out.
  const empty = w.tabs.length === 0;
  const d = el("div", "session" + (selected ? " active" : "") + (empty ? " empty" : ""));
  d.dataset.i = String(idx);
  const icon = el("div", "s-icon", sessionGlyph(w.name));
  if (empty) {
    icon.style.background = "#2d3237";
    icon.style.color = "var(--dim)";
    icon.style.border = "1px solid #3a4147";
  } else {
    icon.style.background = `${col}22`;
    icon.style.color = col;
    icon.style.border = `1px solid ${col}55`;
  }
  const meta = el("div", "s-meta");
  meta.append(
    el("div", "s-name", w.name),
    el("div", "s-sub", `${w.tabs.length} tab${w.tabs.length === 1 ? "" : "s"} · ${shortPath(w.dir)}`),
  );
  d.append(icon, meta);
  // Hidden sessions surface work the user has not dealt with: a fresh alert
  // pulses with a count, one the user already glanced at (or a dead pty)
  // only leaves a steady dot.
  const attention = sidebarAttention(
    w.tabs.map((t) => tabAttention(t.id)),
    selected,
  );
  if (attention.count > 0) {
    d.append(el("span", "s-badge", String(attention.count)));
    d.append(el("div", "s-dot notify unseen"));
  } else if (attention.dot) {
    d.append(el("div", "s-dot notify"));
  }
  d.addEventListener("click", () => {
    store.switch(idx);
    renderAll();
    focusActiveTerminalOnSelect();
  });
  d.addEventListener("dblclick", () => renameSession(w));
  attachReorder(d, "session", idx, "y", (from, to) => {
    const moved = store.moveWorkspace(from, to);
    if (moved) renderAll();
    return moved;
  });
  // A tab dragged over a session drops into that session.
  d.addEventListener("dragover", (e) => {
    if (dragKind !== "tab") return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    clearDropMarks();
    d.classList.add("drop-into");
  });
  d.addEventListener("drop", (e) => {
    if (dragKind !== "tab") return;
    e.preventDefault();
    const tabId = dragTabId;
    clearDropMarks();
    dragKind = null;
    dragFrom = -1;
    dragTabId = null;
    if (tabId) moveTabToSession(tabId, w.id);
  });
  return d;
}

function renderTabs(): void {
  tabScroller.innerHTML = "";
  const ws = store.currentWorkspace();
  if (!ws) return;
  ws.tabs.forEach((t, i) => {
    tabScroller.append(tabElement(ws, t, i === ws.active));
  });
}

function addShellTab(ws: Workspace): void {
  store.newTab(`Terminal ${ws.tabs.length + 1}`, newTabCwd(ws));
  renderAll();
  // Deferred: the click that opened the tab would otherwise move focus back
  // to the (now replaced) button after this handler returns.
  window.setTimeout(focusActiveTerminal, 0);
}

function renameTab(ws: Workspace, tab: Tab): void {
  const name = window.prompt("Tab name", tab.title);
  if (name) {
    tab.title = name;
    tab.manual = true;
    renderAll();
  }
}

function toggleFreezeTitle(ws: Workspace, tab: Tab): void {
  tab.manual = !tab.manual;
  if (!tab.manual) {
    // Back to the shell-driven title: reset to the placeholder and let
    // the next OSC 0/2 update land.
    const i = ws.tabs.findIndex((t) => t.id === tab.id);
    tab.title = `Terminal ${i + 1}`;
  }
  renderTabs();
}

/** Tab label: the last command run, unless the user pinned a name. */
function tabLabel(tab: Tab): string {
  return tabLabelOf(tab.title, tab.manual, commandTracker.state(tab.id).lastCmd);
}

function tabElement(ws: Workspace, tab: Tab, selected: boolean): HTMLElement {
  const done = isTabDone(tab.id);
  const d = el("div", "tab" + (selected ? " active" : "") + (done ? " done" : ""));
  d.dataset.i = String(ws.tabs.findIndex((t) => t.id === tab.id));
  const label = tabLabel(tab);
  const tcol = colorFor(label);
  const dot = el("span", "t-dot");
  dot.style.background = tcol;
  const title = el("span", "t-name", label);
  d.append(dot, title);
  // A pending completion reads "waiting" — it wants the user. It blinks
  // until the tab is looked at, then stays lit (green, no animation) until
  // the agent resumes or the tab closes. Only a dead pty reads "done".
  const attention = tabAttention(tab.id);
  const chip = tabAttentionLabel(attention, activity.exited.has(tab.id));
  if (chip) d.append(el("span", "t-state" + (attention === "unseen" ? " unseen" : ""), chip));
  const x = el("span", "x", "✕");
  x.addEventListener("click", (e) => {
    e.stopPropagation();
    closeTab(ws.id, tab.id);
  });
  d.append(x);
  d.addEventListener("click", () => {
    ws.active = ws.tabs.findIndex((t) => t.id === tab.id);
    renderAll();
    focusActiveTerminalOnSelect();
  });
  // Middle-click closes the tab, same path as the ✕ button. auxclick is
  // button-specific, so this leaves the select handler and the reorder
  // drag (both primary-button) untouched.
  d.addEventListener("auxclick", (e) => {
    if (e.button !== 1) return;
    e.preventDefault();
    e.stopPropagation();
    closeTab(ws.id, tab.id);
  });
  d.addEventListener("dblclick", () => renameTab(ws, tab));
  attachReorder(d, "tab", ws.tabs.findIndex((t) => t.id === tab.id), "x", (from, to) => {
    const moved = store.moveTab(store.workspaces.indexOf(ws), from, to);
    if (moved) renderAll();
    return moved;
  });
  d.addEventListener("dragstart", () => {
    dragTabId = tab.id;
  });
  d.addEventListener("dragend", () => {
    dragTabId = null;
  });
  return d;
}

function renderTerms(ws: Workspace | undefined): void {
  for (const [id, view] of views) {
    if (!store.workspaces.some((w) => w.tabs.some((t) => t.id === id))) {
      view.ro?.disconnect(); view.term.dispose();
      view.box.remove();
      views.delete(id);
      oscParsers.delete(id);
    } else {
      // Switching workspaces leaves their live xterm instances in the
      // shared terminal host. Hide every old surface first; then the
      // current workspace's active tab below is the only one displayed.
      view.box.classList.remove("active");
    }
  }
  // No sessions: every terminal view was just disposed above.
  if (!ws) return;
  for (const t of ws.tabs) {
    const show = ws.tabs[ws.active]?.id === t.id;
    const view = ensureView(ws.id, t);
    view.box.classList.toggle("active", show);
    if (show) fitShown(ws.id, t, view);
  }
}

/* ---------------- statusbar ---------------- */

const stCwd = el("span", "", "");
const stGit = el("span", "pill", "");
const stClock = el("span", "clock", "");
// Context on the left, clock on the far right.
statusbar.append(stCwd, stGit, el("span", ""), stClock);
(statusbar.children[2] as HTMLElement).style.marginLeft = "auto";

const gitCache = new Map<string, string>();

function renderStatusbar(): void {
  const ws = store.currentWorkspace();
  const tab = ws?.tabs[ws.active];
  stCwd.textContent = tab ? shortPath(tab.cwd) : "";
  if (tab) {
    const cwd = tab.cwd;
    const cached = gitCache.get(cwd);
    stGit.textContent = cached ?? "⎇ …";
    if (cached === undefined && BACKEND) {
      void shellCall<string>("git_branch", { cwd })
        .then((branch) => {
          gitCache.set(cwd, branch ? `⎇ ${branch} ✓` : "⎇ —");
          if (store.currentWorkspace()?.tabs[ws?.active ?? -1]?.cwd === cwd) renderStatusbar();
        })
        .catch(() => {
          gitCache.set(cwd, "⎇ —");
        });
    }
  } else {
    stGit.textContent = "";
  }
  applyStatusbarVisibility();
}

/** Show/hide each statusbar segment per config. */
function applyStatusbarVisibility(): void {
  const segs: [string, HTMLElement][] = [
    ["cwd", stCwd],
    ["git", stGit],
    ["clock", stClock],
  ];
  for (const [id, node] of segs) {
    node.style.display = segmentVisible(cfg.statusbar, id) ? "" : "none";
  }
}

function tickClock(): void {
  const f = () => {
    const text = clockText(new Date());
    if (stClock.textContent !== text) stClock.textContent = text;
  };
  f();
  setInterval(f, 1000);
}

/* ---------------- side panel slot ---------------- */

/** Content host for a visible panel; null when nothing is docked. */
export function panelContent(id: string): HTMLElement | null {
  if (sidePanels.visible() !== id) return null;
  return panelSlot.querySelector<HTMLElement>("[data-panel-body]");
}

function renderPanelSlot(): void {
  const id = sidePanels.visible();
  panelSlot.style.display = id ? "flex" : "none";
  if (!id) return;
  panelSlot.innerHTML = "";
  const def = sidePanels.list().find((p) => p.id === id);
  const head = el("div", "side-label");
  head.append(el("span", "", def?.title ?? id));
  const close = el("button", "btn", "×");
  close.addEventListener("click", () => {
    sidePanels.close();
    renderPanelSlot();
  });
  head.append(close);
  const body = el("div", "");
  body.setAttribute("data-panel-body", "");
  panelSlot.append(head, body);
}

/* ---------------- browser pane ---------------- */

let previewOpen = false;
/** Last page the pane was pointed at; survives closing so it can reopen. */
let previewUrl = "";
/** User-chosen pane width in px; 0 means "share of the window". */
let previewWidth = 0;

function applyPreviewWidth(): void {
  const width = previewWidth > 0 ? previewWidth : defaultPreviewWidth(window.innerWidth);
  previewSlot.style.width = `${clampPreviewWidth(width, window.innerWidth)}px`;
}

/** Terminals are sized from the DOM, so every layout change must refit. */
function refitActive(): void {
  const ws = store.currentWorkspace();
  const active = ws?.tabs[ws.active];
  const view = active ? views.get(active.id) : undefined;
  if (!ws || !active || !view) return;
  fitShown(ws.id, active, view);
}

/**
 * Mirror the pane state into the DOM. A hidden or empty pane unloads its
 * document, so a preview cannot keep playing audio after it is closed.
 */
function renderPreview(): void {
  previewSlot.style.display = previewOpen ? "flex" : "none";
  paneBtn.classList.toggle("on", previewOpen);
  const loaded = previewOpen && previewUrl !== "";
  previewEmpty.style.display = loaded ? "none" : "flex";
  const want = loaded ? previewUrl : "about:blank";
  if (previewFrame.getAttribute("src") !== want) previewFrame.src = want;
}

/** Show `raw` in the pane. Returns false for input that is not http(s). */
function openPreview(raw: string, focus = false): boolean {
  const url = normalizeUrl(raw);
  if (!url) {
    previewUrlInput.classList.add("bad");
    return false;
  }
  previewUrlInput.classList.remove("bad");
  previewUrlInput.value = url;
  previewUrl = url;
  previewOpen = true;
  applyPreviewWidth();
  renderPreview();
  refitActive();
  if (focus) previewUrlInput.focus();
  return true;
}

function closePreview(): void {
  if (!previewOpen) return;
  previewOpen = false;
  renderPreview();
  refitActive();
}

function focusPreview(): void {
  if (!previewOpen) {
    previewOpen = true;
    applyPreviewWidth();
    renderPreview();
    refitActive();
  }
  previewUrlInput.focus();
  previewUrlInput.select();
}

/** Toggle the pane; closing hands the keyboard back to the terminal. */
function togglePreview(): void {
  if (previewOpen) {
    closePreview();
    focusActiveTerminal();
  } else {
    focusPreview();
  }
}

/** Open a URL outside muis; the pane keeps working when a site blocks frames. */
function openExternally(url: string): void {
  if (!url) return;
  shellCall("open_url", { url }).catch(() => {
    window.open(url, "_blank", "noopener");
  });
}

/** A `muis-agent open` request forwarded by the shell. */
function handleOpenUrl(req: CliOpenUrl): void {
  if (!req?.url) return;
  if (!openPreview(req.url)) return;
  // Never take keyboard focus here: the agent is usually mid-turn.
  reportDebugStage("preview-opened", { url: previewUrl, tab: req.tab_id ?? null });
}

previewClose.addEventListener("click", () => {
  closePreview();
  focusActiveTerminal();
});
previewReload.addEventListener("click", () => {
  if (!previewUrl) return;
  previewFrame.src = "about:blank";
  window.setTimeout(() => {
    if (previewOpen && previewUrl) previewFrame.src = previewUrl;
  }, 0);
});
previewExternal.addEventListener("click", () => openExternally(previewUrl));
previewUrlInput.addEventListener("keydown", (e) => {
  e.stopPropagation();
  if (e.key === "Enter") {
    e.preventDefault();
    openPreview(previewUrlInput.value);
  } else if (e.key === "Escape") {
    e.preventDefault();
    previewUrlInput.value = previewUrl;
    previewUrlInput.classList.remove("bad");
    focusActiveTerminal();
  }
});
previewUrlInput.addEventListener("input", () => previewUrlInput.classList.remove("bad"));
previewGrip.addEventListener("dblclick", () => {
  previewWidth = 0;
  applyPreviewWidth();
  refitActive();
});
previewGrip.addEventListener("pointerdown", (e) => {
  e.preventDefault();
  const startX = e.clientX;
  const startWidth = previewWidth > 0 ? previewWidth : defaultPreviewWidth(window.innerWidth);
  const move = (ev: PointerEvent): void => {
    previewWidth = clampPreviewWidth(startWidth + (startX - ev.clientX), window.innerWidth);
    applyPreviewWidth();
  };
  const up = (): void => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    refitActive();
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
});

/* ---------------- quit guard + shortcuts ---------------- */

if (IN_TAURI) {
  // Flush the layout before the window closes, not just on the 400 ms
  // debounce, then confirm if a process is still running. The busy check
  // lives here rather than in beforeunload: WebKitGTK cancels a close
  // whenever a beforeunload listener exists, and Tauri windows have no
  // native beforeunload dialog to accept it.
  void getCurrentWindow()
    .onCloseRequested(async (event) => {
      await saveNow();
      const ids = store.workspaces.flatMap((w) => w.tabs.map((t) => t.id));
      if (!anyTabBusy(activity, ids, Date.now())) return;
      try {
        const { confirm } = await import("@tauri-apps/plugin-dialog");
        const close = await confirm("A process is still running. Close muis anyway?", {
          title: "Close muis",
          kind: "warning",
        });
        if (!close) event.preventDefault();
      } catch {
        // No dialog available: keep the window open rather than kill a run.
        event.preventDefault();
      }
    })
    .catch(() => {});
}

if (IS_SERVE) {
  // Closing the browser tab closes the "window": flush the layout before the
  // server tears the ptys down. A page cannot await inside beforeunload, so
  // this is the same best-effort path the 400 ms debounce uses.
  window.addEventListener("beforeunload", () => {
    void saveNow();
  });
}

window.addEventListener("resize", () => {
  // Keep a drag-resized pane inside the new window width before fitting.
  if (previewOpen) applyPreviewWidth();
  refitActive();
});

const onShortcut = (e: KeyboardEvent): void => {
  const target = e.target as HTMLElement | null;
  if (target === titleSearch) return; // search box handles its own keys
  const ws = store.currentWorkspace();
  if (!ws) return;
  const action = resolveShortcut(e, cfg.keybindings);
  if (!action) return;
  switch (action.type) {
    case "new-tab":
      e.preventDefault();
      addShellTab(ws);
      break;
    case "close-tab": {
      e.preventDefault();
      const active = ws.tabs[ws.active];
      if (active) closeTab(ws.id, active.id);
      break;
    }
    case "focus-search":
      e.preventDefault();
      titleSearch.focus();
      titleSearch.select();
      break;
    case "focus-terminal": {
      e.preventDefault();
      const active = ws.tabs[ws.active];
      if (active) views.get(active.id)?.term.focus();
      break;
    }
    case "open-settings":
      e.preventDefault();
      openSettings();
      break;
    case "command-palette":
      e.preventDefault();
      openPalette();
      break;
    case "switch-tab":
      if (action.index < ws.tabs.length) {
        e.preventDefault();
        ws.active = action.index;
        renderAll();
      }
      break;
    case "cycle-tab": {
      e.preventDefault();
      const n = ws.tabs.length;
      if (n > 0) {
        ws.active = (ws.active + action.delta + n) % n;
        renderAll();
      }
      break;
    }
    case "cycle-session": {
      e.preventDefault();
      const n = store.workspaces.length;
      if (n > 0) {
        store.switch((store.current + action.delta + n) % n);
        renderAll();
        focusActiveTerminal();
      }
      break;
    }
    case "toggle-preview":
      e.preventDefault();
      togglePreview();
      break;
    case "focus-preview":
      e.preventDefault();
      focusPreview();
      break;
    case "close-overlay":
      if (sessionOverlay.style.display !== "none") closeSessionDialog();
      else if (settingsOverlay.style.display !== "none") closeSettings();
      break;
  }
};
// Capture phase so the focused terminal cannot swallow these chords first.
window.addEventListener("keydown", onShortcut, true);

/* ---------------- settings dialog ---------------- */

const settingsOverlay = document.createElement("div");
settingsOverlay.className = "settings-overlay";
settingsOverlay.style.display = "none";
const settingsBox = document.createElement("div");
settingsBox.className = "settings-box";
const settingsTitle = document.createElement("h2");
settingsTitle.textContent = "Settings";
const settingsClose = document.createElement("button");
settingsClose.type = "button";
settingsClose.className = "settings-close";
settingsClose.title = "Close";
settingsClose.textContent = "✕";
const settingsHeader = document.createElement("div");
settingsHeader.className = "settings-header";
settingsHeader.append(settingsTitle, settingsClose);
const settingsBody = document.createElement("div");
settingsBody.className = "settings-body";

function settingsRow(label: string, input: HTMLElement): HTMLElement {
  const row = document.createElement("label");
  row.className = "settings-row";
  const span = document.createElement("span");
  span.textContent = label;
  row.append(span, input);
  return row;
}

/** A labelled group of settings rows. */
function settingsSection(title: string, rows: HTMLElement[]): HTMLElement {
  const section = document.createElement("section");
  section.className = "settings-section";
  const h = document.createElement("h3");
  h.className = "settings-section-title";
  h.textContent = title;
  section.append(h, ...rows);
  return section;
}

/** A row whose control sits on its own line (e.g. a checkbox grid). */
function settingsBlock(label: string, input: HTMLElement): HTMLElement {
  const row = document.createElement("div");
  row.className = "settings-row settings-row-block";
  const span = document.createElement("span");
  span.textContent = label;
  row.append(span, input);
  return row;
}

const optSessions = document.createElement("input");
optSessions.type = "checkbox";
const optFontSize = document.createElement("input");
optFontSize.type = "number";
optFontSize.min = "6";
optFontSize.max = "32";
optFontSize.placeholder = "System";
optFontSize.title = "Empty = default size";
const optTheme = document.createElement("select");
for (const name of themeNames()) {
  const o = document.createElement("option");
  o.value = name;
  o.textContent = name;
  optTheme.append(o);
}
const optScrollback = document.createElement("input");
optScrollback.type = "number";
optScrollback.min = "1000";
optScrollback.max = "500000";
optScrollback.step = "1000";
optScrollback.title = "Terminal scrollback lines (1000-500000)";
const optFontFamily = document.createElement("input");
optFontFamily.type = "text";
optFontFamily.placeholder = "Default";
optFontFamily.title = "Terminal font family; empty = default";
const optCursorStyle = document.createElement("select");
for (const s of ["block", "bar", "underline"]) {
  const o = document.createElement("option");
  o.value = s;
  o.textContent = s;
  optCursorStyle.append(o);
}
const optCursorBlink = document.createElement("input");
optCursorBlink.type = "checkbox";
const optBell = document.createElement("select");
for (const s of ["none", "visual", "audible", "both"]) {
  const o = document.createElement("option");
  o.value = s;
  o.textContent = s;
  optBell.append(o);
}
const SEGMENTS: [string, string][] = [
  ["cwd", "cwd"],
  ["git", "git"],
  ["clock", "clock"],
];
const segBox = document.createElement("div");
segBox.className = "seg-opts";
const segOpts: Record<string, HTMLInputElement> = {};
for (const [id, label] of SEGMENTS) {
  const wrap = document.createElement("label");
  wrap.className = "seg-opt";
  const cb = document.createElement("input");
  cb.type = "checkbox";
  segOpts[id] = cb;
  wrap.append(cb, document.createTextNode(label));
  segBox.append(wrap);
}

const settingsButtons = document.createElement("div");
settingsButtons.className = "settings-buttons";
const settingsSave = document.createElement("button");
settingsSave.textContent = "Save";
settingsSave.classList.add("primary");
const settingsCancel = document.createElement("button");
settingsCancel.textContent = "Cancel";
settingsButtons.append(settingsCancel, settingsSave);
settingsBox.append(settingsHeader, settingsBody, settingsButtons);
settingsBody.append(
  settingsSection("Appearance", [
    settingsRow("Show sessions panel", optSessions),
    settingsRow("Theme", optTheme),
    settingsRow("Font family", optFontFamily),
    settingsRow("Font size", optFontSize),
    settingsRow("Cursor style", optCursorStyle),
    settingsRow("Cursor blink", optCursorBlink),
    settingsRow("Bell", optBell),
  ]),
  settingsSection("Terminal", [
    settingsRow("Scrollback lines", optScrollback),
  ]),
  settingsSection("Statusbar", [
    settingsBlock("Visible segments", segBox),
  ]),
);
settingsOverlay.append(settingsBox);
document.body.append(settingsOverlay);

function openSettings(): void {
  optSessions.checked = cfg.showSessions;
  optFontSize.value = cfg.fontSize?.toString() ?? "";
  optTheme.value = cfg.theme ?? "default";
  optScrollback.value = cfg.scrollback.toString();
  optFontFamily.value = cfg.fontFamily ?? "";
  optCursorStyle.value = cfg.cursorStyle;
  optCursorBlink.checked = cfg.cursorBlink;
  optBell.value = cfg.bell;
  for (const [id, cb] of Object.entries(segOpts)) {
    cb.checked = segmentVisible(cfg.statusbar, id);
  }
  settingsOverlay.style.display = "flex";
}

function closeSettings(): void {
  settingsOverlay.style.display = "none";
}

async function applySettings(): Promise<void> {
  const size = optFontSize.value.trim();
  cfg = {
    showSessions: optSessions.checked,
    fontSize: size === "" ? null : Math.max(6, Math.min(32, Math.floor(Number(size)) || 0)) || null,
    theme: optTheme.value === "default" ? null : optTheme.value,
    scrollback: clampScrollback(Number(optScrollback.value)),
    keybindings: cfg.keybindings,
    copyOnSelect: cfg.copyOnSelect,
    middleClickPaste: cfg.middleClickPaste,
    fontFamily: optFontFamily.value.trim() || null,
    cursorStyle: optCursorStyle.value as AppConfig["cursorStyle"],
    cursorBlink: optCursorBlink.checked,
    bell: optBell.value as AppConfig["bell"],
    statusbar: Object.fromEntries(
      Object.entries(segOpts).map(([id, cb]) => [id, cb.checked]),
    ),
    workspaceThemes: cfg.workspaceThemes,
  };
  if (BACKEND) void shellCall("config_save", { json: JSON.stringify(cfg) }).catch(() => {});
  applyTheme(cfg.theme);
  const nextTheme = xtermTheme(cfg.theme);
  const px = effectiveFontSize(cfg);
  for (const [, view] of views) {
    view.term.options.fontSize = px;
    view.term.options.theme = nextTheme;
    view.term.options.scrollback = cfg.scrollback;
    view.term.options.fontFamily = cfg.fontFamily ?? MUIS_THEME.termFont;
    view.term.options.cursorStyle = cfg.cursorStyle;
    view.term.options.cursorBlink = cfg.cursorBlink;
  }
  appliedTheme = null; // settings may have changed the theme
  closeSettings();
  renderAll();
}

settingsSave.addEventListener("click", () => void applySettings());
settingsCancel.addEventListener("click", closeSettings);
settingsClose.addEventListener("click", closeSettings);
settingsOverlay.addEventListener("click", (e) => {
  if (e.target === settingsOverlay) closeSettings();
});

/* ---------------- command palette ---------------- */

interface PaletteItem extends Command {
  run: () => void;
}

const paletteOverlay = document.createElement("div");
paletteOverlay.className = "palette-overlay";
paletteOverlay.style.display = "none";
const paletteBox = document.createElement("div");
paletteBox.className = "palette-box";
const paletteInput = document.createElement("input");
paletteInput.className = "palette-input";
paletteInput.placeholder = "Type a command…";
paletteInput.autocomplete = "off";
paletteInput.spellcheck = false;
const paletteList = document.createElement("div");
paletteList.className = "palette-list";
paletteBox.append(paletteInput, paletteList);
paletteOverlay.append(paletteBox);
document.body.append(paletteOverlay);

let paletteItems: PaletteItem[] = [];
let paletteMatches: PaletteItem[] = [];
let paletteIndex = 0;

function renderPalette(): void {
  paletteMatches = filterCommands(paletteInput.value, paletteItems);
  paletteIndex = clampIndex(paletteIndex, paletteMatches.length);
  paletteList.innerHTML = "";
  paletteMatches.forEach((c, i) => {
    const row = document.createElement("div");
    row.className = "palette-item" + (i === paletteIndex ? " on" : "");
    row.textContent = c.title;
    row.addEventListener("mousedown", (e) => {
      e.preventDefault();
      runPalette(i);
    });
    paletteList.append(row);
  });
}

function runPalette(i: number): void {
  const item = paletteMatches[i];
  closePalette();
  if (item) item.run();
}

function closePalette(): void {
  paletteOverlay.style.display = "none";
}

function openPalette(): void {
  const ws = store.currentWorkspace();
  const active = ws?.tabs[ws.active];
  paletteItems = [];
  const add = (id: string, title: string, run: () => void): void => {
    paletteItems.push({ id, title, run });
  };
  if (ws) {
    add("new-tab", "New terminal tab", () => addShellTab(ws));
    if (active) add("close-tab", "Close tab", () => closeTab(ws.id, active.id));
    if (ws.tabs.length > 1) {
      add("next-tab", "Next tab", () => {
        ws.active = (ws.active + 1) % ws.tabs.length;
        renderAll();
      });
      add("prev-tab", "Previous tab", () => {
        ws.active = (ws.active - 1 + ws.tabs.length) % ws.tabs.length;
        renderAll();
      });
    }
    if (store.workspaces.length > 1) {
      add("next-session", "Next session", () => {
        store.switch((store.current + 1) % store.workspaces.length);
        renderAll();
        window.setTimeout(focusActiveTerminal, 0);
      });
      add("prev-session", "Previous session", () => {
        store.switch((store.current - 1 + store.workspaces.length) % store.workspaces.length);
        renderAll();
        window.setTimeout(focusActiveTerminal, 0);
      });
    }
    if (active) {
      add("clear-scrollback", "Clear scrollback", () => clearScrollback(active.id));
      add("focus-terminal", "Focus terminal", () => views.get(active.id)?.term.focus());
    }
    add("close-session", "Close session", () => closeSession(ws));
  }
  add("settings", "Open settings", () => openSettings());
  add("toggle-preview", previewOpen ? "Hide browser pane" : "Show browser pane", () =>
    togglePreview(),
  );
  if (previewOpen) {
    add("focus-preview", "Focus browser address bar", () => focusPreview());
  }
  add("search", "Search", () => {
    titleSearch.focus();
    titleSearch.select();
  });
  paletteInput.value = "";
  paletteIndex = 0;
  paletteOverlay.style.display = "flex";
  renderPalette();
  paletteInput.focus();
}

paletteInput.addEventListener("input", () => {
  paletteIndex = 0;
  renderPalette();
});
paletteInput.addEventListener("keydown", (e) => {
  if (e.key === "ArrowDown") {
    e.preventDefault();
    paletteIndex = clampIndex(paletteIndex + 1, paletteMatches.length);
    renderPalette();
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    paletteIndex = clampIndex(paletteIndex - 1, paletteMatches.length);
    renderPalette();
  } else if (e.key === "Enter") {
    e.preventDefault();
    runPalette(paletteIndex);
  } else if (e.key === "Escape") {
    e.preventDefault();
    closePalette();
  }
});
paletteOverlay.addEventListener("click", (e) => {
  if (e.target === paletteOverlay) closePalette();
});

/* ---------------- diagnosis hook ---------------- */

/**
 * Headless diagnosis hook: window.__muisDebug() reports terminal
 * geometry and any window errors as JSON. Used with WebKit2.WebView
 * when the Tauri window itself can't be inspected.
 */
function installDebugHook(): void {
  const w = window as unknown as {
    __muisErrors?: string[];
    __muisDebug?: () => unknown;
  };
  w.__muisErrors = [];
  window.addEventListener("error", (e) => {
    w.__muisErrors?.push(String(e.message));
  });
  w.__muisDebug = () => {
    const boxes = [...document.querySelectorAll(".tabbox")].map((b) => {
      const bEl = b as HTMLElement;
      const r = bEl.getBoundingClientRect();
      const cs = getComputedStyle(bEl);
      return { active: bEl.classList.contains("active"), w: r.width, h: r.height, display: cs.display };
    });
    const tabs = [...document.querySelectorAll("#tabbar .tab")].map((t) => ({
      cls: (t as HTMLElement).className,
      text: (t as HTMLElement).innerText.slice(0, 20),
    }));
    const termRect = termWrap.getBoundingClientRect();
    return {
      href: location.href,
      errors: w.__muisErrors ?? [],
      views: [...views.entries()].map(([id, v]) => ({ id, cols: v.term.cols, rows: v.term.rows })),
      boxes,
      tabs,
      store: { workspaces: store.workspaces, current: store.current },
      termRect: { w: termRect.width, h: termRect.height },
      fonts: document.fonts?.status ?? "unknown",
    };
  };
  // In the Tauri window there are no devtools: phone the snapshot home.
  if (BACKEND) {
    setTimeout(() => {
      try {
        const snap = JSON.stringify((w.__muisDebug as () => unknown)());
        void shellCall("debug_report", { json: snap }).catch(() => {});
      } catch {
        /* ignore */
      }
    }, 3000);
  }
}

/* ---------------- last-command bar ---------------- */

const commandTracker = new CommandTracker();
const doneTabs = new DoneTracker();
const notifyRouter = new NotifyRouter();
const idleFallback = new Map<string, number>();

/** A tab is "done" when a finished command or pty exit needs attention. */
function isTabDone(tabId: string): boolean {
  return doneTabs.has(tabId) || activity.exited.has(tabId);
}

/** One tab's alert state, collapsed for the chrome. */
function tabAttention(tabId: string): Attention {
  if (doneTabs.isUnseen(tabId)) return "unseen";
  if (doneTabs.has(tabId) || activity.exited.has(tabId)) return "seen";
  return null;
}

function isTabVisible(tabId: string): boolean {
  const ws = store.currentWorkspace();
  return ws?.tabs[ws.active]?.id === tabId;
}

function renderCommandHead(tabId: string): void {
  const view = views.get(tabId);
  if (!view) return;
  const head = view.box.querySelector<HTMLElement>(".pane-head");
  if (!head) return;
  const st = commandTracker.state(tabId);
  head.innerHTML = "";
  if (st.running) {
    head.append(el("span", "ok", "⏺"), el("span", "cmd-full", st.lastCmd ?? "…"));
    head.append(el("span", "dim exit", "running"));
    return;
  }
  if (!st.lastCmd) {
    head.append(el("span", "dim", `${shellBaseName() || "shell"} — ready`));
    return;
  }
  const ok = st.lastExit === 0 || st.lastExit === null;
  head.append(el("span", ok ? "ok" : "fail", ok ? "✓" : "✗"), el("span", "cmd-full", st.lastCmd));
  const bits: string[] = [];
  if (st.lastExit !== null) bits.push(`exit ${st.lastExit}`);
  if (st.lastMs !== null) bits.push(`${st.lastMs}ms`);
  if (bits.length) head.append(el("span", "dim exit", bits.join(" · ")));
}

/** Read the command from the on-screen line (fallback when keys weren't captured). */
function currentInputLine(tabId: string): string {
  const view = views.get(tabId);
  if (!view) return "";
  const buf = view.term.buffer.active;
  const line = buf.getLine(buf.baseY + buf.cursorY);
  const raw = line ? line.translateToString(true) : "";
  const marker = raw.lastIndexOf("❯");
  return (marker >= 0 ? raw.slice(marker + 1) : raw).trim();
}

/** With no OSC 133 (non-fish shells), infer completion from output settling. */
function armIdleFallback(tabId: string): void {
  const st = commandTracker.state(tabId);
  if (!st.running || st.sawOsc) return;
  window.clearTimeout(idleFallback.get(tabId));
  idleFallback.set(
    tabId,
    window.setTimeout(() => {
      const s = commandTracker.state(tabId);
      if (s.running && !s.sawOsc) finishCommand(tabId, null);
    }, 500),
  );
}

/**
 * Record keystrokes for the command bar. `fromUser` is false for lines muis
 * types itself (the debug hook), which must not count as the user resuming
 * work: the restore path relies on that rule.
 */
function trackInput(tabId: string, data: string, fromUser = true): void {
  if (!commandTracker.onInput(tabId, data)) return;
  renderCommandHead(tabId);
  armIdleFallback(tabId);
  // Submitting a line is the user picking the work back up. Shells without
  // OSC 133 markers (cmd and PowerShell — every Windows default) never report
  // a command start, so without this a waiting mark would stay lit until the
  // tab was closed. It also refreshes the tab label on submit rather than only
  // once the command ends.
  if (fromUser && doneTabs.has(tabId)) {
    doneTabs.clear(tabId);
    reportDebugStage("attention-cleared", { tab: tabId, reason: "input" });
  }
  renderTabs();
}

function finishCommand(tabId: string, exit: number | null): void {
  const finished = commandTracker.state(tabId);
  commandTracker.onCmdEnd(tabId, exit);
  // A shell-reported end for opencode means the TUI is gone: forget the
  // session so a restart does not resurrect a tab the user left at a prompt.
  if (shouldClearAgentOnCommandEnd(finished.sawOsc, finished.lastCmd)) {
    clearAgentSession(tabId);
  }
  window.clearTimeout(idleFallback.get(tabId));
  idleFallback.delete(tabId);
  renderCommandHead(tabId);
  // The tab now shows (and colors by) the command that just ran.
  renderTabs();
  maybeNotifyFinished(tabId);
  if (!isTabVisible(tabId)) {
    doneTabs.mark(tabId);
    renderSessions();
    updateWindowTitle();
    }
}

/**
 * Toast when a long command ended while the window was unfocused. Skipped
 * when an agent already notified for the same run, so there is never a
 * double toast.
 */
function maybeNotifyFinished(tabId: string): void {
  const st = commandTracker.state(tabId);
  const startedAt = st.lastMs === null ? null : Date.now() - st.lastMs;
  const agentNotified =
    startedAt !== null && (notifyRouter.lastAt(tabId) ?? 0) >= startedAt;
  if (
    !shouldToastOnFinish({
      durationMs: st.lastMs,
      sawOsc: st.sawOsc,
      windowFocused,
      agentNotified,
    })
  ) {
    return;
  }
  const ws = store.workspaces.find((w) => w.tabs.some((t) => t.id === tabId));
  const tab = ws?.tabs.find((t) => t.id === tabId);
  if (!tab) return;
  const label = tabLabelOf(tab.title, tab.manual, st.lastCmd);
  applyNotification(
    tab,
    notifyEventFromCommand(label, `${st.lastCmd ?? "command"} finished`),
  );
}

function forgetTabState(tabId: string): void {
  commandTracker.forget(tabId);
  doneTabs.forget(tabId);
  restoreCommands.delete(tabId);
  notifyRouter.forget(tabId);
  window.clearTimeout(idleFallback.get(tabId));
  idleFallback.delete(tabId);
}

/**
 * Surface one notification from a tab. Returns true when the chrome
 * changed (the tab got a waiting mark) so callers can re-render.
 */
function applyNotification(tab: Tab, ev: NotifyEvent): boolean {
  reportDebugStage("notify-received", { source: ev.source });
  const decision = notifyRouter.route(
    tab.id,
    ev,
    { visible: isTabVisible(tab.id), windowFocused },
    Date.now(),
  );
  if (!decision) return false;
  reportDebugStage("notify-routed", {
    source: ev.source,
    attention: decision.attention,
    toast: decision.toast,
  });
  if (decision.toast) {
    void sendDesktopNotification(ev.title ?? tab.title ?? "muis", ev.body);
  }
  if (!decision.attention) return false;
  doneTabs.mark(tab.id, decision.attention);
  reportDebugStage("attention-marked", {
    tab: tab.id,
    state: decision.attention,
    reason: "notification",
  });
  return true;
}

/** A `muis-agent session` report forwarded by the shell. */
function handleAgentSession(req: CliAgentSession): void {
  if (!req?.tab_id) return;
  if (applyAgentReport(store.workspaces, req.tab_id, req.session, req.directory) !== "ignored") {
    scheduleSave();
  }
}

/**
 * The agent in a tab picked the work back up (opencode started a turn or the
 * user's prompt was admitted), so the tab is no longer waiting. Looking at a
 * tab does not clear the mark; replying does.
 */
function handleAgentActivity(req: CliAgentActivity): void {
  if (req.state !== "running") return;
  if (!req.tab_id && !req.session) return;
  const tab = tabForReport(store.workspaces, req.tab_id, req.session);
  if (!tab || !doneTabs.has(tab.id)) return;
  doneTabs.clear(tab.id);
  reportDebugStage("attention-cleared", { tab: tab.id, reason: "resumed" });
  renderTabs();
  renderSessions();
  updateWindowTitle();
}

/** Forget a tab's agent session (opencode left the session or exited). */
function clearAgentSession(tabId: string): void {
  if (applyAgentReport(store.workspaces, tabId, null, null) !== "ignored") {
    scheduleSave();
  }
}

/** A `muis-notify` request forwarded by the shell, routed to its tab. */
function handleNotifyRequest(req: CliNotify): void {
  if (!req?.tab_id && !req?.session) return;
  const tab = tabForReport(store.workspaces, req.tab_id, req.session);
  if (!tab) return;
  if (applyNotification(tab, notifyEventFromCli(req))) {
    renderTabs();
    renderSessions();
    renderStatusbar();
    updateWindowTitle();
    }
}

/* ---------------- cross-session search ---------------- */

let searchScope: SearchScope = "all";

function hideSearchResults(): void {
  searchResults.style.display = "none";
  searchResults.innerHTML = "";
}

/** Snapshot the open tabs and their visible scrollback for matching. */
function searchableTabs(): SearchableTab[] {
  const out: SearchableTab[] = [];
  store.workspaces.forEach((w, wsIndex) => {
    w.tabs.forEach((t, ti) => {
      const view = views.get(t.id);
      const lines: string[] = [];
      if (view) {
        const buf = view.term.buffer.active;
        for (let i = 0; i < buf.length; i++) {
          const line = buf.getLine(i);
          lines.push(line ? line.translateToString(true) : "");
        }
      }
      out.push({
        wsIndex,
        tabId: t.id,
        wsName: w.name,
        title: t.title,
        active: wsIndex === store.current && ti === w.active,
        lines,
      });
    });
  });
  return out;
}

function renderSearchResults(): void {
  const q = titleSearch.value.trim();
  if (q.length < 2) {
    hideSearchResults();
    return;
  }
  const { total, items } = collectMatches(q, searchScope, store.current, searchableTabs(), 50, {
    regex: searchRegex,
    caseSensitive: searchCase,
  });
  searchResults.innerHTML = "";
  const head = el("div", "r-head");
  head.append(el("span", "", `${total} match${total === 1 ? "" : "es"}`));
  const scopes = el("span", "r-scopes");
  for (const [value, label] of [
    ["all", "Everywhere"],
    ["session", "Session"],
    ["tab", "Tab"],
  ] as const) {
    const b = el("button", "r-scope" + (searchScope === value ? " on" : ""), label);
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      searchScope = value;
      renderSearchResults();
      titleSearch.focus();
    });
    scopes.append(b);
  }
  head.append(scopes);
  searchResults.append(head);
  if (total === 0) {
    searchResults.append(el("div", "r-empty", `no matches for “${q}”`));
  } else {
    for (const hit of items) {
      const item = el("div", "r-item");
      item.append(el("div", "r-crumb", hit.crumb), el("div", "r-line", hit.text));
      item.addEventListener("click", () => jumpToResult(hit, q));
      searchResults.append(item);
    }
  }
  searchResults.style.display = "block";
}

function jumpToResult(hit: SearchHit, q: string): void {
  searchHistory.add(q);
  historyIndex = -1;
  store.switch(hit.wsIndex);
  const ws = store.currentWorkspace();
  const ti = ws?.tabs.findIndex((t) => t.id === hit.tabId) ?? -1;
  if (ws && ti >= 0) ws.active = ti;
  hideSearchResults();
  titleSearch.blur();
  renderAll();
  // Focus on the next tick: the Enter that picked this result must not
  // land in xterm (which would run the typed line).
  window.setTimeout(() => {
    const view = views.get(hit.tabId);
    if (!view) return;
    try {
      view.search.findNext(q);
    } catch {
      /* no match in the xterm buffer */
    }
    view.term.focus();
  }, 0);
}

/* ---------------- context menu ---------------- */

const ctxMenu = document.createElement("div");
ctxMenu.className = "ctxmenu";
ctxMenu.style.display = "none";
document.body.append(ctxMenu);

function closeCtx(): void {
  ctxMenu.style.display = "none";
  ctxMenu.innerHTML = "";
}

function openCtx(
  x: number,
  y: number,
  items: { label: string; hint?: string; fn: () => void }[],
): void {
  ctxMenu.innerHTML = "";
  for (const it of items) {
    const d = el("div", "ctx-item");
    d.append(el("span", "ctx-label", it.label));
    if (it.hint) d.append(el("span", "ctx-hint", it.hint));
    d.addEventListener("click", (e) => {
      e.stopPropagation();
      closeCtx();
      it.fn();
    });
    ctxMenu.append(d);
  }
  ctxMenu.style.display = "block";
  const w = ctxMenu.offsetWidth || 200;
  const h = ctxMenu.offsetHeight || 120;
  ctxMenu.style.left = `${Math.min(x, window.innerWidth - w - 8)}px`;
  ctxMenu.style.top = `${Math.min(y, window.innerHeight - h - 8)}px`;
}

document.addEventListener("contextmenu", (e) => {
  const target = e.target as HTMLElement;
  const sEl = target.closest<HTMLElement>(".session");
  const tEl = sEl ? null : target.closest<HTMLElement>(".tab");
  if (sEl) {
    e.preventDefault();
    const w = store.workspaces[Number(sEl.dataset.i)];
    if (w) {
      openCtx(e.clientX, e.clientY, [
        { label: "Rename session", fn: () => renameSession(w) },
        { label: "Close session", fn: () => closeSession(w) },
      ]);
    }
  } else if (tEl) {
    e.preventDefault();
    const ws = store.currentWorkspace();
    const tab = ws?.tabs[Number(tEl.dataset.i)];
    if (ws && tab) {
      openCtx(e.clientX, e.clientY, [
        { label: "Rename tab", hint: tab.manual ? "manual" : "auto", fn: () => renameTab(ws, tab) },
        {
          label: tab.manual ? "Use automatic title" : "Freeze current title",
          fn: () => toggleFreezeTitle(ws, tab),
        },
        { label: "Clear scrollback", fn: () => clearScrollback(tab.id) },
        { label: "Close tab", fn: () => closeTab(ws.id, tab.id) },
      ]);
    }
  } else {
    closeCtx();
  }
});

document.addEventListener("click", (e) => {
  const target = e.target as HTMLElement;
  if (!searchBox.contains(target)) hideSearchResults();
  if (!ctxMenu.contains(target)) closeCtx();
});

/* ---------------- window title ---------------- */

function updateWindowTitle(): void {
  const n = doneTabs.count();
  const base = n > 0 ? `● (${n}) waiting — muis` : `muis — ${store.currentWorkspace()?.name ?? ""}`;
  document.title = testModeActive ? `[test] ${base}` : base;
}

/* ---------------- agent restore ---------------- */

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

/** Resolve when the pty has produced output, or after a timeout. Typing a
 *  restore command before the shell is ready loses it, so wait for the
 *  prompt instead of guessing with a fixed delay. */
function waitForTabOutput(tabId: string, timeoutMs: number): Promise<void> {
  if (sawOutput.has(tabId)) return Promise.resolve();
  return new Promise((resolve) => {
    let done = false;
    const finish = (): void => {
      if (done) return;
      done = true;
      window.clearTimeout(timer);
      outputWaiters.delete(tabId);
      resolve();
    };
    const timer = window.setTimeout(finish, timeoutMs);
    outputWaiters.set(tabId, finish);
  });
}

/** opencode's session list for a directory, fetched at most once a launch. */
function opencodeSessions(cwd: string): Promise<AgentSessionInfo[]> {
  if (!BACKEND) return Promise.resolve([]);
  let pending = opencodeSessionsCache.get(cwd);
  if (!pending) {
    pending = shellCall<AgentSessionInfo[]>("opencode_sessions", { cwd }).catch(() => []);
    opencodeSessionsCache.set(cwd, pending);
  }
  return pending;
}

/** Relaunch opencode in one restored tab, exactly once. */
async function restoreAgentTab(entry: RestoreEntry): Promise<void> {
  if (!BACKEND || testModeActive || agentLaunched.has(entry.tabId)) return;
  agentLaunched.add(entry.tabId);
  const ws = store.workspaces.find((w) => w.tabs.some((t) => t.id === entry.tabId));
  const tab = ws?.tabs.find((t) => t.id === entry.tabId);
  if (!ws || !tab) return;

  ensureView(ws.id, tab);
  const sessions = await opencodeSessions(tab.cwd);
  const id = resolveRestoreSession(tab, sessions);
  if (!id || !/^ses_[A-Za-z0-9]+$/.test(id)) return; // deleted/unknown session
  // Keep the resolved id so a title match becomes an exact id next launch.
  tab.agentSession = id;
  const known = sessions.find((s) => s.id === id);
  if (known?.directory) tab.agentSessionDir = known.directory;
  scheduleSave();

  // Wait for the shell prompt before typing into the pty.
  await waitForTabOutput(tab.id, 1500);
  await delay(150);
  if (!views.has(tab.id)) return;
  const resume = opencodeResume(id);
  restoreCommands.set(tab.id, resume);
  await client.write(ws.id, tab.id, enc.encode(`${resume}\n`)).catch(() => {});
  // A resumed session is sitting at its prompt waiting for the user, exactly
  // like a tab that just finished a turn: mark it as seen so the restored
  // window still says where the work was left off.
  doneTabs.mark(tab.id, "seen");
  reportDebugStage("attention-marked", { tab: tab.id, state: "seen", reason: "restore" });
  renderTabs();
  renderSessions();
  updateWindowTitle();
}

const restorePump = new RestorePump({
  start: restoreAgentTab,
  schedule: (fn, ms) => void window.setTimeout(fn, ms),
});

function startRestoredAgents(): void {
  restorePump.push(restoreQueue(store.workspaces, store.current));
}

/* ---------------- init ---------------- */

async function init(): Promise<void> {
  reportDebugStage("init-start");
  let home = "~";
  let resumed = false;
  if (BACKEND) {
    try {
      testModeActive = await shellCall<boolean>("is_test_mode");
      if (testModeActive) testBadge.style.display = "";
      reportDebugStage("test-mode", testModeActive);
    } catch {
      /* the test-mode marker is best-effort */
    }
    try {
      home = await shellCall<string>("default_cwd");
      reportDebugStage("default-cwd-loaded");
    } catch {
      reportDebugStage("default-cwd-failed");
    }
    // Install the opencode notification plugin (idempotent; only written
    // when missing or still ours).
    try {
      await shellCall("install_opencode_plugin");
    } catch {
      /* plugin install is best-effort */
    }
    try {
      const raw = await shellCall<string>("config_load");
      if (raw) cfg = configFromJSON(raw);
      reportDebugStage("config-loaded", Boolean(raw));
    } catch {
      reportDebugStage("config-load-failed");
    }
    // Restore where we left off. A corrupt file starts fresh, never broken.
    try {
      const saved = await shellCall<string>("sessions_load");
      if (saved) {
        store = SessionStore.fromJSON(saved);
        resumed = true;
      }
      reportDebugStage("sessions-loaded", Boolean(saved));
    } catch {
      reportDebugStage("sessions-load-failed");
    }
  }
  homeDir = /^([/\\]|[A-Za-z]:)/.test(home) ? home : "";
  // Only mint a default session on a fresh start. An empty resumed store
  // means the user closed every session and quit that way on purpose.
  if (store.workspaces.length === 0 && !resumed) store.ensureDefault("home", home);
  applyTheme(cfg.theme);
  renderAll();
  startRestoredAgents();
  reportDebugStage("initial-render-complete", { workspaces: store.workspaces.length, views: views.size });
  installDebugHook();
  tickClock();
}

void init();
