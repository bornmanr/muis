/**
 * Snor UI helpers. Pure so they unit test without a DOM or engine.
 *
 * The panel owns the conversation; these helpers decide what terminal
 * context is attached to a prompt and how the model file is labelled.
 * Nothing here touches the network or the filesystem.
 */

/** One conversation turn. `context` is the attached terminal context. */
export interface SnorTurn {
  role: "user" | "assistant";
  text: string;
  /** Never written to disk; re-attached explicitly on the next prompt. */
  context?: string;
  /** Where the attached context came from, for the transcript note. */
  source?: string;
}

export interface SnorHistory {
  conversations: Record<string, SnorTurn[]>;
}

export const SNOR_HISTORY_VERSION = 1;
export const SNOR_MAX_SAVED_TURNS = 60;
export const SNOR_MAX_SAVED_TEXT = 20000;

/**
 * Parse the on-disk conversation file. Junk fields are dropped, not
 * fatal: a corrupt file starts with empty conversations. Attached
 * context is deliberately not restored.
 */
export function parseSnorHistory(json: string): SnorHistory {
  if (!json.trim()) return { conversations: {} };
  let raw: unknown;
  try {
    raw = JSON.parse(json);
  } catch {
    return { conversations: {} };
  }
  if (typeof raw !== "object" || raw === null) return { conversations: {} };
  const source = (raw as { conversations?: unknown }).conversations;
  if (typeof source !== "object" || source === null || Array.isArray(source)) {
    return { conversations: {} };
  }
  const conversations: Record<string, SnorTurn[]> = {};
  for (const [id, value] of Object.entries(source as Record<string, unknown>)) {
    if (!id || !Array.isArray(value)) continue;
    const turns: SnorTurn[] = [];
    for (const item of value) {
      if (typeof item !== "object" || item === null) continue;
      const r = item as Record<string, unknown>;
      if (r.role !== "user" && r.role !== "assistant") continue;
      if (typeof r.text !== "string") continue;
      const text = r.text.slice(0, SNOR_MAX_SAVED_TEXT);
      if (r.role === "assistant" && !text.trim()) continue;
      turns.push({ role: r.role, text });
    }
    if (turns.length > 0) conversations[id] = turns.slice(-SNOR_MAX_SAVED_TURNS);
  }
  return { conversations };
}

/**
 * Serialize conversations for disk. The attached terminal context is
 * dropped so scrollback (which routinely contains secrets) never lands
 * in the state file; empty assistant placeholders are dropped too.
 */
export function serializeSnorHistory(
  conversations: Iterable<[string, SnorTurn[]]>,
): string {
  const out: Record<string, { role: string; text: string }[]> = {};
  for (const [id, turns] of conversations) {
    const saved = turns
      .filter((turn) => turn.text.trim() || turn.role === "user")
      .map((turn) => ({
        role: turn.role,
        text: turn.text.slice(0, SNOR_MAX_SAVED_TEXT),
      }))
      .slice(-SNOR_MAX_SAVED_TURNS);
    if (saved.length > 0) out[id] = saved;
  }
  return JSON.stringify({ version: SNOR_HISTORY_VERSION, conversations: out });
}

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

export type SnorSlash =
  | { command: "git"; arg: string }
  | { command: "docs"; arg: string }
  | { command: "file"; arg: string };

/** Recognize the read-only slash commands; anything else is a prompt. */
export function parseSnorSlash(text: string): SnorSlash | null {
  const m = /^\/(git|docs|file)(?:\s+([\s\S]*))?$/i.exec(text.trim());
  if (!m) return null;
  return {
    command: m[1].toLowerCase() as SnorSlash["command"],
    arg: (m[2] ?? "").trim(),
  };
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

/** Compact byte size for the model catalog. */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value.toFixed(unit >= 1 ? 2 : 0)} ${units[unit]}`;
}
