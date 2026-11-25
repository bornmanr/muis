import { describe, expect, it } from "vitest";
import { opencodeCommand, shellQuote, tabTitleFor } from "./agentTab";

describe("shellQuote", () => {
  it("wraps plain text", () => {
    expect(shellQuote("add pagination")).toBe("'add pagination'");
  });

  it("escapes embedded single quotes instead of ending the string", () => {
    expect(shellQuote("it's done")).toBe("'it'\\''s done'");
    expect(shellQuote("'; rm -rf ~")).toBe("''\\''; rm -rf ~'");
    expect(shellQuote("$(whoami) `id`")).toBe("'$(whoami) `id`'");
  });

  it("keeps newlines inside the quotes", () => {
    expect(shellQuote("a\nb")).toBe("'a\nb'");
  });
});

describe("opencodeCommand", () => {
  it("uses the prompt flag and quotes the task", () => {
    expect(opencodeCommand("  add pagination  ")).toBe("opencode --prompt 'add pagination'");
  });

  it("survives quotes and substitutions", () => {
    expect(opencodeCommand("don't `rm` it")).toBe("opencode --prompt 'don'\\''t `rm` it'");
  });
});

describe("tabTitleFor", () => {
  it("takes the first line and collapses spaces", () => {
    expect(tabTitleFor("  add   pagination\nthen tests")).toBe("add pagination");
  });

  it("truncates long tasks", () => {
    const title = tabTitleFor("x".repeat(80));
    expect(title.length).toBe(40);
    expect(title.endsWith("…")).toBe(true);
  });

  it("falls back when the task is blank", () => {
    expect(tabTitleFor("   ")).toBe("Agent");
  });
});
