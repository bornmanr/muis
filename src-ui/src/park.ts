/**
 * Parking policy: which idle agent tabs may have their process closed.
 *
 * Parking exists because a finished tab should not hold an agent process
 * (and its memory) until the window closes. The tab stays, the pty goes,
 * and clicking the tab resumes the saved agent session — so the rule must be
 * conservative: only a tab that is off screen, owned by a resumable agent
 * session, finished or idle, and untouched for a long time is eligible.
 *
 * `blocked` is deliberately near-unparkable: a tab waiting for an approval
 * is the one case where killing the process loses the user's place.
 */

import type { AgentState } from "./agentState";

export interface ParkInput {
  /** What the agent last reported, or null when it never reported. */
  state: AgentState | null;
  /** The tab is the visible one in its session. */
  visible: boolean;
  /** The pty already exited on its own. */
  exited: boolean;
  /** A resumable agent session id is known for the tab. */
  hasAgentSession: boolean;
  /** Milliseconds since the user last typed in the tab. */
  idleMs: number;
  /** The configured idle threshold (PARK_IDLE_MS by default). */
  idleLimitMs: number;
  /** Milliseconds since the last agent report, null when never. */
  sinceReportMs: number | null;
}

/** No keystrokes for this long makes a finished tab eligible. */
export const PARK_IDLE_MS = 30 * 60_000;

/** A blocked tab is only parked after this much idle time (8 hours). */
export const PARK_BLOCKED_GRACE_MS = 8 * 60 * 60_000;

/** Never park right after the agent reported: give the user time to look. */
export const PARK_REPORT_GRACE_MS = 5 * 60_000;

export function shouldPark(input: ParkInput): boolean {
  if (input.exited || input.visible) return false;
  if (!input.hasAgentSession) return false;
  // Unknown state means no hook ever reported: never guess with a kill.
  if (!input.state) return false;
  if (input.state === "working") return false;
  if (input.idleMs < input.idleLimitMs) return false;
  if (input.sinceReportMs !== null && input.sinceReportMs < PARK_REPORT_GRACE_MS) return false;
  if (input.state === "blocked") {
    return input.idleMs >= Math.max(input.idleLimitMs, PARK_BLOCKED_GRACE_MS);
  }
  return true;
}
