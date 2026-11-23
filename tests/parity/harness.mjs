/**
 * Starts either browser host — the Rust binary or the Node script — behind one
 * interface, so the same assertions can be run against both.
 *
 * Requires a built frontend for the Node host (`src-ui/dist`) and a built Rust
 * host at `MUIS_RUST_BIN` (or `target/debug/muis-server`); the Rust cases are
 * skipped when that binary is absent.
 */

import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
export const REPO = path.resolve(HERE, "..", "..");
export const DIST = path.join(REPO, "src-ui", "dist");
export const NODE_HOST = path.join(REPO, "node-host", "muis-server.mjs");
export const RUST_HOST =
  process.env.MUIS_RUST_BIN ?? path.join(REPO, "target", "debug", "muis-server");

export const hasRustHost = fs.existsSync(RUST_HOST);
export const hasDist = fs.existsSync(path.join(DIST, "index.html"));

let counter = 0;

export function tmpDir(prefix) {
  return fs.mkdtempSync(path.join(os.tmpdir(), prefix));
}

/** A directory tree both hosts must describe identically. */
export function fixtureTree() {
  const dir = tmpDir("muis-parity-tree-");
  for (const name of ["alpha", "Beta", ".hidden", "$Recycle"]) {
    fs.mkdirSync(path.join(dir, name));
  }
  fs.writeFileSync(path.join(dir, "file.txt"), "x");
  return dir;
}

/** A repo on a known branch, for `git_branch`. */
export function fixtureRepo() {
  const dir = tmpDir("muis-parity-git-");
  const git = (...args) => execFileSync("git", args, { cwd: dir, stdio: "ignore" });
  git("init", "-q");
  git("-c", "user.email=parity@example.test", "-c", "user.name=parity", "commit", "-q", "--allow-empty", "-m", "x");
  git("checkout", "-q", "-b", "feature/parity");
  return dir;
}

/**
 * A stub `opencode` first on PATH, so `opencode_sessions` exercises the real
 * spawn-and-parse path in both hosts without needing the tool installed.
 */
export function opencodeStub() {
  const dir = tmpDir("muis-parity-bin-");
  if (process.platform === "win32") {
    fs.writeFileSync(
      path.join(dir, "opencode.cmd"),
      '@echo off\r\nif "%1"=="session" (\r\n  echo [{"id":"ses_parity","title":"Parity","directory":"%CD%","extra":1}]\r\n) else (exit /b 0)\r\n',
    );
  } else {
    fs.writeFileSync(
      path.join(dir, "opencode"),
      `#!/bin/sh
case "$1" in
  session) printf '[{"id":"ses_parity","title":"Parity","directory":"%s","extra":1}]\\n' "$PWD" ;;
  *) exit 0 ;;
esac
`,
      { mode: 0o755 },
    );
  }
  return dir;
}

export const stubDir = opencodeStub();

/**
 * Start one host on an OS-assigned port. Both print
 * `muis serve: http://127.0.0.1:<port>/?t=<token>` once they are listening.
 */
export async function startHost(kind, options = {}) {
  const home = options.home ?? tmpDir(`muis-parity-home-${kind}-`);
  const env = {
    ...process.env,
    MUIS_HOME: home,
    MUIS_SOCKET_NAME: `muis-parity-${kind}-${process.pid}-${counter++}`,
    XDG_CONFIG_HOME: path.join(home, "xdg"),
    PATH: `${stubDir}${path.delimiter}${process.env.PATH ?? ""}`,
  };

  const flags = ["--no-open", "--port", "0"];
  if (options.testmode !== false) flags.push("--testmode");
  if (options.extra) flags.push(...options.extra);

  const [command, args] =
    kind === "node" ? [process.execPath, [NODE_HOST, ...flags, "--root", DIST]] : [RUST_HOST, flags];

  const child = spawn(command, args, { env, stdio: ["ignore", "pipe", "pipe"] });

  const log = [];
  child.stderr.on("data", (chunk) => log.push(String(chunk)));

  const banner = await new Promise((resolve, reject) => {
    let out = "";
    const timer = setTimeout(
      () => reject(new Error(`${kind} host never announced itself:\n${out}\n${log.join("")}`)),
      20000,
    );
    child.stdout.on("data", (chunk) => {
      out += chunk;
      const match = out.match(/muis serve: http:\/\/127\.0\.0\.1:(\d+)\/\?t=([0-9a-f]+)/);
      if (!match) return;
      clearTimeout(timer);
      resolve({ port: Number(match[1]), token: match[2] });
    });
    child.on("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`${kind} host exited early (${code}):\n${out}\n${log.join("")}`));
    });
  });

  const base = `http://127.0.0.1:${banner.port}`;
  return {
    kind,
    port: banner.port,
    token: banner.token,
    home,
    url: `${base}/?t=${banner.token}`,
    base,
    /** POST a command the way the frontend does. */
    async rpc(method, args = {}, opts = {}) {
      const token = opts.token ?? banner.token;
      const response = await fetch(`${base}/rpc/${method}?t=${token}`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(args),
      });
      const text = await response.text();
      return { status: response.status, text };
    },
    /** Raw HTTP, for cases that must control headers fetch forbids. */
    raw(headers, requestPath = "/") {
      return new Promise((resolve, reject) => {
        const socket = net.connect(banner.port, "127.0.0.1", () => {
          const extra = Object.entries(headers)
            .map(([name, value]) => `${name}: ${value}\r\n`)
            .join("");
          socket.write(`GET ${requestPath} HTTP/1.1\r\n${extra}Connection: close\r\n\r\n`);
        });
        let raw = "";
        socket.on("data", (chunk) => (raw += chunk));
        socket.on("end", () => resolve(raw));
        socket.on("error", reject);
      });
    },
    stop() {
      child.kill("SIGKILL");
      return new Promise((resolve) => child.on("exit", resolve));
    },
  };
}
