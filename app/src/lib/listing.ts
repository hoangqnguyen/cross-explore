// Behaviour shared by every file view (details, icons, columns, gallery):
// selection with the mouse, keyboard navigation, type-to-filter, context
// menus and drag-and-drop.
import type { Item } from "./api";
import { byId, enabled, run, shortcut } from "./commands.svelte";
import { extOf } from "./format";
import { keyOf } from "./folder.svelte";
import { isMac, primary } from "./keys";
import { menu, type MenuItem } from "./menu.svelte";
import { dialogs } from "./stores/dialogs.svelte";
import { settings, typeAction } from "./stores/settings.svelte";
import { ui } from "./stores/ui.svelte";
import { ws, isArchive, type Tab } from "./workspace.svelte";
import { appsForExtension, errorText, inTauri, openWithDialog } from "./api";
import { openings } from "./opening.svelte";
import { toasts } from "./toasts.svelte";
import { invoke } from "@tauri-apps/api/core";

/** Common file types for "New file", plus "Other…" for anything else. */
const NEW_FILE_TYPES: { label: string; ext: string }[] = [
  { label: "Text File", ext: "txt" },
  { label: "Markdown", ext: "md" },
  { label: "JSON", ext: "json" },
  { label: "YAML", ext: "yaml" },
  { label: "HTML", ext: "html" },
  { label: "CSS", ext: "css" },
  { label: "JavaScript", ext: "js" },
  { label: "TypeScript", ext: "ts" },
  { label: "Python", ext: "py" },
  { label: "Shell Script", ext: "sh" },
  { label: "CSV", ext: "csv" },
];

function newFileMenu(tab: Tab): MenuItem {
  return {
    label: "New File",
    icon: "plus",
    disabled: !tab.writable,
    items: [
      ...NEW_FILE_TYPES.map((t): MenuItem => ({ label: t.label, fileIcon: `x.${t.ext}`, action: () => void tab.newFile("Untitled", t.ext) })),
      { separator: true },
      {
        label: "Other…",
        action: () => {
          void (async () => {
            const typed = await dialogs.prompt("New File", "File name", "", "Create", false, true);
            if (!typed) return;
            const dot = typed.lastIndexOf(".");
            const [stem, ext] = dot > 0 ? [typed.slice(0, dot), typed.slice(dot + 1)] : [typed, ""];
            void tab.newFile(stem, ext);
          })();
        },
      },
    ],
  };
}

/** "Open With": on Windows the OS already has a picker, so skip straight to
 * it; elsewhere (no such standalone dialog exists) list candidate apps. */
function openWithItem(tab: Tab, item: Item): MenuItem {
  const uri = tab.uriOf(item);
  const openFail = (e: unknown) => toasts.show(errorText(e), "error");
  if (ws.platform === "windows") {
    return { label: "Open With…", icon: "external", action: () => void openWithDialog(uri).catch(openFail) };
  }
  return {
    label: "Open With",
    icon: "external",
    items: async () => {
      const apps = await appsForExtension(extOf(item.name)).catch(() => []);
      if (!apps.length) return [{ label: "No apps found", disabled: true, action: () => {} }];
      return apps.map((a): MenuItem => ({ label: a.name, action: () => void openings.open(uri, a.id) }));
    },
  };
}

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

/** The row a mouse press began on, so only a click on it (not a selection drag ending there) narrows the selection. */
let pressedKey: string | null = null;

export function onItemPointerDown(e: PointerEvent, tab: Tab, item: Item) {
  ws.focusPane(tab.pane.id);
  if (e.pointerType === "touch") return touchDown(e, tab, item);
  const key = keyOf(item);
  pressedKey = e.button === 0 ? key : null;
  if (e.button === 2) {
    if (!tab.selection.has(key)) tab.selectOnly(key);
    return;
  }
  if (e.button !== 0) return;
  if (e.shiftKey) tab.selectRange(key, primary(e));
  else if (primary(e)) tab.toggle(key);
  else if (!tab.selection.has(key) || tab.selection.size === 1) tab.selectOnly(key);
  else tab.cursor = key;
  if (e.pointerType === "mouse" && tab.selection.has(key)) prefetchForDrag(tab);
}

export function onItemPointerUp(e: PointerEvent, tab: Tab, item: Item) {
  if (e.pointerType === "touch") return touchUp(e, tab, item);
  // Clicking one row of a multi-selection narrows to it on release (a drag
  // of the whole selection would have started instead).
  const key = keyOf(item);
  const pressedHere = pressedKey === key;
  pressedKey = null;
  if (e.button === 0 && pressedHere && !e.shiftKey && !primary(e) && tab.selection.size > 1 && !dragging) tab.selectOnly(key);
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
  const cur = tab.cursor == null ? -1 : tab.indexOf(tab.cursor);
  const move = (to: number) => {
    if (!rows.length) return;
    const i = Math.max(0, Math.min(rows.length - 1, to));
    const key = keyOf(rows[i]);
    tab.indexHint = i;
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
    if (typeAction(settings.data) === "filter" || tab.filter) tab.filter += k; // Total Commander style
    else typeToSelect(tab, k, layout);
  } else return false;
  e.preventDefault();
  return true;
}

// ---------------- type to select (Finder / Explorer) ----------------

/** Pause after which typing starts a new search, like Finder. */
const TYPE_AHEAD_MS = 1000;
const typed = new WeakMap<Tab, { text: string; at: number }>();
const folded = new WeakMap<Item, string>();

/** Lower-case and without accents, so "bien" finds "BIÊN BẢN". */
export const foldName = (s: string) => s.normalize("NFD").replace(/\p{M}/gu, "").replace(/đ/g, "d").replace(/Đ/g, "D").toLowerCase();

/**
 * Jump to the first item whose name starts with what was typed. Pressing the
 * same letter again steps through the items starting with it (Explorer).
 */
export function typeToSelect(tab: Tab, key: string, layout: Pick<NavLayout, "reveal">) {
  const rows = tab.visible;
  if (!rows.length) return;
  const now = performance.now();
  const prev = typed.get(tab);
  const text = prev && now - prev.at < TYPE_AHEAD_MS ? prev.text + key : key;
  typed.set(tab, { text, at: now });
  const want = foldName(text);
  // Folded lazily and remembered per row: folding a 100k folder up front
  // cost ~80 ms per keystroke, and most searches stop within a few rows.
  const names = {
    length: rows.length,
    at: (i: number) => {
      const r = rows[i];
      let f = folded.get(r);
      if (f === undefined) folded.set(r, (f = foldName(r.name)));
      return f;
    },
  };
  const cur = tab.cursor == null ? -1 : tab.indexOf(tab.cursor);

  let hit = -1;
  const same = [...want].every((c) => c === want[0]);
  const first = (prefix: string) => {
    for (let i = 0; i < names.length; i++) if (names.at(i).startsWith(prefix)) return i;
    return -1;
  };
  const next = (prefix: string) => {
    for (let n = 1; n <= rows.length; n++) if (names.at((cur + n) % rows.length).startsWith(prefix)) return (cur + n) % rows.length;
    return -1;
  };
  if (same && want.length > 1 && first(want) < 0) {
    // "aaa": cycle through the "a" items.
    hit = next(want[0]);
  } else if (want.length === 1 && cur >= 0 && names.at(cur).startsWith(want)) {
    // A single letter when already on a match moves to the next match.
    hit = next(want);
  } else {
    hit = first(want);
  }
  if (hit < 0) return;
  tab.indexHint = hit;
  tab.selectOnly(keyOf(rows[hit]));
  layout.reveal();
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
    ...(!item.isDir && !many ? [openWithItem(tab, item)] : []),
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
    ...favoritesItem(tab),
    { separator: true },
    ...(isArchive(item.name) ? [cmd("file.extract")] : []),
    cmd("file.compress"),
    ...(item.isDir ? [cmd("file.calcSize")] : []),
    ...(tab.selectedEntries.filter((x) => !x.isDir).length === 2 ? [cmd("file.diff")] : []),
    cmd("file.sendTo"),
    { separator: true },
    cmd("file.trash", undefined, true),
    cmd("file.delete", undefined, true),
  ];
  menu.show(items, e.clientX, e.clientY);
}

/** Add the selected folders to Favorites (or remove them, when they all are). */
function favoritesItem(tab: Tab): MenuItem[] {
  const folders = tab.selectedEntries.filter((x) => x.isDir || isArchive(x.name)).map((x) => ({ name: x.name, uri: tab.uriOf(x) }));
  if (!folders.length) return [];
  const saved = (uri: string) => settings.data.bookmarks.some((b) => b.uri === uri);
  const all = folders.every((f) => saved(f.uri));
  const what = folders.length === 1 ? "" : ` (${folders.length} folders)`;
  return [
    {
      label: all ? `Remove from Favorites${what}` : `Add to Favorites${what}`,
      icon: "star",
      action: () => {
        const uris = new Set(folders.map((f) => f.uri));
        settings.data.bookmarks = all
          ? settings.data.bookmarks.filter((b) => !uris.has(b.uri))
          : [...settings.data.bookmarks, ...folders.filter((f) => !saved(f.uri))];
      },
    },
  ];
}

export function blankMenu(e: MouseEvent, tab: Tab) {
  e.preventDefault();
  tab.selectOnly(null);
  const s = settings.data;
  menu.show(
    [
      cmd("file.newFolder"),
      newFileMenu(tab),
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

// ---- remote files as real files in other apps ----

/** Remote files are fetched to a local cache before an OS drag can carry them. */
const STAGE_LIMIT = 512 << 20;
const staged = new Map<string, Promise<string>>();

/**
 * Local copies of `uris`. Only a download in flight is shared here (the press
 * and the drag that follows); every new drag asks the backend again, which
 * checks the server and re-downloads when the file changed (its cache is keyed
 * by size and modification time).
 */
function stage(uris: string[]): Promise<string[]> {
  const missing = uris.filter((u) => !staged.has(u));
  if (missing.length) {
    const all = invoke<string[]>("stage_for_drag", { uris: missing });
    missing.forEach((u, i) => {
      const p = all.then((paths) => paths[i]);
      const done = () => staged.get(u) === p && staged.delete(u);
      p.then(done, done);
      staged.set(u, p);
    });
  }
  return Promise.all(uris.map((u) => staged.get(u)!));
}

/** Remote files small enough to fetch for a drag (folders go by address). */
function stageable(tab: Tab): string[] | null {
  const sel = tab.selectedEntries;
  if (!sel.length || sel.some((x) => x.isDir)) return null;
  const uris = sel.map((x) => tab.uriOf(x));
  if (uris.every((u) => u.startsWith("file:"))) return null;
  return sel.reduce((n, x) => n + x.size, 0) <= STAGE_LIMIT ? uris : null;
}

/** Only this much is fetched on a mere press (a click to select shouldn't download a movie). */
const PREFETCH_LIMIT = 32 << 20;

/** Pressing on a small remote file starts fetching it, so a drag that follows is instant. */
function prefetchForDrag(tab: Tab) {
  if (!inTauri || ui.phone) return;
  const uris = stageable(tab);
  if (uris && tab.selectedEntries.reduce((n, x) => n + x.size, 0) <= PREFETCH_LIMIT) void stage(uris).catch(() => {});
}

function nativeDragOf(uris: string[], paths: string[]) {
  nativeDrag = { uris };
  dragIconPath ??= invoke<string>("drag_icon");
  return Promise.all([dragIconPath, import("@crabnebula/tauri-plugin-drag")]).then(([icon, { startDrag }]) =>
    startDrag({ item: paths, icon }, () => {
      // Keep the marker briefly: the drop event may arrive after this.
      setTimeout(() => (nativeDrag = null), 500);
    }),
  );
}

export function onDragStart(e: DragEvent, tab: Tab, item: Item) {
  const key = keyOf(item);
  if (!tab.selection.has(key)) tab.selectOnly(key);
  const uris = tab.selectedEntries.map((x) => tab.uriOf(x));
  // Local files get a real OS drag, so they can land in Finder, Explorer,
  // mail or chat apps too. Drops back into our window arrive via Tauri.
  if (inTauri && !ui.phone && uris.every((u) => u.startsWith("file:"))) {
    e.preventDefault();
    void nativeDragOf(uris, uris.map(localPath));
    return;
  }
  // Remote files: the same real-file drag, from a local copy. Usually it's
  // ready already (fetching began on mouse down); otherwise the drag starts
  // as soon as it is, if the button is still held.
  const remote = inTauri && !ui.phone ? stageable(tab) : null;
  if (remote) {
    e.preventDefault();
    let released = false;
    const up = () => (released = true);
    window.addEventListener("pointerup", up, { once: true, capture: true });
    const slow = setTimeout(() => toasts.show(`Downloading ${remote.length === 1 ? item.name : `${remote.length} files`} to drag…`), 250);
    stage(remote)
      .then((paths) => {
        clearTimeout(slow);
        window.removeEventListener("pointerup", up, { capture: true });
        if (released) toasts.show("Ready — drag it again to drop it into another app");
        else return nativeDragOf(remote, paths);
      })
      .catch((err) => {
        clearTimeout(slow);
        window.removeEventListener("pointerup", up, { capture: true });
        toasts.show(`Couldn't download for dragging: ${errorText(err)}`, "error");
      });
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

// ---------------- rubber-band (marquee) selection ----------------

export interface MarqueeRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * Drag a selection rectangle over `scroller`. `hitTest` maps a rectangle in
 * content coordinates to row keys. A press without movement falls back to
 * `onClick`. ⌘/Ctrl or Shift adds to the existing selection.
 */
export function startMarquee(
  e: PointerEvent,
  scroller: HTMLElement,
  tab: Tab,
  hitTest: (r: MarqueeRect) => string[],
  onRect: (r: MarqueeRect | null) => void,
  onClick: () => void,
) {
  if (e.button !== 0 || e.pointerType === "touch") return;
  const box = () => scroller.getBoundingClientRect();
  const at = (cx: number, cy: number) => ({ x: cx - box().left + scroller.scrollLeft, y: cy - box().top + scroller.scrollTop });
  const start = at(e.clientX, e.clientY);
  const additive = primary(e) || e.shiftKey;
  const base = additive ? new Set(tab.selection) : new Set<string>();
  let last = { cx: e.clientX, cy: e.clientY };
  let moved = false;
  let raf = 0;

  const update = () => {
    const p = at(last.cx, last.cy);
    const r = { x: Math.min(start.x, p.x), y: Math.min(start.y, p.y), w: Math.abs(p.x - start.x), h: Math.abs(p.y - start.y) };
    onRect(r);
    const hits = hitTest(r);
    const next = new Set(base);
    for (const k of hits) next.add(k);
    tab.selection = next;
    if (hits.length) tab.cursor = hits[hits.length - 1];
  };

  // Keep scrolling while the pointer rests near an edge.
  const autoscroll = () => {
    const b = box();
    const edge = 28;
    let dy = 0;
    if (last.cy < b.top + edge) dy = -Math.ceil((b.top + edge - last.cy) / 2);
    else if (last.cy > b.bottom - edge) dy = Math.ceil((last.cy - (b.bottom - edge)) / 2);
    if (dy) {
      scroller.scrollTop += dy;
      update();
    }
    raf = requestAnimationFrame(autoscroll);
  };

  const move = (ev: PointerEvent) => {
    last = { cx: ev.clientX, cy: ev.clientY };
    if (!moved && Math.hypot(ev.clientX - e.clientX, ev.clientY - e.clientY) < 4) return;
    if (!moved) {
      moved = true;
      raf = requestAnimationFrame(autoscroll);
    }
    update();
  };
  const up = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    cancelAnimationFrame(raf);
    onRect(null);
    if (!moved) onClick();
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
  e.preventDefault();
}

/** Whether a press on a row began on its name (drag-the-file) rather than its whitespace (marquee). */
export const pressedOnName = (e: Event) => !!(e.target as HTMLElement | null)?.closest?.(".cell.name .text, .cell.name svg, .cell.name .rename, .cell.name .disclosure, .thumb, .cell .name");
