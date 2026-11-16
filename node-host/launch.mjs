#!/usr/bin/env node
/**
 * One command to start muis: keep the frontend bundle current, make sure the
 * host's dependencies exist, then hand off to `muis-server.mjs`.
 *
 * The host serves `src-ui/dist` from disk, so a UI change needs a build before
 * it shows up. Doing it here means the daily "open muis" step is one command
 * and the bundle can never be stale.
 *
 *   node node-host/launch.mjs [--port N] [--no-open] [--testmode] [--root DIR]
 *
 * A default port is supplied (7800) so a bookmark keeps working across runs;
 * pass `--port` to override, or `--port 0` to let the OS choose.
 */

import { spawn } from "node:child_process";
import { existsSync, readdirSync, statSync } from "node:fs";
import fs from "node:fs/promises";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";

const IS_WIN = process.platform === "win32";
const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, "..");
const UI = path.join(REPO, "src-ui");
const DIST = path.join(UI, "dist");
const HOST = path.join(HERE, "muis-server.mjs");
const DEFAULT_PORT = 7800;

/** Newest mtime (ms) under `target`, or 0 when it does not exist. */
export function newestMtime(target) {
  let newest = 0;
  const visit = (file) => {
    let stat;
    try {
      stat = statSync(file);
    } catch {
      return;
    }
    if (stat.isDirectory()) {
      for (const entry of readdirNames(file)) visit(path.join(file, entry));
    } else if (stat.mtimeMs > newest) {
      newest = stat.mtimeMs;
    }
  };
  visit(target);
  return newest;
}

function readdirNames(dir) {
  try {
    return readdirSync(dir);
  } catch {
    return [];
  }
}

/**
 * Whether the bundle must be rebuilt: missing, or older than anything that
 * feeds it. Pure apart from the filesystem, so it is testable directly.
 */
export function needsBuild({ dist, sources }) {
  const index = path.join(dist, "index.html");
  if (!existsSync(index)) return true;
  const built = statSync(index).mtimeMs;
  return sources.some((source) => newestMtime(source) > built);
}

/** The files that make `src-ui/dist` out of date when they change. */
function uiSources() {
  const candidates = [
    path.join(UI, "src"),
    path.join(UI, "index.html"),
    path.join(UI, "package.json"),
    path.join(UI, "package-lock.json"),
    path.join(UI, "tsconfig.json"),
    path.join(UI, "vite.config.ts"),
  ];
  return candidates.filter((candidate) => existsSync(candidate));
}

function run(command, args, cwd) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd,
      stdio: "inherit",
      shell: IS_WIN, // npm on Windows is `npm.cmd`, which needs a shell
    });
    child.on("error", reject);
    child.on("exit", (code) =>
      code === 0 ? resolve() : reject(new Error(`${command} ${args.join(" ")} exited ${code}`)),
    );
  });
}

function portFree(port) {
  return new Promise((resolve) => {
    const probe = net.createServer();
    probe.once("error", () => resolve(false));
    probe.once("listening", () => probe.close(() => resolve(true)));
    probe.listen(port, "127.0.0.1");
  });
}

function openBrowser(url) {
  const [command, args] = IS_WIN
    ? ["cmd", ["/c", "start", "", url]]
    : process.platform === "darwin"
      ? ["open", [url]]
      : ["xdg-open", [url]];
  spawn(command, args, { windowsHide: true, detached: true, stdio: "ignore" }).unref();
}

function parse(argv) {
  const options = { port: null, passthrough: [] };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--port") options.port = Number(argv[++i]);
    else options.passthrough.push(argv[i]);
  }
  return options;
}

async function main() {
  const options = parse(process.argv.slice(2));
  const port = options.port ?? DEFAULT_PORT;

  // A second double-click should bring the window back, not start a second
  // server on a different port.
  if (port !== 0 && !(await portFree(port))) {
    const url = `http://127.0.0.1:${port}/`;
    console.log(`muis is already running: ${url}`);
    if (!options.passthrough.includes("--no-open")) openBrowser(url);
    return;
  }

  if (!existsSync(path.join(HERE, "node_modules"))) {
    console.log("muis: installing host dependencies (once)");
    await run("npm", ["ci"], HERE);
  }

  if (!process.env.MUIS_UI_DIST && needsBuild({ dist: DIST, sources: uiSources() })) {
    if (!existsSync(path.join(UI, "node_modules"))) {
      console.log("muis: installing frontend dependencies (once)");
      await run("npm", ["ci"], UI);
    }
    console.log("muis: building the frontend bundle");
    await run("npm", ["run", "build"], UI);
  }

  const args = [HOST, "--port", String(port), ...options.passthrough];
  const child = spawn(process.execPath, args, { stdio: "inherit" });
  const forward = (signal) => () => child.kill(signal);
  process.on("SIGINT", forward("SIGINT"));
  process.on("SIGTERM", forward("SIGTERM"));
  child.on("exit", (code) => process.exit(code ?? 0));
}

// Only run when executed, so the tests can import the helpers.
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(`muis: ${error.message}`);
    process.exit(1);
  });
}
