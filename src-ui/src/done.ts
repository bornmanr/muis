/**
 * Tracks tabs with a completion the user has not dealt with yet, so the
 * chrome can badge them and the window title can count them.
 *
 * Two states keep the chrome calm: a fresh alert is `unseen` (the sidebar
 * counts and pulses it), and looking at the tab demotes it to `seen` (the
 * tab keeps a steady "waiting" mark and the sidebar only shows a dot). The
 * alert goes away when the work is picked back up — the agent reports that
 * it is running again, or the next command starts — never because the user
 * merely glanced at the tab.
 */
export type AlertState = "unseen" | "seen";

export class DoneTracker {
  private alerts = new Map<string, AlertState>();

  /** A completion arrived for a tab the user was not looking at. */
  mark(id: string): void {
    this.alerts.set(id, "unseen");
  }

  /** The user looked at the tab: keep the alert, but stop shouting. */
  see(id: string): void {
    if (this.alerts.get(id) === "unseen") this.alerts.set(id, "seen");
  }

  clear(id: string): void {
    this.alerts.delete(id);
  }

  has(id: string): boolean {
    return this.alerts.has(id);
  }

  /** Whether the tab still needs a counted, pulsing indicator. */
  isUnseen(id: string): boolean {
    return this.alerts.get(id) === "unseen";
  }

  count(): number {
    return this.alerts.size;
  }

  forget(id: string): void {
    this.alerts.delete(id);
  }
}

/** One tab's contribution to the chrome, from most to least urgent. */
export type Attention = "unseen" | "seen" | null;

/** What a session row shows: a pulsing count, a steady dot, or nothing. */
export interface SidebarAttention {
  /** Number of fresh alerts, rendered as a counting badge. */
  count: number;
  /** Any pending work at all, rendered as a dot. */
  dot: boolean;
}

/**
 * Sidebar indicator for one session. Unseen alerts win with a count; work
 * the user already looked at, or a dead pty, only earns a steady dot. The
 * session on screen stays quiet — its tab bar already shows the marks.
 */
export function sidebarAttention(tabs: Attention[], selected: boolean): SidebarAttention {
  if (selected) return { count: 0, dot: false };
  const count = tabs.filter((s) => s === "unseen").length;
  return { count, dot: count > 0 || tabs.some((s) => s !== null) };
}

/**
 * Chip text for a tab: a dead pty is "done", pending work the user has not
 * handled yet is "waiting", everything else shows no chip.
 */
export function tabAttentionLabel(state: Attention, exited: boolean): string | null {
  if (exited) return "✓ done";
  return state === null ? null : "waiting";
}
