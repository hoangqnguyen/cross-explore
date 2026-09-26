// In-memory stand-in for the Rust backend, used when the UI runs in a plain
// browser. It fakes a small home folder and, for folders being watched,
// occasionally adds and removes files so live updates are visible.
import type { Change, Entry, ListEvent, Places } from "./api";

const HOME = "/Users/demo";
const now = Date.now();
const min = 60_000;
const day = 86_400_000;

type Node = Entry & { children?: Map<string, Node> };

function dir(name: string, age: number, children: Node[] = [], hidden = false): Node {
  return { name, kind: "dir", isDir: true, size: 0, modified: now - age, created: now - age, hidden, readonly: false, children: new Map(children.map((c) => [c.name, c])) };
}
function file(name: string, size: number, age: number, hidden = false): Node {
  return { name, kind: "file", isDir: false, size, modified: now - age, created: now - age, hidden, readonly: false };
}

const root = dir("", 0, [
  dir("Users", 400 * day, [
    dir("demo", 90 * day, [
      dir("Desktop", 2 * day, [file("Screenshot 2026-09-25 at 10.41.03.png", 2_400_000, 26 * 3600_000), file("todo.md", 1_200, 3 * min)]),
      dir("Documents", 5 * day, [
        dir("Invoices", 12 * day, Array.from({ length: 24 }, (_, i) => file(`Invoice-2026-${String(i + 1).padStart(3, "0")}.pdf`, 80_000 + i * 913, (i + 3) * day))),
        dir("Projects", 1 * day),
        file("Budget 2026.xlsx", 48_200, 2 * day),
        file("Pitch deck.pptx", 5_800_000, 9 * day),
        file("Resume.docx", 32_100, 40 * day),
        file("notes.txt", 2_000, 50 * min),
      ]),
      dir("Downloads", 3 * 3600_000, [
        file("cross-explore-0.1.0.dmg", 14_800_000, 20 * min),
        file("holiday-photos.zip", 482_000_000, 3 * 3600_000),
        file("IMG_2041.HEIC", 3_100_000, day + 2 * 3600_000),
        file("IMG_2042.HEIC", 2_950_000, day + 2 * 3600_000),
        file("lecture-07.mp4", 1_240_000_000, 3 * day),
        file("podcast-ep12.mp3", 58_000_000, 4 * day),
        file("report-final-v2.pdf", 1_900_000, 6 * day),
        file("setup.sh", 4_100, 8 * day),
        file("Inter-4.0.zip", 6_200_000, 30 * day),
        file("dataset.csv", 12_400_000, 12 * day),
      ]),
      dir("Movies", 60 * day),
      dir("Music", 60 * day),
      dir("Pictures", 7 * day, Array.from({ length: 300 }, (_, i) => file(`IMG_${1000 + i}.jpg`, 1_500_000 + ((i * 7919) % 3_000_000), i * 3 * 3600_000))),
      dir("Code", 3 * 3600_000, [dir("cross-explore", 10 * min), dir("dotfiles", 20 * day)]),
      dir(".config", 10 * day, [], true),
      file(".zshrc", 3_400, 15 * day, true),
      dir("Library", 90 * day, [], true),
    ]),
  ]),
  dir("Applications", 30 * day),
  dir("System", 300 * day),
]);

const stripSlash = (s: string) => s.replace(/\/+$/, "") || "/";

function pathOf(uri: string): string {
  const p = uri.startsWith("file://") ? decodeURIComponent(uri.slice(7)) : uri.startsWith("~") ? HOME + uri.slice(1) : uri;
  return stripSlash(p);
}
const uriOf = (p: string) => "file://" + p.split("/").map(encodeURIComponent).join("/");

function lookup(path: string): Node | null {
  let n: Node | undefined = root;
  for (const part of path.split("/").filter(Boolean)) n = n?.children?.get(part);
  return n ?? null;
}

function info(path: string) {
  const crumbs: { label: string; uri: string; icon: "home" | "drive" | "folder" }[] = [];
  let acc = "";
  const inHome = path === HOME || path.startsWith(HOME + "/");
  const rest = inHome ? path.slice(HOME.length) : path;
  if (inHome) {
    acc = HOME;
    crumbs.push({ label: "demo", uri: uriOf(HOME), icon: "home" });
  } else crumbs.push({ label: "Macintosh HD", uri: "file:///", icon: "drive" });
  for (const part of rest.split("/").filter(Boolean)) {
    acc += "/" + part;
    crumbs.push({ label: part, uri: uriOf(acc), icon: "folder" });
  }
  const parent = path === "/" ? null : stripSlash(path.slice(0, path.lastIndexOf("/")));
  return { uri: uriOf(path), scheme: "file", display: path, name: crumbs.at(-1)!.label, parent: parent && uriOf(parent), crumbs };
}

const strip = ({ children, ...e }: Node): Entry => e;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const watchers = new Map<number, ReturnType<typeof setInterval>>();
let nextWatch = 1;

const handlers: Record<string, (a: any) => unknown> = {
  places: (): Places => ({
    platform: "macos",
    translucent: false,
    home: { name: "demo", uri: uriOf(HOME), icon: "home" },
    favorites: ["Desktop", "Documents", "Downloads", "Pictures", "Music", "Movies"].map((n, i) => ({
      name: n,
      uri: uriOf(`${HOME}/${n}`),
      icon: ["desktop", "documents", "downloads", "pictures", "music", "videos"][i],
    })),
    volumes: [
      { name: "Macintosh HD", uri: "file:///", total: 994e9, free: 212e9, removable: false },
      { name: "Data", uri: uriOf("/Volumes/Data"), total: 2e12, free: 1.31e12, removable: true },
    ],
  }),
  free_space: () => ({ free: 212e9, total: 994e9 }),
  async list_dir({ uri, onEvent }: { uri: string; onEvent: (e: ListEvent) => void }) {
    const path = pathOf(uri);
    const node = lookup(path);
    if (!node?.isDir) throw { kind: "notFound", message: path };
    onEvent({ type: "meta", info: info(path), capabilities: { liveWatch: true, polling: false, serverCopy: true, trash: true, posix: true, writable: true } });
    await sleep(5);
    const entries = [...node.children!.values()].map(strip);
    onEvent({ type: "batch", entries });
    onEvent({ type: "done", total: entries.length, elapsedMs: 5 });
  },
  watch_dir({ uri, onChange }: { uri: string; onChange: (c: Change[]) => void }) {
    const node = lookup(pathOf(uri));
    if (!node?.children) throw { kind: "notFound", message: uri };
    const id = nextWatch++;
    let n = 1;
    // Simulate someone else working in this folder.
    watchers.set(
      id,
      setInterval(() => {
        const name = `Shared note ${n++}.md`;
        const f = file(name, 800 + n * 10, 0);
        node.children!.set(name, f);
        onChange([{ type: "upsert", entry: strip(f) }]);
        if (n > 3) {
          const old = `Shared note ${n - 3}.md`;
          node.children!.delete(old);
          onChange([{ type: "remove", name: old }]);
        }
      }, 6000),
    );
    return { id, mode: "live" };
  },
  unwatch_dir({ id }: { id: number }) {
    clearInterval(watchers.get(id));
    watchers.delete(id);
  },
  create_folder({ uri, name }: { uri: string; name?: string }) {
    const node = lookup(pathOf(uri))!;
    let n = 1;
    let pick = name ?? "New folder";
    while (!name && node.children!.has(pick)) pick = `New folder (${++n})`;
    const d = dir(pick, 0);
    node.children!.set(pick, d);
    return strip(d);
  },
  rename_entry({ uri, from, to }: { uri: string; from: string; to: string }) {
    const node = lookup(pathOf(uri))!;
    if (node.children!.has(to)) throw { kind: "alreadyExists", message: to };
    const e = node.children!.get(from)!;
    node.children!.delete(from);
    e.name = to;
    node.children!.set(to, e);
    return strip(e);
  },
  trash_entries({ uri, names }: { uri: string; names: string[] }) {
    const node = lookup(pathOf(uri))!;
    names.forEach((n) => node.children!.delete(n));
    return names.map((n) => ({ original: uri + "/" + n, trashed: null }));
  },
  open_entry: () => undefined,
};

export async function invoke<T>(cmd: string, args: Record<string, unknown>): Promise<T> {
  const h = handlers[cmd];
  if (!h) throw { kind: "unsupported", message: cmd };
  return (await h(args)) as T;
}
