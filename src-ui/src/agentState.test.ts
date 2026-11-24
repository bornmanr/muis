import { describe, it, expect } from "vitest";
import {
  AgentStateTracker,
  agentLabel,
  isAgentState,
  stateChip,
  stateRank,
  WORKING_TTL_MS,
  type AgentReport,
} from "./agentState";

function report(over: Partial<AgentReport> = {}): AgentReport {
  return {
    state: "working",
    agent: "opencode",
    model: null,
    session: null,
    at: 1_000,
    ...over,
  };
}

describe("isAgentState", () => {
  it("accepts exactly the four states", () => {
    for (const s of ["working", "blocked", "done", "idle"]) {
      expect(isAgentState(s)).toBe(true);
    }
    for (const s of ["", "running", "waiting", "DONE", null, 4, undefined]) {
      expect(isAgentState(s)).toBe(false);
    }
  });
});

describe("AgentStateTracker", () => {
  it("reports and reads back the latest state", () => {
    const t = new AgentStateTracker();
    expect(t.stateOf("t1", 1_000)).toBe(null);
    t.report("t1", report({ state: "blocked" }));
    expect(t.stateOf("t1", 1_000)).toBe("blocked");
    t.report("t1", report({ state: "working", at: 2_000 }));
    expect(t.stateOf("t1", 2_000)).toBe("working");
  });

  it("expires a stale working state to idle", () => {
    const t = new AgentStateTracker();
    t.report("t1", report({ state: "working", at: 1_000 }));
    expect(t.stateOf("t1", 1_000 + WORKING_TTL_MS)).toBe("working");
    expect(t.stateOf("t1", 1_000 + WORKING_TTL_MS + 1)).toBe("idle");
  });

  it("never expires blocked or done", () => {
    const t = new AgentStateTracker();
    t.report("t1", report({ state: "blocked", at: 1_000 }));
    t.report("t2", report({ state: "done", at: 1_000 }));
    const later = 1_000 + WORKING_TTL_MS * 100;
    expect(t.stateOf("t1", later)).toBe("blocked");
    expect(t.stateOf("t2", later)).toBe("done");
  });

  it("orders the waiting queue blocked-first then oldest-first", () => {
    const t = new AgentStateTracker();
    t.report("done-new", report({ state: "done", at: 5_000 }));
    t.report("blocked-new", report({ state: "blocked", at: 4_000 }));
    t.report("done-old", report({ state: "done", at: 2_000 }));
    t.report("blocked-old", report({ state: "blocked", at: 3_000 }));
    t.report("busy", report({ state: "working", at: 1_000 }));
    expect(t.waiting(5_000).map((w) => w.tabId)).toEqual([
      "blocked-old",
      "blocked-new",
      "done-old",
      "done-new",
    ]);
  });

  it("drops a demoted working tab out of the waiting queue", () => {
    const t = new AgentStateTracker();
    t.report("t1", report({ state: "working", at: 1_000 }));
    expect(t.waiting(1_000 + WORKING_TTL_MS + 1)).toEqual([]);
  });

  it("expires stale working reports in place and says so", () => {
    const t = new AgentStateTracker();
    t.report("busy", report({ state: "working", at: 1_000 }));
    t.report("waiting", report({ state: "blocked", at: 1_000 }));
    expect(t.expireWorking(1_000 + WORKING_TTL_MS)).toBe(false);
    expect(t.expireWorking(1_000 + WORKING_TTL_MS + 1)).toBe(true);
    // The report itself was rewritten, so a later read is still idle.
    expect(t.get("busy")?.state).toBe("idle");
    expect(t.stateOf("waiting", 1_000 + WORKING_TTL_MS * 10)).toBe("blocked");
    expect(t.expireWorking(1_000 + WORKING_TTL_MS + 2)).toBe(false);
  });

  it("forgets a tab on clear", () => {    const t = new AgentStateTracker();
    t.report("t1", report({ state: "blocked" }));
    t.clear("t1");
    expect(t.stateOf("t1", 2_000)).toBe(null);
    expect(t.waiting(2_000)).toEqual([]);
  });
});

describe("labels", () => {
  it("joins the known parts of the agent label", () => {
    expect(agentLabel(report({ agent: "opencode", model: "opus" }))).toBe("opencode · opus");
    expect(agentLabel(report({ agent: "opencode", model: null }))).toBe("opencode");
    expect(agentLabel(report({ agent: null, model: null }))).toBe(null);
    expect(agentLabel(undefined)).toBe(null);
  });

  it("only chips states that ask something of the user", () => {
    expect(stateChip("blocked")).toBe("needs you");
    expect(stateChip("working")).toBe("working");
    expect(stateChip("done")).toBe(null);
    expect(stateChip("idle")).toBe(null);
    expect(stateChip(null)).toBe(null);
  });

  it("ranks blocked before done before working", () => {
    expect(stateRank("blocked")).toBeLessThan(stateRank("done"));
    expect(stateRank("done")).toBeLessThan(stateRank("working"));
    expect(stateRank("working")).toBeLessThan(stateRank("idle"));
  });
});
