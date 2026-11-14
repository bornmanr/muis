import { describe, expect, it } from "vitest";
import {
  SNOR_MAX_CONTEXT_CHARS,
  SNOR_MAX_SAVED_TURNS,
  buildSnorContext,
  scrubSecrets,
  parseSnorHistory,
  parseSnorSlash,
  serializeSnorHistory,
  snorModelName,
  snorPrompt,
  snorStateLabel,
  tailLines,
  type SnorTurn,
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

describe("snor slash commands", () => {
  it("recognizes the read-only commands and their optional argument", () => {
    expect(parseSnorSlash("/git")).toEqual({ command: "git", arg: "" });
    expect(parseSnorSlash("  /GIT  ")).toEqual({ command: "git", arg: "" });
    expect(parseSnorSlash("/git summarize the diff")).toEqual({
      command: "git",
      arg: "summarize the diff",
    });
    expect(parseSnorSlash("/docs")).toEqual({ command: "docs", arg: "" });
    expect(parseSnorSlash("/file docs/plan.md")).toEqual({
      command: "file",
      arg: "docs/plan.md",
    });
  });

  it("leaves prompts and unknown commands alone", () => {
    expect(parseSnorSlash("what changed?")).toBeNull();
    expect(parseSnorSlash("/rm -rf /")).toBeNull();
    expect(parseSnorSlash("")).toBeNull();
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

describe("snor history", () => {
  it("saves prompts and answers but never terminal context", () => {
    const turns: SnorTurn[] = [
      { role: "user", text: "why?", context: "secret scrollback" },
      { role: "assistant", text: "because" },
    ];
    const json = serializeSnorHistory([["w1", turns]]);
    expect(json).not.toContain("secret scrollback");
    expect(json).not.toContain("context");
    expect(parseSnorHistory(json)).toEqual({
      conversations: {
        w1: [
          { role: "user", text: "why?" },
          { role: "assistant", text: "because" },
        ],
      },
    });
  });

  it("drops empty assistant placeholders, empty conversations, and junk", () => {
    const json = serializeSnorHistory([
      ["a", [{ role: "user", text: "hi" }, { role: "assistant", text: "  " }]],
      ["b", [{ role: "assistant", text: "" }]],
    ]);
    expect(parseSnorHistory(json).conversations).toEqual({ a: [{ role: "user", text: "hi" }] });
    expect(parseSnorHistory("not json")).toEqual({ conversations: {} });
    expect(parseSnorHistory("")).toEqual({ conversations: {} });
    expect(parseSnorHistory('{"conversations":[]}')).toEqual({ conversations: {} });
    expect(
      parseSnorHistory(
        '{"conversations":{"w":[{"role":"system","text":"x"},{"role":"user","text":5},{"role":"assistant","text":"ok"}]}}',
      ).conversations,
    ).toEqual({ w: [{ role: "assistant", text: "ok" }] });
    expect(parseSnorHistory('{"conversations":{"w":"nope"}}')).toEqual({ conversations: {} });
  });

  it("caps long conversations to the newest turns", () => {
    const turns: SnorTurn[] = Array.from({ length: SNOR_MAX_SAVED_TURNS + 10 }, (_, i) => ({
      role: i % 2 === 0 ? "user" : "assistant",
      text: `t${i}`,
    }));
    const saved = parseSnorHistory(serializeSnorHistory([["w", turns]])).conversations.w;
    expect(saved).toHaveLength(SNOR_MAX_SAVED_TURNS);
    expect(saved[saved.length - 1].text).toBe(`t${turns.length - 1}`);
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
