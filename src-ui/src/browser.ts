/**
 * Browser pane: URL handling for the right-docked preview and the width
 * rules for its drag handle. Pure, so the shell-facing behavior is testable
 * without a DOM.
 *
 * The pane only ever loads http(s). Agent output is hostile input: a
 * `javascript:` or `file:` URL from a terminal must never reach the frame,
 * and the same check runs in `muis-agent` before the request is sent.
 */

/** Narrowest useful pane: about 40 terminal columns. */
export const MIN_PREVIEW_WIDTH = 320;
/** Widest the drag handle may go, so the terminal always keeps room. */
export const MAX_PREVIEW_WIDTH = 960;
/** Default share of the window the pane takes when first opened. */
export const DEFAULT_PREVIEW_SHARE = 0.42;

const SCHEME = /^([a-z][a-z0-9+.-]*):/i;
const HTTP = /^https?:\/\//i;

/**
 * Normalize user or agent input into an absolute http(s) URL.
 *
 * Accepts `https://…`, `http://…`, and the bare `host:port/path` form agents
 * print for dev servers (`localhost:5173`, `127.0.0.1:8000/x`, `[::1]:3000`).
 * Returns null for everything else, including other schemes.
 */
export function normalizeUrl(raw: string): string | null {
  const text = raw.trim();
  if (!text) return null;
  // Whitespace and control characters are never part of a URL and are the
  // shape of an injection attempt.
  if (/[\s\u0000-\u001f\u007f]/.test(text)) return null;

  const scheme = SCHEME.exec(text);
  if (scheme && !HTTP.test(text)) {
    // `localhost:5173` looks like a scheme to the regex; a numeric port after
    // the colon means it is a host, not a protocol.
    const rest = text.slice(scheme[0].length);
    if (!/^\d+(?:[/?#].*)?$/.test(rest)) return null;
  }
  const candidate = HTTP.test(text) ? text : `http://${text}`;

  let url: URL;
  try {
    url = new URL(candidate);
  } catch {
    return null;
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return null;
  if (!url.hostname) return null;
  return url.href;
}

/** Width the pane opens with for a given window width. */
export function defaultPreviewWidth(windowWidth: number): number {
  return clampPreviewWidth(Math.round(windowWidth * DEFAULT_PREVIEW_SHARE), windowWidth);
}

/**
 * Clamp a requested pane width: never narrower than the minimum, never wider
 * than the maximum, and always leave at least 320px for the terminal.
 */
export function clampPreviewWidth(width: number, windowWidth: number): number {
  if (!Number.isFinite(width)) return MIN_PREVIEW_WIDTH;
  const room = Math.max(MIN_PREVIEW_WIDTH, Math.floor(windowWidth) - 320);
  return Math.round(Math.max(MIN_PREVIEW_WIDTH, Math.min(width, MAX_PREVIEW_WIDTH, room)));
}

/** Host shown in the pane header, or an empty string for invalid input. */
export function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return "";
  }
}
