/**
 * Unit test for the opencode server plugin. The plugin runs inside opencode,
 * so we drive the exported core with injected helpers and assert what it
 * asks `muis-notify` / `muis-agent` to send.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import plugin, {
  agentCommand,
  createMuisPlugin,
  createPreviewTool,
  permissionOf,
  previewUrl,
  runHelper,
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
    "form.replied",
    "permission.asked",
    "permission.replied",
    "permission.rejected",
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

test("prompt also clears the waiting mark", () => {
  const { plugin, calls } = harness();
  plugin.prompt("ses_exact2");
  assert.match(calls.join("\n"), /notify --running --session ses_exact2/);
});

test("notifications carry the session so muis can find the tab", () => {
  // The server plugin runs in the shared opencode server, where MUIS_TAB_ID
  // names whichever tab started it: the session is the real route.
  const { plugin, calls } = harness();
  plugin.event(created("ses_route", { title: "Route me" }));
  plugin.event({ type: "session.execution.succeeded", data: { sessionID: "ses_route" } });
  assert.match(calls.join("\n"), /--session ses_route/);
});

test("answering a permission or a form clears the waiting mark", () => {
  const { plugin, calls } = harness();
  calls.length = 0;
  plugin.event({ type: "permission.replied", data: { sessionID: "ses_perm" } });
  plugin.event({ type: "form.replied", data: { sessionID: "ses_form" } });
  assert.deepEqual(calls, [
    "notify --running --session ses_perm",
    "notify --running --session ses_form",
  ]);
});

test("execution events alone never clear the mark", () => {
  // Background work (title generation, subagents, compaction) starts
  // executions; only the user answering should clear a waiting tab.
  const { plugin, calls } = harness();
  plugin.event(created("ses_bg", { title: "Background" }));
  calls.length = 0;
  plugin.event({ type: "session.execution.started", data: { sessionID: "ses_bg" } });
  assert.deepEqual(calls, []);
});

test("a completion after a reply still raises an alert", () => {
  const { plugin, calls } = harness();
  plugin.event(created("ses_cycle", { title: "Cycle" }));
  plugin.prompt("ses_cycle");
  plugin.event({ type: "session.execution.succeeded", data: { sessionID: "ses_cycle" } });
  const joined = calls.join("\n");
  assert.match(joined, /notify --running --session ses_cycle/);
  assert.match(joined, /--body Cycle has finished/);
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
  plugin.event({ type: "session.deleted", data: { sessionID: "ses_gone" } });
  plugin.event(undefined);
  assert.deepEqual(calls, []);
});

test("alert contract: every alert is session-routed and every reply clears", () => {
  // One table for the whole contract. If opencode renames or drops an event,
  // this test has to change with it — that is the point: the pipeline fails
  // instead of the alert silently disappearing.
  const cases = [
    {
      name: "turn finished",
      event: { type: "session.execution.succeeded", data: { sessionID: "ses_c" } },
      want: /--body Project has finished --session ses_c/,
    },
    {
      name: "turn failed",
      event: { type: "session.execution.failed", data: { sessionID: "ses_c" } },
      want: /--body Project failed --session ses_c/,
    },
    {
      name: "permission asked",
      event: { type: "permission.asked", data: { sessionID: "ses_c", action: "bash" } },
      want: /needs permission: bash --session ses_c/,
    },
    {
      name: "form asked",
      event: { type: "form.created", data: { form: { sessionID: "ses_c", title: "Pick" } } },
      want: /is waiting for your input: Pick --session ses_c/,
    },
    {
      name: "permission replied",
      event: { type: "permission.replied", data: { sessionID: "ses_c" } },
      want: /^notify --running --session ses_c$/,
    },
    {
      name: "permission rejected",
      event: { type: "permission.rejected", data: { sessionID: "ses_c" } },
      want: /^notify --running --session ses_c$/,
    },
    {
      name: "form replied",
      event: { type: "form.replied", data: { sessionID: "ses_c" } },
      want: /^notify --running --session ses_c$/,
    },
  ];

  for (const c of cases) {
    const { plugin, calls } = harness();
    plugin.event(created("ses_c", { title: "Project", location: { directory: "/w/Project" } }));
    calls.length = 0;
    plugin.event(c.event);
    assert.equal(calls.length, 1, `${c.name}: expected exactly one helper call`);
    assert.match(calls[0], c.want, `${c.name}: unexpected helper call ${calls[0]}`);
  }

  // Every case must be subscribed, or opencode would never deliver it.
  for (const c of cases) {
    assert.ok(SESSION_EVENTS.includes(c.event.type), `${c.name}: not subscribed`);
  }
});

test("a prompt reply clears the mark for its own session", () => {
  const { plugin, calls } = harness();
  plugin.prompt("ses_prompt");
  assert.deepEqual(calls, [
    "agent session ses_prompt",
    "notify --running --session ses_prompt",
  ]);
});

test("the browser pane tool opens only absolute http(s) URLs", () => {
  assert.equal(previewUrl("  http://localhost:5173/  "), "http://localhost:5173/");
  assert.equal(previewUrl("https://example.com/a"), "https://example.com/a");
  assert.equal(previewUrl("localhost:5173"), "");
  assert.equal(previewUrl("javascript:alert(1)"), "");
  assert.equal(previewUrl("file:///etc/passwd"), "");
  assert.equal(previewUrl(undefined), "");
});

test("muis_open_url asks muis-agent to open the pane for this session", async () => {
  const calls = [];
  const tool = createPreviewTool({
    openUrl: async (url, session) => {
      calls.push([url, session]);
      return null;
    },
  });
  assert.equal(tool.name, "muis_open_url");
  assert.deepEqual(tool.input.required, ["url"]);

  const answer = await tool.execute({ url: "http://localhost:5173/" }, { sessionID: "ses_pane" });
  assert.deepEqual(calls, [["http://localhost:5173/", "ses_pane"]]);
  assert.match(answer, /Opened http:\/\/localhost:5173\//);

  // Hostile input never reaches the helper.
  calls.length = 0;
  const refused = await tool.execute({ url: "file:///etc/passwd" }, { sessionID: "ses_pane" });
  assert.deepEqual(calls, []);
  assert.match(refused, /needs an http\(s\) URL/);
});

test("muis_open_url reports a shell it could not reach instead of claiming success", async () => {
  const tool = createPreviewTool({
    openUrl: async () => "Could not open http://localhost:5173/: muis is not running.",
  });
  const answer = await tool.execute({ url: "http://localhost:5173/" }, {});
  assert.equal(answer, "Could not open http://localhost:5173/: muis is not running.");
  assert.doesNotMatch(answer, /^Opened /);

  // A throwing helper is still a tool result, never a crash.
  const broken = createPreviewTool({
    openUrl: async () => {
      throw new Error("spawn ENOENT");
    },
  });
  assert.match(await broken.execute({ url: "http://x/" }, {}), /spawn ENOENT/);
});

test("agentCommand picks the sidecar, the single-exe build, or PATH", () => {
  assert.deepEqual(agentCommand({ MUIS_AGENT_BIN: "/opt/muis-agent" }), {
    command: "/opt/muis-agent",
    args: [],
  });
  assert.deepEqual(agentCommand({ MUIS_BIN: "/usr/bin/muis" }), {
    command: "/usr/bin/muis",
    args: ["--muis-agent"],
  });
  assert.deepEqual(agentCommand({}), { command: "muis-agent", args: [] });
});

test("runHelper resolves the real exit status and stderr", async () => {
  const ok = await runHelper(process.execPath, ["-e", "process.exit(0)"]);
  assert.equal(ok.ok, true);
  assert.equal(ok.code, 0);

  const failed = await runHelper(process.execPath, [
    "-e",
    'console.error("could not reach muis"); process.exit(1)',
  ]);
  assert.equal(failed.ok, false);
  assert.equal(failed.code, 1);
  assert.match(failed.reason, /could not reach muis/);

  // An older helper answers with its own error and then usage; only the
  // error line is useful to hand back to the model.
  const usage = await runHelper(process.execPath, [
    "-e",
    'console.error("muis-agent: unknown command: open"); console.error("usage: muis-agent open <URL>"); process.exit(2)',
  ]);
  assert.equal(usage.code, 2);
  assert.equal(usage.reason, "muis-agent: unknown command: open");

  const missing = await runHelper("/nonexistent/muis-agent-xyz", ["open", "http://x/"]);
  assert.equal(missing.ok, false);
  assert.equal(missing.code, null);
  assert.ok(missing.reason.length > 0);
});

test("setup registers the pane tool when opencode has a tool domain", async () => {
  const added = [];
  const ctx = {
    session: { hook: async () => {} },
    tool: { transform: async (fn) => fn({ add: (def) => added.push(def) }) },
    event: { subscribe: () => (async function* () {})() },
  };
  const cleanup = await plugin.setup(ctx);
  assert.deepEqual(
    added.map((t) => t.name),
    ["muis_open_url"],
  );
  assert.equal(typeof cleanup, "function");
});

test("setup still loads on a build without the tool domain", async () => {
  const ctx = {
    session: { hook: async () => {} },
    event: { subscribe: () => (async function* () {})() },
  };
  const cleanup = await plugin.setup(ctx);
  assert.equal(typeof cleanup, "function");
});
