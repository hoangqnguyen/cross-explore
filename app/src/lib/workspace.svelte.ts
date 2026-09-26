// Panes, tabs, navigation history, selection and the file actions on them.
import {
  childUri,
  createFolder,
  errorText,
  inTauri,
  openEntry,
  places as loadPlaces,
  renameEntry,
  subscribe,
  trashEntries,
  uriName,
  type Entry,
  type Item,
  type Places,
} from "./api";
import { Folder, keyOf, type Source } from "./folder.svelte";
import { SearchResults, StaticSource, TagResults } from "./search.svelte";
import type { SortSpec } from "./sort";
import { clipboard } from "./stores/clipboard.svelte";
import { devices } from "./stores/devices.svelte";
import { dialogs } from "./stores/dialogs.svelte";
import { settings, type SavedWorkspace, type ViewMode } from "./stores/settings.svelte";
import { transfers } from "./stores/transfers.svelte";
import { ui } from "./stores/ui.svelte";
import { toasts } from "./toasts.svelte";

export const HOME_URI = "cx:home";

const ARCHIVE_RE = /\.(zip|jar|tar|tgz|tbz2?|txz|tzst|7z)$|\.tar\.(gz|bz2|xz|zst)$/i;
export const isArchive = (name: string) => ARCHIVE_RE.test(name);

function makeSource(uri: string, sort: SortSpec): Source {
  if (uri === HOME_URI) return new StaticSource(uri, "Home", "home", sort);
  if (uri.startsWith("cx:search")) return new SearchResults(uri, sort);
  if (uri.startsWith("cx:tag")) return new TagResults(uri, sort);
  if (uri.startsWith("cx:compare")) return new StaticSource(uri, "Compare folders", "search", sort, "compare");
  return new Folder(uri, sort);
}

/** Where a tab was in a folder, restored when coming back to it. */
interface Memory {
  cursor: string | null;
  scrollTop: number;
}

let nextId = 1;

export class Tab {
  readonly id = nextId++;
  readonly pane: Pane;
  folder = $state.raw<Source>(null!);
  history = $state.raw<string[]>([]);
  index = $state(0);
  selection = $state.raw<ReadonlySet<string>>(new Set());
  /** Keyboard focus row (a row key). */
  cursor = $state<string | null>(null);
  /** Anchor for shift-click / shift-arrow range selection. */
  anchor: string | null = null;
  filter = $state("");
  renaming = $state<string | null>(null);
  view = $state<ViewMode>(settings.data.defaultView);
  /** Scroll position the view should restore after navigation (-1: reveal cursor). */
  restoreScroll = $state(0);
  scrollTop = 0;
  #memory = new Map<string, Memory>();

  constructor(pane: Pane, uri: string, view?: ViewMode) {
    this.pane = pane;
    if (view) this.view = view;
    this.#open(uri, null);
    this.history = [uri];
  }

  title = $derived(this.folder.info?.name ?? "");
  canBack = $derived(this.index > 0);
  canForward = $derived(this.index < this.history.length - 1);
  isHome = $derived(this.folder.kind === "home");
  /** The folder's canonical URI (what child URIs are built from). */
  dirUri = $derived(this.folder.info?.uri ?? this.folder.uri);
  writable = $derived(this.folder.kind === "folder" && this.folder.status === "ready" && (this.folder.caps?.writable ?? true));

  /** Rows on screen: hidden files and the quick filter applied. */
  visible = $derived.by(() => {
    const showHidden = settings.data.showHidden;
    const q = this.filter.trim().toLowerCase();
    const items = this.folder.items;
    if (showHidden && !q) return items;
    return items.filter((e) => (showHidden || !e.hidden) && (!q || e.name.toLowerCase().includes(q)));
  });

  selectedEntries = $derived.by(() => this.visible.filter((e) => this.selection.has(keyOf(e))));
  selectedUris = $derived(this.selectedEntries.map((e) => this.uriOf(e)));
  cursorEntry = $derived(this.cursor == null ? null : (this.visible.find((e) => keyOf(e) === this.cursor) ?? null));

  uriOf(e: Item): string {
    return e.uri ?? childUri(this.dirUri, e.name);
  }

  #open(uri: string, select: string | null) {
    const mem = this.#memory.get(uri);
    this.folder?.dispose();
    const source = makeSource(uri, settings.data.sort);
    this.folder = source;
    this.filter = "";
    this.renaming = null;
    this.cursor = select ?? mem?.cursor ?? null;
    this.anchor = this.cursor;
    this.selection = select ? new Set([select]) : new Set();
    this.restoreScroll = select ? -1 : (mem?.scrollTop ?? 0);
    // Remote folders are polled against this listing, so watch after it.
    const loaded = source.load().then(() => {
      if (source.status === "ready" && source.kind === "folder") settings.addRecent(source.info?.uri ?? uri);
    });
    if (this.isActive()) {
      if (uri.startsWith("file:") || uri.startsWith("~") || uri.startsWith("/")) void source.watch();
      else
        void loaded.then(() => {
          if (this.folder === source && source.status === "ready") void source.watch();
        });
    }
  }

  isActive() {
    return ws.activeTab === this || (this.pane.activeId === this.id && ws.dual);
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
    if (info?.parent) this.navigate(info.parent, this.folder.kind === "folder" ? (info.crumbs.at(-1)?.label ?? null) : null);
  }

  activate() {
    // Hidden tabs don't watch; catch up on whatever changed meanwhile, then
    // watch (after the listing, which polled folders use as their baseline).
    const f = this.folder;
    if (f.status !== "loading" && f.kind === "folder")
      void f.load().then(() => {
        if (this.folder === f) void f.watch();
      });
    else void f.watch();
  }

  deactivate() {
    this.folder.unwatch();
  }

  close() {
    this.folder.dispose();
  }

  reload() {
    // A folder that failed to load (e.g. before signing in) never started
    // watching; start now that it may work.
    const f = this.folder;
    void f.load().then(() => {
      if (this.folder === f && this.isActive() && f.status === "ready") void f.watch();
    });
  }

  // ---- selection ----

  selectOnly(key: string | null) {
    this.selection = key ? new Set([key]) : new Set();
    this.cursor = key;
    this.anchor = key;
  }

  toggle(key: string) {
    const s = new Set(this.selection);
    if (s.has(key)) s.delete(key);
    else s.add(key);
    this.selection = s;
    this.cursor = key;
    this.anchor = key;
  }

  selectRange(to: string, additive = false) {
    const keys = this.visible.map(keyOf);
    const a = keys.indexOf(this.anchor ?? to);
    const b = keys.indexOf(to);
    if (b < 0) return;
    const [lo, hi] = a < 0 ? [b, b] : [Math.min(a, b), Math.max(a, b)];
    const s = additive ? new Set(this.selection) : new Set<string>();
    for (let i = lo; i <= hi; i++) s.add(keys[i]);
    this.selection = s;
    this.cursor = to;
  }

  selectAll() {
    this.selection = new Set(this.visible.map(keyOf));
  }

  invertSelection() {
    this.selection = new Set(this.visible.map(keyOf).filter((k) => !this.selection.has(k)));
  }

  /** Total Commander's + and -: select or deselect by wildcard. */
  selectPattern(pattern: string, select: boolean) {
    const re = globToRegExp(pattern);
    const s = new Set(this.selection);
    for (const e of this.visible) if (re.test(e.name)) select ? s.add(keyOf(e)) : s.delete(keyOf(e));
    this.selection = s;
  }

  /** Selected entries, or the cursor row when nothing is selected. */
  targets(): Item[] {
    return this.selectedEntries.length ? this.selectedEntries : this.cursorEntry ? [this.cursorEntry] : [];
  }

  // ---- actions ----

  open(entry: Item, inNewTab = false) {
    const uri = this.uriOf(entry);
    const target = entry.isDir && !entry.name.endsWith(".app") ? uri : isArchive(entry.name) ? `archive://${uri}!/` : null;
    if (target) {
      if (inNewTab) ws.newTab(target, false);
      else this.navigate(target);
    } else openEntry(uri).catch((e) => toasts.show(errorText(e), "error"));
  }

  async newFolder() {
    if (!this.writable) return;
    try {
      const entry = await createFolder(this.dirUri);
      this.filter = "";
      this.folder.upsertLocal(entry);
      this.selectOnly(entry.name);
      this.renaming = entry.name;
      transfers.pushUndo(`New folder “${entry.name}”`, { type: "newFolder", uri: childUri(this.dirUri, entry.name) });
    } catch (e) {
      toasts.show(errorText(e), "error");
    }
  }

  async rename(key: string, to: string) {
    this.renaming = null;
    const prev = this.folder.get(key);
    to = to.trim();
    if (!prev || !to || to === prev.name) return;
    const dir = prev.parent ?? this.dirUri;
    const from = prev.name;
    try {
      // Show the new name immediately; the watcher confirms it.
      if (this.folder.kind === "folder") {
        this.folder.removeLocal([from]);
        this.folder.upsertLocal({ ...prev, name: to });
        this.selectOnly(to);
      }
      const entry = await renameEntry(dir, from, to);
      if (this.folder.kind === "folder") this.folder.upsertLocal(entry);
      else this.reload();
      transfers.pushUndo(`Rename “${from}”`, { type: "rename", dir, from, to });
    } catch (e) {
      if (this.folder.kind === "folder") {
        this.folder.removeLocal([to]);
        this.folder.upsertLocal(prev);
        this.selectOnly(from);
      }
      toasts.show(errorText(e), "error");
    }
  }

  async trashSelection() {
    const targets = this.targets();
    if (!targets.length) return;
    const trashName = ws.platform === "windows" ? "Recycle Bin" : "Trash";
    const canTrash = this.folder.caps?.trash ?? true;
    if (!canTrash) return this.deletePermanently(true);
    if (settings.data.confirmTrash && !(await dialogs.confirm(`Move to ${trashName}?`, describe(targets), `Move to ${trashName}`))) return;
    const keys = targets.map(keyOf);
    // Move the cursor to the row after the deleted block, like Explorer.
    const vis = this.visible;
    const last = Math.max(...keys.map((k) => vis.findIndex((e) => keyOf(e) === k)));
    const gone = new Set(keys);
    const next = vis.slice(last + 1).find((e) => !gone.has(keyOf(e))) ?? vis.slice(0, last).reverse().find((e) => !gone.has(keyOf(e)));
    this.folder.removeLocal(keys);
    this.selectOnly(next ? keyOf(next) : null);
    // Group by folder (search results can span many).
    const byDir = new Map<string, Item[]>();
    for (const t of targets) byDir.set(t.parent ?? this.dirUri, [...(byDir.get(t.parent ?? this.dirUri) ?? []), t]);
    try {
      const items = (await Promise.all([...byDir].map(([dir, es]) => trashEntries(dir, es.map((e) => e.name))))).flat();
      const label = targets.length === 1 ? `“${targets[0].name}”` : `${targets.length} items`;
      transfers.pushUndo(`Move ${label} to ${trashName}`, { type: "trash", items });
      toasts.show(`Moved ${label} to ${trashName}`);
    } catch (e) {
      this.reload();
      toasts.show(errorText(e), "error");
    }
  }

  async deletePermanently(noTrash = false) {
    const targets = this.targets();
    if (!targets.length) return;
    const title = noTrash ? "This location has no trash. Delete permanently?" : "Delete permanently?";
    if (settings.data.confirmPermanentDelete || noTrash) {
      if (!(await dialogs.confirm(title, `${describe(targets)} will be deleted immediately. You can't undo this.`, "Delete", true))) return;
    }
    await transfers.submit({ kind: "delete", sources: targets.map((e) => this.uriOf(e)) });
  }

  copyPath() {
    const info = this.folder.info;
    if (!info) return;
    const sel = this.targets();
    const toPath = (uri: string) => {
      if (!uri.startsWith("file://")) return uri;
      const p = decodeURIComponent(uri.slice(7));
      return ws.platform === "windows" ? p.replace(/^\//, "").replaceAll("/", "\\") : p;
    };
    const text = sel.length ? sel.map((e) => toPath(this.uriOf(e))).join("\n") : toPath(info.uri);
    navigator.clipboard.writeText(text).then(
      () => toasts.show(sel.length > 1 ? `Copied ${sel.length} paths` : "Copied path"),
      () => toasts.show("Couldn't copy to the clipboard", "error"),
    );
  }
}

function describe(items: Item[]) {
  return items.length === 1 ? `“${items[0].name}”` : `${items.length} items`;
}

export function globToRegExp(pattern: string): RegExp {
  const parts = pattern
    .split(/[;,]/)
    .map((p) => p.trim())
    .filter(Boolean)
    .map((p) => "^" + p.replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*").replace(/\?/g, ".") + "$");
  return new RegExp(parts.join("|") || "^$", "i");
}

export class Pane {
  readonly id: number;
  tabs = $state.raw<Tab[]>([]);
  activeId = $state(0);
  closed: { uri: string; view: ViewMode }[] = [];

  constructor(id: number) {
    this.id = id;
  }

  active = $derived(this.tabs.find((t) => t.id === this.activeId) ?? this.tabs[0]);

  add(uri: string, activate = true, view?: ViewMode, at?: number) {
    const tab = new Tab(this, uri, view);
    const tabs = [...this.tabs];
    tabs.splice(at ?? tabs.length, 0, tab);
    this.tabs = tabs;
    if (activate || this.tabs.length === 1) this.activate(tab.id);
    return tab;
  }

  activate(id: number) {
    const prev = this.active;
    this.activeId = id;
    if (prev && prev.id !== id) prev.deactivate();
    this.active?.activate();
  }

  close(id: number) {
    const i = this.tabs.findIndex((t) => t.id === id);
    if (i < 0) return;
    const tab = this.tabs[i];
    this.closed.push({ uri: tab.folder.uri, view: tab.view });
    if (this.tabs.length === 1) {
      // Closing the last tab goes home instead of leaving an empty pane.
      tab.navigate(HOME_URI);
      return;
    }
    this.tabs = this.tabs.filter((t) => t.id !== id);
    tab.close();
    if (id === this.activeId) this.activate(this.tabs[Math.min(i, this.tabs.length - 1)].id);
  }

  reopenClosed() {
    const last = this.closed.pop();
    if (last) this.add(last.uri, true, last.view);
  }

  move(id: number, to: number) {
    const tabs = [...this.tabs];
    const from = tabs.findIndex((t) => t.id === id);
    if (from < 0) return;
    const [t] = tabs.splice(from, 1);
    tabs.splice(to > from ? to - 1 : to, 0, t);
    this.tabs = tabs;
  }

  cycle(step: number) {
    const i = this.tabs.findIndex((t) => t.id === this.activeId);
    this.activate(this.tabs[(i + step + this.tabs.length) % this.tabs.length].id);
  }
}

export class Workspace {
  panes = $state.raw<Pane[]>([new Pane(0), new Pane(1)]);
  activePane = $state(0);
  places = $state.raw<Places | null>(null);
  platform = $state("macos");
  ready = $state(false);

  dual = $derived(settings.data.dual && !ui.mobile);
  pane = $derived(this.panes[this.dual ? this.activePane : 0]);
  otherPane = $derived(this.dual ? this.panes[1 - this.activePane] : null);
  activeTab = $derived(this.pane.active);
  otherTab = $derived(this.otherPane?.active ?? null);
  /** All tabs, for the palette and session saving. */
  allTabs = $derived(this.panes.flatMap((p) => p.tabs));

  async init() {
    const p = await loadPlaces();
    this.places = p;
    this.platform = p.platform;
    ui.platform = p.platform;
    // Our own finished jobs refresh the polled folders they touched right away.
    transfers.onFinished = (job) => {
      const dirs = new Set([job.dest, ...job.sources.map((s) => s.replace(/\/[^/]*\/?$/, ""))].filter(Boolean).map((u) => u!.replace(/\/+$/, "")));
      for (const t of this.allTabs) {
        if (t.folder.kind === "folder" && t.folder.live !== "live" && dirs.has(t.dirUri.replace(/\/+$/, ""))) void t.folder.load();
      }
    };
    try {
      await subscribe((e) => {
        if (e.type === "job") transfers.onJob(e.job);
        else if (e.type === "devices") devices.list = e.devices;
        else if (e.type === "offer") {
          devices.addOffer(e.offer);
          void dialogs.ask("offer", { offer: e.offer });
        } else if (e.type === "peer") devices.peer = e.status;
      });
    } catch {
      /* events unavailable */
    }
    void devices.init();

    const start = inTauri ? null : new URLSearchParams(location.search).get("path");
    const session = settings.data.session;
    if (start) this.panes[0].add(start);
    else if (session?.panes.some((p) => p.tabs.length)) this.restore(session);
    else this.panes[0].add(p.home.uri);
    if (!this.panes[1].tabs.length) this.panes[1].add(this.panes[0].active?.folder.uri ?? p.home.uri, true);
    this.ready = true;

    // Keep the session current so the next launch picks up where we left off.
    $effect.root(() => {
      $effect(() => {
        settings.data.session = this.snapshot("session");
      });
    });
  }

  snapshot(name: string): SavedWorkspace {
    return {
      name,
      dual: this.dual,
      panes: this.panes.map((p) => ({ tabs: p.tabs.filter((t) => !t.folder.uri.startsWith("cx:search")).map((t) => ({ uri: t.folder.uri, view: t.view })), active: Math.max(0, p.tabs.findIndex((t) => t.id === p.activeId)) })),
    };
  }

  restore(w: SavedWorkspace) {
    const panes = [new Pane(0), new Pane(1)];
    for (const old of this.panes) for (const t of old.tabs) t.close();
    this.panes = panes;
    const home = this.places?.home.uri ?? "~";
    w.panes.forEach((pw, i) => {
      pw.tabs.forEach((t, j) => {
        const tab = panes[i]?.add(t.uri, j === pw.active, t.view);
        // A restored folder may be gone (unplugged drive, or a phone app's
        // sandbox that moved on reinstall): fall back to home quietly.
        const f = tab?.folder;
        if (tab && f && t.uri.startsWith("file:"))
          void f.load().then(() => {
            if (tab.folder === f && f.status === "error" && (f.errorDetail?.kind === "notFound" || f.errorDetail?.kind === "permissionDenied")) tab.navigate(home);
          });
      });
    });
    if (!panes[0].tabs.length) panes[0].add(this.places?.home.uri ?? "~");
    if (!panes[1].tabs.length) panes[1].add(panes[0].active.folder.uri);
    settings.data.dual = w.dual;
    this.activePane = 0;
  }

  newTab(uri = this.activeTab?.folder.uri ?? HOME_URI, activate = true) {
    const pane = this.pane;
    const i = pane.tabs.findIndex((t) => t.id === pane.activeId);
    return pane.add(uri, activate, undefined, i + 1);
  }

  closeTab(id = this.activeTab.id) {
    this.pane.close(id);
  }

  focusPane(i: number) {
    if (!this.dual || i === this.activePane) return;
    this.activePane = i;
  }

  toggleDual() {
    settings.data.dual = !settings.data.dual;
    if (settings.data.dual) this.panes[1].active?.activate();
    else {
      if (this.activePane === 1) {
        this.activePane = 0;
      }
      this.panes[1].active?.deactivate();
    }
  }

  swapPanes() {
    if (!this.dual) return;
    this.panes = [this.panes[1], this.panes[0]];
    this.activePane = 1 - this.activePane;
  }

  /** Show the active folder in the other pane too. */
  mirror() {
    if (this.otherTab) this.otherTab.navigate(this.activeTab.dirUri);
  }

  setSort(sort: SortSpec) {
    settings.data.sort = sort;
    for (const t of this.allTabs) t.folder.setSort(sort);
  }

  sortBy(key: SortSpec["key"]) {
    const cur = settings.data.sort;
    // Dates and sizes read best newest/largest first on the first click.
    this.setSort({ key, desc: cur.key === key ? !cur.desc : key === "modified" || key === "size" });
  }

  // ---- clipboard & transfers ----

  copy(cut = false) {
    const t = this.activeTab;
    const uris = t.targets().map((e) => t.uriOf(e));
    if (!uris.length) return;
    clipboard.set(uris, cut ? "cut" : "copy");
    toasts.show(`${cut ? "Cut" : "Copied"} ${uris.length === 1 ? `“${uriName(uris[0])}”` : `${uris.length} items`}`);
  }

  async paste(dest = this.activeTab.dirUri) {
    if (!clipboard.uris.length) return;
    const move = clipboard.mode === "cut";
    await transfers.submit({ kind: move ? "move" : "copy", sources: clipboard.uris, dest });
    if (move) clipboard.clear();
  }

  async transfer(uris: string[], dest: string, move: boolean) {
    if (!uris.length) return;
    await transfers.submit({ kind: move ? "move" : "copy", sources: uris, dest });
  }

  /** F5 / F6: copy or move the selection to the other pane. */
  async toOtherPane(move: boolean) {
    const src = this.activeTab;
    const dst = this.otherTab;
    if (!dst) {
      toasts.show("Turn on dual pane to copy between panes");
      return;
    }
    const uris = src.targets().map((e) => src.uriOf(e));
    if (!uris.length) return;
    const ok = await dialogs.confirm(
      `${move ? "Move" : "Copy"} ${uris.length === 1 ? `“${uriName(uris[0])}”` : `${uris.length} items`}?`,
      `To ${dst.folder.info?.display ?? dst.dirUri}`,
      move ? "Move" : "Copy",
    );
    if (ok) await this.transfer(uris, dst.dirUri, move);
  }

  async duplicate() {
    const t = this.activeTab;
    const uris = t.targets().map((e) => t.uriOf(e));
    if (uris.length) await transfers.submit({ kind: "copy", sources: uris, dest: t.dirUri, conflict: "keepBoth" });
  }
}

export const ws = new Workspace();

export type { Entry };
