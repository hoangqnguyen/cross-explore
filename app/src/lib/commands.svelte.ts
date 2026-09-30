// Every user action, defined once. Menus, the command palette and the
// keyboard all run commands from this registry, so a shortcut shown in a
// menu is always the one that works.
import { errorText, openTerminal, revealEntry, uriName } from "./api";
import type { IconName } from "./components/Icon.svelte";
import { keyOf } from "./folder.svelte";
import { comboOf, formatCombo, isMac, isTextInput } from "./keys";
import { menu } from "./menu.svelte";
import { searchUri } from "./search.svelte";
import { clipboard } from "./stores/clipboard.svelte";
import { dialogs } from "./stores/dialogs.svelte";
import { settings, type ViewMode } from "./stores/settings.svelte";
import { transfers } from "./stores/transfers.svelte";
import { toasts } from "./toasts.svelte";
import { quicklook } from "./stores/quicklook.svelte";
import { sizes } from "./stores/sizes.svelte";
import { resolveSshUri } from "./ssh";
import { HOME_URI, isArchive, ws } from "./workspace.svelte";
import { ui } from "./stores/ui.svelte";

export interface Command {
  id: string;
  label: string;
  group: "Navigate" | "File" | "Edit" | "View" | "Tabs" | "Panes" | "Go" | "Tools" | "Network" | "App";
  icon?: IconName;
  /**
   * Shortcuts per keymap. `all` applies to every keymap; Commander also
   * inherits Explorer's keys (Total Commander is a Windows tool), with its
   * own keys winning where they clash.
   */
  keys?: { all?: string[]; finder?: string[]; explorer?: string[]; commander?: string[] };
  /** Only when a file list has focus (so typing in fields isn't hijacked). */
  list?: boolean;
  when?: () => boolean;
  run: () => unknown;
}

const tab = () => ws.activeTab;
const hasTargets = () => tab().targets().length > 0;
const oneTarget = () => tab().targets().length === 1;
const writable = () => tab().writable;
const trashName = () => (ws.platform === "windows" ? "Recycle Bin" : "Trash");

function setView(v: ViewMode) {
  if (v !== "details") tab().collapseAll();
  tab().view = v;
  settings.data.defaultView = v;
}

export function focusList() {
  requestAnimationFrame(() => (document.querySelector(".pane.active .file-view") as HTMLElement | null)?.focus());
}

export const commands: Command[] = [
  // ---- navigate ----
  { id: "nav.back", label: "Back", group: "Navigate", icon: "back", keys: { all: ["BrowserBack"], finder: ["Mod+[", "Mod+Left"], explorer: ["Alt+Left", "Backspace"], commander: ["Alt+Left"] }, when: () => tab().canBack, run: () => tab().back() },
  { id: "nav.forward", label: "Forward", group: "Navigate", icon: "forward", keys: { all: ["BrowserForward"], finder: ["Mod+]", "Mod+Right"], explorer: ["Alt+Right"] }, when: () => tab().canForward, run: () => tab().forward() },
  { id: "nav.up", label: "Enclosing folder", group: "Navigate", icon: "up", keys: { finder: ["Mod+Up"], explorer: ["Alt+Up"], commander: ["Backspace", "Mod+PageUp"] }, list: false, when: () => !!tab().folder.info?.parent, run: () => tab().up() },
  { id: "nav.home", label: "Go to Home page", group: "Go", icon: "home", keys: { finder: ["Mod+Shift+H"], explorer: ["Alt+Home"] }, run: () => tab().navigate(HOME_URI) },
  { id: "nav.userHome", label: "Go to your home folder", group: "Go", icon: "home", run: () => ws.places && tab().navigate(ws.places.home.uri) },
  { id: "nav.editPath", label: "Go to folder or path…", group: "Go", icon: "forward", keys: { all: ["Mod+L"], finder: ["Mod+Shift+G"], explorer: ["Alt+D"] }, run: () => document.dispatchEvent(new CustomEvent("cx:edit-path")) },
  { id: "nav.reload", label: "Refresh", group: "Navigate", icon: "reload", keys: { all: ["Mod+R"], explorer: ["F5"] }, run: () => tab().reload() },

  // ---- tabs & panes ----
  { id: "tab.new", label: "New tab", group: "Tabs", icon: "plus", keys: { all: ["Mod+T"] }, run: () => ws.newTab() },
  { id: "tab.close", label: "Close tab", group: "Tabs", icon: "close", keys: { all: ["Mod+W"] }, run: () => ws.closeTab() },
  { id: "tab.reopen", label: "Reopen closed tab", group: "Tabs", keys: { all: ["Mod+Shift+T"] }, run: () => ws.pane.reopenClosed() },
  { id: "tab.next", label: "Next tab", group: "Tabs", keys: { all: ["Ctrl+Tab", "Mod+Tab"], finder: ["Mod+Shift+]"], explorer: ["Mod+PageDown"] }, run: () => ws.pane.cycle(1) },
  { id: "tab.prev", label: "Previous tab", group: "Tabs", keys: { all: ["Ctrl+Shift+Tab", "Mod+Shift+Tab"], finder: ["Mod+Shift+["], explorer: ["Mod+PageUp"] }, run: () => ws.pane.cycle(-1) },
  { id: "pane.dual", label: "Toggle dual pane", group: "Panes", icon: "columns", keys: { all: ["F9", "Mod+\\"] }, run: () => ws.toggleDual() },
  { id: "pane.switch", label: "Switch to other pane", group: "Panes", keys: { all: ["Mod+Alt+Right"], commander: ["Tab"] }, list: true, when: () => ws.dual, run: () => (ws.focusPane(1 - ws.activePane), focusList()) },
  { id: "pane.swap", label: "Swap panes", group: "Panes", keys: { all: ["Mod+U"] }, when: () => ws.dual, run: () => ws.swapPanes() },
  { id: "pane.mirror", label: "Show this folder in the other pane", group: "Panes", keys: { all: ["Mod+Shift+M"] }, when: () => ws.dual, run: () => ws.mirror() },

  // ---- file ----
  { id: "file.open", label: "Open", group: "File", icon: "open", keys: { finder: ["Mod+O", "Mod+Down"], explorer: ["Enter"] }, list: true, when: hasTargets, run: () => openTargets() },
  { id: "file.openTab", label: "Open in new tab", group: "File", icon: "plus", keys: { finder: ["Mod+Alt+O"], explorer: ["Mod+Enter"] }, list: true, when: () => tab().targets().some((e) => e.isDir || isArchive(e.name)), run: () => tab().targets().forEach((e) => tab().open(e, true)) },
  { id: "file.quicklook", label: "Quick Look", group: "File", icon: "eye", keys: { all: ["Space"], commander: ["F3"] }, list: true, when: hasTargets, run: () => quicklook.toggle() },
  { id: "file.rename", label: "Rename", group: "File", icon: "rename", keys: { finder: ["Enter"], explorer: ["F2"], commander: ["Shift+F6"] }, list: true, when: () => oneTarget() && tab().folder.kind !== "home", run: () => (tab().renaming = keyOf(tab().targets()[0])) },
  { id: "file.newFolder", label: "New folder", group: "File", icon: "newFolder", keys: { all: ["Mod+Shift+N"], commander: ["F7"] }, when: writable, run: () => tab().newFolder() },
  { id: "file.trash", label: `Move to ${isMac ? "Trash" : "Recycle Bin"}`, group: "File", icon: "trash", keys: { finder: ["Mod+Backspace"], explorer: ["Delete"], commander: ["F8"] }, list: true, when: hasTargets, run: () => tab().trashSelection() },
  { id: "file.delete", label: "Delete permanently…", group: "File", icon: "trash", keys: { finder: ["Mod+Alt+Backspace"], explorer: ["Shift+Delete"], commander: ["Shift+F8"] }, list: true, when: hasTargets, run: () => tab().deletePermanently() },
  { id: "edit.copy", label: "Copy", group: "Edit", icon: "copy", keys: { all: ["Mod+C"] }, list: true, when: hasTargets, run: () => ws.copy(false) },
  { id: "edit.cut", label: "Cut", group: "Edit", icon: "cut", keys: { all: ["Mod+X"] }, list: true, when: hasTargets, run: () => ws.copy(true) },
  { id: "edit.paste", label: "Paste", group: "Edit", icon: "paste", keys: { all: ["Mod+V"] }, list: true, when: writable, run: () => ws.paste() },
  { id: "edit.duplicate", label: "Duplicate", group: "Edit", icon: "copy", keys: { finder: ["Mod+D"], explorer: ["Mod+Shift+D"] }, list: true, when: () => hasTargets() && writable(), run: () => ws.duplicate() },
  { id: "edit.undo", label: "Undo", group: "Edit", icon: "undo", keys: { all: ["Mod+Z"] }, list: true, when: () => transfers.undoStack.length > 0, run: () => transfers.undo() },
  { id: "file.copyTo", label: "Copy to…", group: "File", icon: "copy", keys: { all: ["Shift+F5"] }, list: true, when: hasTargets, run: () => dialogs.ask("destination", { uris: tab().targets().map((e) => tab().uriOf(e)), mode: "copy" }) },
  { id: "file.moveTo", label: "Move to…", group: "File", icon: "move", keys: { all: ["Shift+F6"] }, list: true, when: () => hasTargets() && writable(), run: () => dialogs.ask("destination", { uris: tab().targets().map((e) => tab().uriOf(e)), mode: "move" }) },
  { id: "file.copyOther", label: "Copy to other pane", group: "Panes", icon: "copy", keys: { commander: ["F5"] }, list: true, when: () => ws.dual && hasTargets(), run: () => ws.toOtherPane(false) },
  { id: "file.moveOther", label: "Move to other pane", group: "Panes", icon: "move", keys: { commander: ["F6"] }, list: true, when: () => ws.dual && hasTargets(), run: () => ws.toOtherPane(true) },
  { id: "file.copyPath", label: "Copy path", group: "File", icon: "link", keys: { finder: ["Mod+Alt+C"], explorer: ["Mod+Shift+C"] }, run: () => tab().copyPath() },
  { id: "file.reveal", label: isMac ? "Show in Finder" : "Show in Explorer", group: "File", icon: "open", keys: { finder: ["Mod+Shift+R"], explorer: ["Mod+Shift+E"] }, when: () => tab().folder.info?.local === true && tab().folder.kind !== "home", run: () => revealEntry(tab().targets()[0] ? tab().uriOf(tab().targets()[0]) : tab().dirUri).catch((e) => toasts.show(errorText(e), "error")) },
  { id: "view.terminal", label: "Toggle terminal panel", group: "View", icon: "terminal", keys: { finder: ["Ctrl+`"], explorer: ["Mod+`"] }, when: () => !ui.phone, run: () => (ui.terminalOpen = !ui.terminalOpen) },
  { id: "file.terminal", label: "Open in Terminal app", group: "Tools", icon: "terminal", keys: { finder: ["Mod+Alt+T"], explorer: ["Mod+Shift+`"] }, when: () => tab().folder.kind === "folder", run: () => void openExternalTerminal() },
  { id: "file.calcSize", label: "Calculate folder sizes", group: "Tools", icon: "sigma", keys: { all: ["Alt+Shift+Enter"], commander: ["Space"] }, list: true, when: () => tab().targets().some((e) => e.isDir) || tab().visible.some((e) => e.isDir), run: () => calcSizes() },
  { id: "file.compress", label: "Compress to ZIP", group: "Tools", icon: "archive", when: () => hasTargets() && writable(), run: () => compress() },
  { id: "file.extract", label: "Extract here", group: "Tools", icon: "archive", when: () => tab().targets().some((e) => isArchive(e.name)) && writable(), run: () => extract() },
  { id: "file.multiRename", label: "Rename multiple…", group: "Tools", icon: "rename", keys: { finder: ["Mod+Shift+Enter"], explorer: ["Mod+M"] }, list: true, when: () => tab().targets().length > 0, run: () => dialogs.ask("multiRename", { tab: tab() }) },
  { id: "file.tags", label: "Tags…", group: "File", icon: "tag", keys: { all: ["Mod+Alt+G"] }, list: true, when: hasTargets, run: () => dialogs.ask("tags", { uris: tab().targets().map((e) => tab().uriOf(e)) }) },
  { id: "file.sendTo", label: "Send to device…", group: "Network", icon: "send", when: () => hasTargets() && tab().folder.info?.local === true, run: () => dialogs.ask("sendTo", { uris: tab().targets().map((e) => tab().uriOf(e)) }) },
  { id: "file.diff", label: "Compare files", group: "Tools", icon: "diff", when: () => tab().selectedEntries.filter((e) => !e.isDir).length === 2 || (oneTarget() && !!ws.otherTab?.cursorEntry), run: () => diffFiles() },
  { id: "file.compareDirs", label: "Compare & sync folders", group: "Tools", icon: "sync", keys: { all: ["Mod+Shift+K"] }, when: () => ws.dual, run: () => compareDirs() },

  // ---- selection ----
  { id: "sel.all", label: "Select all", group: "Edit", keys: { all: ["Mod+A"] }, list: true, run: () => tab().selectAll() },
  { id: "sel.none", label: "Select none", group: "Edit", keys: { finder: ["Mod+Alt+A"], explorer: ["Mod+Shift+A"] }, list: true, run: () => tab().selectOnly(null) },
  { id: "sel.invert", label: "Invert selection", group: "Edit", keys: { explorer: ["Mod+I"], commander: ["Num*"] }, list: true, run: () => tab().invertSelection() },
  { id: "sel.pattern", label: "Select by pattern…", group: "Edit", keys: { all: ["Mod+Num+"], commander: ["Num+"] }, list: true, run: () => selectPattern(true) },
  { id: "sel.unpattern", label: "Deselect by pattern…", group: "Edit", keys: { all: ["Mod+Num-"], commander: ["Num-"] }, list: true, run: () => selectPattern(false) },

  // ---- view ----
  { id: "view.details", label: "Details view", group: "View", icon: "rows", keys: { finder: ["Mod+2"], explorer: ["Mod+Shift+6"] }, run: () => setView("details") },
  { id: "view.icons", label: "Icons view", group: "View", icon: "grid", keys: { finder: ["Mod+1"], explorer: ["Mod+Shift+2"] }, run: () => setView("icons") },
  { id: "view.columns", label: "Columns view", group: "View", icon: "columns", keys: { finder: ["Mod+3"], explorer: ["Mod+Shift+5"] }, run: () => setView("columns") },
  { id: "view.gallery", label: "Gallery view", group: "View", icon: "gallery", keys: { finder: ["Mod+4"], explorer: ["Mod+Shift+8"] }, run: () => setView("gallery") },
  { id: "view.preview", label: "Toggle preview pane", group: "View", icon: "sidebarRight", keys: { finder: ["Mod+Shift+P"], explorer: ["Alt+P"] }, run: () => (settings.data.previewPane = !settings.data.previewPane) },
  { id: "file.info", label: "Get Info", group: "File", icon: "info", keys: { finder: ["Mod+I"], explorer: ["Alt+Enter"] }, run: () => (settings.data.previewPane = true) },
  { id: "view.hidden", label: "Toggle hidden items", group: "View", icon: "eye", keys: { finder: ["Mod+Shift+."], explorer: ["Mod+H"] }, run: () => (settings.data.showHidden = !settings.data.showHidden) },
  { id: "view.compact", label: "Toggle compact spacing", group: "View", run: () => (settings.data.compact = !settings.data.compact) },
  { id: "view.stripes", label: "Toggle alternating row colors", group: "View", run: () => (settings.data.stripes = !settings.data.stripes) },
  { id: "view.pathBar", label: "Toggle path bar", group: "View", keys: { finder: ["Mod+Alt+P"] }, run: () => (settings.data.pathBar = !settings.data.pathBar) },
  { id: "view.collapseAll", label: "Collapse all folders", group: "View", keys: { all: ["Mod+Alt+Left"] }, when: () => tab().expanded.size > 0, run: () => tab().collapseAll() },
  { id: "view.find", label: "Filter this folder", group: "View", icon: "search", keys: { all: ["Mod+F"], explorer: ["Mod+E"] }, run: () => document.dispatchEvent(new CustomEvent("cx:focus-search")) },
  { id: "view.search", label: "Search in subfolders…", group: "Tools", icon: "search", keys: { finder: ["Mod+Alt+F"], explorer: ["Mod+Shift+F"], commander: ["Alt+F7"] }, when: () => tab().folder.kind === "folder", run: () => deepSearch(false) },
  { id: "view.searchContent", label: "Find text in files…", group: "Tools", icon: "search", when: () => tab().folder.kind === "folder", run: () => deepSearch(true) },

  // ---- bookmarks, network, app ----
  { id: "bookmark.toggle", label: "Add to / remove from Favorites", group: "Go", icon: "star", keys: { finder: ["Mod+Ctrl+T"], explorer: ["Mod+B"], commander: ["Mod+D"] }, when: () => tab().folder.kind === "folder", run: () => toggleBookmark() },
  { id: "net.connect", label: "Connect to server…", group: "Network", icon: "server", keys: { all: ["Mod+K"] }, run: () => dialogs.ask("connect") },
  { id: "net.cloud", label: "Add cloud account (Google Drive, Dropbox, OneDrive)…", group: "Network", icon: "cloud", run: () => dialogs.ask("cloud") },
  { id: "net.pair", label: "Pair a device…", group: "Network", icon: "link", run: () => dialogs.ask("pair") },
  { id: "net.scan", label: "Scan for nearby devices", group: "Network", icon: "radar", run: () => import("./stores/devices.svelte").then((d) => d.devices.scan()) },
  { id: "transfers.show", label: "Show transfers", group: "App", icon: "transfer", keys: { finder: ["Mod+Alt+L"], explorer: ["Mod+J"] }, run: () => (transfers.flyoutOpen = !transfers.flyoutOpen) },
  { id: "app.palette", label: "Command palette…", group: "App", icon: "command", keys: { all: ["Mod+P"], explorer: ["Mod+Shift+P"] }, run: () => document.dispatchEvent(new CustomEvent("cx:palette")) },
  { id: "app.settings", label: "Settings…", group: "App", icon: "settings", keys: { all: ["Mod+,"] }, run: () => dialogs.ask("settings") },
  { id: "app.keymap", label: "Switch keyboard style (Finder / Explorer / Commander)", group: "App", run: () => { const order = ["finder", "explorer", "commander"] as const; settings.data.keymap = order[(order.indexOf(settings.data.keymap) + 1) % 3]; toasts.show(`${{ finder: "Finder", explorer: "Explorer", commander: "Commander" }[settings.data.keymap]} keys`); } },
  { id: "app.saveWorkspace", label: "Save workspace…", group: "App", run: () => saveWorkspace() },
];

export const byId = new Map(commands.map((c) => [c.id, c]));

let lastRun = { id: "", at: 0 };

/**
 * Run a command. A menu item and a page shortcut can both fire for one key
 * press (macOS menu key equivalents), so an identical command within 250 ms
 * is ignored.
 */
export function run(id: string, fromMenu = false) {
  const now = performance.now();
  if (fromMenu && lastRun.id === id && now - lastRun.at < 250) return;
  lastRun = { id, at: now };
  const c = byId.get(id);
  if (c && (!c.when || c.when())) void c.run();
}

export function enabled(id: string) {
  const c = byId.get(id);
  return !!c && (!c.when || c.when());
}

/** Keys for a command in the active keymap, most specific first. */
export function keysFor(c: Command): string[] {
  return ranked(c).map((r) => r.key);
}

function ranked(c: Command): { key: string; rank: number }[] {
  const k = c.keys;
  if (!k) return [];
  const map = settings.data.keymap;
  const own = (map === "finder" ? k.finder : map === "explorer" ? k.explorer : k.commander) ?? [];
  const inherited = map === "commander" ? (k.explorer ?? []) : [];
  return [...own.map((key) => ({ key, rank: 0 })), ...(k.all ?? []).map((key) => ({ key, rank: 1 })), ...inherited.map((key) => ({ key, rank: 2 }))];
}

/** Shortcut label to show next to a command (first binding). */
export function shortcut(id: string): string | undefined {
  const c = byId.get(id);
  const k = c && keysFor(c)[0];
  return k ? formatCombo(k) : undefined;
}

/** Handle a key press; returns true when a command ran. */
export function handleKey(e: KeyboardEvent): boolean {
  if (e.isComposing || menu.open) return false;
  if (e.defaultPrevented) return false;
  const combo = comboOf(e);
  const inText = isTextInput(e.target);
  const inList = !!(e.target as HTMLElement | null)?.closest?.(".file-view");
  // A keymap's own keys win over shared ones, which win over inherited ones
  // (so Commander's F5 = copy beats Explorer's F5 = refresh).
  const matches = commands
    .map((c) => ({ c, rank: ranked(c).find((r) => r.key === combo)?.rank }))
    .filter((m): m is { c: Command; rank: number } => m.rank !== undefined)
    .sort((a, b) => a.rank - b.rank)
    .map((m) => m.c);
  for (const c of matches) {
    if (c.list && !inList) continue;
    // In a text field only modifier shortcuts and F-keys apply.
    if (inText && !/^(Mod|Ctrl|Alt)\+|^F\d+$/.test(combo)) continue;
    if (inText && ["Mod+A", "Mod+C", "Mod+V", "Mod+X", "Mod+Z", "Mod+Backspace", "Mod+Left", "Mod+Right"].includes(combo)) continue;
    if (c.when && !c.when()) continue;
    e.preventDefault();
    lastRun = { id: c.id, at: performance.now() };
    void c.run();
    return true;
  }
  return false;
}

// ---- command implementations that need a bit more ----

function openTargets() {
  const t = tab();
  const sel = t.targets();
  const dirs = sel.filter((e) => e.isDir || isArchive(e.name));
  if (dirs.length === 1 && sel.length === 1) return t.open(sel[0]);
  // Several folders open in tabs; files open in their apps.
  sel.forEach((e) => t.open(e, e.isDir || isArchive(e.name)));
}

async function selectPattern(select: boolean) {
  const p = await dialogs.ask<string>("selectPattern", { select });
  if (p) tab().selectPattern(p, select);
}

function calcSizes() {
  const t = tab();
  const dirs = (t.selectedEntries.length ? t.selectedEntries : t.visible).filter((e) => e.isDir);
  for (const d of dirs) sizes.compute(t.uriOf(d));
  if (settings.data.keymap === "commander" && t.cursorEntry) t.toggle(keyOf(t.cursorEntry));
}

async function openExternalTerminal() {
  const uri = await resolveSshUri(tab().dirUri);
  if (!uri) return;
  openTerminal(uri).catch((e) => toasts.show(errorText(e), "error"));
}

async function compress() {
  const t = tab();
  const uris = t.targets().map((e) => t.uriOf(e));
  const base = uris.length === 1 ? uriName(uris[0]).replace(/\.[^.]+$/, "") : "Archive";
  const name = await dialogs.prompt("Compress to ZIP", "Archive name", `${base}.zip`, "Compress");
  if (name) await transfers.submit({ kind: "compress", sources: uris, dest: t.dirUri + "/" + encodeURIComponent(name), conflict: "keepBoth" });
}

async function extract() {
  const t = tab();
  const uris = t.targets().filter((e) => isArchive(e.name)).map((e) => t.uriOf(e));
  await transfers.submit({ kind: "extract", sources: uris, dest: t.dirUri, conflict: "keepBoth" });
}

async function deepSearch(content: boolean) {
  const t = tab();
  const q = await dialogs.prompt(content ? "Find text in files" : "Search in subfolders", content ? `Text to find in “${t.title}” and below` : `Name contains (wildcards like *.pdf work) — in “${t.title}” and below`, t.filter, "Search");
  if (q) t.navigate(searchUri(t.dirUri, q, content));
}

function toggleBookmark() {
  const t = tab();
  const added = settings.toggleBookmark(t.title, t.dirUri);
  toasts.show(added ? `Added “${t.title}” to Favorites` : `Removed “${t.title}” from Favorites`);
}

function diffFiles() {
  const t = tab();
  const files = t.selectedEntries.filter((e) => !e.isDir);
  const [a, b] = files.length === 2 ? files.map((e) => t.uriOf(e)) : [t.uriOf(t.targets()[0]), ws.otherTab!.uriOf(ws.otherTab!.cursorEntry!)];
  void dialogs.ask("diff", { left: a, right: b });
}

function compareDirs() {
  const left = ws.panes[0].active.dirUri;
  const right = ws.panes[1].active.dirUri;
  tab().navigate(`cx:compare?${new URLSearchParams({ left, right })}`);
}

async function saveWorkspace() {
  const name = await dialogs.prompt("Save workspace", "Name", "My workspace", "Save");
  if (!name) return;
  settings.data.workspaces = [...settings.data.workspaces.filter((w) => w.name !== name), ws.snapshot(name)];
  toasts.show(`Saved workspace “${name}”`);
}
