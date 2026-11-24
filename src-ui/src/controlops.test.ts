import { describe, expect, it } from "vitest";
import { sessionPayload, tailLines, trimTrailingBlank } from "./controlops";
import type { Workspace } from "./sessions";

const ws: Workspace = {
  id: "w1",
  name: "home",
  dir: "/home/user",
  active: 1,
  tabs: [
    { id: "t1", title: "Terminal 1", cwd: "/home/user" },
    { id: "t2", title: "opencode", cwd: "/home/user/project", pinned: true },
  ],
};

describe("sessionPayload", () => {
  it("marks the current session and active tab, and carries agent state", () => {
    const payload = sessionPayload([ws, { ...ws, id: "w2", name: "other", tabs: [] }], 0, (tab) =>
      tab === "t2" ? { state: "blocked", agent: "opencode", model: "opus" } : null,
    );
    expect(payload.map((s) => s.id)).toEqual(["w1", "w2"]);
    expect(payload[0].active).toBe(true);
    expect(payload[1].active).toBe(false);
    expect(payload[0].tabs.map((t) => t.active)).toEqual([false, true]);
    expect(payload[0].tabs[1].pinned).toBe(true);
    expect(payload[0].tabs[1].agent).toEqual({
      state: "blocked",
      agent: "opencode",
      model: "opus",
    });
    expect(payload[0].tabs[0].agent).toBeNull();
  });

  it("is JSON-safe: no undefined fields leak", () => {
    const payload = sessionPayload([ws], 0, () => null);
    const round = JSON.parse(JSON.stringify(payload));
    expect(round[0].tabs[1].pinned).toBe(true);
    expect(round[0].tabs[0].pinned).toBe(false);
  });
});

describe("tailLines", () => {
  it("keeps the last n in order and handles edges", () => {
    expect(tailLines(["a", "b", "c"], 2)).toEqual(["b", "c"]);
    expect(tailLines(["a"], 10)).toEqual(["a"]);
    expect(tailLines(["a"], 0)).toEqual([]);
    expect(tailLines(["a"], -3)).toEqual([]);
  });
});

describe("trimTrailingBlank", () => {
  it("drops only the trailing blanks", () => {
    expect(trimTrailingBlank(["a", "", "b", "", "  "])).toEqual(["a", "", "b"]);
    expect(trimTrailingBlank(["", ""])).toEqual([]);
  });
});
