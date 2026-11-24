import { describe, expect, it } from "vitest";
import {
  PARK_BLOCKED_GRACE_MS,
  PARK_IDLE_MS,
  PARK_REPORT_GRACE_MS,
  shouldPark,
  type ParkInput,
} from "./park";

const base: ParkInput = {
  state: "done",
  visible: false,
  exited: false,
  hasAgentSession: true,
  idleMs: PARK_IDLE_MS + 1,
  idleLimitMs: PARK_IDLE_MS,
  sinceReportMs: PARK_REPORT_GRACE_MS + 1,
};

describe("shouldPark", () => {
  it("parks a finished, off-screen, untouched agent tab", () => {
    expect(shouldPark(base)).toBe(true);
    expect(shouldPark({ ...base, state: "idle" })).toBe(true);
  });

  it("never parks the visible tab", () => {
    expect(shouldPark({ ...base, visible: true })).toBe(false);
  });

  it("never parks a working agent", () => {
    expect(shouldPark({ ...base, state: "working" })).toBe(false);
  });

  it("never parks an exited pty or a plain shell", () => {
    expect(shouldPark({ ...base, exited: true })).toBe(false);
    expect(shouldPark({ ...base, hasAgentSession: false })).toBe(false);
  });

  it("never parks without a report: unknown state is not authorization", () => {
    expect(shouldPark({ ...base, state: null })).toBe(false);
  });

  it("waits for the idle window", () => {
    expect(shouldPark({ ...base, idleMs: PARK_IDLE_MS - 1 })).toBe(false);
  });

  it("gives the user a grace period after the agent reports", () => {
    expect(shouldPark({ ...base, sinceReportMs: 0 })).toBe(false);
    expect(shouldPark({ ...base, sinceReportMs: PARK_REPORT_GRACE_MS - 1 })).toBe(false);
    // A tab that has not reported for a long time is still eligible.
    expect(shouldPark({ ...base, sinceReportMs: null })).toBe(true);
  });

  it("keeps a blocked tab alive far longer", () => {
    expect(shouldPark({ ...base, state: "blocked" })).toBe(false);
    expect(
      shouldPark({
        ...base,
        state: "blocked",
        idleMs: PARK_BLOCKED_GRACE_MS - 1,
        sinceReportMs: PARK_BLOCKED_GRACE_MS,
      }),
    ).toBe(false);
    expect(
      shouldPark({
        ...base,
        state: "blocked",
        idleMs: PARK_BLOCKED_GRACE_MS,
        sinceReportMs: PARK_BLOCKED_GRACE_MS,
      }),
    ).toBe(true);
  });
});
