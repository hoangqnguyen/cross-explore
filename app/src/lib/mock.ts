// In-memory stand-in for the Rust backend, used when the UI runs in a plain
// browser (`npm run dev`). It fakes a home folder, a NAS that needs a
// password ("demo"), nearby devices and a transfer engine, so every screen
// can be developed and tested without Tauri.
import type { AppEvent, Change, Conflict, Device, DiffItem, Entry, JobRequest, JobSnapshot, ListEvent, PeerStatus, Places, SearchEvent, SearchHit, SearchQuery, SizeProgress } from "./api";

const HOME = "/Users/demo";
const now = Date.now();
const min = 60_000;
const day = 86_400_000;

type Node = Entry & { children?: Map<string, Node>; content?: string };

function dir(name: string, age: number, children: Node[] = [], hidden = false): Node {
  return { name, kind: "dir", isDir: true, size: 0, modified: now - age, created: now - age, hidden, readonly: false, children: new Map(children.map((c) => [c.name, c])) };
}
function file(name: string, size: number, age: number, hidden = false, content?: string): Node {
  return { name, kind: "file", isDir: false, size, modified: now - age, created: now - age, hidden, readonly: false, content };
}

const readme = `# Cross Explore\n\nA fast, **live** file explorer.\n\n- Dual pane\n- Quick Look\n- Network shares & peers\n`;
const code = `use std::fs;\n\nfn main() {\n    for e in fs::read_dir(".").unwrap() {\n        println!("{}", e.unwrap().path().display());\n    }\n}\n`;
const reportHtml = `<!doctype html>\n<html lang="en">\n<head>\n<meta charset="utf-8">\n<title>Run Report</title>\n<style>body{font-family:sans-serif;padding:24px}.status{display:inline-block;padding:4px 12px;border-radius:999px;background:#2fa66a;color:#fff}</style>\n</head>\n<body>\n<h1>Full Run Report</h1>\n<p>Status: <span class="status">All checks passed</span></p>\n</body>\n</html>\n`;

const py = `import sys\n\n\nclass Greeter:\n    def __init__(self, name):\n        self.name = name\n\n    def greet(self):\n        if self.name:\n            print("hi", self.name)\n        else:\n            print("hi")\n\n\nif __name__ == "__main__":\n    Greeter(sys.argv[1]).greet()\n`;

const roots: Record<string, Node> = {
  "file://": dir("", 0, [
    dir("Users", 400 * day, [
      dir("demo", 90 * day, [
        dir("Desktop", 2 * day, [file("Screenshot 2026-09-25 at 10.41.03.png", 2_400_000, 26 * 3600_000), file("todo.md", 1_200, 3 * min, false, "- [ ] ship Phase 1\n- [x] live updates\n")]),
        dir("Documents", 5 * day, [
          dir("Invoices", 12 * day, Array.from({ length: 24 }, (_, i) => file(`Invoice-2026-${String(i + 1).padStart(3, "0")}.pdf`, 80_000 + i * 913, (i + 3) * day))),
          dir("Projects", 1 * day, [file("README.md", readme.length, 2 * day, false, readme), file("main.rs", code.length, day, false, code), file("greet.py", py.length, day, false, py), file("report.html", reportHtml.length, day, false, reportHtml)]),
          file("Budget 2026.xlsx", 48_200, 2 * day),
          file("Pitch deck.pptx", 5_800_000, 9 * day),
          file("Resume.docx", 32_100, 40 * day),
          file("notes.txt", 2_000, 50 * min, false, "Call the NAS guy.\nBuy more drives.\n"),
          file("archive-2025.zip", 88_000_000, 60 * day),
        ]),
        dir("Downloads", 3 * 3600_000, [
          file("cross-explore-0.1.0.dmg", 14_800_000, 20 * min),
          file("holiday-photos.zip", 482_000_000, 3 * 3600_000),
          file("IMG_2041.HEIC", 3_100_000, day + 2 * 3600_000),
          file("IMG_2042.HEIC", 2_950_000, day + 2 * 3600_000),
          file("lecture-07.mp4", 1_240_000_000, 3 * day),
          file("podcast-ep12.mp3", 58_000_000, 4 * day),
          file("report-final-v2.pdf", 1_900_000, 6 * day),
          file("setup.sh", 4_100, 8 * day, false, "#!/bin/sh\nset -e\necho installing\n"),
          file("Inter-4.0.zip", 6_200_000, 30 * day),
          file("dataset.csv", 12_400_000, 12 * day, false, "id,name,score\n1,ada,99\n2,linus,97\n"),
        ]),
        dir("Movies", 60 * day),
        dir("Music", 60 * day),
        dir("Pictures", 7 * day, Array.from({ length: 300 }, (_, i) => file(`IMG_${1000 + i}.jpg`, 1_500_000 + ((i * 7919) % 3_000_000), i * 3 * 3600_000))),
        dir("Code", 3 * 3600_000, [
          dir("cross-explore", 10 * min),
          dir("dotfiles", 20 * day),
          // Every kind of file icon, for eyeballing (tests/ui-shots.mjs).
          dir(
            "icon-gallery",
            day,
            [
              "distribution.cer", "Staging_AdHoc.mobileprovision", "google-services.json", "release-001.zip", "model_zoom.pth", "VID_0001.insv", "LRV_0001.lrv",
              "PawnIO_setup.exe", "installer.msi", "segment_01.webm", "Full Run Report.html", "invoice.pdf", "HOADON.xml", "report.doc", "budget.xlsx",
              "deck.pptx", "photo.jpg", "song.flac", "notes.md", "readme.txt", "server.log", "main.rs", "app.ts", "script.py", "index.js", "style.css",
              "App.svelte", "settings.yaml", "data.csv", "backup.tar.gz", "macOS.dmg", "font.ttf", "scene.blend", "design.fig", "book.epub", "movie.srt",
              "cal.ics", "card.vcf", "mail.eml", "site.url", "db.sqlite", "weights.onnx", "id_ed25519.pub", "package.deb", "app.apk", "tool.jar",
              "lib.dll", "shortcut.lnk", "run.bat", "deploy.sh", "Cargo.lock", "unknown.xyz",
            ]
              .map((n) => file(n, 10_000, day))
              // A Unix program without an extension (execute bit set).
              .concat([{ ...file("cx-helper", 2_000_000, day), executable: true }]),
          ),
        ]),
        dir("Big", day),
        dir(".config", 10 * day, [], true),
        file(".zshrc", 3_400, 15 * day, true, "export PATH=$HOME/bin:$PATH\n"),
        dir("Library", 90 * day, [], true),
      ]),
    ]),
    dir("Applications", 30 * day),
    dir("System", 300 * day),
  ]),
  "smb://nas.local": dir("", 0, [
    dir("Media", 3 * day, [dir("Movies", 5 * day, [file("Dune (2021).mkv", 18_000_000_000, 90 * day)]), dir("Photos", 2 * day)]),
    dir("Backups", 1 * day, [file("mbp-2026-09-20.sparsebundle", 220_000_000_000, 6 * day)]),
    dir("Public", 10 * day, [file("welcome.txt", 120, 100 * day, false, "Welcome to the NAS\n")]),
  ]),
};
let nasSignedIn = false;

const uriOf = (base: string, p: string) => base + (base === "file://" ? "" : "") + p.split("/").map(encodeURIComponent).join("/");

function parse(uri: string): { base: string; path: string } {
  if (uri.startsWith("~")) uri = "file://" + HOME + uri.slice(1);
  if (uri.startsWith("/")) uri = "file://" + uri;
  const m = /^([a-z]+:\/\/[^/]*)(\/.*)?$/.exec(uri);
  if (!m) throw { kind: "invalidLocation", message: uri };
  const base = m[1].startsWith("file://") ? "file://" : m[1].replace(/^smb:\/\/[^@]*@/, "smb://");
  const path = decodeURIComponent(m[2] ?? "/").replace(/\/+$/, "") || "/";
  return { base, path };
}

// A 100,000-item folder for performance checks, generated on first use.
/** Opening a server's file downloads it first: about a second and a half, in steps. */
const openCancels = new Set<number>();
async function mockDownload({ uri, id, onProgress }: { uri: string; id: number; onProgress: (p: { done: number; total: number }) => void }) {
  if (parse(uri).base === "file://") return;
  const total = lookup(uri)?.size ?? 10_000_000;
  await sleep(200); // connecting
  for (let i = 0; i <= 15; i++) {
    if (openCancels.delete(id)) throw { kind: "cancelled" };
    onProgress({ done: Math.round((total * i) / 15), total });
    await sleep(100);
  }
}

function bigFolder(n: Node) {
  if (n.children!.size) return;
  const exts = ["jpg", "pdf", "txt", "mp4", "zip", "md", "rs", "png"];
  for (let i = 0; i < 100_000; i++) {
    const name = `file-${String(i).padStart(6, "0")}.${exts[i % exts.length]}`;
    n.children!.set(name, file(name, (i * 7919) % 5_000_000, (i % 1000) * min));
  }
}

function lookup(uri: string): Node | null {
  const { base, path } = parse(uri);
  let n: Node | undefined = roots[base];
  if (!n) throw { kind: "connection", message: `${base} is not reachable` };
  if (base.startsWith("smb://") && !nasSignedIn) throw { kind: "authRequired", message: { uri: base, user: null, reason: "" } };
  for (const part of path.split("/").filter(Boolean)) n = n?.children?.get(part);
  if (n && path.endsWith("/Big")) bigFolder(n);
  return n ?? null;
}

function info(uri: string) {
  const { base, path } = parse(uri);
  const crumbs: { label: string; uri: string; icon: string }[] = [];
  let acc = "";
  if (base !== "file://") {
    crumbs.push({ label: base.replace(/^\w+:\/\//, ""), uri: base + "/", icon: "server" });
    for (const [i, part] of path.split("/").filter(Boolean).entries()) {
      acc += "/" + part;
      crumbs.push({ label: part, uri: uriOf(base, acc), icon: i === 0 ? "share" : "folder" });
    }
  } else {
    const inHome = path === HOME || path.startsWith(HOME + "/");
    const rest = inHome ? path.slice(HOME.length) : path;
    if (inHome) {
      acc = HOME;
      crumbs.push({ label: "demo", uri: uriOf(base, HOME), icon: "home" });
    } else crumbs.push({ label: "Macintosh HD", uri: "file:///", icon: "drive" });
    for (const part of rest.split("/").filter(Boolean)) {
      acc += "/" + part;
      crumbs.push({ label: part, uri: uriOf(base, acc), icon: "folder" });
    }
  }
  const parent = path === "/" ? null : path.slice(0, path.lastIndexOf("/")) || "/";
  return {
    uri: uriOf(base, path === "/" ? "/" : path),
    scheme: base.split(":")[0],
    display: base === "file://" ? path : base + path,
    name: crumbs.at(-1)!.label,
    parent: parent && uriOf(base, parent),
    crumbs,
    local: base === "file://",
  };
}

const strip = ({ children, content, ...e }: Node): Entry => e;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const watchers = new Map<number, { node: Node; fn: (c: Change[]) => void; timer?: ReturnType<typeof setInterval> }>();
let nextId = 1;
let emit: (e: AppEvent) => void = () => {};

function notify(parentNode: Node, changes: Change[]) {
  for (const w of watchers.values()) if (w.node === parentNode) w.fn(changes);
}

function uniqueName(node: Node, name: string) {
  if (!node.children!.has(name)) return name;
  const dot = name.lastIndexOf(".");
  const [stem, ext] = dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, ""];
  for (let n = 2; ; n++) if (!node.children!.has(`${stem} (${n})${ext}`)) return `${stem} (${n})${ext}`;
}

function clone(n: Node, name = n.name): Node {
  return { ...n, name, children: n.children && new Map([...n.children].map(([k, v]) => [k, clone(v)])) };
}

function totalSize(n: Node): number {
  return n.isDir ? [...n.children!.values()].reduce((a, c) => a + totalSize(c), 0) : n.size;
}

// ---------- jobs ----------

const jobs = new Map<number, JobSnapshot & { resolve?: (r: string) => void; paused?: boolean; cancelled?: boolean }>();

async function runJob(id: number, req: JobRequest) {
  const job = jobs.get(id)!;
  const push = () => emit({ type: "job", job: { ...job, resolve: undefined } as JobSnapshot });
  const srcs = req.sources.map((u) => ({ uri: u, parentUri: u.slice(0, u.lastIndexOf("/")), name: decodeURIComponent(u.slice(u.lastIndexOf("/") + 1)) }));
  job.bytesTotal = srcs.reduce((a, s) => a + totalSize(lookup(s.uri)!), 0);
  job.filesTotal = srcs.length;
  job.state = "running";
  push();
  const destNode = req.dest ? lookup(req.dest) : null;
  let applyAll: string | null = null;
  const created: string[] = [];
  for (const s of srcs) {
    if (job.cancelled) break;
    const parentNode = lookup(s.parentUri)!;
    const node = parentNode.children!.get(s.name);
    if (!node) continue;
    if (req.kind === "trash" || req.kind === "delete") {
      parentNode.children!.delete(s.name);
      notify(parentNode, [{ type: "remove", name: s.name }]);
    } else if (destNode) {
      let name = s.name;
      if (destNode === parentNode && req.kind === "copy") name = uniqueName(destNode, s.name.replace(/(\.[^.]*)?$/, " - Copy$1"));
      else if (destNode.children!.has(name)) {
        let res: string | null = applyAll ?? (req.conflict === "ask" || !req.conflict ? null : req.conflict);
        if (!res) {
          const c: Conflict = { id: nextId++, source: strip(node), sourceUri: s.uri, dest: strip(destNode.children!.get(name)!), destUri: req.dest!.replace(/\/$/, "") + "/" + encodeURIComponent(name) };
          job.state = "waitingForConflict";
          job.conflict = c;
          push();
          res = await new Promise<string>((r) => (job.resolve = r));
          job.conflict = null;
          job.state = "running";
          if (res.endsWith("!")) applyAll = res = res.slice(0, -1);
        }
        if (res === "skip") continue;
        if (res === "keepBoth") name = uniqueName(destNode, name);
      }
      // Fake progress in slices.
      const size = totalSize(node);
      for (let i = 1; i <= 10; i++) {
        while (job.paused && !job.cancelled) await sleep(100);
        if (job.cancelled) break;
        await sleep(Math.min(250, 20 + size / 5e7));
        job.bytesDone += size / 10;
        job.speed = 48e6 + Math.random() * 8e6;
        job.eta = Math.max(0, (job.bytesTotal - job.bytesDone) / job.speed);
        job.current = s.name;
        push();
      }
      if (job.cancelled) break;
      const copy = clone(node, name);
      copy.modified = node.modified;
      destNode.children!.set(name, copy);
      created.push(req.dest!.replace(/\/$/, "") + "/" + encodeURIComponent(name));
      notify(destNode, [{ type: "upsert", entry: strip(copy) }]);
      if (req.kind === "move") {
        parentNode.children!.delete(s.name);
        notify(parentNode, [{ type: "remove", name: s.name }]);
      }
    }
    job.filesDone++;
    push();
  }
  job.state = job.cancelled ? "cancelled" : "done";
  job.speed = 0;
  job.undo = req.kind === "copy" ? { type: "copy", created } : req.kind === "move" ? { type: "move", sources: req.sources, dest: req.dest } : null;
  push();
}

// ---------- fake images for thumbnails ----------

function colorFor(s: string) {
  let h = 0;
  for (const c of s) h = (h * 31 + c.charCodeAt(0)) % 360;
  return h;
}

function fakeImage(uri: string, size: number) {
  const h = colorFor(uri);
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${size * 1.5}" height="${size}" viewBox="0 0 150 100"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="hsl(${h},70%,62%)"/><stop offset="1" stop-color="hsl(${(h + 60) % 360},70%,40%)"/></linearGradient></defs><rect width="150" height="100" fill="url(#g)"/><circle cx="110" cy="30" r="12" fill="hsl(${(h + 30) % 360},90%,85%)"/><path d="M0 100 L45 50 L75 80 L100 60 L150 100Z" fill="hsl(${h},40%,25%)" opacity=".7"/></svg>`;
  return "data:image/svg+xml;charset=utf-8," + encodeURIComponent(svg);
}

/** A small valid 3-page PDF (so the pdf.js viewer has something real to draw). */
function fakePdf(uri: string) {
  const title = decodeURIComponent(uri.split("/").pop() ?? "").replace(/[()\\]/g, "");
  const n = 3;
  const objs: string[] = ["<< /Type /Catalog /Pages 2 0 R >>", `<< /Type /Pages /Kids [${Array.from({ length: n }, (_, i) => `${4 + i * 2} 0 R`).join(" ")}] /Count ${n} >>`, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"];
  for (let i = 0; i < n; i++) {
    const text = `BT /F1 28 Tf 72 700 Td (${title}) Tj 0 -40 Td /F1 18 Tf (Page ${i + 1} of ${n}) Tj ET 0.2 0.5 0.9 rg 72 200 468 300 re f`;
    objs.push(`<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents ${5 + i * 2} 0 R >>`);
    objs.push(`<< /Length ${text.length} >>\nstream\n${text}\nendstream`);
  }
  let out = "%PDF-1.4\n";
  const offsets: number[] = [];
  objs.forEach((o, i) => {
    offsets.push(out.length);
    out += `${i + 1} 0 obj\n${o}\nendobj\n`;
  });
  const xref = out.length;
  out += `xref\n0 ${objs.length + 1}\n0000000000 65535 f \n${offsets.map((o) => `${String(o).padStart(10, "0")} 00000 n \n`).join("")}`;
  out += `trailer\n<< /Size ${objs.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return "data:application/pdf;base64," + btoa(out);
}

// A quarter-second 440Hz tone, real enough for <video>/<audio> to actually
// load and play (Chrome plays the audio track of a video-less file fine),
// so a UI test can check playback state instead of just DOM presence.
const TONE_WAV =
  "data:audio/wav;base64,UklGRvQHAABXQVZFZm10IBAAAAABAAEAQB8AAEAfAAABAAgAZGF0YdAHAACAh42RlJSSjYeBenRvbGxucnh/hoyRk5SSjomCe3VwbWxucXd9hIuQk5STj4qDfHZxbWxtcXZ8g4qPk5STkIuEfXdxbmxtcHV7gomOkpSTkYyGf3hybmxsb3R6gYeNkpSUkY2HgHlzb2xsbnN5f4aMkZSUko6IgXp0b21sbnJ3foWLkJOUko+Jg3x1cG1sbXF2fYSKj5OUk4+KhH12cW1sbXB1fIOJj5KUk5CLhX53cm5sbW90eoGIjpKUlJGMhn95c25sbG9zeYCHjZGUlJKNh4F6dG9sbG5yeH+GjJGTlJKOiYJ7dXBtbG5xd32Ei5CTlJOPioN8dnFtbG1xdnyDio+TlJOQi4R9d3FubG1wdXuCiY6SlJORjIZ/eHJubGxvdHqBh42SlJSRjYeAeXNvbGxuc3l/hoyRlJSSjoiBenRvbWxucnd+hYuQk5SSj4mDfHVwbWxtcXZ9hIqPk5STj4qEfXZxbWxtcHV8g4mPkpSTkIuFfndybmxtb3R6gYiOkpSUkYyGf3lzbmxsb3N5gIeNkZSUko2HgXp0b2xsbnJ4f4aMkZOUko6Jgnt1cG1sbnF3fYSLkJOUk4+Kg3x2cW1sbXF2fIOKj5OUk5CLhH13cW5sbXB1e4KJjpKUk5GMhn94cm5sbG90eoGHjZKUlJGNh4B5c29sbG5zeX+GjJGUlJKOiIF6dG9tbG5yd36Fi5CTlJKPiYN8dXBtbG1xdn2Eio+TlJOPioR9dnFtbG1wdXyDiY+SlJOQi4V+d3JubG1vdHqBiI6SlJSRjIZ/eXNubGxvc3mAh42RlJSSjYeBenRvbGxucnh/hoyRk5SSjomCe3VwbWxucXd9hIuQk5STj4qDfHZxbWxtcXZ8g4qPk5STkIuEfXdxbmxtcHV7gomOkpSTkYyGf3hybmxsb3R6gYeNkpSUkY2HgHlzb2xsbnN5f4aMkZSUko6IgXp0b21sbnJ3foWLkJOUko+Jg3x1cG1sbXF2fYSKj5OUk4+KhH12cW1sbXB1fIOJj5KUk5CLhX53cm5sbW90eoGIjpKUlJGMhn95c25sbG9zeYCHjZGUlJKNh4F6dG9sbG5yeH+GjJGTlJKOiYJ7dXBtbG5xd32Ei5CTlJOPioN8dnFtbG1xdnyDio+TlJOQi4R9d3FubG1wdXuCiY6SlJORjIZ/eHJubGxvdHqBh42SlJSRjYeAeXNvbGxuc3l/hoyRlJSSjoiBenRvbWxucnd+hYuQk5SSj4mDfHVwbWxtcXZ9hIqPk5STj4qEfXZxbWxtcHV8g4mPkpSTkIuFfndybmxtb3R6gYiOkpSUkYyGf3lzbmxsb3N5gIeNkZSUko2HgXp0b2xsbnJ4f4aMkZOUko6Jgnt1cG1sbnF3fYSLkJOUk4+Kg3x2cW1sbXF2fIOKj5OUk5CLhH13cW5sbXB1e4KJjpKUk5GMhn94cm5sbG90eoGHjZKUlJGNh4B5c29sbG5zeX+GjJGUlJKOiIF6dG9tbG5yd36Fi5CTlJKPiYN8dXBtbG1xdn2Eio+TlJOPioR9dnFtbG1wdXyDiY+SlJOQi4V+d3JubG1vdHqBiI6SlJSRjIZ/eXNubGxvc3mAh42RlJSSjYeBenRvbGxucnh/hoyRk5SSjomCe3VwbWxucXd9hIuQk5STj4qDfHZxbWxtcXZ8g4qPk5STkIuEfXdxbmxtcHV7gomOkpSTkYyGf3hybmxsb3R6gYeNkpSUkY2HgHlzb2xsbnN5f4aMkZSUko6IgXp0b21sbnJ3foWLkJOUko+Jg3x1cG1sbXF2fYSKj5OUk4+KhH12cW1sbXB1fIOJj5KUk5CLhX53cm5sbW90eoGIjpKUlJGMhn95c25sbG9zeYCHjZGUlJKNh4F6dG9sbG5yeH+GjJGTlJKOiYJ7dXBtbG5xd32Ei5CTlJOPioN8dnFtbG1xdnyDio+TlJOQi4R9d3FubG1wdXuCiY6SlJORjIZ/eHJubGxvdHqBh42SlJSRjYeAeXNvbGxuc3l/hoyRlJSSjoiBenRvbWxucnd+hYuQk5SSj4mDfHVwbWxtcXZ9hIqPk5STj4qEfXZxbWxtcHV8g4mPkpSTkIuFfndybmxtb3R6gYiOkpSUkYyGf3lzbmxsb3N5gIeNkZSUko2HgXp0b2xsbnJ4f4aMkZOUko6Jgnt1cG1sbnF3fYSLkJOUk4+Kg3x2cW1sbXF2fIOKj5OUk5CLhH13cW5sbXB1e4KJjpKUk5GMhn94cm5sbG90eoGHjZKUlJGNh4B5c29sbG5zeX+GjJGUlJKOiIF6dG9tbG5yd36Fi5CTlJKPiYN8dXBtbG1xdn2Eio+TlJOPioR9dnFtbG1wdXyDiY+SlJOQi4V+d3JubG1vdHqBiI6SlJSRjIZ/eXNubGxvc3mAh42RlJSSjYeBenRvbGxucnh/hoyRk5SSjomCe3VwbWxucXd9hIuQk5STj4qDfHZxbWxtcXZ8g4qPk5STkIuEfXdxbmxtcHV7gomOkpSTkYyGf3hybmxsb3R6gYeNkpSUkY2HgHlzb2xsbnN5f4aMkZSUko6IgXp0b21sbnJ3foWLkJOUko+Jg3x1cG1sbXF2fYSKj5OUk4+KhH12cW1sbXB1fIOJj5KUk5CLhX53cm5sbW90eoGIjpKUlJGMhn95c25sbG9zeQ==";

export function fileUrl(uri: string) {
  if (/\.pdf$/i.test(uri)) return fakePdf(uri);
  if (/\.(png|jpe?g|heic|gif|webp)$/i.test(uri)) return fakeImage(uri, 800);
  if (/\.(mp4|mov|mkv|avi|webm|m4v|wmv|flv|mpg|mpeg|3gp|mp3|wav|flac|aac|m4a|ogg|opus|aiff|wma|alac)$/i.test(uri)) return TONE_WAV;
  return "data:text/plain,preview";
}

export function thumbUrl(uri: string, size: number) {
  return /\.(png|jpe?g|heic|gif|webp)$/i.test(uri) ? fakeImage(uri, size) : "";
}

// ---------- devices ----------

const fakeDevices: Device[] = [
  {
    id: "nas",
    name: "synology",
    kind: "nas",
    addresses: ["192.168.1.20"],
    hostname: "nas.local",
    sources: ["mdns", "ssdp"],
    tailnet: null,
    services: [{ scheme: "smb", port: 445, uri: "smb://nas.local/", label: "Files (SMB)", source: "mdns" }],
    shares: [
      { name: "Media", uri: "smb://nas.local/Media" },
      { name: "Backups", uri: "smb://nas.local/Backups" },
    ],
    lastSeen: now,
  },
  {
    id: "hpc",
    name: "hpc",
    kind: "pc",
    addresses: ["100.101.1.1"],
    hostname: "hpc.tail.ts.net",
    sources: ["tailscale"],
    tailnet: { online: true, os: "windows", owner: "demo@", isSelf: false, dnsName: "hpc.tail.ts.net", lastSeen: null },
    services: [{ scheme: "peer", port: 47470, uri: "peer://hpc/", label: "Cross Explore", source: "probe" }],
    shares: [],
    lastSeen: now,
  },
  {
    id: "alpha",
    name: "alpha",
    kind: "linux",
    addresses: ["100.101.1.2"],
    hostname: "alpha.tail.ts.net",
    sources: ["tailscale"],
    tailnet: { online: true, os: "linux", owner: "demo@", isSelf: false, dnsName: "alpha.tail.ts.net", lastSeen: null },
    services: [{ scheme: "sftp", port: 22, uri: "sftp://alpha.tail.ts.net/", label: "SFTP", source: "probe" }],
    shares: [],
    lastSeen: now,
  },
];

let peer: PeerStatus = { enabled: false, deviceId: "k7d2-mq4x", name: "demo's MacBook Pro", port: 47470, shares: [{ name: "Downloads", path: HOME + "/Downloads", readOnly: false }], trusted: [], tailnetAutoTrust: true };
const terms = new Map<number, (t: string) => void>();
const tags = new Map<string, string[]>([["file://" + HOME + "/Documents/Budget%202026.xlsx", ["Red", "Work"]]]);

// ---------- handlers ----------

type Args = any;
const mockCloud = { gdrive: false, dropbox: false, onedrive: false };
const handlers: Record<string, (a: Args) => unknown> = {
  subscribe({ onEvent }: Args) {
    emit = onEvent;
    setTimeout(() => emit({ type: "devices", devices: fakeDevices }), 800);
  },
  places: (): Places => ({
    platform: /Windows/.test(navigator.userAgent) ? "windows" : "macos",
    translucent: false,
    home: { name: "demo", uri: "file://" + HOME, icon: "home" },
    favorites: ["Desktop", "Documents", "Downloads", "Pictures", "Music", "Movies"].map((n, i) => ({
      name: n,
      uri: `file://${HOME}/${n}`,
      icon: ["desktop", "documents", "downloads", "pictures", "music", "videos"][i],
    })),
    volumes: [
      { name: "Macintosh HD", uri: "file:///", total: 994e9, free: 212e9, removable: false },
      { name: "Data", uri: "file:///Volumes/Data", total: 2e12, free: 1.31e12, removable: true },
    ],
    cloud: [
      { name: "Google Drive", uri: `file://${HOME}/Documents`, provider: "google", account: "demo@example.com" },
      { name: "iCloud Drive", uri: `file://${HOME}/Desktop`, provider: "icloud", account: null },
    ],
  }),
  free_space: () => ({ free: 212e9, total: 994e9 }),
  async list_dir({ uri, onEvent }: { uri: string; onEvent: (e: ListEvent) => void }) {
    // Tests can make every listing as slow as a far-away server (meta included).
    const slow = (globalThis as { __cxSlowList?: number }).__cxSlowList;
    if (slow) await sleep(slow);
    const node = lookup(uri);
    if (!node?.isDir) throw { kind: "notFound", message: parse(uri).path };
    const remote = parse(uri).base !== "file://";
    onEvent({ type: "meta", info: info(uri) as any, capabilities: { liveWatch: !remote, polling: remote, serverCopy: true, trash: !remote, posix: true, writable: true } });
    await sleep(remote ? 120 : 5);
    // Stream like the real backend: a small first batch, then big ones.
    let batch: Entry[] = [];
    let total = 0;
    let limit = 128;
    for (const child of node.children!.values()) {
      batch.push(strip(child));
      if (batch.length >= limit) {
        total += batch.length;
        onEvent({ type: "batch", entries: batch });
        await sleep(0); // IPC delivers each batch as its own task
        batch = [];
        limit = 4096;
      }
    }
    total += batch.length;
    if (batch.length) onEvent({ type: "batch", entries: batch });
    onEvent({ type: "done", total, elapsedMs: 5 });
  },
  watch_dir({ uri, onChange }: { uri: string; onChange: (c: Change[]) => void }) {
    const node = lookup(uri);
    if (!node?.children) throw { kind: "notFound", message: uri };
    const id = nextId++;
    const w: { node: Node; fn: (c: Change[]) => void; timer?: ReturnType<typeof setInterval> } = { node, fn: onChange };
    if (parse(uri).path.endsWith("/Downloads")) {
      // Simulate someone else working in this folder.
      let n = 1;
      w.timer = setInterval(() => {
        const name = `Shared note ${n++}.md`;
        const f = file(name, 800 + n * 10, 0, false, `# Note ${n}\n`);
        node.children!.set(name, f);
        onChange([{ type: "upsert", entry: strip(f) }]);
        if (n > 3) {
          const old = `Shared note ${n - 3}.md`;
          node.children!.delete(old);
          onChange([{ type: "remove", name: old }]);
        }
      }, 6000);
    }
    watchers.set(id, w);
    return { id, mode: parse(uri).base === "file://" ? "live" : "polling" };
  },
  unwatch_dir({ id }: Args) {
    clearInterval(watchers.get(id)?.timer);
    watchers.delete(id);
  },
  stat_entry({ uri }: Args) {
    const n = lookup(uri);
    if (!n) throw { kind: "notFound", message: uri };
    return strip(n);
  },
  create_folder({ uri, name }: Args) {
    const node = lookup(uri)!;
    const pick = name ?? uniqueName(node, "New folder").replace(/ \((\d+)\)$/, " ($1)");
    const d = dir(pick, 0);
    node.children!.set(pick, d);
    notify(node, [{ type: "upsert", entry: strip(d) }]);
    return strip(d);
  },
  create_file({ uri, stem, ext }: Args) {
    const node = lookup(uri)!;
    const wanted = ext ? `${stem}.${ext}` : stem;
    const pick = uniqueName(node, wanted);
    const f = file(pick, 0, 0, false, "");
    node.children!.set(pick, f);
    notify(node, [{ type: "upsert", entry: strip(f) }]);
    return strip(f);
  },
  rename_entry({ uri, from, to }: Args) {
    const node = lookup(uri)!;
    if (node.children!.has(to)) throw { kind: "alreadyExists", message: to };
    const e = node.children!.get(from)!;
    node.children!.delete(from);
    e.name = to;
    node.children!.set(to, e);
    return strip(e);
  },
  trash_entries({ uri, names }: Args) {
    const node = lookup(uri)!;
    names.forEach((n: string) => node.children!.delete(n));
    return names.map((n: string) => ({ original: uri + "/" + encodeURIComponent(n), trashed: null }));
  },
  open_entry: (a: Args) => mockDownload(a),
  apps_for_extension({ ext }: Args) {
    const pretty = (e: string) => e.charAt(0).toUpperCase() + e.slice(1);
    return [
      { name: `${pretty(ext)} Opener`, id: `${ext}-opener` },
      { name: "TextEdit", id: "TextEdit" },
      { name: "Preview", id: "Preview" },
    ];
  },
  open_entry_with: (a: Args) => mockDownload(a),
  cancel_open({ id }: Args) {
    openCancels.add(id);
  },
  open_with_dialog: () => undefined,
  os_clipboard_set: () => undefined,
  os_clipboard_get: () => [],
  full_disk_access: () => true,
  open_full_disk_access_settings: () => undefined,
  reveal_entry: () => undefined,
  open_terminal: () => undefined,
  async dir_size({ uri, onProgress }: Args) {
    const n = lookup(uri)!;
    const total = totalSize(n);
    for (let i = 1; i <= 5; i++) {
      await sleep(60);
      onProgress({ bytes: (total * i) / 5, files: i * 10, dirs: i, done: i === 5 } satisfies SizeProgress);
    }
    return total;
  },
  preview_text({ uri }: Args) {
    const n = lookup(uri);
    if (!n || n.isDir) throw { kind: "notFound", message: uri };
    if (n.content == null) throw { kind: "unsupported", message: "binary file" };
    const ext = n.name.split(".").pop() ?? "";
    return { text: n.content, truncated: false, encoding: "utf-8", languageGuess: ext };
  },
  transfer_submit({ req }: { req: JobRequest }) {
    const id = nextId++;
    jobs.set(id, {
      id,
      kind: req.kind,
      state: "queued",
      sources: req.sources,
      dest: req.dest ?? null,
      bytesDone: 0,
      bytesTotal: 0,
      filesDone: 0,
      filesTotal: 0,
      current: null,
      speed: 0,
      eta: null,
      errors: [],
      conflict: null,
      undo: null,
      startedAt: Date.now(),
    });
    void runJob(id, req);
    return id;
  },
  transfer_pause({ id }: Args) {
    const j = jobs.get(id)!;
    j.paused = true;
    j.state = "paused";
    emit({ type: "job", job: { ...j, resolve: undefined } as JobSnapshot });
  },
  transfer_resume({ id }: Args) {
    const j = jobs.get(id)!;
    j.paused = false;
    j.state = "running";
  },
  transfer_cancel({ id }: Args) {
    const j = jobs.get(id)!;
    j.cancelled = true;
    j.resolve?.("skip");
  },
  transfer_resolve({ id, resolution, applyToAll }: Args) {
    jobs.get(id)?.resolve?.(resolution + (applyToAll ? "!" : ""));
  },
  transfer_list: () => [...jobs.values()].map((j) => ({ ...j, resolve: undefined })),
  transfer_clear: () => undefined,
  undo({ op }: Args) {
    if (op.type === "copy") for (const u of op.created as string[]) {
      const parentUri = u.slice(0, u.lastIndexOf("/"));
      const name = decodeURIComponent(u.slice(u.lastIndexOf("/") + 1));
      const p = lookup(parentUri)!;
      p.children!.delete(name);
      notify(p, [{ type: "remove", name }]);
    }
    if (op.type === "rename") {
      const p = lookup(op.dir as string)!;
      const e = p.children!.get(op.to as string)!;
      p.children!.delete(op.to as string);
      e.name = op.from as string;
      p.children!.set(e.name, e);
      notify(p, [{ type: "remove", name: op.to as string }, { type: "upsert", entry: strip(e) }]);
    }
  },
  async search_start({ root, query, onEvent }: { root: string; query: SearchQuery; onEvent: (e: SearchEvent) => void }) {
    const id = nextId++;
    const q = query.text.toLowerCase();
    const hits: SearchHit[] = [];
    const walk = (n: Node, uri: string, rel: string) => {
      for (const c of n.children?.values() ?? []) {
        const cu = uri.replace(/\/$/, "") + "/" + encodeURIComponent(c.name);
        const r = rel ? rel + "/" + c.name : c.name;
        if (c.name.toLowerCase().includes(q) && (query.includeHidden || !c.hidden)) hits.push({ uri: cu, parent: uri, relPath: r, entry: strip(c) });
        if (c.isDir) walk(c, cu, r);
      }
    };
    setTimeout(() => {
      walk(lookup(root)!, root, "");
      onEvent({ type: "hits", hits });
      onEvent({ type: "done", scanned: 500, elapsedMs: 12, truncated: false });
    }, 50);
    return id;
  },
  connect_server({ uri, credentials }: Args) {
    if (uri.startsWith("smb://nas")) {
      if (credentials?.secret?.password !== "demo") throw { kind: "authRequired", message: { uri, user: credentials?.user ?? null, reason: credentials ? "Wrong user name or password" : "" } };
      nasSignedIn = true;
      return;
    }
    throw { kind: "connection", message: `${uri} did not respond` };
  },
  disconnect_server() {
    nasSignedIn = false;
  },
  trust_host_key: () => undefined,
  cloud_services: () => [
    { service: "gdrive", label: "Google Drive", configured: mockCloud.gdrive, clientId: mockCloud.gdrive ? "demo.apps.googleusercontent.com" : null, hasSecret: true, env: ["CX_GDRIVE_CLIENT_ID", "CX_GDRIVE_CLIENT_SECRET"] },
    { service: "dropbox", label: "Dropbox", configured: mockCloud.dropbox, clientId: null, hasSecret: false, env: ["CX_DROPBOX_APP_KEY", "CX_DROPBOX_APP_SECRET"] },
    { service: "onedrive", label: "OneDrive", configured: mockCloud.onedrive, clientId: null, hasSecret: false, env: ["CX_ONEDRIVE_CLIENT_ID", null] },
  ],
  cloud_set_client: ({ service, clientId }: { service: keyof typeof mockCloud; clientId: string }) => void (mockCloud[service] = !!clientId),
  async cloud_sign_in({ service }: { service: string }) {
    await sleep(300);
    return `${service}://demo@example.com/`;
  },
  open_web_page: () => undefined,
  async preview_office({ uri }: { uri: string }) {
    await sleep(80);
    const name = decodeURIComponent(uri.split("/").pop() ?? "");
    return { kind: "html", title: name, pages: 1, html: `<!doctype html><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'"><body style="font:14px system-ui;padding:24px"><h1>${name.replace(/[<&]/g, "")}</h1><p>Office preview (demo)</p><table border=1><tr><td>A1</td><td>B1</td></tr></table></body>` };
  },
  connections: () => (nasSignedIn ? ["smb://nas.local"] : []),
  discovery_devices: () => fakeDevices,
  discovery_refresh: () => undefined,
  peer_status: () => peer,
  peer_set_enabled({ enabled }: Args) {
    peer = { ...peer, enabled };
    return peer;
  },
  peer_set_shares({ shares }: Args) {
    peer = { ...peer, shares };
    return peer;
  },
  peer_set_auto_trust({ on }: Args) {
    peer = { ...peer, tailnetAutoTrust: on };
    return peer;
  },
  peer_pair_code: () => "482 193",
  peer_pair({ address }: Args) {
    const d = { id: "x1", name: address };
    peer = { ...peer, trusted: [...peer.trusted, { ...d, addedAt: Date.now() }] };
    return d;
  },
  peer_forget({ id }: Args) {
    peer = { ...peer, trusted: peer.trusted.filter((t) => t.id !== id) };
    return peer;
  },
  peer_send: () => String(nextId++),
  peer_respond: () => undefined,
  tags_get({ uris }: Args) {
    return Object.fromEntries((uris as string[]).map((u) => [u, tags.get(u) ?? []]));
  },
  tags_set({ uri, tags: t }: Args) {
    tags.set(uri, t);
  },
  tags_find({ tag }: Args) {
    return [...tags].filter(([, t]) => t.includes(tag)).map(([uri]) => ({ uri, parent: uri.slice(0, uri.lastIndexOf("/")), relPath: decodeURIComponent(uri.slice(uri.lastIndexOf("/") + 1)), entry: strip(lookup(uri)!) }));
  },
  compare_dirs({ left, right }: Args) {
    const l = lookup(left)!;
    const r = lookup(right)!;
    const names = new Set([...l.children!.keys(), ...r.children!.keys()]);
    return [...names].sort().map((n): DiffItem => {
      const a = l.children!.get(n);
      const b = r.children!.get(n);
      const kind = !b ? "leftOnly" : !a ? "rightOnly" : a.size !== b.size ? "different" : (a.modified ?? 0) > (b.modified ?? 0) + 2000 ? "newerLeft" : (b.modified ?? 0) > (a.modified ?? 0) + 2000 ? "newerRight" : "same";
      return { relPath: n, kind, left: a ? strip(a) : null, right: b ? strip(b) : null };
    });
  },
  cancel_task: () => undefined,
  // A toy shell that echoes, for the browser preview.
  term_open({ uri, onEvent }: Args) {
    const id = nextId++;
    const say = (t: string) => onEvent({ kind: "output", data: btoa(unescape(encodeURIComponent(t))) });
    terms.set(id, say);
    say(`\x1b[1;32mdemo@preview\x1b[0m:${parse(uri).path}$ `);
    return id;
  },
  term_write({ id, data }: Args) {
    const say = terms.get(id);
    if (!say) return;
    say(data === "\r" ? `\r\n(preview: no real shell)\r\n$ ` : data);
  },
  term_resize: () => undefined,
  term_close({ id }: Args) {
    terms.delete(id);
  },
  term_cwd: () => null,
  ssh_saved_user: () => null,
  ssh_copy_id: () => "Copied the public key (preview).",
  ui_log: () => undefined,
};

export async function invoke<T>(cmd: string, args: Record<string, unknown>): Promise<T> {
  const h = handlers[cmd];
  if (!h) throw { kind: "unsupported", message: cmd };
  return (await h(args)) as T;
}
