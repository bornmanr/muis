#!/usr/bin/env node
/**
 * muis browser host, JavaScript build.
 *
 * Same job as `crates/muis-server`: serve `src-ui/dist` on loopback and own
 * the ptys. This variant exists for machines where policy refuses to start
 * freshly compiled executables; it runs entirely inside `node.exe`, which is
 * already installed and allowed, so there is nothing to compile.
 *
 *   node muis-server.mjs [--port N] [--no-open] [--testmode] [--root DIR]
 *
 * Protocol is identical to the Rust host, so the frontend cannot tell them
 * apart: `POST /rpc/<command>` mirrors the shell command surface, and one
 * WebSocket per tab carries the same `{session_id, frame}` worker frames.
 */

import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync } from "node:fs";
import fs from "node:fs/promises";
import { createServer } from "node:http";
import { createConnection, createServer as createPipeServer } from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import nodePty from "node-pty";
import { WebSocketServer } from "ws";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const IS_WIN = process.platform === "win32";
const USE = "node muis-server.mjs (no compiled binary)";

/* ------------------------------- options ------------------------------- */

function parseArgs(argv) {
  const options = {
    port: 0,
    open: true,
    testmode: false,
    root: path.resolve(HERE, "..", "src-ui", "dist"),
  };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--port") options.port = Number(argv[++i]);
    else if (arg === "--no-open") options.open = false;
    else if (arg === "--testmode") options.testmode = true;
    else if (arg === "--root") options.root = path.resolve(argv[++i]);
    else if (arg === "-h" || arg === "--help") {
      console.log(
        `${USE}\n\n` +
          "usage: node muis-server.mjs [--port N] [--no-open] [--testmode] [--root DIR]\n\n" +
          "  --port N    listen port (default: the OS picks one)\n" +
          "  --no-open   do not launch a browser\n" +
          "  --testmode  fresh sessions, nothing persisted, separate notify pipe\n" +
          "  --root DIR  frontend bundle directory (default: src-ui/dist)\n",
      );
      process.exit(0);
    } else {
      console.error(`unknown argument: ${arg}\n\n${USE} --help`);
      process.exit(2);
    }
  }
  if (!Number.isInteger(options.port) || options.port < 0 || options.port > 65535) {
    console.error("--port must be a number between 0 and 65535");
    process.exit(2);
  }
  return options;
}

/* -------------------------------- paths -------------------------------- */

function portableDir() {
  if (!IS_WIN) return null;
  // Next to this script, not next to node.exe: the host may be a shared
  // install, and "the folder I put muis in" is what a portable copy means.
  return existsSync(path.join(HERE, "muis.portable")) ? path.join(HERE, "data") : null;
}

function homeOverride() {
  if (process.env.MUIS_HOME) return process.env.MUIS_HOME;
  return portableDir();
}

function configDir() {
  const home = homeOverride();
  if (home) return path.join(home, "config");
  if (IS_WIN && process.env.APPDATA) return path.join(process.env.APPDATA, "muis");
  return path.join(os.homedir(), ".config", "muis");
}

function stateDir() {
  const home = homeOverride();
  if (home) return path.join(home, "state");
  if (IS_WIN) return configDir();
  return path.join(os.homedir(), ".local", "share", "muis");
}

function logDir() {
  if (homeOverride()) return path.join(stateDir(), "logs");
  if (IS_WIN && process.env.LOCALAPPDATA) return path.join(process.env.LOCALAPPDATA, "muis", "logs");
  return path.join(stateDir(), "logs");
}

const LOG_FILE = path.join(logDir(), "muis-server.log");

function log(level, message) {
  const line = `${new Date().toISOString()} ${level.toUpperCase()} ${message}`;
  if (level === "error" || level === "warn") console.error(line);
  else if (process.env.MUIS_LOG === "debug" || level === "info") console.log(line);
  fs.appendFile(LOG_FILE, line + "\n").catch(() => {});
}

/* ------------------------------- helpers ------------------------------- */

function findOnPath(name) {
  const dirs = (process.env.PATH ?? "").split(path.delimiter).filter(Boolean);
  for (const dir of dirs) {
    const full = path.join(dir, name);
    if (existsSync(full)) return full;
  }
  return null;
}

function defaultShell() {
  if (IS_WIN) {
    for (const name of ["pwsh.exe", "powershell.exe", "cmd.exe"]) {
      const found = findOnPath(name);
      if (found) return found;
    }
    return path.join(process.env.SystemRoot ?? "C:\\Windows", "System32", "cmd.exe");
  }
  const shell = process.env.SHELL;
  if (shell && shell.startsWith("/") && existsSync(shell)) return shell;
  for (const candidate of ["/usr/bin/fish", "/bin/bash", "/bin/sh"]) {
    if (existsSync(candidate)) return candidate;
  }
  return "/bin/sh";
}

/** Run a console tool; npm-installed `.cmd` shims need a shell on Windows. */
function runTool(command, args, options = {}) {
  return new Promise((resolve) => {
    const child = spawn(command, args, {
      shell: IS_WIN,
      windowsHide: true,
      ...options,
    });
    let stdout = "";
    let stderr = "";
    const timer = setTimeout(() => child.kill(), options.timeout ?? 15000);
    child.stdout?.on("data", (chunk) => (stdout += chunk));
    child.stderr?.on("data", (chunk) => (stderr += chunk));
    child.on("error", () => {
      clearTimeout(timer);
      resolve({ ok: false, stdout: "", stderr: "" });
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      resolve({ ok: code === 0, stdout, stderr });
    });
  });
}

async function readText(file) {
  try {
    return await fs.readFile(file, "utf8");
  } catch (error) {
    if (error.code === "ENOENT") return "";
    throw error;
  }
}

async function writeJsonFile(file, json) {
  JSON.parse(json); // refuse to write anything that is not JSON
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.writeFile(file, JSON.stringify(JSON.parse(json), null, 2), "utf8");
}

function base64(bytes) {
  return Buffer.from(bytes).toString("base64");
}

/* ------------------------------- opencode ------------------------------ */

const PLUGIN_SRC = path.resolve(HERE, "..", "crates", "muis-shell", "opencode-plugin");

function opencodeConfigBase() {
  if (process.env.XDG_CONFIG_HOME) return process.env.XDG_CONFIG_HOME;
  if (process.env.HOME) return path.join(process.env.HOME, ".config");
  if (process.env.APPDATA) return process.env.APPDATA;
  return path.join(os.homedir(), ".config");
}

function pluginsDir() {
  return path.join(opencodeConfigBase(), "opencode", "plugins");
}

/** Write a plugin file when it is missing or already ours; never clobber. */
async function installPluginFile(target, source, marker) {
  const existing = await readText(target);
  if (existing && !existing.includes(marker)) return false;
  const body = await fs.readFile(source, "utf8");
  await fs.mkdir(path.dirname(target), { recursive: true });
  await fs.writeFile(target, body, "utf8");
  return true;
}

async function installOpencodePlugin() {
  const dir = pluginsDir();
  const server = path.join(dir, "muis.js");
  await installPluginFile(server, path.join(PLUGIN_SRC, "muis.js"), "muis-plugin");
  await installPluginFile(path.join(dir, "muis-tui", "tui.js"), path.join(PLUGIN_SRC, "muis-tui.js"), "muis-tui-plugin");
  await fs.rm(path.join(dir, "muis.mjs"), { force: true }).catch(() => {});
  return server;
}

async function opencodeSessions(cwd) {
  const { ok, stdout } = await runTool(
    "opencode",
    ["session", "list", "--format", "json", "--max-count", "1000"],
    { cwd: cwd && existsSync(cwd) ? cwd : undefined },
  );
  if (!ok || !stdout.trim()) return [];
  try {
    const list = JSON.parse(stdout);
    return Array.isArray(list)
      ? list.map((entry) => ({
          id: String(entry.id ?? ""),
          title: String(entry.title ?? ""),
          directory: String(entry.directory ?? ""),
        }))
      : [];
  } catch {
    return [];
  }
}

/* --------------------------------- rpc --------------------------------- */

async function rpc(method, args, state) {
  switch (method) {
    case "ping":
      return "pong";
    case "worker_available":
      return true;
    case "is_test_mode":
      return state.testmode;
    case "default_shell":
      return defaultShell();
    case "default_cwd":
      return os.homedir();
    case "live_test_command":
      return null;
    case "log_path":
      return LOG_FILE;
    case "debug_report":
      log("debug", `frontend report: ${args?.json ?? ""}`);
      return null;

    case "config_load":
      return readText(path.join(configDir(), "muis.json"));
    case "config_save":
      if (!state.testmode) await writeJsonFile(path.join(configDir(), "muis.json"), args?.json ?? "{}");
      return null;
    case "sessions_load":
      return state.testmode ? "" : readText(path.join(stateDir(), "sessions.json"));
    case "sessions_save":
      if (!state.testmode) await writeJsonFile(path.join(stateDir(), "sessions.json"), args?.json ?? "{}");
      return null;

    case "git_branch": {
      const cwd = args?.cwd;
      const { ok, stdout } = await runTool("git", ["branch", "--show-current"], {
        cwd: cwd && existsSync(cwd) ? cwd : undefined,
        timeout: 5000,
      });
      return ok ? stdout.trim() : "";
    }

    case "opencode_sessions":
      return opencodeSessions(args?.cwd);

    case "install_opencode_plugin":
      return installOpencodePlugin();

    case "list_subdirs": {
      const limit = Math.min(Number(args?.limit ?? 50), 50);
      try {
        const entries = await fs.readdir(args?.path ?? "", { withFileTypes: true });
        return entries
          .filter((entry) => entry.isDirectory() && !entry.name.startsWith(".") && !entry.name.startsWith("$"))
          .map((entry) => entry.name)
          .sort()
          .slice(0, limit);
      } catch {
        return [];
      }
    }

    case "list_dir": {
      try {
        const entries = await fs.readdir(args?.path ?? "", { withFileTypes: true });
        return entries
          .filter((entry) => entry.isDirectory() && !entry.name.startsWith(".") && !entry.name.startsWith("$"))
          .map((entry) => ({ name: entry.name, path: path.join(args.path, entry.name), dir: true }))
          .sort((a, b) => a.name.toLowerCase().localeCompare(b.name.toLowerCase()))
          .slice(0, 500);
      } catch {
        return [];
      }
    }

    case "open_url": {
      const url = String(args?.url ?? "");
      if (!/^https?:\/\//.test(url)) throw new Error("only http(s) URLs can be opened");
      const [, command, commandArgs] = IS_WIN
        ? [null, "cmd", ["/c", "start", "", url]]
        : process.platform === "darwin"
          ? [null, "open", [url]]
          : [null, "xdg-open", [url]];
      spawn(command, commandArgs, { windowsHide: true, detached: true, stdio: "ignore" }).unref();
      return null;
    }

    case "worker_spawn":
    case "worker_send":
    case "worker_stop":
      throw new Error("worker commands are served over /ws in browser mode");

    default:
      throw new Error(`unknown command: ${method}`);
  }
}

/* ------------------------------ websocket ------------------------------ */

async function ensureShims(scriptPath) {
  const dir = path.join(stateDir(), "bin");
  await fs.mkdir(dir, { recursive: true });
  const line = (flags) => `@echo off\r\n"${process.execPath}" "${scriptPath}"${flags} %*\r\n`;
  await fs.writeFile(path.join(dir, "muis.cmd"), line(""), "utf8");
  await fs.writeFile(path.join(dir, "muis-notify.cmd"), line(" --muis-notify"), "utf8");
  await fs.writeFile(path.join(dir, "muis-agent.cmd"), line(" --muis-agent"), "utf8");
  return dir;
}

function createPool(send, socketName, extraEnv = {}) {
  /** session id -> Map(pty id -> pty) */
  const sessions = new Map();

  function session(sessionId) {
    let map = sessions.get(sessionId);
    if (!map) {
      map = new Map();
      sessions.set(sessionId, map);
    }
    return map;
  }

  function emit(sessionId, frame) {
    send({ session_id: sessionId, frame });
  }

  function spawnPty(sessionId, msg) {
    const map = session(sessionId);
    if (map.has(msg.pty_id)) return; // idempotent
    const shell = msg.shell || defaultShell();
    let pty;
    try {
      pty = nodePty.spawn(shell, [], {
        name: "xterm-256color",
        cols: msg.cols || 80,
        rows: msg.rows || 24,
        cwd: msg.cwd || os.homedir(),
        env: {
          ...process.env,
          ...extraEnv,
          TERM: "xterm-256color",
          TERM_PROGRAM: "muis",
          MUIS_TAB_ID: msg.pty_id,
          MUIS_SOCKET: socketName,
        },
      });
    } catch (error) {
      emit(sessionId, { type: "error", pty_id: msg.pty_id, message: `spawn failed: ${error.message}` });
      return;
    }
    map.set(msg.pty_id, pty);
    emit(sessionId, { type: "spawned", pty_id: msg.pty_id });

    pty.onData((data) => emit(sessionId, { type: "output", pty_id: msg.pty_id, data_b64: base64(data) }));
    pty.onExit(({ exitCode }) => {
      map.delete(msg.pty_id);
      emit(sessionId, { type: "exited", pty_id: msg.pty_id, code: exitCode ?? null });
    });
  }

  return {
    frame(text) {
      let message;
      try {
        message = JSON.parse(text);
      } catch {
        return;
      }
      if (message.op === "spawn") {
        session(message.session_id); // a session with no ptys yet
      } else if (message.op === "send") {
        let line;
        try {
          line = JSON.parse(message.line);
        } catch {
          return;
        }
        const map = session(message.session_id);
        if (line.type === "spawn") spawnPty(message.session_id, line);
        else if (line.type === "write") {
          const pty = map.get(line.pty_id);
          if (!pty) return;
          pty.write(Buffer.from(String(line.data_b64 ?? ""), "base64").toString("utf8"));
        } else if (line.type === "resize") {
          const pty = map.get(line.pty_id);
          if (pty && line.cols > 0 && line.rows > 0) pty.resize(line.cols, line.rows);
        } else if (line.type === "kill") {
          map.get(line.pty_id)?.kill();
          map.delete(line.pty_id);
        }
      } else if (message.op === "stop") {
        this.stop(message.session_id);
      }
    },

    stop(sessionId) {
      const map = sessions.get(sessionId);
      if (!map) return;
      for (const pty of map.values()) {
        try {
          pty.kill();
        } catch {
          /* already gone */
        }
      }
      sessions.delete(sessionId);
    },

    stopAll() {
      for (const sessionId of [...sessions.keys()]) this.stop(sessionId);
    },
  };
}

/* --------------------------- CLI helper modes --------------------------- */

const DEFAULT_SOCKET = () => `muis-notify-${process.env.USER ?? process.env.USERNAME ?? "default"}`;

function pipePathFor(name) {
  return IS_WIN ? `\\\\.\\pipe\\${name}` : path.join(os.tmpdir(), `${name}.sock`);
}

async function sendToPipe(socketName, payload) {
  return new Promise((resolve) => {
    const connection = createConnection(pipePathFor(socketName), () => {
      connection.end(JSON.stringify(payload) + "\n");
      resolve(true);
    });
    connection.on("error", () => resolve(false));
    connection.on("data", () => {});
  });
}

/** `--flag value` pairs plus bare flags, mirroring the Rust CLI parsers. */
function parseFlags(argv) {
  const flags = {};
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--running") flags.running = true;
    else if (arg === "--title") flags.title = argv[++i];
    else if (arg === "--body") flags.body = argv[++i];
    else if (arg === "--tab") flags.tab = argv[++i];
    else if (arg === "--socket") flags.socket = argv[++i];
    else if (arg === "--session") flags.session = argv[++i];
    else if (arg === "--urgency") flags.urgency = Number(argv[++i]);
    else if (arg === "--dir") flags.dir = argv[++i];
    else if (arg.startsWith("-")) throw new Error(`unknown argument: ${arg}`);
    else if (flags.command === undefined) flags.command = arg;
    else if (flags.positional === undefined) flags.positional = arg;
    else throw new Error(`unexpected argument: ${arg}`);
  }
  return flags;
}

function tabFrom(flags) {
  return flags.tab ?? process.env.MUIS_TAB_ID ?? undefined;
}

/** `--muis-notify`: the toast/activity client the opencode plugin calls. */
async function notifyMode(argv) {
  const flags = parseFlags(argv);
  const socketName = flags.socket || process.env.MUIS_SOCKET || DEFAULT_SOCKET();
  const payload = flags.running
    ? { kind: "agent-activity", state: "running", tab_id: tabFrom(flags), session: flags.session }
    : {
        ...(flags.title ? { title: flags.title } : {}),
        body: flags.body ?? "",
        ...(tabFrom(flags) ? { tab_id: tabFrom(flags) } : {}),
        ...(flags.urgency !== undefined && !Number.isNaN(flags.urgency) ? { urgency: flags.urgency } : {}),
        ...(flags.session ? { session: flags.session } : {}),
      };
  if (!flags.running && flags.body === undefined) {
    console.error("muis-notify: --body is required");
    return 2;
  }
  return (await sendToPipe(socketName, payload)) ? 0 : 1;
}

/** `--muis-agent`: report which agent session a tab is showing. */
async function agentMode(argv) {
  const flags = parseFlags(argv);
  const socketName = flags.socket || process.env.MUIS_SOCKET || DEFAULT_SOCKET();
  let session;
  if (flags.command === "clear") {
    if (flags.positional !== undefined) {
      console.error("muis-agent: clear takes no session id");
      return 2;
    }
    session = undefined;
  } else if (flags.command === "session") {
    if (!/^ses_[A-Za-z0-9_-]*[A-Za-z0-9]$/.test(flags.positional ?? "")) {
      console.error(`muis-agent: not a session id: ${flags.positional ?? ""}`);
      return 2;
    }
    session = flags.positional;
  } else {
    console.error("usage: muis-agent session <ses_ID> [--dir DIR] [--tab ID] [--socket NAME]");
    return 2;
  }
  const payload = {
    kind: "agent-session",
    ...(session ? { session } : {}),
    ...(flags.dir ? { directory: flags.dir } : {}),
    ...(tabFrom(flags) ? { tab_id: tabFrom(flags) } : {}),
  };
  return (await sendToPipe(socketName, payload)) ? 0 : 1;
}

/* -------------------------------- http --------------------------------- */

const MIME = {
  html: "text/html; charset=utf-8",
  js: "text/javascript; charset=utf-8",
  mjs: "text/javascript; charset=utf-8",
  css: "text/css; charset=utf-8",
  json: "application/json",
  svg: "image/svg+xml",
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  ico: "image/x-icon",
  woff: "font/woff",
  woff2: "font/woff2",
  ttf: "font/ttf",
  map: "application/json",
  wasm: "application/wasm",
};

function contentType(file) {
  return MIME[path.extname(file).slice(1).toLowerCase()] ?? "application/octet-stream";
}

function hostAllowed(host) {
  if (!host) return true;
  const name = host.replace(/:\d+$/, "");
  return ["127.0.0.1", "localhost", "[::1]", "::1"].includes(name);
}

/** Reply with an explicit length, so a loopback client needs no chunk parser. */
function send(res, status, headers, body) {
  const buffer = Buffer.isBuffer(body) ? body : Buffer.from(String(body ?? ""));
  res.writeHead(status, { ...headers, "content-length": buffer.length });
  res.end(buffer);
}

function tokenMatches(expected, given) {
  if (typeof given !== "string" || given.length !== expected.length) return false;
  let diff = 0;
  for (let i = 0; i < expected.length; i++) diff |= expected.charCodeAt(i) ^ given.charCodeAt(i);
  return diff === 0;
}

function requestToken(req, url) {
  return url.searchParams.get("t") ?? req.headers["x-muis-token"];
}

async function main() {
  const argv = process.argv.slice(2);
  // Hidden modes: the opencode plugin and `muis-notify` call this same script
  // through the shims created below, exactly like the single-binary Rust host.
  if (argv[0] === "--muis-notify") process.exit(await notifyMode(argv.slice(1)));
  if (argv[0] === "--muis-agent") process.exit(await agentMode(argv.slice(1)));

  const options = parseArgs(argv);
  const token = randomBytes(16).toString("hex");
  const user = process.env.USER ?? process.env.USERNAME ?? "default";
  const socketName = (() => {
    const base = process.env.MUIS_SOCKET_NAME || `muis-notify-${user}`;
    return options.testmode ? `${base}-testmode` : base;
  })();

  const state = { testmode: options.testmode };
  const clients = new Set();

  // A tab must be able to run `muis-notify --body done`, and the opencode
  // plugin needs `MUIS_BIN` to point at something it can spawn. Windows cannot
  // spawn a `.cmd` without a shell, which the plugin asks for on win32.
  const shimDir = IS_WIN ? await ensureShims(fileURLToPath(import.meta.url)) : null;
  const ptyEnv = shimDir
    ? {
        PATH: `${shimDir}${path.delimiter}${process.env.PATH ?? ""}`,
        PATHEXT: process.env.PATHEXT ?? ".COM;.EXE;.BAT;.CMD",
        MUIS_BIN: path.join(shimDir, "muis.cmd"),
      }
    : {};

  // CLI notifications (`muis-notify`, `muis-agent`, the opencode plugin)
  // reach the host over one local socket and fan out to every open tab.
  const pipePath = IS_WIN
    ? `\\\\.\\pipe\\${socketName}`
    : path.join(os.tmpdir(), `${socketName}.sock`);
  if (!IS_WIN) await fs.rm(pipePath, { force: true }).catch(() => {});
  createPipeServer((connection) => {
    let buffer = "";
    connection.on("data", (chunk) => (buffer += chunk));
    connection.on("end", () => {
      const line = buffer.trim().split("\n")[0];
      if (!line) return;
      try {
        const request = JSON.parse(line);
        const kind = request.kind;
        const name = kind === "agent-session" ? "muis-agent-session" : kind === "agent-activity" ? "muis-agent-activity" : "muis-notify";
        const frame = JSON.stringify({ event: name, payload: request });
        for (const client of clients) if (client.readyState === 1) client.send(frame);
      } catch {
        /* malformed request: ignore, same as the Rust host */
      }
    });
    connection.on("error", () => {});
  })
    .on("error", (error) => log("warn", `notify pipe unavailable (${socketName}): ${error.message}`))
    .listen(pipePath);

  const wss = new WebSocketServer({ noServer: true });

  const server = createServer(async (req, res) => {
    const url = new URL(req.url ?? "/", `http://127.0.0.1:${server.address()?.port ?? 0}`);
    if (!hostAllowed(req.headers.host)) {
      send(res, 403, {}, "host not allowed");
      return;
    }
    const origin = req.headers.origin;
    if (origin && !origin.endsWith(`:${server.address()?.port}`)) {
      send(res, 403, {}, "origin not allowed");
      return;
    }

    if (req.method === "POST" && url.pathname.startsWith("/rpc/")) {
      if (!tokenMatches(token, requestToken(req, url))) {
        send(res, 401, {}, "bad token");
        return;
      }
      let body = "";
      req.on("data", (chunk) => (body += chunk));
      req.on("end", async () => {
        let args = {};
        if (body.trim()) {
          try {
            args = JSON.parse(body);
          } catch {
            send(res, 400, {}, "invalid json");
            return;
          }
        }
        const method = url.pathname.slice("/rpc/".length);
        try {
          const value = await rpc(method, args, state);
          send(res, 200, { "content-type": "application/json" }, JSON.stringify(value ?? null));
        } catch (error) {
          log("warn", `rpc ${method} failed: ${error.message}`);
          send(res, 500, {}, error.message);
        }
      });
      return;
    }

    if (url.pathname === "/healthz") {
      send(res, 200, { "content-type": "text/plain" }, "ok");
      return;
    }

    // Static bundle, with the serve flag injected into index.html.
    const relative = url.pathname === "/" ? "index.html" : decodeURIComponent(url.pathname.slice(1));
    if (relative.includes("..")) {
      send(res, 404, {}, "not found");
      return;
    }
    const file = path.join(options.root, relative);
    let body;
    try {
      body = await fs.readFile(file);
    } catch {
      send(res, 404, {}, "not found");
      return;
    }
    const headers = { "content-type": contentType(file) };
    if (relative === "index.html") {
      const boot = `<script>window.__MUIS_SERVE__={"token":"${token}"};</script>`;
      body = Buffer.from(body.toString("utf8").replace("</head>", `${boot}</head>`));
      headers["cache-control"] = "no-store";
    } else if (relative.startsWith("assets/")) {
      headers["cache-control"] = "public, max-age=31536000, immutable";
    } else {
      headers["cache-control"] = "no-store";
    }
    send(res, 200, headers, body);
  });

  server.on("upgrade", (req, socket, head) => {
    const url = new URL(req.url ?? "/", `http://127.0.0.1:${server.address()?.port ?? 0}`);
    if (
      url.pathname !== "/ws" ||
      !hostAllowed(req.headers.host) ||
      !tokenMatches(token, requestToken(req, url))
    ) {
      socket.destroy();
      return;
    }
    wss.handleUpgrade(req, socket, head, (ws) => wss.emit("connection", ws));
  });

  wss.on("connection", (ws) => {
    clients.add(ws);
    const pool = createPool((frame) => {
      if (ws.readyState === 1) ws.send(JSON.stringify(frame));
    }, socketName, ptyEnv);
    ws.on("message", (data) => pool.frame(String(data)));
    ws.on("close", () => {
      clients.delete(ws);
      log("info", "browser tab gone; stopping its ptys");
      pool.stopAll();
    });
    ws.on("error", () => {});
  });

  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(options.port, "127.0.0.1", resolve);
  });

  const port = server.address().port;
  const url = `http://127.0.0.1:${port}/?t=${token}`;
  console.log(`muis serve: ${url}`);
  console.log(`state:     ${stateDir()}`);
  if (!existsSync(path.join(options.root, "index.html"))) {
    console.log(`warning:    no index.html in ${options.root}`);
  }
  log("info", `serving on port ${port}`);

  if (options.open) {
    const opener = IS_WIN ? ["cmd", ["/c", "start", "", url]] : process.platform === "darwin" ? ["open", [url]] : ["xdg-open", [url]];
    spawn(opener[0], opener[1], { windowsHide: true, detached: true, stdio: "ignore" }).unref();
  }

  const shutdown = () => {
    for (const ws of clients) ws.terminate();
    server.close();
    process.exit(0);
  };
  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);
}

main().catch((error) => {
  console.error(`muis-server: ${error.message}`);
  process.exit(1);
});
