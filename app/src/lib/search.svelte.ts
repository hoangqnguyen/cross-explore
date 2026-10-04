// Search results and tag collections, shown in a tab like a folder.
import { cancelTask, errorText, findTagged, search, uriName, type Entry, type Item, type LocationInfo, type SearchQuery } from "./api";
import type { Source } from "./folder.svelte";
import { comparator, mergeSorted, type SortSpec } from "./sort";

export function searchUri(root: string, text: string, content = false) {
  const p = new URLSearchParams({ root, q: text });
  if (content) p.set("content", "1");
  return `cx:search?${p}`;
}

export const tagUri = (tag: string) => `cx:tag?${new URLSearchParams({ name: tag })}`;

abstract class Results implements Source {
  abstract readonly uri: string;
  kind: "search" | "home" | "compare" = "search";
  info = $state.raw<LocationInfo | null>(null);
  caps = $state.raw(null);
  status = $state<"loading" | "ready" | "error">("loading");
  refreshing = $state(false);
  error = $state<string | null>(null);
  errorDetail = $state.raw(null);
  live = $state<"live" | "polling" | null>(null);
  items = $state.raw<Item[]>([]);
  fresh = $state.raw<ReadonlySet<string>>(new Set());
  timing = $state.raw<{ firstRowsMs: number; totalMs: number; count: number } | null>(null);
  protected cmp: (a: Item, b: Item) => number;
  protected gen = 0;

  constructor(sort: SortSpec) {
    this.cmp = comparator(sort);
  }

  abstract load(): Promise<void>;
  async watch() {}
  unwatch() {}
  dispose() {
    this.gen++;
  }
  setSort(spec: SortSpec) {
    this.cmp = comparator(spec);
    this.items = [...this.items].sort(this.cmp);
  }
  get(key: string) {
    return this.items.find((i) => i.uri === key);
  }
  upsertLocal(_e: Entry) {}
  removeLocal(keys: string[]) {
    const gone = new Set(keys);
    this.items = this.items.filter((i) => !gone.has(i.uri!));
  }
}

export class SearchResults extends Results {
  readonly uri: string;
  readonly root: string;
  readonly query: SearchQuery;
  #task: number | null = null;

  constructor(uri: string, sort: SortSpec) {
    super(sort);
    this.uri = uri;
    const p = new URLSearchParams(uri.slice(uri.indexOf("?") + 1));
    this.root = p.get("root") ?? "~";
    const text = p.get("q") ?? "";
    const content = p.get("content") === "1";
    this.query = content ? { text: "", content: text } : { text, mode: /[*?]/.test(text) ? "glob" : "substring" };
    const where = uriName(this.root) || this.root;
    this.info = {
      uri,
      scheme: "search",
      display: text,
      name: `“${text}”`,
      parent: this.root,
      crumbs: [
        { label: where, uri: this.root, icon: "folder" },
        { label: `${content ? "Files containing" : "Search"} “${text}”`, uri, icon: "search" },
      ],
      local: this.root.startsWith("file:"),
    };
  }

  async load() {
    const gen = ++this.gen;
    this.items = [];
    this.status = "loading";
    this.refreshing = true;
    const t0 = performance.now();
    try {
      this.#task = await search(this.root, this.query, (ev) => {
        if (gen !== this.gen) return;
        if (ev.type === "hits") {
          const batch: Item[] = ev.hits.map((h) => ({ ...h.entry, uri: h.uri, parent: h.parent, relPath: h.relPath, snippet: h.snippet ?? null, line: h.line ?? null }));
          this.items = mergeSorted(this.items, batch.sort(this.cmp), this.cmp);
          this.status = "ready";
        } else {
          this.refreshing = false;
          this.status = "ready";
          this.timing = { firstRowsMs: 0, totalMs: performance.now() - t0, count: this.items.length };
        }
      });
    } catch (e) {
      if (gen !== this.gen) return;
      this.refreshing = false;
      this.status = "error";
      this.error = errorText(e);
    }
  }

  override dispose() {
    super.dispose();
    if (this.#task != null) void cancelTask(this.#task).catch(() => {});
  }
}

export class TagResults extends Results {
  readonly uri: string;
  readonly tag: string;

  constructor(uri: string, sort: SortSpec) {
    super(sort);
    this.uri = uri;
    this.tag = new URLSearchParams(uri.slice(uri.indexOf("?") + 1)).get("name") ?? "";
    this.info = { uri, scheme: "tag", display: this.tag, name: this.tag, parent: null, crumbs: [{ label: this.tag, uri, icon: "tag" }], local: true };
  }

  async load() {
    const gen = ++this.gen;
    this.status = "loading";
    try {
      const hits = await findTagged(this.tag);
      if (gen !== this.gen) return;
      this.items = hits.map((h) => ({ ...h.entry, uri: h.uri, parent: h.parent, relPath: h.relPath })).sort(this.cmp);
      this.status = "ready";
    } catch (e) {
      this.status = "error";
      this.error = errorText(e);
    }
  }
}

/** Placeholder source for the Home page and other non-folder tabs. */
export class StaticSource extends Results {
  readonly uri: string;

  constructor(uri: string, name: string, icon: "home" | "search", sort: SortSpec, kind: "home" | "compare" = "home") {
    super(sort);
    this.uri = uri;
    this.kind = kind;
    this.info = { uri, scheme: "cx", display: name, name, parent: null, crumbs: [{ label: name, uri, icon }], local: true };
    this.status = "ready";
  }

  async load() {}
}
