// muis-tui-plugin v2
// muis opencode TUI plugin: watch the TUI route and report the session that
// is actually on screen. Server plugins never see user-driven navigation
// (opencode only publishes `tui.session.select` for inbound API calls), so
// this is the only way to follow `/new` and `/resume` exactly.
//
// Installed by muis at ~/.config/opencode/plugins/muis-tui/tui.js. opencode
// 2.x discovers TUI entrypoints from plugin directories under its global
// plugin dir and only loads a module whose default export is a definition
// with an `id` and a `setup` function; the V1 `{ id, tui(api) }` module is
// rejected with "Invalid V2 TUI plugin module", and the V1 `api.route`
// store is gone. The route now comes from `setup(ctx)`'s `ctx.ui.router`,
// which is polled because `router.current()` is not reactive.
//
// It calls `muis-agent` (or `MUIS_BIN --muis-agent` on the Windows single-exe
// build); the tab id comes from MUIS_TAB_ID, injected into every pty by muis.
// No-op outside muis.
import { spawn } from "node:child_process";

const SESSION_RE = /^ses_[A-Za-z0-9]+$/;
const POLL_MS = 250;

/** Resolve the agent CLI for this process (Windows single-exe vs sidecar). */
export function agentInvocation(env = process.env) {
  if (env.MUIS_AGENT_BIN) return { command: env.MUIS_AGENT_BIN, args: [] };
  const bin = env.MUIS_BIN;
  return { command: bin || "muis-agent", args: bin ? ["--muis-agent"] : [] };
}

/** Fire one request and never block the TUI. */
export function sendAgent(args, env = process.env) {
  const { command, args: prefix } = agentInvocation(env);
  try {
    const child = spawn(command, [...prefix, ...args], {
      stdio: "ignore",
      windowsHide: true,
      shell: process.platform === "win32",
    });
    child.on("error", () => {});
    child.unref();
  } catch {
    // muis not running, or the helper is missing — nothing to do.
  }
}

/** Session id of a V2 TUI route (`{ type: "session", sessionID }`). */
export function routeSessionID(route) {
  if (route?.type !== "session") return null;
  const raw = route.sessionID;
  return typeof raw === "string" && SESSION_RE.test(raw) ? raw : null;
}

/**
 * muis injects `MUIS_TAB_ID` into every pty. Without it the TUI is not one of
 * muis's tabs, so there is nothing to report and no timer to run.
 */
export function inMuis(env = process.env) {
  return typeof env.MUIS_TAB_ID === "string" && env.MUIS_TAB_ID !== "";
}

/**
 * Track route changes. `report(id)` runs when a valid session becomes
 * current; `clear()` runs only after a session was seen and the TUI moves
 * away from it. The initial route is never cleared, so a restored tab keeps
 * its saved id while opencode boots into it.
 */
export function createRouteTracker(report, clear) {
  let last;
  return (route) => {
    const id = routeSessionID(route);
    if (id === last) return;
    if (id) report(id);
    else if (last !== undefined) clear();
    last = id;
  };
}

/**
 * Poll the TUI router and report every session it lands on. Returns the
 * cleanup function opencode calls when the plugin unloads. `readRoute` and
 * `send` are injected so the loop stays testable outside opencode.
 */
export function startRouteTracking(
  readRoute,
  { env = process.env, interval = POLL_MS, send = sendAgent } = {},
) {
  const track = createRouteTracker(
    (id) => send(["session", id], env),
    () => send(["clear"], env),
  );
  const tick = () => {
    try {
      track(readRoute());
    } catch {
      // Router store not ready yet.
    }
  };
  tick();
  const timer = setInterval(tick, interval);
  return () => clearInterval(timer);
}

export default {
  id: "muis-tui",
  setup(context) {
    // Outside muis there is no tab to report to; skip the poll loop instead
    // of running a timer in every other opencode TUI on the machine.
    if (!inMuis()) return;
    return startRouteTracking(() => context.ui.router.current());
  },
};
