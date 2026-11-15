/**
 * Snor UI helpers. Pure so they unit test without a DOM or engine.
 *
 * The panel owns the conversation; these helpers decide what terminal
 * context is attached to a prompt and how the model file is labelled.
 * Nothing here touches the network or the filesystem.
 */

export interface SnorContextInput {
  tabTitle?: string;
  cwd?: string;
  /** Scrollback lines, oldest first. */
  scrollback?: string[];
  /** How many trailing scrollback lines to attach; 0 disables them. */
  maxLines: number;
  /** Mask likely secrets before the text is shown or sent. */
  scrub?: boolean;
}

export interface SnorContext {
  /** Human-readable chips describing what is attached. */
  chips: string[];
  /** Text block appended to the prompt; empty when nothing is attached. */
  text: string;
}

/** Hard cap so a pathological buffer cannot blow the context window. */
export const SNOR_MAX_CONTEXT_CHARS = 8000;

/** Trailing `max` lines, joined, trimmed to the character cap. */
export function tailLines(lines: string[], max: number): string[] {
  if (max <= 0 || lines.length === 0) return [];
  return lines.slice(-max);
}

export function buildSnorContext(input: SnorContextInput): SnorContext {
  const chips: string[] = [];
  const parts: string[] = [];
  if (input.tabTitle?.trim()) {
    chips.push(`tab: ${input.tabTitle.trim()}`);
    parts.push(`tab: ${input.tabTitle.trim()}`);
  }
  if (input.cwd?.trim()) {
    chips.push(`cwd: ${input.cwd.trim()}`);
    parts.push(`cwd: ${input.cwd.trim()}`);
  }
  const tail = tailLines(input.scrollback ?? [], input.maxLines);
  if (tail.length > 0) {
    chips.push(`${tail.length} scrollback lines`);
    parts.push("recent terminal output:\n" + tail.join("\n"));
  }
  if (parts.length === 0) return { chips, text: "" };
  let text = "Terminal context:\n" + parts.join("\n");
  if (input.scrub) text = scrubSecrets(text);
  if (text.length > SNOR_MAX_CONTEXT_CHARS) {
    text = text.slice(text.length - SNOR_MAX_CONTEXT_CHARS);
  }
  return { chips, text };
}

/**
 * Mask likely secrets in terminal text. Best effort, deliberately
 * conservative: known token shapes and `key = value` credential lines,
 * plus userinfo in URLs and PEM private-key blocks. Users still see the
 * exact payload before sending.
 */
export function scrubSecrets(text: string): string {
  return text
    .replace(/-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----/g, "[redacted private key]")
    .replace(/\bAKIA[0-9A-Z]{16}\b/g, "[redacted aws key]")
    .replace(/\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})\b/g, "[redacted github token]")
    .replace(/\bsk-[A-Za-z0-9_-]{20,}\b/g, "[redacted api key]")
    .replace(/\b(Bearer)\s+[A-Za-z0-9._~+/-]{12,}=*/gi, "$1 [redacted]")
    .replace(
      /\b((?:api[_-]?key|token|secret|password|passwd|pwd)\s*[:=]\s*)("[^"]*"|'[^']*'|[^\s"']{6,})/gi,
      "$1[redacted]",
    )
    .replace(/\b([a-z][a-z0-9+.-]*:\/\/[^\s/:@]+):[^\s/@]+@/gi, "$1:[redacted]@");
}

/** Compose the user message: their prompt plus the attached context. */
export function snorPrompt(text: string, context: string): string {
  const prompt = text.trim();
  if (!context) return prompt;
  return `${prompt}\n\n${context}`;
}

/** File name of the configured GGUF, for the panel header. */
export function snorModelName(path: string | null | undefined): string {
  const value = path?.trim();
  if (!value) return "not configured";
  const parts = value.split(/[\\/]/);
  return parts[parts.length - 1] || value;
}

const STATE_LABELS: Record<string, string> = {
  stopped: "stopped",
  starting: "starting…",
  ready: "ready",
  streaming: "answering…",
  error: "error",
};

export function snorStateLabel(state: string | undefined): string {
  if (!state) return STATE_LABELS.stopped;
  return STATE_LABELS[state] ?? state;
}
