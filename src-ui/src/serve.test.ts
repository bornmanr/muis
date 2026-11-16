import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * Browser-mode backend tests. The module reads `window.__MUIS_SERVE__` at
 * import time, so each case stubs a minimal window, resets the module
 * registry, and re-imports.
 */
type ServeModule = typeof import("./serve");

async function load(token: string | null): Promise<ServeModule> {
  vi.resetModules();
  const win: Record<string, unknown> = {
    location: { origin: "http://127.0.0.1:9000" },
    addEventListener: () => {},
  };
  if (token !== null) win.__MUIS_SERVE__ = { token };
  vi.stubGlobal("window", win);
  return import("./serve");
}

/** Minimal WebSocket stand-in: records sends, lets tests drive messages. */
class FakeSocket {
  static last: FakeSocket | null = null;
  static OPEN = 1;
  readyState = 0;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  sent: string[] = [];
  url: string;

  constructor(url: string | URL) {
    this.url = String(url);
    FakeSocket.last = this;
  }

  send(line: string) {
    this.sent.push(line);
  }
}

describe("serve backend", () => {
  beforeEach(() => {
    vi.stubGlobal("fetch", vi.fn());
    vi.stubGlobal("WebSocket", FakeSocket as unknown as typeof WebSocket);
    FakeSocket.last = null;
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("is inactive without the injected token", async () => {
    const serve = await load(null);
    expect(serve.IS_SERVE).toBe(false);
  });

  it("is active when the server injects a token", async () => {
    const serve = await load("abc123");
    expect(serve.IS_SERVE).toBe(true);
  });

  it("posts commands with the token and returns parsed json", async () => {
    const serve = await load("abc123");
    const fetchMock = vi.mocked(fetch);
    fetchMock.mockResolvedValue(new Response(JSON.stringify("pong"), { status: 200 }));

    await expect(serve.rpc<string>("ping")).resolves.toBe("pong");
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(String(url)).toBe("/rpc/ping?t=abc123");
    expect(init?.method).toBe("POST");
    expect(init?.body).toBe("{}");
  });

  it("rejects with the server message on failure", async () => {
    const serve = await load("abc123");
    vi.mocked(fetch).mockResolvedValue(new Response("no worker", { status: 500 }));
    await expect(serve.rpc("worker_spawn", { sessionId: "s1" })).rejects.toThrow("no worker");
  });

  it("treats an empty body as a unit result", async () => {
    const serve = await load("abc123");
    vi.mocked(fetch).mockResolvedValue(new Response("", { status: 200 }));
    await expect(serve.rpc("sessions_save", { json: "{}" })).resolves.toBeNull();
  });

  it("routes events to their subscribers by name", async () => {
    const serve = await load("abc123");
    const seen: unknown[] = [];
    serve.onServeEvent("muis-notify", (payload) => seen.push(payload));
    serve.onServeEvent("muis-agent-session", () => seen.push("agent"));

    new serve.WsTransport();
    const socket = FakeSocket.last!;
    socket.onmessage?.({
      data: JSON.stringify({ event: "muis-notify", payload: { body: "hi" } }),
    });
    expect(seen).toEqual([{ body: "hi" }]);
  });

  it("queues worker frames until the socket is open", async () => {
    const serve = await load("abc123");
    const frames: unknown[] = [];
    const transport = new serve.WsTransport();
    transport.onEvent((sessionId, frame) => frames.push([sessionId, frame]));
    await transport.spawn("s1");

    const socket = FakeSocket.last!;
    expect(String(socket.url)).toContain("/ws?t=abc123");
    expect(socket.sent).toEqual([]); // not open yet

    socket.readyState = 1;
    socket.onopen?.();
    expect(socket.sent).toEqual([JSON.stringify({ op: "spawn", session_id: "s1" })]);

    socket.onmessage?.({
      data: JSON.stringify({ session_id: "s1", frame: { type: "spawned", pty_id: "t1" } }),
    });
    expect(frames).toEqual([["s1", { type: "spawned", pty_id: "t1" }]]);
  });

  it("sends worker payload lines verbatim", async () => {
    const serve = await load("abc123");
    const transport = new serve.WsTransport();
    const socket = FakeSocket.last!;
    socket.readyState = 1;
    socket.onopen?.();
    await transport.send("s1", { type: "write", pty_id: "t1", data_b64: "aGk=" });
    expect(JSON.parse(socket.sent[0]!)).toEqual({
      op: "send",
      session_id: "s1",
      line: JSON.stringify({ type: "write", pty_id: "t1", data_b64: "aGk=" }),
    });
  });
});
