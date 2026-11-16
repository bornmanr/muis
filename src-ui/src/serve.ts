/**
 * Browser-mode backend.
 *
 * Outside Tauri the UI normally echoes (dev preview, Selenium). When the
 * muis-server serves the bundle it injects `window.__MUIS_SERVE__`, and this
 * module becomes the shell: the same commands over `POST /rpc/<name>` and the
 * same worker frames over one WebSocket. Nothing here runs in the Tauri app.
 */

import type { UiToWorker, WorkerToUi } from "./ipc";
import type { Transport } from "./worker";

interface ServeInfo {
  token: string;
}

const injected = (window as unknown as { __MUIS_SERVE__?: ServeInfo }).__MUIS_SERVE__;

/** True when the bundle was served by muis-server rather than by Tauri. */
export const IS_SERVE = typeof injected?.token === "string" && injected.token.length > 0;

function token(): string {
  return injected?.token ?? "";
}

/** Append the run token to a loopback URL. */
function withToken(path: string): string {
  const sep = path.includes("?") ? "&" : "?";
  return `${path}${sep}t=${encodeURIComponent(token())}`;
}

/** Run one shell command. Errors reject with the server's message. */
export async function rpc<T>(
  method: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  const response = await fetch(withToken(`/rpc/${method}`), {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(args),
  });
  const text = await response.text();
  if (!response.ok) {
    throw new Error(text || `rpc ${method} failed (${response.status})`);
  }
  // Unit commands answer with an empty body or `null`.
  return (text ? JSON.parse(text) : null) as T;
}

type ServeEventHandler = (payload: unknown) => void;

const eventHandlers = new Map<string, Set<ServeEventHandler>>();

/** Subscribe to a shell-originated event (`muis-notify`, agent reports). */
export function onServeEvent(name: string, handler: ServeEventHandler): void {
  let handlers = eventHandlers.get(name);
  if (!handlers) {
    handlers = new Set();
    eventHandlers.set(name, handlers);
  }
  handlers.add(handler);
}

function dispatchEvent(frame: { event: string; payload: unknown }): void {
  for (const handler of eventHandlers.get(frame.event) ?? []) handler(frame.payload);
}

/**
 * The worker transport over WebSocket. Server→client frames are the shell's
 * own shapes, so `WorkerClient` routes them unchanged:
 *   `{session_id, frame}`   worker frame (output/exited/error)
 *   `{event, payload}`      CLI notification or agent report
 */
export class WsTransport implements Transport {
  private socket: WebSocket | null = null;
  private queue: string[] = [];
  private handler: ((sessionId: string, frame: WorkerToUi) => void) | null = null;

  constructor() {
    this.connect();
  }

  private connect(): void {
    const url = new URL("/ws", window.location.origin);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    url.searchParams.set("t", token());
    const socket = new WebSocket(url);
    this.socket = socket;

    socket.onopen = () => {
      for (const line of this.queue) socket.send(line);
      this.queue = [];
    };
    socket.onmessage = (message) => this.receive(String(message.data));
    socket.onclose = () => {
      this.socket = null;
      // No auto-reconnect: the pool lives and dies with the connection, so a
      // silently reopened socket would look alive while every pty is gone.
      // A reload is the honest recovery.
      const w = window as unknown as { __muisErrors?: string[] };
      w.__muisErrors ??= [];
      w.__muisErrors.push("serve socket closed");
    };
  }

  private receive(data: string): void {
    let parsed: unknown;
    try {
      parsed = JSON.parse(data);
    } catch {
      return;
    }
    if (!parsed || typeof parsed !== "object") return;
    const frame = parsed as {
      event?: string;
      payload?: unknown;
      session_id?: string;
      frame?: WorkerToUi;
    };
    if (typeof frame.event === "string") {
      dispatchEvent({ event: frame.event, payload: frame.payload });
      return;
    }
    if (typeof frame.session_id === "string" && frame.frame) {
      this.handler?.(frame.session_id, frame.frame);
    }
  }

  private post(payload: Record<string, unknown>): void {
    const line = JSON.stringify(payload);
    if (this.socket?.readyState === WebSocket.OPEN) this.socket.send(line);
    else this.queue.push(line);
  }

  async spawn(sessionId: string): Promise<void> {
    this.post({ op: "spawn", session_id: sessionId });
  }

  async send(sessionId: string, msg: UiToWorker): Promise<void> {
    this.post({ op: "send", session_id: sessionId, line: JSON.stringify(msg) });
  }

  async stop(sessionId: string): Promise<void> {
    this.post({ op: "stop", session_id: sessionId });
  }

  onEvent(cb: (sessionId: string, frame: WorkerToUi) => void): void {
    this.handler = cb;
  }
}

/** Desktop toast through the browser's Notification API. */
export function notifyDesktop(title: string, body: string): void {
  if (typeof Notification === "undefined") return;
  try {
    const fire = () => {
      try {
        new Notification(title || "muis", { body });
      } catch {
        /* notification API refused; the tab badge still shows it */
      }
    };
    if (Notification.permission === "granted") fire();
    else if (Notification.permission !== "denied") {
      void Notification.requestPermission().then((state) => {
        if (state === "granted") fire();
      });
    }
  } catch {
    /* notifications are best-effort */
  }
}

/** One directory in the browser-mode folder picker. */
export interface DirEntry {
  name: string;
  path: string;
}

function parentOf(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  if (cut <= 0) return trimmed;
  const head = trimmed.slice(0, cut);
  // Drive roots (`C:`) keep their slash so the listing stays absolute.
  return /^[A-Za-z]:$/.test(head) ? `${head}\\` : head;
}

/**
 * Directory chooser for browser mode: the server lists folders, because a
 * page cannot open a native dialog. Resolves `null` on cancel.
 */
export function pickDirectoryServe(start: string): Promise<string | null> {
  return new Promise((resolve) => {
    let current = start || "";
    const overlay = document.createElement("div");
    overlay.className = "dirpicker-overlay";
    const box = document.createElement("div");
    box.className = "dirpicker";
    const head = document.createElement("div");
    head.className = "dirpicker-head";
    const pathLabel = document.createElement("div");
    pathLabel.className = "dirpicker-path";
    const upBtn = document.createElement("button");
    upBtn.type = "button";
    upBtn.className = "btn";
    upBtn.textContent = "Up";
    head.append(pathLabel, upBtn);
    const list = document.createElement("div");
    list.className = "dirpicker-list";
    const foot = document.createElement("div");
    foot.className = "dirpicker-foot";
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.className = "btn";
    cancel.textContent = "Cancel";
    const use = document.createElement("button");
    use.type = "button";
    use.className = "btn primary";
    use.textContent = "Use this folder";
    foot.append(cancel, use);
    box.append(head, list, foot);
    overlay.append(box);
    document.body.append(overlay);

    const close = (picked: string | null) => {
      overlay.remove();
      document.removeEventListener("keydown", onKey, true);
      resolve(picked);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        close(null);
      }
    };
    document.addEventListener("keydown", onKey, true);

    const render = async (path: string) => {
      current = path;
      pathLabel.textContent = path;
      upBtn.disabled = !path;
      list.textContent = "";
      let entries: DirEntry[] = [];
      try {
        entries = await rpc<DirEntry[]>("list_dir", { path });
      } catch {
        entries = [];
      }
      if (current !== path) return; // a newer navigation won
      if (entries.length === 0) {
        const empty = document.createElement("div");
        empty.className = "dirpicker-empty";
        empty.textContent = "No subfolders";
        list.append(empty);
      }
      for (const entry of entries) {
        const row = document.createElement("button");
        row.type = "button";
        row.className = "dirpicker-row";
        row.textContent = `📁 ${entry.name}`;
        row.addEventListener("click", () => void render(entry.path));
        list.append(row);
      }
    };

    upBtn.addEventListener("click", () => {
      if (current) void render(parentOf(current));
    });
    use.addEventListener("click", () => close(current || null));
    cancel.addEventListener("click", () => close(null));
    overlay.addEventListener("pointerdown", (event) => {
      if (event.target === overlay) close(null);
    });
    void render(current);
  });
}
