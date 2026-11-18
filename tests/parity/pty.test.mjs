/**
 * Worker frames over the WebSocket: both hosts must accept the same ops and
 * emit the same shapes the frontend's `WorkerClient` routes.
 *
 * Unix-only, because the assertions run `/bin/sh`; the protocol code under
 * test is shared.
 */

import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";

import WebSocket from "ws";

import { hasDist, hasRustHost, startHost } from "./harness.mjs";

const UNIX = process.platform !== "win32";
const HOSTS = [{ kind: "node", skip: !hasDist || !UNIX }];
if (hasRustHost) HOSTS.unshift({ kind: "rust", skip: !UNIX });

function connect(host) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(`ws://127.0.0.1:${host.port}/ws?t=${host.token}`);
    socket.once("open", () => resolve(socket));
    socket.once("error", reject);
  });
}

function collect(socket) {
  const frames = [];
  socket.on("message", (data) => {
    try {
      frames.push(JSON.parse(String(data)));
    } catch {
      /* ignore */
    }
  });
  return frames;
}

async function until(predicate, timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return true;
    await new Promise((resolve) => setTimeout(resolve, 40));
  }
  return false;
}

const ui = (message) => JSON.stringify(message);

for (const { kind, skip } of HOSTS) {
  describe(`${kind} host: worker socket`, { skip }, () => {
    let host;
    before(async () => {
      host = await startHost(kind, { testmode: true });
    });
    after(async () => {
      await host?.stop();
    });

    it("refuses a socket without the token", async () => {
      await assert.rejects(
        new Promise((resolve, reject) => {
          const socket = new WebSocket(`ws://127.0.0.1:${host.port}/ws?t=wrong`);
          socket.once("open", () => {
            socket.close();
            resolve();
          });
          socket.once("error", reject);
        }),
      );
    });

    it("spawns a pty, streams its output, and reports the exit", async () => {
      const socket = await connect(host);
      const frames = collect(socket);

      socket.send(JSON.stringify({ op: "spawn", session_id: "s1" }));
      socket.send(
        JSON.stringify({
          op: "send",
          session_id: "s1",
          line: ui({ type: "spawn", pty_id: "t1", shell: "/bin/sh", cwd: "/tmp", cols: 80, rows: 24 }),
        }),
      );

      assert.ok(
        await until(() => frames.some((f) => f.frame?.type === "spawned")),
        `no spawned frame: ${JSON.stringify(frames)}`,
      );
      const spawned = frames.find((f) => f.frame?.type === "spawned");
      assert.deepEqual(spawned, { session_id: "s1", frame: { type: "spawned", pty_id: "t1" } });

      socket.send(
        JSON.stringify({
          op: "send",
          session_id: "s1",
          line: ui({
            type: "write",
            pty_id: "t1",
            data_b64: Buffer.from("printf 'parity-ok\\n'\n").toString("base64"),
          }),
        }),
      );

      assert.ok(
        await until(() =>
          frames
            .filter((f) => f.frame?.type === "output")
            .map((f) => Buffer.from(f.frame.data_b64, "base64").toString("utf8"))
            .join("")
            .includes("parity-ok"),
        ),
        "the marker never reached the socket",
      );

      const output = frames.find((f) => f.frame?.type === "output");
      assert.equal(output.session_id, "s1");
      assert.equal(output.frame.pty_id, "t1");
      assert.equal(typeof output.frame.data_b64, "string");

      socket.send(JSON.stringify({ op: "send", session_id: "s1", line: ui({ type: "kill", pty_id: "t1" }) }));
      assert.ok(
        await until(() => frames.some((f) => f.frame?.type === "exited" && f.frame.pty_id === "t1")),
        `no exited frame: ${JSON.stringify(frames.slice(-3))}`,
      );

      socket.close();
    });

    it("rejects a write to a pty that does not exist", async () => {
      const socket = await connect(host);
      const frames = collect(socket);
      socket.send(
        JSON.stringify({
          op: "send",
          session_id: "s1",
          line: ui({ type: "write", pty_id: "nope", data_b64: "" }),
        }),
      );
      // Neither host streams a frame for this; the point is that the socket
      // stays healthy and the connection still answers afterwards.
      socket.send(JSON.stringify({ op: "spawn", session_id: "s2" }));
      socket.send(
        JSON.stringify({
          op: "send",
          session_id: "s2",
          line: ui({ type: "spawn", pty_id: "t2", shell: "/bin/sh", cwd: "/tmp", cols: 80, rows: 24 }),
        }),
      );
      assert.ok(
        await until(() => frames.some((f) => f.frame?.type === "spawned" && f.frame.pty_id === "t2")),
        "the socket stopped working after an unknown pty id",
      );
      socket.close();
    });

    it("isolates sessions: stopping one leaves the other alive", async () => {
      const socket = await connect(host);
      const frames = collect(socket);
      for (const id of ["a", "b"]) {
        socket.send(JSON.stringify({ op: "spawn", session_id: id }));
        socket.send(
          JSON.stringify({
            op: "send",
            session_id: id,
            line: ui({ type: "spawn", pty_id: `p${id}`, shell: "/bin/sh", cwd: "/tmp", cols: 80, rows: 24 }),
          }),
        );
      }
      assert.ok(
        await until(() => frames.filter((f) => f.frame?.type === "spawned").length === 2),
        "both sessions should spawn",
      );

      socket.send(JSON.stringify({ op: "stop", session_id: "a" }));
      await new Promise((resolve) => setTimeout(resolve, 300));

      socket.send(
        JSON.stringify({
          op: "send",
          session_id: "b",
          line: ui({
            type: "write",
            pty_id: "pb",
            data_b64: Buffer.from("printf 'still-here\\n'\n").toString("base64"),
          }),
        }),
      );
      assert.ok(
        await until(() =>
          frames
            .filter((f) => f.frame?.type === "output" && f.session_id === "b")
            .map((f) => Buffer.from(f.frame.data_b64, "base64").toString("utf8"))
            .join("")
            .includes("still-here"),
        ),
        "stopping one session killed the other",
      );
      socket.close();
    });
  });
}
