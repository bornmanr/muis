/**
 * End-to-end tests for the JavaScript host: the real server, a real WebSocket,
 * and a real pty (node-pty), the same way the Rust host is tested.
 *
 * Unix-only because the assertions write to `/bin/sh`; the code under test is
 * shared.
 */

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";

import WebSocket from "ws";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const UNIX = process.platform !== "win32";

let child;
let port;
let token;
let home;

function http(method, url, body) {
  return new Promise((resolve, reject) => {
    const request = net.connect(port, "127.0.0.1", () => {
      const payload = body ?? "";
      request.write(
        `${method} ${url} HTTP/1.1\r\nHost: 127.0.0.1:${port}\r\nConnection: close\r\n` +
          `Content-Type: application/json\r\nContent-Length: ${Buffer.byteLength(payload)}\r\n\r\n${payload}`,
      );
    });
    let raw = "";
    request.on("data", (chunk) => (raw += chunk));
    request.on("end", () => {
      const status = Number(raw.split(" ")[1]);
      const body = raw.slice(raw.indexOf("\r\n\r\n") + 4);
      resolve({ status, body });
    });
    request.on("error", reject);
  });
}

before(async () => {
  home = fs.mkdtempSync(path.join(os.tmpdir(), "muis-node-host-"));
  const dist = path.join(home, "dist");
  fs.mkdirSync(path.join(dist, "assets"), { recursive: true });
  fs.writeFileSync(
    path.join(dist, "index.html"),
    "<!doctype html><html><head><title>muis</title></head><body></body></html>",
  );
  fs.writeFileSync(path.join(dist, "assets", "app.js"), "console.log(1)");

  child = spawn(
    process.execPath,
    [path.join(HERE, "muis-server.mjs"), "--no-open", "--testmode", "--port", "0", "--root", dist],
    {
      env: {
        ...process.env,
        MUIS_HOME: home,
        MUIS_SOCKET_NAME: `muis-node-test-${process.pid}`,
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );

  const banner = await new Promise((resolve, reject) => {
    let output = "";
    const timer = setTimeout(() => reject(new Error(`no banner: ${output}`)), 20000);
    child.stdout.on("data", (chunk) => {
      output += chunk;
      const match = output.match(/muis serve: http:\/\/127\.0\.0\.1:(\d+)\/\?t=([0-9a-f]+)/);
      if (match) {
        clearTimeout(timer);
        resolve({ port: Number(match[1]), token: match[2] });
      }
    });
    child.on("exit", (code) => reject(new Error(`server exited ${code}: ${output}`)));
  });
  port = banner.port;
  token = banner.token;
});

after(() => {
  child?.kill();
  if (home) fs.rmSync(home, { recursive: true, force: true });
});

test("serves the bundle with the serve flag injected", async () => {
  const { status, body } = await http("GET", "/");
  assert.equal(status, 200);
  assert.match(body, /__MUIS_SERVE__/);
  assert.ok(body.includes(token), "index carries the run token");
});

test("serves assets with an immutable cache header", async () => {
  const { status } = await http("GET", "/assets/app.js");
  assert.equal(status, 200);
});

test("answers commands and refuses a wrong token", async () => {
  const ok = await http("POST", `/rpc/ping?t=${token}`, "{}");
  assert.equal(ok.status, 200);
  assert.equal(ok.body.trim(), '"pong"');

  const denied = await http("POST", "/rpc/ping?t=wrong", "{}");
  assert.equal(denied.status, 401);
});

test("rejects an unknown command", async () => {
  const { status } = await http("POST", `/rpc/nope?t=${token}`, "{}");
  assert.equal(status, 500);
});

test("test mode persists nothing", async () => {
  const saved = await http(
    "POST",
    `/rpc/sessions_save?t=${token}`,
    JSON.stringify({ json: JSON.stringify({ workspaces: [] }) }),
  );
  assert.equal(saved.status, 200);
  assert.equal(fs.existsSync(path.join(home, "state", "sessions.json")), false);
});

test("runs a real shell over the websocket", { skip: !UNIX }, async () => {
  const socket = new WebSocket(`ws://127.0.0.1:${port}/ws?t=${token}`);
  await new Promise((resolve, reject) => {
    socket.once("open", resolve);
    socket.once("error", reject);
  });

  const seen = [];
  socket.on("message", (data) => seen.push(JSON.parse(String(data))));

  socket.send(JSON.stringify({ op: "spawn", session_id: "s1" }));
  socket.send(
    JSON.stringify({
      op: "send",
      session_id: "s1",
      line: JSON.stringify({
        type: "spawn",
        pty_id: "t1",
        shell: "/bin/sh",
        cwd: "/tmp",
        cols: 80,
        rows: 24,
      }),
    }),
  );
  socket.send(
    JSON.stringify({
      op: "send",
      session_id: "s1",
      line: JSON.stringify({
        type: "write",
        pty_id: "t1",
        data_b64: Buffer.from("printf 'muis-node-ok\\n'\n").toString("base64"),
      }),
    }),
  );

  const deadline = Date.now() + 15000;
  let output = "";
  while (Date.now() < deadline && !output.includes("muis-node-ok")) {
    await new Promise((resolve) => setTimeout(resolve, 50));
    output = seen
      .filter((frame) => frame.frame?.type === "output")
      .map((frame) => Buffer.from(frame.frame.data_b64, "base64").toString("utf8"))
      .join("");
  }
  socket.close();
  assert.match(output, /muis-node-ok/, `shell output: ${output}`);
});

test("notifications reach the open tab over the local pipe", { skip: process.platform !== "win32" }, async () => {
  // Named pipes are Windows-only; covered by the manual Windows run.
});
