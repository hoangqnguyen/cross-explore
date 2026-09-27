// Behaviour shared by every file view (details, icons, columns, gallery):
// selection with the mouse, keyboard navigation, type-to-filter, context
// menus and drag-and-drop.
import type { Item } from "./api";
import { byId, enabled, run, shortcut } from "./commands.svelte";
import { keyOf } from "./folder.svelte";
import { isMac, primary } from "./keys";
import { menu, type MenuItem } from "./menu.svelte";
import { settings } from "./stores/settings.svelte";
import { ui } from "./stores/ui.svelte";
import { ws, isArchive, type Tab } from "./workspace.svelte";
import { inTauri } from "./api";
import { invoke } from "@tauri-apps/api/core";

// ---- touch: tap opens, long-press selects (and shows the menu) ----

let pressTimer: ReturnType<typeof setTimeout> | undefined;
let pressStart: { x: number; y: number } | null = null;
let longPressed = false;

function touchDown(e: PointerEvent, tab: Tab, item: Item) {
  longPressed = false;
  pressStart = { x: e.clientX, y: e.clientY };
  clearTimeout(pressTimer);
  pressTimer = setTimeout(() => {
    longPressed = true;
    ui.selecting = true;
    const key = keyOf(item);
    if (!tab.selection.has(key)) tab.toggle(key);
    navigator.vibrate?.(10);
  }, 480);
}

function touchUp(e: PointerEvent, tab: Tab, item: Item) {
  clearTimeout(pressTimer);
  const moved = pressStart && Math.hypot(e.clientX - pressStart.x, e.clientY - pressStart.y) > 10;
  pressStart = null;
  if (longPressed || moved) return;
  const key = keyOf(item);
  if (ui.selecting) {
    tab.toggle(key);
    if (!tab.selection.size) ui.selecting = false;
  } else {
    tab.selectOnly(key);
    tab.open(item);
  }
}

export function onItemPointerDown(e: PointerEvent, tab: Tab, item: Item) {
  ws.focusPane(tab.pane.id);
  if (e.pointerType === "touch") return touchDown(e, tab, item);
  const key = keyOf(item);
  if (e.button === 2) {
    if (!tab.selection.has(key)) tab.selectOnly(key);
    return;
  }
  if (e.button !== 0) return;
  if (e.shiftKey) tab.selectRange(key, primary(e));
  else if (primary(e)) tab.toggle(key);
  else if (!tab.selection.has(key) || tab.selection.size === 1) tab.selectOnly(key);
  else tab.cursor = key;
}

export function onItemPointerUp(e: PointerEvent, tab: Tab, item: Item) {
  if (e.pointerType === "touch") return touchUp(e, tab, item);
  // Clicking one row of a multi-selection narrows to it on release (a drag
  // of the whole selection would have started instead).
  if (e.button === 0 && !e.shiftKey && !primary(e) && tab.selection.size > 1 && !dragging) tab.selectOnly(keyOf(item));
}

export interface NavLayout {
  /** Items per row (1 for lists). */
  cols: number;
  /** Items per page (for PageUp/PageDown). */
  page: number;
  reveal: () => void;
  /** Left/Right step through items too (grids and filmstrips). */
  horizontal?: boolean;
}

/** Arrow keys, Home/End, paging, Escape and type-to-filter. Returns true if handled. */
export function handleNavKey(e: KeyboardEvent, tab: Tab, layout: NavLayout): boolean {
  const rows = tab.visible;
  const cur = tab.cursor == null ? -1 : rows.findIndex((r) => keyOf(r) === tab.cursor);
  const move = (to: number) => {
    if (!rows.length) return;
    const i = Math.max(0, Math.min(rows.length - 1, to));
    const key = keyOf(rows[i]);
    if (e.shiftKey) tab.selectRange(key);
    else if (primary(e) && !isMac) tab.cursor = key; // Ctrl+arrows move focus only, like Explorer
    else tab.selectOnly(key);
    layout.reveal();
  };
  const k = e.key;
  const cmdArrow = isMac && e.metaKey;
  if (k === "ArrowDown" && !cmdArrow && !e.altKey) move(cur < 0 ? 0 : cur + layout.cols);
  else if (k === "ArrowUp" && !cmdArrow && !e.altKey) move(cur < 0 ? 0 : cur - layout.cols);
  else if (k === "ArrowRight" && (layout.cols > 1 || layout.horizontal) && !e.altKey && !cmdArrow) move(cur + 1);
  else if (k === "ArrowLeft" && (layout.cols > 1 || layout.horizontal) && !e.altKey && !cmdArrow) move(cur - 1);
  else if (k === "Home" && !e.altKey) move(0);
  else if (k === "End") move(rows.length - 1);
  else if (k === "PageDown") move(cur + layout.page);
  else if (k === "PageUp") move(cur - layout.page);
  else if (k === "Insert" && settings.data.keymap === "commander" && cur >= 0) {
    // Total Commander: Insert toggles the row and moves down.
    const key = keyOf(rows[cur]);
    const s = new Set(tab.selection);
    if (s.has(key)) s.delete(key);
    else s.add(key);
    tab.selection = s;
    tab.cursor = keyOf(rows[Math.min(rows.length - 1, cur + 1)]);
    layout.reveal();
  } else if (k === "Escape") {
    if (tab.filter) tab.filter = "";
    else if (tab.selection.size) tab.selectOnly(null);
    else return false;
  } else if (k === "Backspace" && tab.filter && !e.metaKey && !e.ctrlKey) tab.filter = tab.filter.slice(0, -1);
  else if (k.length === 1 && k !== " " && !e.ctrlKey && !e.metaKey && !e.altKey && !e.code.startsWith("Numpad")) {
    // Type to filter, Total Commander style.
    tab.filter += k;
  } else return false;
  e.preventDefault();
  return true;
}

function cmd(id: string, label?: string, danger = false): MenuItem {
  const c = byId.get(id)!;
  return { label: label ?? c.label, icon: c.icon, shortcut: shortcut(id), disabled: !enabled(id), danger, action: () => run(id) };
}

export function itemMenu(e: MouseEvent, tab: Tab, item: Item) {
  e.preventDefault();
  e.stopPropagation();
  const key = keyOf(item);
  if (!tab.selection.has(key)) tab.selectOnly(key);
  const many = tab.selection.size > 1;
  const items: MenuItem[] = [
    cmd("file.open"),
    ...(item.isDir || isArchive(item.name) ? [cmd("file.openTab")] : []),
    ...(item.parent ? [{ label: "Show in enclosing folder", icon: "up" as const, action: () => tab.navigate(item.parent!, item.name) }] : []),
    cmd("file.quicklook"),
    { separator: true },
    cmd("edit.cut"),
    cmd("edit.copy"),
    cmd("file.copyTo"),
    cmd("file.moveTo"),
    ...(ws.dual ? [cmd("file.copyOther"), cmd("file.moveOther")] : []),
    cmd("edit.duplicate"),
    { separator: true },
    ...(many ? [cmd("file.multiRename")] : [cmd("file.rename")]),
    cmd("file.tags"),
    cmd("file.copyPath"),
    cmd("file.reveal"),
    { separator: true },
    ...(isArchive(item.name) ? [cmd("file.extract")] : []),
    cmd("file.compress"),
    ...(item.isDir ? [cmd("file.calcSize")] : []),
    ...(tab.selectedEntries.filter((x) => !x.isDir).length === 2 ? [cmd("file.diff")] : []),
    cmd("file.sendTo"),
    { separator: true },
    cmd("file.trash", undefined, true),
  ];
  menu.show(items, e.clientX, e.clientY);
}

export function blankMenu(e: MouseEvent, tab: Tab) {
  e.preventDefault();
  tab.selectOnly(null);
  const s = settings.data;
  menu.show(
    [
      cmd("file.newFolder"),
      cmd("edit.paste"),
      { separator: true },
      { label: "Details", icon: "rows", checked: tab.view === "details", action: () => run("view.details") },
      { label: "Icons", icon: "grid", checked: tab.view === "icons", action: () => run("view.icons") },
      { label: "Columns", icon: "columns", checked: tab.view === "columns", action: () => run("view.columns") },
      { label: "Gallery", icon: "gallery", checked: tab.view === "gallery", action: () => run("view.gallery") },
      { separator: true },
      { label: "Hidden items", checked: s.showHidden, action: () => (s.showHidden = !s.showHidden) },
      cmd("file.copyPath"),
      cmd("file.terminal"),
      cmd("bookmark.toggle", s.bookmarks.some((b) => b.uri === tab.dirUri) ? "Remove from Favorites" : "Add to Favorites"),
      cmd("file.calcSize"),
    ],
    e.clientX,
    e.clientY,
  );
}

// ---------------- drag and drop ----------------

const MIME = "application/x-cx-uris";
let dragging = false;

/** Our own native drag in flight (so drops back into the window stay internal). */
export let nativeDrag: { uris: string[] } | null = null;
let dragIconPath: Promise<string> | null = null;

function localPath(uri: string) {
  const p = decodeURIComponent(uri.replace(/^file:\/\//, ""));
  return /^\/[A-Za-z]:/.test(p) ? p.slice(1).replaceAll("/", "\\") : p;
}

export function onDragStart(e: DragEvent, tab: Tab, item: Item) {
  const key = keyOf(item);
  if (!tab.selection.has(key)) tab.selectOnly(key);
  const uris = tab.selectedEntries.map((x) => tab.uriOf(x));
  // Local files get a real OS drag, so they can land in Finder, Explorer,
  // mail or chat apps too. Drops back into our window arrive via Tauri.
  if (inTauri && !ui.phone && uris.every((u) => u.startsWith("file:"))) {
    e.preventDefault();
    nativeDrag = { uris };
    dragIconPath ??= invoke<string>("drag_icon");
    void Promise.all([dragIconPath, import("@crabnebula/tauri-plugin-drag")]).then(([icon, { startDrag }]) =>
      startDrag({ item: uris.map(localPath), icon }, () => {
        // Keep the marker briefly: the drop event may arrive after this.
        setTimeout(() => (nativeDrag = null), 500);
      }),
    );
    return;
  }
  dragging = true;
  e.dataTransfer!.effectAllowed = "copyMove";
  e.dataTransfer!.setData(MIME, JSON.stringify(uris));
  e.dataTransfer!.setData("text/uri-list", uris.join("\r\n"));
  e.dataTransfer!.setData("text/plain", uris.map((u) => decodeURIComponent(u.replace(/^file:\/\//, ""))).join("\n"));
  const ghost = document.createElement("div");
  ghost.className = "drag-ghost";
  ghost.textContent = uris.length === 1 ? item.name : `${uris.length} items`;
  document.body.appendChild(ghost);
  e.dataTransfer!.setDragImage(ghost, -8, -8);
  setTimeout(() => ghost.remove());
}

export function onDragEnd() {
  dragging = false;
}

/** Top-level folder of a URI: a macOS volume, a Windows drive, or a server. */
function volumeOf(uri: string): string {
  if (!uri.startsWith("file://")) return uri.replace(/^(\w+:\/\/[^/]*).*$/, "$1");
  const p = decodeURIComponent(uri.slice(7));
  const vol = /^\/Volumes\/[^/]+/.exec(p) ?? /^\/?[A-Za-z]:/.exec(p) ?? /^\/(media|mnt|run\/media)\/[^/]+(\/[^/]+)?/.exec(p);
  return vol ? vol[0].toLowerCase() : "/";
}

/** Explorer/Finder rule: same volume moves, different volume copies; modifiers override. */
export function dropIsMove(e: DragEvent | null, uris: string[], dest: string): boolean {
  if (e && (e.altKey || (!isMac && e.ctrlKey))) return false;
  if (e && (e.shiftKey || (isMac && e.metaKey))) return true;
  return uris.every((u) => volumeOf(u) === volumeOf(dest));
}

/** The drop target element under a window point, if any. */
export function dropElementAt(x: number, y: number): (HTMLElement & { cxDropDest?: () => string | null }) | null {
  let el = document.elementFromPoint(x, y) as (HTMLElement & { cxDropDest?: () => string | null }) | null;
  while (el) {
    if (el.cxDropDest?.()) return el;
    el = el.parentElement as typeof el;
  }
  return null;
}

function dragUris(e: DragEvent): string[] | null {
  if (!e.dataTransfer?.types.includes(MIME)) return null;
  try {
    return JSON.parse(e.dataTransfer.getData(MIME));
  } catch {
    return [];
  }
}

/**
 * Handlers that make an element a drop target for `dest()` (a folder URI).
 * Hovering for a moment while dragging opens the folder (spring-loading).
 */
export function dropTarget(node: HTMLElement, opts: { dest: () => string | null; spring?: () => void }) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const clear = () => {
    node.classList.remove("drop-hover");
    clearTimeout(timer);
    timer = undefined;
  };
  const over = (e: DragEvent) => {
    const dest = opts.dest();
    if (!dest || !e.dataTransfer?.types.includes(MIME)) return;
    e.preventDefault();
    e.stopPropagation();
    e.dataTransfer.dropEffect = e.altKey || (!isMac && e.ctrlKey) ? "copy" : "move";
    node.classList.add("drop-hover");
    if (opts.spring && !timer) timer = setTimeout(() => opts.spring?.(), 750);
  };
  const drop = (e: DragEvent) => {
    const dest = opts.dest();
    const uris = dragUris(e);
    clear();
    if (!dest || !uris) return;
    e.preventDefault();
    e.stopPropagation();
    // Don't drop a folder into itself or where it already is.
    const moving = uris.filter((u) => u !== dest && u.replace(/\/[^/]*$/, "") !== dest.replace(/\/$/, ""));
    if (!moving.length) return;
    void ws.transfer(moving, dest, dropIsMove(e, moving, dest));
  };
  // Drops from other apps arrive through Tauri, not the DOM; they find their
  // target through this.
  (node as HTMLElement & { cxDropDest?: () => string | null }).cxDropDest = () => opts.dest();
  node.classList.add("cx-drop");
  node.addEventListener("dragover", over);
  node.addEventListener("dragleave", clear);
  node.addEventListener("drop", drop);
  return {
    update(o: typeof opts) {
      opts = o;
    },
    destroy() {
      clear();
      node.removeEventListener("dragover", over);
      node.removeEventListener("dragleave", clear);
      node.removeEventListener("drop", drop);
    },
  };
}

/** Folder URI under a window point, for files dropped in from other apps. */
export function dropDestAt(x: number, y: number): string | null {
  return dropElementAt(x, y)?.cxDropDest?.() ?? null;
}
