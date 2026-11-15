import { describe, expect, it } from "vitest";
import { DoneTracker, sidebarAttention, tabAttentionLabel } from "./done";

describe("DoneTracker", () => {
  it("marks, counts, clears and forgets", () => {
    const d = new DoneTracker();
    expect(d.count()).toBe(0);
    d.mark("a");
    d.mark("b");
    expect(d.count()).toBe(2);
    expect(d.has("a")).toBe(true);
    d.clear("a");
    expect(d.has("a")).toBe(false);
    expect(d.count()).toBe(1);
    d.forget("b");
    expect(d.count()).toBe(0);
  });

  it("marking twice is idempotent", () => {
    const d = new DoneTracker();
    d.mark("a");
    d.mark("a");
    expect(d.count()).toBe(1);
  });

  it("demotes an unseen alert to seen when the tab is looked at", () => {
    const d = new DoneTracker();
    d.mark("a");
    expect(d.isUnseen("a")).toBe(true);
    d.see("a");
    expect(d.isUnseen("a")).toBe(false);
    // Seen is still pending: the tab keeps its mark until the user works.
    expect(d.has("a")).toBe(true);
    expect(d.count()).toBe(1);
  });

  it("marks a completion that landed in a watched tab as already seen", () => {
    const d = new DoneTracker();
    d.mark("a", "seen");
    expect(d.has("a")).toBe(true);
    expect(d.isUnseen("a")).toBe(false);
  });

  it("a seen alert becomes unseen again on a new completion", () => {
    const d = new DoneTracker();
    d.mark("a");
    d.see("a");
    d.mark("a");
    expect(d.isUnseen("a")).toBe(true);
  });

  it("ignores see for tabs without an alert", () => {
    const d = new DoneTracker();
    d.see("a");
    expect(d.has("a")).toBe(false);
    expect(d.isUnseen("a")).toBe(false);
  });
});

describe("sidebarAttention", () => {
  it("counts unseen alerts", () => {
    expect(sidebarAttention(["unseen", null], false)).toEqual({ count: 1, dot: true });
    expect(sidebarAttention(["unseen", "seen"], false)).toEqual({ count: 1, dot: true });
  });

  it("shows only a dot once the alert was seen", () => {
    expect(sidebarAttention(["seen", null], false)).toEqual({ count: 0, dot: true });
  });

  it("stays quiet with nothing pending or while the session is on screen", () => {
    expect(sidebarAttention([null, null], false)).toEqual({ count: 0, dot: false });
    expect(sidebarAttention(["unseen", "seen"], true)).toEqual({ count: 0, dot: false });
  });
});

describe("tabAttentionLabel", () => {
  it("says waiting for pending work and done for a dead pty", () => {
    expect(tabAttentionLabel("unseen", false)).toBe("waiting");
    expect(tabAttentionLabel("seen", false)).toBe("waiting");
    expect(tabAttentionLabel(null, true)).toBe("✓ done");
    expect(tabAttentionLabel("seen", true)).toBe("✓ done");
  });

  it("has no chip for a quiet tab", () => {
    expect(tabAttentionLabel(null, false)).toBeNull();
  });
});
