/** View options. Mirrors muis-core AppConfig; persisted as JSON. */
export interface AppConfig {
  showSessions: boolean;
  /** Terminal font size in points. null = system fixed font + 2. */
  fontSize: number | null;
  /** Accent theme name; null keeps the default look. */
  theme: string | null;
  /** Terminal scrollback buffer, in lines (clamped 1000..500000). */
  scrollback: number;
  /** Chord overrides per action, e.g. { "close-tab": "ctrl+q" }. */
  keybindings: Record<string, string>;
  /** Copy the selection to the clipboard automatically. */
  copyOnSelect: boolean;
  /** Middle-click pastes the clipboard into the terminal. */
  middleClickPaste: boolean;
  /** Terminal font family; null keeps the muis default. */
  fontFamily: string | null;
  cursorStyle: "block" | "bar" | "underline";
  cursorBlink: boolean;
  /** What the terminal bell does. */
  bell: "none" | "visual" | "audible" | "both";
  /** Statusbar segment visibility (id -> visible; absent = shown). */
  statusbar: Record<string, boolean>;
  /** Accent theme per workspace name (falls back to `theme`). */
  workspaceThemes: Record<string, string>;
  /** Local assistant (Snor) settings; the engine is optional. */
  snor: SnorConfig;
}

export interface SnorConfig {
  /** Path to a user-supplied GGUF model; null means unconfigured. */
  modelPath: string | null;
  /** Optional override for the `llama-server` binary. */
  serverPath: string | null;
  /** Context window per slot. */
  contextSize: number;
  /** GPU layers to offload; 0 keeps inference on the CPU. */
  gpuLayers: number;
  temperature: number;
  /** Scrollback lines offered as context; 0 disables scrollback. */
  contextLines: number;
  /** System prompt sent before the conversation. */
  systemPrompt: string;
  /** Docked panel width in pixels. */
  panelWidth: number;
}

export const SNOR_DEFAULT_SYSTEM_PROMPT =
  "Answer only from the supplied context. If it is not there, say you do not know.";

export function defaultSnorConfig(): SnorConfig {
  return {
    modelPath: null,
    serverPath: null,
    contextSize: 8192,
    gpuLayers: 99,
    temperature: 0.2,
    contextLines: 0,
    systemPrompt: SNOR_DEFAULT_SYSTEM_PROMPT,
    panelWidth: 380,
  };
}

/** Clamp the Snor context window into a sane range. */
export function clampSnorContext(value: number): number {
  if (!Number.isFinite(value)) return defaultSnorConfig().contextSize;
  return Math.min(131072, Math.max(512, Math.floor(value)));
}

/** Clamp the docked panel width. */
export function clampSnorPanelWidth(value: number): number {
  if (!Number.isFinite(value)) return defaultSnorConfig().panelWidth;
  return Math.min(900, Math.max(280, Math.floor(value)));
}

export function snorConfigFromJSON(raw: unknown): SnorConfig {
  const base = defaultSnorConfig();
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return base;
  const r = raw as Record<string, unknown>;
  const text = (v: unknown, fallback: string | null): string | null =>
    typeof v === "string" && v.trim() ? v.trim() : fallback;
  return {
    modelPath: text(r.modelPath, base.modelPath),
    serverPath: text(r.serverPath, base.serverPath),
    contextSize:
      typeof r.contextSize === "number" ? clampSnorContext(r.contextSize) : base.contextSize,
    gpuLayers:
      typeof r.gpuLayers === "number" ? Math.min(999, Math.max(0, Math.floor(r.gpuLayers))) : base.gpuLayers,
    temperature:
      typeof r.temperature === "number" && r.temperature >= 0 && r.temperature <= 2
        ? r.temperature
        : base.temperature,
    contextLines:
      typeof r.contextLines === "number"
        ? Math.min(5000, Math.max(0, Math.floor(r.contextLines)))
        : base.contextLines,
    systemPrompt: typeof r.systemPrompt === "string" ? r.systemPrompt : base.systemPrompt,
    panelWidth:
      typeof r.panelWidth === "number"
        ? clampSnorPanelWidth(r.panelWidth)
        : base.panelWidth,
  };
}

/** Accent theme for a workspace: its own, else the global default. */
export function themeForWorkspace(cfg: AppConfig, wsName: string): string | null {
  return cfg.workspaceThemes[wsName] ?? cfg.theme;
}

/** Whether a statusbar segment is shown (absent = shown). */
export function segmentVisible(map: Record<string, boolean>, id: string): boolean {
  return map[id] !== false;
}

export type BellMode = "none" | "visual" | "audible" | "both";

/** Visual/audible actions for a bell mode. */
export function bellAction(mode: BellMode): { visual: boolean; audible: boolean } {
  return {
    visual: mode === "visual" || mode === "both",
    audible: mode === "audible" || mode === "both",
  };
}

export function defaultConfig(): AppConfig {
  return {
    showSessions: true,
    fontSize: null,
    theme: null,
    scrollback: 50000,
    keybindings: {},
    copyOnSelect: false,
    middleClickPaste: false,
    fontFamily: null,
    cursorStyle: "block",
    cursorBlink: true,
    bell: "none",
    statusbar: {},
    workspaceThemes: {},
    snor: defaultSnorConfig(),
  };
}

/** Keep only non-empty string values from a raw map. */
function parseStringMap(raw: unknown, base: Record<string, string>): Record<string, string> {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return base;
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
    if (typeof v === "string" && v.trim()) out[k] = v.trim();
  }
  return out;
}

/** Keep only boolean values from a raw map. */
function parseBoolMap(raw: unknown, base: Record<string, boolean>): Record<string, boolean> {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return base;
  const out: Record<string, boolean> = {};
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
    if (typeof v === "boolean") out[k] = v;
  }
  return out;
}

/** Keep only non-empty string chord values. */
function parseKeybindings(raw: unknown, base: Record<string, string>): Record<string, string> {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return base;
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
    if (typeof v === "string" && v.trim()) out[k] = v.trim();
  }
  return out;
}

/** Clamp a scrollback value into a sane range. */
export function clampScrollback(value: number): number {
  if (!Number.isFinite(value)) return defaultConfig().scrollback;
  return Math.min(500000, Math.max(1000, Math.floor(value)));
}

export function configFromJSON(json: string): AppConfig {
  const base = defaultConfig();
  if (!json) return base;
  let raw: unknown;
  try {
    raw = JSON.parse(json);
  } catch {
    throw new Error("config file is not JSON");
  }
  if (typeof raw !== "object" || raw === null) throw new Error("config file has no object");
  const r = raw as Partial<Record<keyof AppConfig, unknown>>;
  return {
    showSessions: typeof r.showSessions === "boolean" ? r.showSessions : base.showSessions,
    fontSize:
      typeof r.fontSize === "number" && r.fontSize >= 6 && r.fontSize <= 32
        ? Math.floor(r.fontSize)
        : base.fontSize,
    theme: typeof r.theme === "string" ? r.theme : base.theme,
    scrollback:
      typeof r.scrollback === "number" ? clampScrollback(r.scrollback) : base.scrollback,
    keybindings: parseKeybindings(r.keybindings, base.keybindings),
    copyOnSelect: typeof r.copyOnSelect === "boolean" ? r.copyOnSelect : base.copyOnSelect,
    middleClickPaste:
      typeof r.middleClickPaste === "boolean" ? r.middleClickPaste : base.middleClickPaste,
    fontFamily:
      typeof r.fontFamily === "string" && r.fontFamily.trim() ? r.fontFamily.trim() : base.fontFamily,
    cursorStyle:
      r.cursorStyle === "block" || r.cursorStyle === "bar" || r.cursorStyle === "underline"
        ? r.cursorStyle
        : base.cursorStyle,
    cursorBlink: typeof r.cursorBlink === "boolean" ? r.cursorBlink : base.cursorBlink,
    bell:
      r.bell === "none" || r.bell === "visual" || r.bell === "audible" || r.bell === "both"
        ? r.bell
        : base.bell,
    statusbar: parseBoolMap(r.statusbar, base.statusbar),
    workspaceThemes: parseStringMap(r.workspaceThemes, base.workspaceThemes),
    snor: snorConfigFromJSON(r.snor),
  };
}

/** Effective point size: explicit setting, else 12 (system fixed + 2). */
export function effectiveFontSize(cfg: AppConfig): number {
  return cfg.fontSize ?? 12;
}
