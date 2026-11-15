// muis-plugin v9
// muis opencode server plugin: notify muis when a turn finishes or when
// opencode pauses for input, so the tab gets the waiting mark + toast, and
// report the exact session id as a fallback when the TUI plugin is
// unavailable.
//
// It also reports the other direction: an admitted prompt, a replied
// permission, or a replied form mean opencode picked the work back up, so
// muis clears the tab's waiting mark. The mark therefore survives merely
// looking at the tab and only goes when the user replies or the tab closes.
//
// Every report carries the opencode session id. The server plugin runs in
// the shared `opencode serve` process, so `MUIS_TAB_ID` there belongs to
// whichever tab started that server; only the session id identifies the tab
// that is actually waiting. muis routes by session first, then by tab.
//
// Installed by muis at ~/.config/opencode/plugins/muis.js. opencode 2.x only
// loads a plugin from that directory when the module default-exports a
// definition with an `id` and a `setup` function, so the V1
// `export const MuisPlugin = async ({ $ }) => hooks` shape no longer loads.
// There is no Bun `$` in the V2 plugin context, so helpers are spawned
// directly; nothing waits on them.
//
// opencode 2.3 removed the `session.idle` event from normal turns: a turn
// now ends with `session.execution.succeeded|failed|interrupted`. A question
// or permission request pauses the turn without ending it, so it never
// produces a completion event at all; `form.created` and `permission.asked`
// are the only signals that the tab is waiting for the user.
//
// It calls the `muis-notify` and `muis-agent` binaries, which muis ships and
// routes back to the tab via MUIS_TAB_ID (injected into every pty). No-op
// when muis is not running. The "muis-plugin" marker lets muis safely update
// its own file.

import { spawn } from "node:child_process";

/** Events this plugin keeps state for or turns into notifications. */
/** Events that mean the user answered and the agent is working again. */
const REPLY_EVENTS = ["permission.replied", "permission.rejected", "form.replied"];

export const SESSION_EVENTS = [
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
];

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

/** Project name from a directory, for the notification title. */
function projectOf(directory) {
  return typeof directory === "string" && directory
    ? directory.replace(/\/+$/, "").split("/").pop()
    : "";
}

/** The permission or action name carried by a `permission.asked` event. */
export function permissionOf(data) {
  const raw = data?.permission ?? data?.action;
  return typeof raw === "string" ? raw.trim() : "";
}

/**
 * Session bookkeeping and the helper calls it triggers. Both helpers are
 * injected so the behavior is testable outside opencode.
 */
export function createMuisPlugin({ reportAgent, notify }) {
  // Session id -> { title, directory, parentID }, learned from
  // session.created/renamed (turn events only carry the id), so the
  // notification can name the project and the session.
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

  /** Where (project/tab) and what (session title) a notification is about. */
  const describe = (id, event) => {
    const s = sessions.get(id) ?? {};
    const project = projectOf(s.directory || event?.location?.directory);
    return {
      title: project || "opencode",
      summary: typeof s.title === "string" ? s.title.trim() : "",
    };
  };

  return {
    /** One event from the opencode event stream. */
    event(event) {
      const type = event?.type;
      if (!SESSION_EVENTS.includes(type)) return;
      const data = event.data ?? {};

      if (type === "permission.asked") {
        const id = data.sessionID;
        if (typeof id !== "string") return;
        const where = describe(id, event);
        const what = permissionOf(data);
        const body = `${where.summary || "opencode"} needs permission`;
        notify([
          "--title",
          where.title,
          "--body",
          what ? `${body}: ${what}` : body,
          "--session",
          id,
        ]);
        return;
      }

      if (type === "form.created") {
        const form = data.form ?? {};
        const id = form.sessionID;
        if (typeof id !== "string") return;
        const where = describe(id, event);
        const what = typeof form.title === "string" ? form.title.trim() : "";
        const body = `${where.summary || "opencode"} is waiting for your input`;
        notify([
          "--title",
          where.title,
          "--body",
          what ? `${body}: ${what}` : body,
          "--session",
          id,
        ]);
        return;
      }

      // The user answered: the tab is no longer waiting on them.
      if (REPLY_EVENTS.includes(type)) {
        const id = data.sessionID;
        if (typeof id === "string") notify(["--running", "--session", id]);
        return;
      }

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

      // A turn ended. opencode 2.3 reports execution events; older builds
      // still send session.idle. A failed turn gets its own wording so the
      // notification does not claim work succeeded.
      const where = describe(id, event);
      const failed = type === "session.execution.failed";
      const body = failed
        ? `${where.summary || "opencode"} failed`
        : where.summary
          ? `${where.summary} has finished`
          : "opencode has finished";
      notify(["--title", where.title, "--body", body, "--session", id]);
    },

    /**
     * Prompt admission hook: report the session about to receive a turn. The
     * turn is running again, so the tab stops waiting — this also covers
     * older opencode builds without `session.execution.started`.
     */
    prompt(sessionID) {
      report(sessionID);
      notify(["--running", "--session", sessionID]);
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
