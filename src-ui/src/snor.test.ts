import { describe, expect, it } from "vitest";
import {
  SNOR_MAX_CONTEXT_CHARS,
  buildSnorContext,
  scrubSecrets,
  snorModelName,
  snorPrompt,
  snorStateLabel,
  tailLines,
} from "./snor";

describe("snor context", () => {
  it("attaches tab, cwd, and the requested scrollback tail", () => {
    const ctx = buildSnorContext({
      tabTitle: "build",
      cwd: "/repo",
      scrollback: ["a", "b", "c"],
      maxLines: 2,
    });
    expect(ctx.chips).toEqual(["tab: build", "cwd: /repo", "2 scrollback lines"]);
    expect(ctx.text).toContain("tab: build");
    expect(ctx.text).toContain("cwd: /repo");
    expect(ctx.text).toContain("b\nc");
    expect(ctx.text).not.toContain("a\nb");
  });

  it("honours maxLines = 0 and an empty tab", () => {
    expect(buildSnorContext({ scrollback: ["x"], maxLines: 0 })).toEqual({
      chips: [],
      text: "",
    });
  });

  it("caps oversized context from the end", () => {
    const ctx = buildSnorContext({
      cwd: "/repo",
      scrollback: Array.from({ length: 500 }, (_, i) => `line ${i} ${"x".repeat(40)}`),
      maxLines: 500,
    });
    expect(ctx.text.length).toBeLessThanOrEqual(SNOR_MAX_CONTEXT_CHARS);
    expect(ctx.text.startsWith("|")).toBe(false);
    expect(ctx.text).toContain("line 499");
  });

  it("composes the prompt and keeps it clean without context", () => {
    expect(snorPrompt("  hi  ", "")).toBe("hi");
    expect(snorPrompt("hi", "ctx")).toBe("hi\n\nctx");
  });

  it("tails lines defensively", () => {
    expect(tailLines([], 5)).toEqual([]);
    expect(tailLines(["a"], -1)).toEqual([]);
    expect(tailLines(["a", "b"], 10)).toEqual(["a", "b"]);
  });
});

describe("snor secrets", () => {
  it("masks known token shapes and credential lines", () => {
    const text = [
      "key AKIAABCDEFGHIJKLMNOP here",
      "Authorization: Bearer abcdefghijklmnopqrstuvwxyz",
      "api_key=supersecretvalue",
      "https://user:hunter2@example.com/path",
      "safe: value",
    ].join("\n");
    const out = scrubSecrets(text);
    expect(out).not.toContain("AKIAABCDEFGHIJKLMNOP");
    expect(out).not.toContain("abcdefghijklmnopqrstuvwxyz");
    expect(out).not.toContain("supersecretvalue");
    expect(out).not.toContain("hunter2");
    expect(out).toContain("safe: value");
    expect(out).toContain("[redacted]");
  });

  it("applies scrubbing to attached context only when enabled", () => {
    const input = {
      cwd: "/repo",
      scrollback: ["password = hunter2"],
      maxLines: 5,
    };
    expect(buildSnorContext(input).text).toContain("hunter2");
    expect(buildSnorContext({ ...input, scrub: true }).text).not.toContain("hunter2");
  });
});

describe("snor labels", () => {
  it("shows the model file name only", () => {
    expect(snorModelName("/models/qwen.gguf")).toBe("qwen.gguf");
    expect(snorModelName("C:\\models\\qwen.gguf")).toBe("qwen.gguf");
    expect(snorModelName("")).toBe("not configured");
    expect(snorModelName(null)).toBe("not configured");
  });

  it("maps engine states to short labels", () => {
    expect(snorStateLabel("ready")).toBe("ready");
    expect(snorStateLabel("streaming")).toBe("answering…");
    expect(snorStateLabel("weird")).toBe("weird");
    expect(snorStateLabel(undefined)).toBe("stopped");
  });
});
