// Tabs, navigation history, selection and the actions that act on them.
import { childUri, createFolder, inTauri, errorText, openEntry, places as loadPlaces, renameEntry, trashEntries, type Entry, type Places } from "./api";
import { Folder } from "./folder.svelte";
import type { SortSpec } from "./sort";
import { toasts } from "./toasts.svelte";

interface Settings {
  sort: SortSpec;
  showHidden: boolean;
  compact: boolean;
  sidebarWidth: number;
}

const SETTINGS_KEY = "cx.settings";
const defaults: Settings = { sort: { key: "name", desc: false }, showHidden: false, compact: false, sidebarWidth: 220 };

function loadSettings(): Settings {
  try {
    return { ...defaults, ...JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? "{}") };
  } catch {
    return { ...defaults };
  }
}

/** Where a tab was in a folder, restored when coming back to it. */
interface Memory {
  cursor: string | null;
  scrollTop: number;
}

let nextTabId = 1;

export class Tab {
  readonly id = nextTabId++;
  folder = $state.raw<Folder>(null!);
  history = $state.raw<string[]>([]);
  index = $state(0);
  selection = $state.raw<ReadonlySet<string>>(new Set());
  /** Keyboard focus row. */
  cursor = $state<string | null>(null);
  /** Anchor for shift-click / shift-arrow range selection. */
  anchor: string | null = null;
  filter = $state("");
  renaming = $state<string | null>(null);
  /** Scroll position the view should restore after navigation. */
  restoreScroll = $state(0);
  scrollTop = 0;
  #memory = new Map<string, Memory>();

  constructor(
    private ws: Workspace,
    uri: string,
  ) {
    this.#open(uri, null);
    this.history = [uri];
  }

  title = $derived(this.folder.info?.name ?? "");
  canBack = $derived(this.index > 0);
  canForward = $derived(this.index < this.history.length - 1);

  /** Rows on screen: hidden files and the quick filter applied. */
  visible = $derived.by(() => {
    const showHidden = this.ws.settings.showHidden;
    const q = this.filter.trim().toLowerCase();
    const items = this.folder.items;
    if (showHidden && !q) return items;
    return items.filter((e) => (showHidden || !e.hidden) && (!q || e.name.toLowerCase().includes(q)));
  });

  selectedEntries = $derived.by(() => this.visible.filter((e) => this.selection.has(e.name)));

  #open(uri: string, select: string | null) {
    const mem = this.#memory.get(uri);
    this.folder?.dispose();
    const folder = new Folder(uri, this.ws.settings.sort);
    this.folder = folder;
    this.filter = "";
    this.renaming = null;
    this.cursor = select ?? mem?.cursor ?? null;
    this.anchor = this.cursor;
    this.selection = select ? new Set([select]) : new Set();
    this.restoreScroll = select ? -1 : (mem?.scrollTop ?? 0);
    void folder.load();
    if (this.ws.activeId === this.id || this.ws.tabs.length === 0) void folder.watch();
  }

  #remember() {
    if (this.folder) this.#memory.set(this.folder.uri, { cursor: this.cursor, scrollTop: this.scrollTop });
  }

  navigate(uri: string, select: string | null = null) {
    if (uri === this.folder.uri || uri === this.folder.info?.uri) {
      if (select) this.selectOnly(select);
      return;
    }
    this.#remember();
    this.#open(uri, select);
    this.history = [...this.history.slice(0, this.index + 1), uri];
    this.index = this.history.length - 1;
  }

  back() {
    if (!this.canBack) return;
    this.#remember();
    this.index--;
    this.#open(this.history[this.index], null);
  }

  forward() {
    if (!this.canForward) return;
    this.#remember();
    this.index++;
    this.#open(this.history[this.index], null);
  }

  up() {
    const info = this.folder.info;
    if (info?.parent) this.navigate(info.parent, info.crumbs.at(-1)?.label ?? null);
  }

  activate() {
    // Hidden tabs don't watch; catch up on whatever changed meanwhile.
    if (this.folder.status !== "loading") void this.folder.load();
    void this.folder.watch();
  }

  deactivate() {
    this.folder.unwatch();
  }

  close() {
    this.folder.dispose();
  }

  // ---- selection ----

  selectOnly(name: string | null) {
    this.selection = name ? new Set([name]) : new Set();
    this.cursor = name;
    this.anchor = name;
  }

  toggle(name: string) {
    const s = new Set(this.selection);
    if (s.has(name)) s.delete(name);
    else s.add(name);
    this.selection = s;
    this.cursor = name;
    this.anchor = name;
  }

  selectRange(to: string, additive = false) {
    const names = this.visible.map((e) => e.name);
    const a = names.indexOf(this.anchor ?? to);
    const b = names.indexOf(to);
    if (b < 0) return;
    const [lo, hi] = a < 0 ? [b, b] : [Math.min(a, b), Math.max(a, b)];
    const s = additive ? new Set(this.selection) : new Set<string>();
    for (let i = lo; i <= hi; i++) s.add(names[i]);
    this.selection = s;
    this.cursor = to;
  }

  selectAll() {
    this.selection = new Set(this.visible.map((e) => e.name));
  }

  // ---- actions ----

  open(entry: Entry) {
    const uri = childUri(this.folder.info?.uri ?? this.folder.uri, entry.name);
    if (entry.isDir && !entry.name.endsWith(".app")) this.navigate(uri);
    else openEntry(uri).catch((e) => toasts.show(errorText(e), "error"));
  }

  async newFolder() {
    try {
      const entry = await createFolder(this.folder.uri);
      this.filter = "";
      this.folder.upsertLocal(entry);
      this.selectOnly(entry.name);
      this.renaming = entry.name;
    } catch (e) {
      toasts.show(errorText(e), "error");
    }
  }

  async rename(from: string, to: string) {
    this.renaming = null;
    to = to.trim();
    if (!to || to === from) return;
    const prev = this.folder.get(from);
    try {
      // Show the new name immediately; the watcher confirms it.
      if (prev) {
        this.folder.removeLocal([from]);
        this.folder.upsertLocal({ ...prev, name: to });
        this.selectOnly(to);
      }
      const entry = await renameEntry(this.folder.uri, from, to);
      this.folder.upsertLocal(entry);
    } catch (e) {
      if (prev) {
        this.folder.removeLocal([to]);
        this.folder.upsertLocal(prev);
        this.selectOnly(from);
      }
      toasts.show(errorText(e), "error");
    }
  }

  async trashSelection() {
    const names = this.selectedEntries.map((e) => e.name);
    if (!names.length) return;
    const removed = names.map((n) => this.folder.get(n)).filter((e): e is Entry => !!e);
    // Move the cursor to the row after the deleted block, like Explorer.
    const vis = this.visible;
    const last = Math.max(...names.map((n) => vis.findIndex((e) => e.name === n)));
    const next = vis.slice(last + 1).find((e) => !this.selection.has(e.name)) ?? vis.slice(0, last).reverse().find((e) => !this.selection.has(e.name));
    this.folder.removeLocal(names);
    this.selectOnly(next?.name ?? null);
    try {
      await trashEntries(this.folder.uri, names);
      const trash = this.ws.platform === "windows" ? "Recycle Bin" : "Trash";
      toasts.show(names.length === 1 ? `Moved "${names[0]}" to ${trash}` : `Moved ${names.length} items to ${trash}`);
    } catch (e) {
      removed.forEach((e) => this.folder.upsertLocal(e));
      toasts.show(errorText(e), "error");
    }
  }

  copyPath() {
    const info = this.folder.info;
    if (!info) return;
    const sel = this.selectedEntries;
    const sep = this.ws.platform === "windows" ? "\\" : "/";
    const base = info.display.replace(/[\\/]+$/, "");
    const text = sel.length ? sel.map((e) => base + sep + e.name).join("\n") : info.display;
    navigator.clipboard.writeText(text).then(
      () => toasts.show(sel.length > 1 ? `Copied ${sel.length} paths` : "Copied path"),
      () => toasts.show("Couldn't copy to the clipboard", "error"),
    );
  }
}

export class Workspace {
  tabs = $state.raw<Tab[]>([]);
  activeId = $state(0);
  settings = $state<Settings>(loadSettings());
  places = $state.raw<Places | null>(null);
  platform = $state("macos");

  active = $derived(this.tabs.find((t) => t.id === this.activeId) ?? this.tabs[0]);

  async init() {
    const p = await loadPlaces();
    this.places = p;
    this.platform = p.platform;
    // The browser preview can open a specific folder: ?path=~/Downloads
    const start = inTauri ? null : new URLSearchParams(location.search).get("path");
    this.newTab(start ?? p.home.uri);
  }

  refreshPlaces() {
    loadPlaces().then((p) => (this.places = p));
  }

  newTab(uri = this.places?.home.uri ?? "~", activate = true) {
    const tab = new Tab(this, uri);
    this.tabs = [...this.tabs, tab];
    if (activate) this.activate(tab.id);
    return tab;
  }

  closeTab(id: number) {
    const i = this.tabs.findIndex((t) => t.id === id);
    if (i < 0) return;
    if (this.tabs.length === 1) {
      // Closing the last tab goes home instead of leaving an empty window.
      this.tabs[0].navigate(this.places?.home.uri ?? "~");
      return;
    }
    const [tab] = this.tabs.splice(i, 1);
    this.tabs = [...this.tabs];
    tab.close();
    if (id === this.activeId) this.activate(this.tabs[Math.min(i, this.tabs.length - 1)].id);
  }

  activate(id: number) {
    if (id === this.activeId && this.active?.folder.live) return;
    this.active?.deactivate();
    this.activeId = id;
    this.active?.activate();
  }

  cycleTab(step: number) {
    const i = this.tabs.findIndex((t) => t.id === this.activeId);
    this.activate(this.tabs[(i + step + this.tabs.length) % this.tabs.length].id);
  }

  setSort(sort: SortSpec) {
    this.settings.sort = sort;
    for (const t of this.tabs) t.folder.setSort(sort);
    this.save();
  }

  sortBy(key: SortSpec["key"]) {
    const cur = this.settings.sort;
    // Dates and sizes read best newest/largest first on the first click.
    this.setSort({ key, desc: cur.key === key ? !cur.desc : key === "modified" || key === "size" });
  }

  toggle(setting: "showHidden" | "compact") {
    this.settings[setting] = !this.settings[setting];
    this.save();
  }

  save() {
    try {
      localStorage.setItem(SETTINGS_KEY, JSON.stringify(this.settings));
    } catch {
      /* settings are a convenience; ignore storage failures */
    }
  }
}

export const ws = new Workspace();
