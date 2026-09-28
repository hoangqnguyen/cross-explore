// A live view of one folder: streamed listing + watcher patches, kept sorted.
import { asCxError, errorText, listDir, unwatchDir, watchDir, type Capabilities, type Change, type CxError, type Entry, type Item, type LocationInfo } from "./api";
import { comparator, insertionIndex, mergeSorted, type SortSpec } from "./sort";

/** Something a tab shows: a folder, search results, a tag… */
export interface Source {
  readonly uri: string;
  readonly kind: "folder" | "search" | "home" | "compare";
  info: LocationInfo | null;
  caps: Capabilities | null;
  status: "loading" | "ready" | "error";
  refreshing: boolean;
  error: string | null;
  /** The raw error, so the UI can offer to sign in, trust a host, … */
  errorDetail: CxError | null;
  live: "live" | "polling" | null;
  items: Item[];
  fresh: ReadonlySet<string>;
  timing: { firstRowsMs: number; totalMs: number; count: number } | null;
  /** `quiet`: a background refresh (no spinner; keep rows if it fails). */
  load(opts?: { quiet?: boolean }): Promise<void>;
  watch(): Promise<void>;
  unwatch(): void;
  dispose(): void;
  setSort(spec: SortSpec): void;
  get(key: string): Item | undefined;
  upsertLocal(entry: Entry): void;
  removeLocal(keys: string[]): void;
}

/** Stable identity of a row within its source. */
export const keyOf = (e: Item) => e.uri ?? e.name;

/** How long a newly appeared row keeps its highlight. */
const FRESH_MS = 1600;
/** Above this many changes at once, re-sorting everything beats patching. */
const BULK_CHANGES = 64;

export class Folder implements Source {
  readonly uri: string;
  readonly kind = "folder" as const;
  info = $state.raw<LocationInfo | null>(null);
  caps = $state.raw<Capabilities | null>(null);
  status = $state<"loading" | "ready" | "error">("loading");
  /** Re-listing in the background while the old rows stay on screen. */
  refreshing = $state(false);
  error = $state<string | null>(null);
  errorDetail = $state.raw<CxError | null>(null);
  /** How changes reach us: pushed ("live"), re-listed ("polling"), or not at all. */
  live = $state<"live" | "polling" | null>(null);
  /** All entries (hidden ones included), sorted. */
  items = $state.raw<Entry[]>([]);
  /** Names that just appeared through a live update. */
  fresh = $state.raw<ReadonlySet<string>>(new Set());
  timing = $state.raw<{ firstRowsMs: number; totalMs: number; count: number } | null>(null);

  #cmp: (a: Entry, b: Entry) => number;
  #byName = new Map<string, Entry>();
  #gen = 0;
  #watchId: number | null = null;
  #wantWatch = false;
  /** Watch patches that arrive while a listing is in flight wait here. */
  #buffer: Change[] | null = null;
  #freshTimers = new Map<string, ReturnType<typeof setTimeout>>();

  constructor(uri: string, sort: SortSpec) {
    this.uri = uri;
    this.#cmp = comparator(sort);
  }

  setSort(spec: SortSpec) {
    this.#cmp = comparator(spec);
    this.items = [...this.items].sort(this.#cmp);
  }

  get(name: string): Entry | undefined {
    return this.#byName.get(name);
  }

  /**
   * List the folder. `quiet` is for refreshes nobody asked for (a server's
   * "re-list please", our own copy landing): no spinner, new rows get the
   * live-change glow, and a failed attempt keeps what's on screen.
   */
  async load(opts: { quiet?: boolean } = {}): Promise<void> {
    const gen = ++this.#gen;
    const refresh = this.items.length > 0;
    const quiet = !!opts.quiet && refresh;
    const before = quiet ? new Set(this.#byName.keys()) : null;
    if (refresh && !quiet) this.refreshing = true;
    else if (!refresh) this.status = "loading";
    this.#buffer = [];

    const cmp = this.#cmp;
    const t0 = performance.now();
    const map = new Map<string, Entry>();
    let acc: Entry[] = [];
    let painted = false;
    let flushQueued = false;
    const flush = () => {
      flushQueued = false;
      if (gen !== this.#gen) return;
      this.items = acc;
      this.#byName = map; // what's shown and the index stay in step
      this.status = "ready";
      if (!painted) {
        painted = true;
        // Time until the first rows are actually on screen.
        requestAnimationFrame(() => {
          if (gen === this.#gen) this.timing = { firstRowsMs: performance.now() - t0, totalMs: 0, count: 0 };
        });
      }
    };

    try {
      await listDir(this.uri, (ev) => {
        if (gen !== this.#gen) return;
        if (ev.type === "meta") {
          this.info = ev.info;
          this.caps = ev.capabilities;
        } else if (ev.type === "batch") {
          for (const e of ev.entries) map.set(e.name, e);
          acc = mergeSorted(acc, ev.entries.sort(cmp), cmp);
          // A refresh swaps rows in once, at the end, so nothing flickers.
          if (!refresh && !flushQueued) {
            flushQueued = true;
            requestAnimationFrame(flush);
          }
        }
      });
      if (gen !== this.#gen) return;
      if (cmp !== this.#cmp) acc.sort(this.#cmp); // sort changed mid-listing
      this.#byName = map;
      this.items = acc;
      this.status = "ready";
      this.error = null;
      this.errorDetail = null;
      this.refreshing = false;
      const total = performance.now() - t0;
      this.timing = { firstRowsMs: this.timing?.firstRowsMs ?? total, totalMs: total, count: acc.length };
      const buffered = this.#buffer;
      this.#buffer = null;
      if (buffered?.length) this.#apply(buffered);
      if (before) {
        const added = acc.filter((e) => !before.has(e.name)).map((e) => e.name);
        if (added.length && added.length < acc.length) this.#markFresh(added);
      }
    } catch (e) {
      if (gen !== this.#gen) return;
      this.#buffer = null;
      if (quiet) {
        // A hiccup while refreshing in the background: keep the rows; the
        // next change or poll will try again.
        return;
      }
      this.#byName = new Map();
      this.items = [];
      this.refreshing = false;
      this.status = "error";
      this.error = errorText(e);
      this.errorDetail = asCxError(e);
    }
  }

  /** Start pushing live changes into `items`. Idempotent. */
  async watch(): Promise<void> {
    if (this.#wantWatch) return;
    this.#wantWatch = true;
    try {
      const { id, mode } = await watchDir(this.uri, (changes) => this.#onChanges(changes));
      if (!this.#wantWatch) {
        void unwatchDir(id); // stopped while we were starting
        return;
      }
      this.#watchId = id;
      this.live = mode;
    } catch {
      this.#wantWatch = false;
      this.live = null;
    }
  }

  unwatch() {
    this.#wantWatch = false;
    this.live = null;
    if (this.#watchId != null) {
      void unwatchDir(this.#watchId);
      this.#watchId = null;
    }
  }

  dispose() {
    this.#gen++;
    this.unwatch();
    for (const t of this.#freshTimers.values()) clearTimeout(t);
  }

  /** Apply the result of our own operation right away; the watcher confirms it later. */
  upsertLocal(entry: Entry) {
    this.#apply([{ type: "upsert", entry }], false);
  }

  removeLocal(names: string[]) {
    // For folders a row's key is its name.
    this.#apply(
      names.map((name) => ({ type: "remove", name })),
      false,
    );
  }

  #onChanges(changes: Change[]) {
    if (this.#buffer) this.#buffer.push(...changes);
    else this.#apply(changes);
  }

  #apply(changes: Change[], highlight = true) {
    if (changes.some((c) => c.type === "reset")) {
      void this.load({ quiet: true });
      return;
    }
    const bulk = changes.length > BULK_CHANGES;
    const items = bulk ? this.items : this.items.slice();
    const added: string[] = [];
    // Rows are matched by name (a folder can't hold two of the same), so a
    // change can never leave a duplicate row behind.
    const drop = (name: string, hint?: Entry) => {
      let i = hint ? items.indexOf(hint) : -1;
      if (i < 0 || items[i].name !== name) i = items.findIndex((x) => x.name === name);
      if (i >= 0) items.splice(i, 1);
    };
    for (const c of changes) {
      if (c.type === "upsert") {
        const prev = this.#byName.get(c.entry.name);
        this.#byName.set(c.entry.name, c.entry);
        if (!prev) added.push(c.entry.name);
        if (!bulk) {
          drop(c.entry.name, prev);
          items.splice(insertionIndex(items, c.entry, this.#cmp), 0, c.entry);
        }
      } else if (c.type === "remove") {
        const prev = this.#byName.get(c.name);
        this.#byName.delete(c.name);
        if (!bulk) drop(c.name, prev);
      }
    }
    this.items = bulk ? [...this.#byName.values()].sort(this.#cmp) : items;
    if (highlight && added.length) this.#markFresh(added);
  }

  #markFresh(names: string[]) {
    const next = new Set(this.fresh);
    for (const n of names) {
      next.add(n);
      clearTimeout(this.#freshTimers.get(n));
      this.#freshTimers.set(
        n,
        setTimeout(() => {
          this.#freshTimers.delete(n);
          const s = new Set(this.fresh);
          s.delete(n);
          this.fresh = s;
        }, FRESH_MS),
      );
    }
    this.fresh = next;
  }
}
