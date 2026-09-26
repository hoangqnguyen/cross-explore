// Keyboard helpers: platform detection and shortcut strings.
//
// Shortcuts are written like "Mod+Shift+N": Mod is ⌘ on macOS and Ctrl
// elsewhere; "Ctrl" means the real Control key on macOS.

export const isMac = /Mac|iPhone|iPad/.test(navigator.platform) || navigator.userAgent.includes("Mac OS");

/** Label for the primary modifier: ⌘ on macOS, Ctrl+ elsewhere. */
export const mod = isMac ? "⌘" : "Ctrl+";

/** Primary modifier pressed (Cmd on macOS, Ctrl elsewhere). */
export const primary = (e: KeyboardEvent | MouseEvent) => (isMac ? e.metaKey : e.ctrlKey);

export function isTextInput(el: EventTarget | null): boolean {
  return el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement || (el instanceof HTMLElement && el.isContentEditable);
}

const codeKeys: Record<string, string> = {
  Period: ".",
  Comma: ",",
  Backslash: "\\",
  BracketLeft: "[",
  BracketRight: "]",
  Equal: "=",
  Minus: "-",
  Slash: "/",
  Semicolon: ";",
  Quote: "'",
  Backquote: "`",
  NumpadAdd: "Num+",
  NumpadSubtract: "Num-",
  NumpadMultiply: "Num*",
  NumpadEnter: "Enter",
  Space: "Space",
};

const keyNames: Record<string, string> = {
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  " ": "Space",
  Esc: "Escape",
  Del: "Delete",
};

/** Canonical combo string for a key event, e.g. "Mod+Shift+N". */
export function comboOf(e: KeyboardEvent): string {
  let key: string;
  if (/^Key[A-Z]$/.test(e.code)) key = e.code.slice(3);
  else if (/^Digit\d$/.test(e.code)) key = e.code.slice(5);
  else if (codeKeys[e.code]) key = codeKeys[e.code];
  else key = keyNames[e.key] ?? (e.key.length === 1 ? e.key.toUpperCase() : e.key);
  const parts: string[] = [];
  if (isMac ? e.metaKey : e.ctrlKey) parts.push("Mod");
  if (isMac && e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey && !["Num+", "Num-", "Num*"].includes(key)) parts.push("Shift");
  parts.push(key);
  return parts.join("+");
}

const macSymbols: Record<string, string> = {
  Mod: "⌘",
  Ctrl: "⌃",
  Alt: "⌥",
  Shift: "⇧",
  Up: "↑",
  Down: "↓",
  Left: "←",
  Right: "→",
  Enter: "↩",
  Backspace: "⌫",
  Delete: "⌦",
  Escape: "⎋",
  Tab: "⇥",
  Space: "Space",
};

/** Human label for a combo: "⌘⇧N" on macOS, "Ctrl+Shift+N" elsewhere. */
export function formatCombo(combo: string): string {
  const parts = combo.split("+").map((p, i, all) => (p === "" && all[i - 1] === "Num" ? "+" : p));
  if (isMac) return parts.map((p) => macSymbols[p] ?? p).join("");
  return parts.map((p) => (p === "Mod" ? "Ctrl" : p === "Up" ? "↑" : p === "Down" ? "↓" : p === "Left" ? "←" : p === "Right" ? "→" : p)).join("+");
}
