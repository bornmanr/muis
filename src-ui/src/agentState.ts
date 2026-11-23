/**
 * The lifecycle state an agent reports about itself.
 *
 * Reports arrive from the agent's own hooks (`muis-agent report`) or the
 * OSC 777 fallback. This is deliberately separate from `DoneTracker`: that
 * one is a UI-side "this needs attention" flag fed by completions, while
 * this is what the agent says it is doing. `blocked` is the state that
 * matters — it is the only one that cannot be inferred from output and the
 * only one where the user's input is required.
 *
 * `working` expires on its own: an agent that crashes or is killed stops
 * reporting, and a tab must not claim to be working forever.
 */

export type AgentState = "working" | "blocked" | "done" | "idle";

export const AGENT_STATES: readonly AgentState[] = ["working", "blocked", "done", "idle"];

/** True for one of the four states. Used to validate untrusted reports. */
export function isAgentState(value: unknown): value is AgentState {
  return typeof value === "string" && (AGENT_STATES as readonly string[]).includes(value);
}

export interface AgentReport {
  state: AgentState;
  /** Agent name, e.g. `opencode`. */
  agent: string | null;
  /** Model id, when the agent exposes one. */
  model: string | null;
  /** Agent session the report belongs to, when routed by session. */
  session: string | null;
  /** Wall clock (ms) when the report arrived. */
  at: number;
}

/**
 * A `working` report older than this is treated as idle. Long turns are
 * fine: a running agent keeps reporting, and the terminal's own activity
 * heuristic still shows motion. Only a silent agent goes quiet.
 */
export const WORKING_TTL_MS = 90_000;

export class AgentStateTracker {
  private reports = new Map<string, AgentReport>();

  report(tabId: string, report: AgentReport): void {
    this.reports.set(tabId, report);
  }

  get(tabId: string): AgentReport | undefined {
    return this.reports.get(tabId);
  }

  /** Effective state at `now`, with a stale `working` demoted to `idle`. */
  stateOf(tabId: string, now: number): AgentState | null {
    const report = this.reports.get(tabId);
    if (!report) return null;
    if (report.state === "working" && now - report.at > WORKING_TTL_MS) return "idle";
    return report.state;
  }

  /** Drop the state without forgetting the tab (agent exited). */
  clear(tabId: string): void {
    this.reports.delete(tabId);
  }

  /**
   * Demote every stale `working` report to `idle` in place. Returns true
   * when something changed, so the one-second tick can skip rendering.
   */
  expireWorking(now: number): boolean {
    let changed = false;
    for (const [tabId, report] of this.reports) {
      if (report.state === "working" && now - report.at > WORKING_TTL_MS) {
        this.reports.set(tabId, { ...report, state: "idle" });
        changed = true;
      }
    }
    return changed;
  }

  forget(tabId: string): void {
    this.reports.delete(tabId);
  }

  /**
   * Tabs that need the user, `blocked` first and then oldest-first, which is
   * the order the needs-you queue shows them in.
   */
  waiting(now: number): Array<{ tabId: string; state: "blocked" | "done"; at: number }> {
    const out: Array<{ tabId: string; state: "blocked" | "done"; at: number }> = [];
    for (const [tabId, report] of this.reports) {
      const state = this.stateOf(tabId, now);
      if (state === "blocked" || state === "done") {
        out.push({ tabId, state, at: report.at });
      }
    }
    out.sort((a, b) => stateRank(a.state) - stateRank(b.state) || a.at - b.at);
    return out;
  }
}

/** Rank for sorting: a blocked tab beats a finished one. */
export function stateRank(state: AgentState): number {
  if (state === "blocked") return 0;
  if (state === "done") return 1;
  if (state === "working") return 2;
  return 3;
}

/** Short label for a tab chip; `null` when the state deserves no chip. */
export function stateChip(state: AgentState | null): string | null {
  if (state === "blocked") return "needs you";
  if (state === "working") return "working";
  return null;
}

/** Tooltip text: `opencode · opus`, or whichever half is known. */
export function agentLabel(report: AgentReport | undefined): string | null {
  if (!report) return null;
  const parts = [report.agent, report.model].filter(
    (p): p is string => typeof p === "string" && p.length > 0,
  );
  return parts.length > 0 ? parts.join(" · ") : null;
}
