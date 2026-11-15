/**
 * Unit test for the opencode server plugin. The plugin runs inside opencode,
 * so we drive the exported core with injected helpers and assert what it
 * asks `muis-notify` / `muis-agent` to send.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import plugin, {
  createMuisPlugin,
  permissionOf,
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
  assert.deepEqual(SESSION_EVENTS, [
    "session.created",
    "session.renamed",
    "session.idle",
    "session.execution.succeeded",
    "session.execution.failed",
    "form.created",
    "permission.asked",
    "todo.updated",
  ]);
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

test("session.execution.succeeded replaces session.idle on opencode 2.3", () => {
  const { plugin, calls } = harness();
  plugin.event(created("ses_exec", { title: "Port the plugin", location: { directory: "/home/x/Projects/muis" } }));
  plugin.event({ type: "session.execution.succeeded", data: { sessionID: "ses_exec" } });
  assert.match(calls.join("\n"), /--title muis/);
  assert.match(calls.join("\n"), /--body Port the plugin has finished/);
});

test("session.execution.failed does not claim the work finished", () => {
  const { plugin, calls } = harness();
  plugin.event(created("ses_fail", { title: "Broken turn" }));
  plugin.event({ type: "session.execution.failed", data: { sessionID: "ses_fail" } });
  const joined = calls.join("\n");
  assert.match(joined, /--body Broken turn failed/);
  assert.doesNotMatch(joined, /has finished/);
});

test("form.created alerts while the turn waits for an answer", () => {
  const { plugin, calls } = harness();
  plugin.event(created("ses_form", { title: "Pick a colour", location: { directory: "/home/x/Projects/muis" } }));
  plugin.event({
    type: "form.created",
    data: {
      form: {
        id: "frm_1",
        sessionID: "ses_form",
        title: "Questions",
        metadata: { kind: "question" },
        fields: [{ key: "q0", type: "multiselect", title: "Pick one" }],
      },
    },
  });
  const joined = calls.join("\n");
  assert.match(joined, /--title muis/);
  assert.match(joined, /--body Pick a colour is waiting for your input: Questions/);
});

test("form.created for a global form falls back to the event location", () => {
  const { plugin, calls } = harness();
  plugin.event({
    type: "form.created",
    location: { directory: "/home/x/Projects/muis" },
    data: {
      form: {
        id: "frm_global",
        sessionID: "global",
        title: "MCP authorization",
        metadata: { kind: "mcp" },
        fields: [],
      },
    },
  });
  const joined = calls.join("\n");
  assert.match(joined, /--title muis/);
  assert.match(joined, /--body opencode is waiting for your input: MCP authorization/);
});

test("permission.asked alerts with the V2 action name", () => {
  const { plugin, calls } = harness();
  plugin.event(created("ses_perm", { title: "Touch a file", location: { directory: "/home/x/Projects/muis" } }));
  plugin.event({
    type: "permission.asked",
    data: { id: "per_1", sessionID: "ses_perm", action: "bash", resources: ["*"] },
  });
  assert.match(calls.join("\n"), /--body Touch a file needs permission: bash/);
});

test("permission.asked also reads the legacy permission field", () => {
  assert.equal(permissionOf({ permission: "  edit " }), "edit");
  assert.equal(permissionOf({ action: "bash" }), "bash");
  assert.equal(permissionOf({}), "");
});

test("todo.updated reports done/total with the session directory", () => {
  const { plugin, calls } = harness();
  plugin.event(created("ses_todo", { location: { directory: "/home/x/Projects/muis" } }));
  plugin.event({
    type: "todo.updated",
    data: {
      sessionID: "ses_todo",
      todos: [
        { content: "a", status: "completed", priority: "high" },
        { content: "b", status: "in_progress", priority: "medium" },
        { content: "c", status: "pending", priority: "low" },
      ],
    },
  });
  const joined = calls.join("\n");
  assert.match(joined, /--todo-done 1 --todo-total 3/);
  assert.match(joined, /--dir \/home\/x\/Projects\/muis/);
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

test("ask events with no session id are ignored", () => {
  const { plugin, calls } = harness();
  plugin.event({ type: "form.created", data: { form: { title: "No session" } } });
  plugin.event({ type: "permission.asked", data: { action: "bash" } });
  assert.deepEqual(calls, []);
});

test("invalid session ids are never reported", () => {
  const { plugin, calls } = harness();
  plugin.prompt("not-a-session");
  assert.doesNotMatch(calls.join("\n"), /agent /);
});

test("unrelated events are ignored", () => {
  const { plugin, calls } = harness();
  plugin.event({ type: "session.status", data: { sessionID: "ses_status" } });
  plugin.event({ type: "session.execution.started", data: { sessionID: "ses_exec" } });
  plugin.event(undefined);
  assert.deepEqual(calls, []);
});
