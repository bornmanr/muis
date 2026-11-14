// muis-plugin v6
// muis opencode server plugin: notify muis when a turn finishes so the tab
// gets the done badge + toast, and report the exact session id as a fallback
// when the TUI plugin is unavailable.
//
// Installed by muis at ~/.config/opencode/plugins/muis.js. opencode 2.x only
// loads a plugin from that directory when the module default-exports a
// definition with an `id` and a `setup` function, so the V1
// `export const MuisPlugin = async ({ $ }) => hooks` shape no longer loads.
// There is no Bun `$` in the V2 plugin context, so helpers are spawned
// directly; nothing waits on them.
//
// It calls the `muis-notify` and `muis-agent` binaries, which muis ships and
// routes back to the tab via MUIS_TAB_ID (injected into every pty). No-op
// when muis is not running. The "muis-plugin" marker lets muis safely update
// its own file.

import { spawn } from "node:child_process";

/** Session events this plugin keeps state for. */
export const SESSION_EVENTS = ["session.created", "session.renamed", "session.idle"];

/** Fire one helper and never block opencode. */
function send(command, args) {
  try {
    const child = spawn(command, args, { stdio: "ignore", windowsHide: true });
    child.on("error", () => {});
    child.unref();
  } catch {
    // muis not running, or the helper is missing: nothing to do.
  }
}

/** Report a session id to muis (`muis-agent`, or the single-exe build). */
function reportAgent(args) {
  const agentBin = process.env.MUIS_AGENT_BIN;
  if (agentBin) return send(agentBin, args);
  const bin = process.env.MUIS_BIN;
  send(bin || "muis-agent", bin ? ["--muis-agent", ...args] : args);
}

/** Raise a desktop toast in muis. */
function notify(args) {
  send("muis-notify", args);
}

/**
 * Session bookkeeping and the helper calls it triggers. Both helpers are
 * injected so the behavior is testable outside opencode.
 */
export function createMuisPlugin({ reportAgent, notify }) {
  // Session id -> { title, directory, parentID }, learned from
  // session.created/renamed (session.idle itself only carries the id), so
  // the notification can name the project and the session.
  const sessions = new Map();
  let lastSession = "";

  // Server-side navigation is invisible to plugins, so the prompt hook only
  // catches turns. The TUI plugin is the exact source for /resume.
  const report = (id) => {
    if (!id || typeof id !== "string" || !id.startsWith("ses_") || id === lastSession) return;
    const info = sessions.get(id);
    if (info?.parentID) return; // subagent session, not the tab's chat
    lastSession = id;
    const args = ["session", id];
    if (info?.directory) args.push("--dir", info.directory);
    reportAgent(args);
  };

  return {
    /** One `session.*` event from the opencode event stream. */
    event(event) {
      const type = event?.type;
      if (!SESSION_EVENTS.includes(type)) return;
      const data = event.data ?? {};
      const id = data.sessionID;
      if (typeof id !== "string") return;

      if (type === "session.created") {
        const directory = data.location?.directory ?? event.location?.directory ?? "";
        sessions.set(id, {
          title: typeof data.title === "string" ? data.title : "",
          directory: typeof directory === "string" ? directory : "",
          parentID: typeof data.parentID === "string" ? data.parentID : "",
        });
        if (!data.parentID) report(id);
        return;
      }

      if (type === "session.renamed") {
        const known = sessions.get(id);
        if (known && typeof data.title === "string") known.title = data.title;
        return;
      }

      // session.idle: a turn ended. Identify where (project/tab) in the title
      // and what (the session title) in the body, instead of a constant text.
      const s = sessions.get(id) ?? {};
      const dir = typeof s.directory === "string" ? s.directory : "";
      const project = dir ? dir.replace(/\/+$/, "").split("/").pop() : "";
      const title = project || "opencode";
      const summary = typeof s.title === "string" ? s.title.trim() : "";
      const body = summary ? `${summary} has finished` : "opencode has finished";
      notify(["--title", title, "--body", body]);
    },

    /** Prompt admission hook: report the session about to receive a turn. */
    prompt(sessionID) {
      report(sessionID);
    },
  };
}

/** Feed events from the subscription into the plugin until it unloads. */
async function consume(events, plugin) {
  try {
    for await (const event of events) plugin.event(event);
  } catch {
    // The subscription ends when the plugin unloads.
  }
}

export default {
  id: "muis",
  async setup(ctx) {
    const plugin = createMuisPlugin({ reportAgent, notify });
    await ctx.session.hook("prompt", (event) => plugin.prompt(event?.sessionID));
    const controller = new AbortController();
    void consume(ctx.event.subscribe({ types: SESSION_EVENTS, signal: controller.signal }), plugin);
    return () => controller.abort();
  },
};
