// One app-wide popup menu, used for dropdowns and context menus.
import type { IconName } from "./components/Icon.svelte";

export type MenuItem =
  | { label: string; icon?: IconName; shortcut?: string; checked?: boolean; disabled?: boolean; danger?: boolean; action: () => void }
  | { separator: true };

class MenuState {
  items = $state.raw<MenuItem[]>([]);
  x = $state(0);
  y = $state(0);
  open = $state(false);
  #restoreFocus: HTMLElement | null = null;

  show(items: MenuItem[], x: number, y: number) {
    this.#restoreFocus = document.activeElement as HTMLElement | null;
    this.items = items;
    this.x = x;
    this.y = y;
    this.open = true;
  }

  /** Open below an anchor element, like a dropdown button. */
  showBelow(items: MenuItem[], anchor: HTMLElement) {
    const r = anchor.getBoundingClientRect();
    this.show(items, r.left, r.bottom + 4);
  }

  close() {
    if (!this.open) return;
    this.open = false;
    this.#restoreFocus?.focus?.();
  }
}

export const menu = new MenuState();
