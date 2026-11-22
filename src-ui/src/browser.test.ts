import { describe, expect, it } from "vitest";
import {
  clampPreviewWidth,
  defaultPreviewWidth,
  hostOf,
  MAX_PREVIEW_WIDTH,
  MIN_PREVIEW_WIDTH,
  normalizeUrl,
} from "./browser";

describe("normalizeUrl", () => {
  it("keeps absolute http(s) URLs and trims them", () => {
    expect(normalizeUrl("  https://example.com/a?b=c#d  ")).toBe(
      "https://example.com/a?b=c#d",
    );
    expect(normalizeUrl("http://127.0.0.1:5173/")).toBe("http://127.0.0.1:5173/");
  });

  it("adds http:// to the bare host:port form agents print", () => {
    expect(normalizeUrl("localhost:5173")).toBe("http://localhost:5173/");
    expect(normalizeUrl("localhost:5173/preview?x=1")).toBe(
      "http://localhost:5173/preview?x=1",
    );
    expect(normalizeUrl("127.0.0.1:8000")).toBe("http://127.0.0.1:8000/");
    expect(normalizeUrl("[::1]:3000")).toBe("http://[::1]:3000/");
  });

  it("refuses other schemes and hostile input", () => {
    expect(normalizeUrl("")).toBeNull();
    expect(normalizeUrl("   ")).toBeNull();
    expect(normalizeUrl("file:///etc/passwd")).toBeNull();
    expect(normalizeUrl("javascript:alert(1)")).toBeNull();
    expect(normalizeUrl("data:text/html,<h1>x</h1>")).toBeNull();
    expect(normalizeUrl("ftp://example.com/x")).toBeNull();
    expect(normalizeUrl("http://localhost:5173/\nrm -rf /")).toBeNull();
    expect(normalizeUrl("http://")).toBeNull();
  });
});

describe("preview width", () => {
  it("opens at a share of the window, inside the bounds", () => {
    expect(defaultPreviewWidth(1100)).toBe(462);
    expect(defaultPreviewWidth(600)).toBe(MIN_PREVIEW_WIDTH);
    expect(defaultPreviewWidth(4000)).toBe(MAX_PREVIEW_WIDTH);
  });

  it("always leaves room for the terminal", () => {
    expect(clampPreviewWidth(900, 1000)).toBe(680);
    expect(clampPreviewWidth(50, 1000)).toBe(MIN_PREVIEW_WIDTH);
    expect(clampPreviewWidth(Number.NaN, 1000)).toBe(MIN_PREVIEW_WIDTH);
    // Degenerate window: the minimum still wins over a negative room.
    expect(clampPreviewWidth(500, 400)).toBe(MIN_PREVIEW_WIDTH);
  });

  it("reports the host for the header", () => {
    expect(hostOf("http://localhost:5173/")).toBe("localhost:5173");
    expect(hostOf("nope")).toBe("");
  });
});
