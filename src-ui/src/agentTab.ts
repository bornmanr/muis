/**
 * Launching an agent tab from a one-line task.
 *
 * The task text ends up in a shell command, so it is quoted here and nowhere
 * else: `opencode --prompt '<task>'` with POSIX single-quote escaping. A task
 * containing `'; rm -rf ~` is a string, not syntax.
 */

/** POSIX single-quote a value. Always quoted, even when it looks safe. */
export function shellQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

/** The command that opens opencode with this task as its first prompt. */
export function opencodeCommand(task: string): string {
  return `opencode --prompt ${shellQuote(task.trim())}`;
}

/**
 * Tab label from the task: first line, collapsed whitespace, at most 40
 * characters. `Agent` when there is nothing usable.
 */
export function tabTitleFor(task: string): string {
  const line = task.trim().split(/\r?\n/, 1)[0] ?? "";
  const clean = line.replace(/\s+/g, " ").trim();
  if (!clean) return "Agent";
  return clean.length <= 40 ? clean : `${clean.slice(0, 39).trimEnd()}…`;
}
