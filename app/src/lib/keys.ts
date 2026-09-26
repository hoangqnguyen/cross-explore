// Platform-aware shortcut helpers.
export const isMac = /Mac|iPhone|iPad/.test(navigator.platform) || navigator.userAgent.includes("Mac OS");

/** Label for the primary modifier: ⌘ on macOS, Ctrl+ elsewhere. */
export const mod = isMac ? "⌘" : "Ctrl+";

/** Primary modifier pressed (Cmd on macOS, Ctrl elsewhere). */
export const primary = (e: KeyboardEvent | MouseEvent) => (isMac ? e.metaKey : e.ctrlKey);

export function isTextInput(el: EventTarget | null): boolean {
  return el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || (el instanceof HTMLElement && el.isContentEditable);
}
