/**
 * Unit tests for the opencode TUI route tracker. The plugin runs inside the
 * TUI, so we drive the exported tracker and the polling loop directly
 * instead of starting a TUI.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  agentInvocation,
  createRouteTracker,
  inMuis,
  routeSessionID,
  startRouteTracking,
} from "../../crates/muis-shell/opencode-plugin/muis-tui.js";
import plugin from "../../crates/muis-shell/opencode-plugin/muis-tui.js";

function tracker() {
  const reported = [];
  const cleared = [];
  const track = createRouteTracker(
    (id) => reported.push(id),
    () => cleared.push(true),
  );
  return { track, reported, cleared };
}

const sessionRoute = (id) => ({ type: "session", sessionID: id });

test("default export is a v2 tui definition", () => {
  // opencode 2.x rejects anything but { id, setup } with
  // "Invalid V2 TUI plugin module", and only reports it in the TUI log.
  assert.equal(typeof plugin, "object");
  assert.equal(plugin.id, "muis-tui");
  assert.equal(typeof plugin.setup, "function");
  // The V1 entrypoint shape must be gone.
  assert.equal("tui" in plugin, false);
});

test("routes only inside a muis tab", () => {
  assert.equal(inMuis({}), false);
  assert.equal(inMuis({ MUIS_TAB_ID: "" }), false);
  assert.equal(inMuis({ MUIS_TAB_ID: "t47" }), true);
});

test("reads the session id out of a v2 route", () => {
  assert.equal(routeSessionID(sessionRoute("ses_first")), "ses_first");
  assert.equal(routeSessionID({ type: "home" }), null);
  assert.equal(routeSessionID({ type: "plugin", id: "p", name: "n" }), null);
  assert.equal(routeSessionID({ type: "session", sessionID: "nope" }), null);
  assert.equal(routeSessionID(undefined), null);
});

test("reports session route changes exactly once", () => {
  const { track, reported } = tracker();
  track(sessionRoute("ses_first"));
  track(sessionRoute("ses_first"));
  track(sessionRoute("ses_second"));
  assert.deepEqual(reported, ["ses_first", "ses_second"]);
});

test("does not clear on the initial home route", () => {
  const { track, cleared } = tracker();
  track({ type: "home" });
  assert.deepEqual(cleared, []);
});

test("clears after leaving a session", () => {
  const { track, reported, cleared } = tracker();
  track(sessionRoute("ses_first"));
  track({ type: "home" });
  assert.deepEqual(reported, ["ses_first"]);
  assert.deepEqual(cleared, [true]);
});

test("ignores malformed route ids", () => {
  const { track, reported } = tracker();
  track({ type: "session", sessionID: "nope" });
  track({ type: "session" });
  track({ type: "plugin", sessionID: "ses_plugin" });
  assert.deepEqual(reported, []);
});

test("polls the router, reports once, and stops on cleanup", () => {
  const sent = [];
  let route = sessionRoute("ses_first");
  let reads = 0;
  const stop = startRouteTracking(() => (reads++, route), {
    interval: 5,
    send: (args) => sent.push(args),
  });
  assert.deepEqual(sent, [["session", "ses_first"]]);

  route = { type: "home" };
  return new Promise((resolve) => setTimeout(resolve, 20)).then(() => {
    stop();
    assert.ok(reads > 1, "kept polling while active");
    assert.deepEqual(sent, [["session", "ses_first"], ["clear"]]);
    const after = reads;
    return new Promise((r) => setTimeout(r, 20)).then(() => {
      assert.equal(reads, after, "stopped polling after cleanup");
    });
  });
});

test("ignores a router that is not ready yet", () => {
  const sent = [];
  const stop = startRouteTracking(
    () => {
      throw new Error("router missing");
    },
    { interval: 1000, send: (args) => sent.push(args) },
  );
  stop();
  assert.deepEqual(sent, []);
});

test("picks the agent CLI for sidecar and Windows single-exe builds", () => {
  assert.deepEqual(agentInvocation({}), { command: "muis-agent", args: [] });
  assert.deepEqual(agentInvocation({ MUIS_BIN: "/opt/muis/muis" }), {
    command: "/opt/muis/muis",
    args: ["--muis-agent"],
  });
  assert.deepEqual(
    agentInvocation({ MUIS_AGENT_BIN: "/opt/muis/muis-agent", MUIS_BIN: "/opt/muis/muis" }),
    {
      command: "/opt/muis/muis-agent",
      args: [],
    },
  );
});
