/**
 * Pure shapes for the control surface (`muis-ctl ui …`). The daemon forwards
 * an op to the window; these functions turn window state into the JSON the
 * caller gets back, so the payload is testable without a terminal.
 */

import type { AgentState } from "./agentState";
import type { Workspace } from "./sessions";

export interface ControlAgent {
  state: AgentState | null;
  agent: string | null;
  model: string | null;
}

export interface ControlTab {
  id: string;
  title: string;
  cwd: string;
  active: boolean;
  pinned: boolean;
  agent: ControlAgent | null;
}

export interface ControlSession {
  id: string;
  name: string;
  dir: string;
  active: boolean;
  tabs: ControlTab[];
}

/** One entry per session, with the active tab and whatever the agent said. */
export function sessionPayload(
  workspaces: ReadonlyArray<Workspace>,
  current: number,
  agentOf: (tabId: string) => ControlAgent | null,
): ControlSession[] {
  return workspaces.map((ws, index) => ({
    id: ws.id,
    name: ws.name,
    dir: ws.dir,
    active: index === current,
    tabs: ws.tabs.map((tab, i) => ({
      id: tab.id,
      title: tab.title,
      cwd: tab.cwd,
      active: i === ws.active,
      pinned: Boolean(tab.pinned),
      agent: agentOf(tab.id),
    })),
  }));
}

/** Last `count` lines, keeping order. `count` is clamped by the caller. */
export function tailLines(lines: ReadonlyArray<string>, count: number): string[] {
  const n = Math.max(0, Math.floor(count));
  return n === 0 ? [] : lines.slice(-n);
}

/** Drop the trailing blank rows xterm keeps below the cursor. */
export function trimTrailingBlank(lines: ReadonlyArray<string>): string[] {
  let end = lines.length;
  while (end > 0 && lines[end - 1].trim() === "") end--;
  return lines.slice(0, end);
}
