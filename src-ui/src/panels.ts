/**
 * Reserved side-panel slot (Snor assistant and future panels).
 *
 * Panels are thin clients: they talk to worker/engine processes over
 * the same IPC patterns as terminals and never touch the network from
 * the UI process. A panel may provide a mount callback that receives
 * the body element when the dock renders it.
 */
export type PanelMount = (body: HTMLElement) => void;

export interface SidePanelDef {
  id: string;
  title: string;
  /** Called once per dock render; the panel owns the body contents. */
  mount?: PanelMount;
}

export class SidePanelRegistry {
  private panels = new Map<string, SidePanelDef>();
  private visibleId: string | null = null;

  register(def: SidePanelDef): void {
    this.panels.set(def.id, def);
  }

  unregister(id: string): void {
    this.panels.delete(id);
    if (this.visibleId === id) this.visibleId = null;
  }

  /** Toggle a panel; returns the now-visible id, or null if none. */
  toggle(id: string): string | null {
    if (!this.panels.has(id)) return this.visibleId;
    this.visibleId = this.visibleId === id ? null : id;
    return this.visibleId;
  }

  close(): void {
    this.visibleId = null;
  }

  visible(): string | null {
    return this.visibleId;
  }

  list(): SidePanelDef[] {
    return [...this.panels.values()];
  }
}
