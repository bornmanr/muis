/**
 * Unit test for the opencode server plugin. The plugin runs inside opencode,
 * so we drive the exported core with injected helpers and assert what it
 * asks `muis-notify` / `muis-agent` to send.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import plugin, {
  createMuisPlugin,
  SESSION_EVENTS,
} from "../../crates/muis-shell/opencode-plugin/muis.js";

/** Build a plugin instance that records each helper call as a string. */
function harness() {
  const calls = [];
  const record = (kind) => (args) => calls.push(`${kind} ${args.join(" ")}`);
  const plugin = createMuisPlugin({ reportAgent: record("agent"), notify: record("notify") });
  return { plugin, calls };
}

const created = (id, extra = {}) => ({
  type: "session.created",
  data: {
    sessionID: id,
    title: "",
    location: { directory: "/p" },
    ...extra,
  },
});

test("default export is a V2 definition opencode can load", () => {
  // opencode 2.x drops plugins without this shape, and says so only in the
  // server log.
  assert.equal(plugin.id, "muis");
  assert.equal(typeof plugin.setup, "function");
  assert.deepEqual(SESSION_EVENTS, ["session.created", "session.renamed", "session.idle"]);
});

test("session.idle toast names the project and the work", () => {
  const { plugin, calls } = harness();
  plugin.event(created("ses_abc", { title: "Fix the clock", location: { directory: "/home/x/Projects/muis" } }));
  plugin.event({ type: "session.idle", data: { sessionID: "ses_abc" } });
  const joined = calls.join("\n");
  assert.match(joined, /--title muis/);
  assert.match(joined, /--body Fix the clock has finished/);
  assert.doesNotMatch(joined, /turn finished/);
});

test("session.renamed updates the title used by the toast", () => {
  const { plugin, calls } = harness();
  plugin.event(created("ses_ren", { title: "Old title" }));
  plugin.event({ type: "session.renamed", data: { sessionID: "ses_ren", title: "New title" } });
  plugin.event({ type: "session.idle", data: { sessionID: "ses_ren" } });
  assert.match(calls.join("\n"), /--body New title has finished/);
});

test("session.idle without known info still avoids the placeholder", () => {
  const { plugin, calls } = harness();
  plugin.event({ type: "session.idle", data: { sessionID: "ses_unknown" } });
  const joined = calls.join("\n");
  assert.match(joined, /--title opencode/);
  assert.match(joined, /--body opencode has finished/);
  assert.doesNotMatch(joined, /turn finished/);
});

test("prompt reports the exact session for resume tracking", () => {
  const { plugin, calls } = harness();
  plugin.prompt("ses_exact1");
  const joined = calls.join("\n");
  assert.match(joined, /agent session ses_exact1/);
  assert.doesNotMatch(joined, /--title/);
});

test("session.created reports a new top-level session but not a subagent", () => {
  const { plugin, calls } = harness();
  plugin.event(created("ses_root"));
  assert.match(calls.join("\n"), /agent session ses_root/);

  calls.length = 0;
  plugin.event(created("ses_child", { parentID: "ses_root" }));
  assert.doesNotMatch(calls.join("\n"), /agent/);
});

test("agent reports dedupe repeated session ids", () => {
  const { plugin, calls } = harness();
  plugin.prompt("ses_same");
  plugin.prompt("ses_same");
  const agentCalls = calls.filter((c) => c.startsWith("agent "));
  assert.equal(agentCalls.length, 1);
});

test("session.idle notifies without overriding the TUI route", () => {
  const { plugin, calls } = harness();
  plugin.event(created("ses_idle", { title: "Fix it" }));
  calls.length = 0;
  plugin.event({ type: "session.idle", data: { sessionID: "ses_idle" } });
  const joined = calls.join("\n");
  assert.doesNotMatch(joined, /agent /);
  assert.match(joined, /notify --title p/);
});

test("invalid session ids are never reported", () => {
  const { plugin, calls } = harness();
  plugin.prompt("not-a-session");
  assert.doesNotMatch(calls.join("\n"), /agent /);
});

test("unrelated events are ignored", () => {
  const { plugin, calls } = harness();
  plugin.event({ type: "session.status", data: { sessionID: "ses_status" } });
  plugin.event(undefined);
  assert.deepEqual(calls, []);
});
