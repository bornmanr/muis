/**
 * The command surface, the HTTP surface, and persistence — asserted once and
 * run against every browser host, so a change to one cannot silently diverge
 * from the other.
 */

import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { after, before, describe, it } from "node:test";

import {
  DIST,
  fixtureRepo,
  fixtureTree,
  hasDist,
  hasRustHost,
  startHost,
  tmpDir,
} from "./harness.mjs";

const HOSTS = [{ kind: "node", skip: !hasDist }];
if (hasRustHost) HOSTS.unshift({ kind: "rust", skip: false });

for (const { kind, skip } of HOSTS) {
  describe(`${kind} host: commands`, { skip }, () => {
    let host;
    before(async () => {
      host = await startHost(kind, { testmode: true });
    });
    after(async () => {
      await host?.stop();
    });

    it("answers ping", async () => {
      const { status, text } = await host.rpc("ping");
      assert.equal(status, 200);
      assert.equal(text.trim(), '"pong"');
    });

    it("reports test mode, a shell, and the home directory", async () => {
      assert.equal((await host.rpc("is_test_mode")).text.trim(), "true");

      const shell = JSON.parse((await host.rpc("default_shell")).text);
      assert.equal(typeof shell, "string");
      assert.ok(shell.length > 0, "default_shell must name a shell");

      const home = JSON.parse((await host.rpc("default_cwd")).text);
      assert.equal(home, os.homedir());
    });

    it("reports itself as available", async () => {
      assert.equal((await host.rpc("worker_available")).text.trim(), "true");
    });

    it("answers the log path inside the isolated home", async () => {
      const logPath = JSON.parse((await host.rpc("log_path")).text);
      assert.ok(
        logPath.startsWith(host.home) || logPath.includes("muis"),
        `log path should live under the isolated home: ${logPath}`,
      );
      assert.ok(logPath.endsWith("muis-server.log"));
    });

    it("rejects an unknown command with the message as the body", async () => {
      const { status, text } = await host.rpc("no_such_command");
      assert.equal(status, 500);
      assert.match(text, /unknown command: no_such_command/);
    });

    it("rejects a wrong token and accepts a header token", async () => {
      const denied = await host.rpc("ping", {}, { token: "0123456789abcdef0123456789abcdef" });
      assert.equal(denied.status, 401);

      const viaHeader = await fetch(`${host.base}/rpc/ping`, {
        method: "POST",
        headers: { "content-type": "application/json", "x-muis-token": host.token },
        body: "{}",
      });
      assert.equal(viaHeader.status, 200);
      assert.equal((await viaHeader.text()).trim(), '"pong"');
    });

    it("describes a directory tree identically", async () => {
      const tree = fixtureTree();
      const listing = JSON.parse((await host.rpc("list_dir", { path: tree })).text);
      const names = listing.map((entry) => entry.name);
      assert.deepEqual(names, ["alpha", "Beta"], "sorted, hidden and $ folders skipped");
      assert.deepEqual(
        listing.map((entry) => entry.path),
        [path.join(tree, "alpha"), path.join(tree, "Beta")],
      );
      assert.ok(listing.every((entry) => entry.dir === true));

      const subdirs = JSON.parse((await host.rpc("list_subdirs", { path: tree, limit: 10 })).text);
      assert.deepEqual(subdirs, ["alpha", "Beta"]);
      const capped = JSON.parse((await host.rpc("list_subdirs", { path: tree, limit: 1 })).text);
      assert.deepEqual(capped, ["alpha"]);
      fs.rmSync(tree, { recursive: true, force: true });
    });

    it("reports the branch of a repository", async () => {
      const repo = fixtureRepo();
      const branch = JSON.parse((await host.rpc("git_branch", { cwd: repo })).text);
      assert.equal(branch, "feature/parity");
      const empty = JSON.parse((await host.rpc("git_branch", { cwd: tmpDir("muis-parity-plain-") })).text);
      assert.equal(empty, "", "a directory that is not a repo yields an empty branch");
      fs.rmSync(repo, { recursive: true, force: true });
    });

    it("parses the opencode session list the same way", async () => {
      const sessions = JSON.parse((await host.rpc("opencode_sessions", { cwd: os.tmpdir() })).text);
      assert.equal(sessions.length, 1);
      assert.deepEqual(Object.keys(sessions[0]).sort(), ["directory", "id", "title"]);
      assert.equal(sessions[0].id, "ses_parity");
      assert.equal(sessions[0].title, "Parity");
    });

    it("installs the same opencode plugin files", async () => {
      const installed = JSON.parse((await host.rpc("install_opencode_plugin")).text);
      assert.ok(installed.endsWith("muis.js"), `plugin path: ${installed}`);
      assert.ok(fs.existsSync(installed));
      assert.match(fs.readFileSync(installed, "utf8"), /muis-plugin/);

      const tui = path.join(path.dirname(installed), "muis-tui", "tui.js");
      assert.ok(fs.existsSync(tui), "the TUI plugin goes in its own directory");
      assert.match(fs.readFileSync(tui, "utf8"), /muis-tui-plugin/);

      // Both hosts ship the same sources, not copies.
      const repoPlugin = path.join(DIST, "..", "..", "crates", "muis-shell", "opencode-plugin");
      assert.equal(
        fs.readFileSync(installed, "utf8"),
        fs.readFileSync(path.join(repoPlugin, "muis.js"), "utf8"),
      );
    });

    it("refuses worker commands on the command endpoint", async () => {
      const { status, text } = await host.rpc("worker_spawn", { session_id: "s1" });
      assert.equal(status, 500);
      assert.match(text, /\/ws/);
    });
  });

  describe(`${kind} host: http surface`, { skip }, () => {
    let host;
    before(async () => {
      host = await startHost(kind, { testmode: true });
    });
    after(async () => {
      await host?.stop();
    });

    it("serves the UI with the serve flag and the run token", async () => {
      const response = await fetch(`${host.base}/`);
      assert.equal(response.status, 200);
      const body = await response.text();
      assert.match(body, /__MUIS_SERVE__/);
      assert.ok(body.includes(host.token), "the page carries the token to call with");
    });

    it("answers the health probe", async () => {
      const response = await fetch(`${host.base}/healthz`);
      assert.equal(response.status, 200);
      assert.equal((await response.text()).trim(), "ok");
    });

    it("404s an unknown path and a traversal attempt", async () => {
      assert.equal((await fetch(`${host.base}/nope`)).status, 404);
      assert.equal((await fetch(`${host.base}/../Cargo.toml`)).status, 404);
    });

    it("refuses a foreign Host header (DNS rebinding)", async () => {
      const raw = await host.raw({ Host: "rebind.example:8080" });
      assert.match(raw, /^HTTP\/1\.1 403/);
    });

    it("accepts the loopback Host header", async () => {
      const raw = await host.raw({ Host: `127.0.0.1:${host.port}` });
      assert.match(raw, /^HTTP\/1\.1 200/);
    });
  });

  describe(`${kind} host: persistence`, { skip }, () => {
    it("round-trips config and sessions outside test mode", async () => {
      const host = await startHost(kind, { testmode: false });
      try {
        const config = JSON.stringify({ theme: "Breeze", fontFamily: "JetBrains Mono" });
        const sessions = JSON.stringify({ version: 1, workspaces: [{ id: "w1", name: "s" }] });

        assert.equal((await host.rpc("config_save", { json: config })).status, 200);
        assert.equal((await host.rpc("sessions_save", { json: sessions })).status, 200);

        assert.deepEqual(JSON.parse(JSON.parse((await host.rpc("config_load")).text)), JSON.parse(config));
        assert.deepEqual(JSON.parse(JSON.parse((await host.rpc("sessions_load")).text)), JSON.parse(sessions));

        // Both hosts write the same files in the same place.
        assert.ok(fs.existsSync(path.join(host.home, "config", "muis.json")));
        assert.ok(fs.existsSync(path.join(host.home, "state", "sessions.json")));
      } finally {
        await host.stop();
      }
    });

    it("refuses to persist invalid json", async () => {
      const host = await startHost(kind, { testmode: false });
      try {
        const { status, text } = await host.rpc("config_save", { json: "not json" });
        assert.equal(status, 500);
        assert.match(text, /invalid json/i);
        assert.equal((await host.rpc("config_load")).text.trim(), '""', "nothing was written");
      } finally {
        await host.stop();
      }
    });

    it("persists nothing in test mode", async () => {
      const host = await startHost(kind, { testmode: true });
      try {
        await host.rpc("sessions_save", { json: JSON.stringify({ workspaces: [] }) });
        assert.equal(fs.existsSync(path.join(host.home, "state", "sessions.json")), false);
      } finally {
        await host.stop();
      }
    });
  });
}
