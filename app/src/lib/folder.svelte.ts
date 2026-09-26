// A live view of one folder: streamed listing + watcher patches, kept sorted.
import { errorText, listDir, unwatchDir, watchDir, type Capabilities, type Change, type Entry, type LocationInfo } from "./api";
import { comparator, insertionIndex, mergeSorted, type SortSpec } from "./sort";

/** How long a newly appeared row keeps its highlight. */
const FRESH_MS = 1600;
/** Above this many changes at once, re-sorting everything beats patching. */
const BULK_CHANGES = 64;

export class Folder {
  readonly uri: string;
  info = $state.raw<LocationInfo | null>(null);
  caps = $state.raw<Capabilities | null>(null);
  status = $state<"loading" | "ready" | "error">("loading");
  /** Re-listing in the background while the old rows stay on screen. */
  refreshing = $state(false);
  error = $state<string | null>(null);
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

  async load(): Promise<void> {
    const gen = ++this.#gen;
    const refresh = this.items.length > 0;
    if (refresh) this.refreshing = true;
    else this.status = "loading";
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
      this.refreshing = false;
      const total = performance.now() - t0;
      this.timing = { firstRowsMs: this.timing?.firstRowsMs ?? total, totalMs: total, count: acc.length };
      const buffered = this.#buffer;
      this.#buffer = null;
      if (buffered?.length) this.#apply(buffered);
    } catch (e) {
      if (gen !== this.#gen) return;
      this.#buffer = null;
      this.#byName = new Map();
      this.items = [];
      this.refreshing = false;
      this.status = "error";
      this.error = errorText(e);
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
      void this.load();
      return;
    }
    const bulk = changes.length > BULK_CHANGES;
    const items = bulk ? this.items : this.items.slice();
    const added: string[] = [];
    const drop = (e: Entry) => {
      const i = items.indexOf(e);
      if (i >= 0) items.splice(i, 1);
    };
    for (const c of changes) {
      if (c.type === "upsert") {
        const prev = this.#byName.get(c.entry.name);
        this.#byName.set(c.entry.name, c.entry);
        if (!prev) added.push(c.entry.name);
        if (!bulk) {
          if (prev) drop(prev);
          items.splice(insertionIndex(items, c.entry, this.#cmp), 0, c.entry);
        }
      } else if (c.type === "remove") {
        const prev = this.#byName.get(c.name);
        if (!prev) continue;
        this.#byName.delete(c.name);
        if (!bulk) drop(prev);
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
