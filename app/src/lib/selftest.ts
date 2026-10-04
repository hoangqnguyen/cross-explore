// End-to-end self test against the real backend. Runs only when the app was
// started with CX_SELFTEST=1: drives the UI state layer through a realistic
// session and reports each check to the terminal, then exits.
import { invoke } from "@tauri-apps/api/core";
import { termClose, termOpen, termWrite, appsForExtension, setMenuKeys, asCxError, connectServer, trustHostKey, childUri, createFile, dirSize, renameEntry, fileUrl, getTags, previewOffice, previewText, search, setTags, thumbUrl, compareDirs, peerStatus, devices as listDevices, listDir, type Entry } from "./api";
import { keyOf } from "./folder.svelte";
import { transfers } from "./stores/transfers.svelte";
import { quicklook } from "./stores/quicklook.svelte";
import { settings } from "./stores/settings.svelte";
import { ws } from "./workspace.svelte";
import { dialogs } from "./stores/dialogs.svelte";

const log = (message: string, level = "test") => invoke("ui_log", { level, message });
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function until<T>(what: string, f: () => T | Promise<T>, ms = 5000): Promise<T> {
  const end = Date.now() + ms;
  let last: T | undefined;
  while (Date.now() < end) {
    last = await f();
    if (last) return last;
    await sleep(50);
  }
  throw new Error(`timed out waiting for ${what}`);
}

export async function selftest() {
  const cfg = await invoke<{ uri: string; remote: boolean } | null>("selftest_config");
  if (!cfg) return;
  const errors: string[] = [];
  window.addEventListener("error", (e) => errors.push(String(e.error?.stack ?? e.message).slice(0, 400)));
  window.addEventListener("unhandledrejection", (e) => errors.push(String((e.reason as Error)?.stack ?? e.reason).slice(0, 400)));
  let failed = 0;
  let passed = 0;
  const check = async (name: string, f: () => Promise<unknown>) => {
    const t0 = performance.now();
    try {
      await f();
      passed++;
      await log(`✓ ${name} (${(performance.now() - t0).toFixed(0)} ms)`);
    } catch (e) {
      failed++;
      await log(`✗ ${name}: ${e instanceof Error ? e.message : JSON.stringify(e)}`);
    }
  };
  const tab = () => ws.activeTab;
  const names = () => tab().folder.items.map((e) => e.name);
  const has = (n: string) => names().includes(n);
  const job = async (id: number | null) => (await until(`job ${id}`, () => transfers.jobs.find((j) => j.id === id && ["done", "failed", "cancelled"].includes(j.state)), 15000))!;
  const dir = cfg.uri;

  await check("lists the fixture folder", async () => {
    tab().navigate(dir);
    await until("listing", () => tab().folder.status === "ready" && has("alpha.txt") && has("sub"));
  });

  await check("live: a file created outside appears without refresh", async () => {
    await until("watch", () => tab().folder.live === "live");
    const t0 = performance.now();
    await invoke("selftest_touch", { name: "external.txt" });
    await until("external.txt", () => has("external.txt") && tab().folder.fresh.has("external.txt"), 3000);
    await log(`  change → row in ${(performance.now() - t0).toFixed(0)} ms`, "test");
  });

  await check("new folder + rename + undo", async () => {
    await tab().newFolder();
    await until("New folder", () => has("New folder"));
    await tab().rename("New folder", "Made");
    await until("Made", () => has("Made") && !has("New folder"));
    await transfers.undo();
    await until("undo rename", () => has("New folder") && !has("Made"));
  });

  await check("new file picks a free name next to the extension", async () => {
    await tab().newFile("Untitled", "txt");
    await until("Untitled.txt", () => has("Untitled.txt"));
    await tab().newFile("Untitled", "txt");
    await until("Untitled (2).txt", () => has("Untitled (2).txt"));
  });

  await check("create_file rejects a path-traversal name", async () => {
    try {
      await createFile(dir, "../evil", "txt");
      throw new Error("should have rejected the name");
    } catch (e) {
      if (asCxError(e)?.kind !== "invalidName") throw new Error(`wrong error: ${JSON.stringify(e)}`);
    }
  });

  await check("apps_for_extension finds real installed apps", async () => {
    const apps = await appsForExtension("txt");
    if (!apps.length) throw new Error("no apps found for .txt");
    if (!apps.every((a) => a.name && a.id)) throw new Error(JSON.stringify(apps));
  });

  await check("custom shortcuts reach the menu bar and change what keys run", async () => {
    const { menuAccelerators, setKeys, resetKeys, commandsOn } = await import("./commands.svelte");
    const before = JSON.stringify(settings.data.keyBindings);
    try {
      setKeys("view.hidden", ["Mod+K"]);
      const keys = menuAccelerators();
      if (keys["net.connect"] !== null) throw new Error(`menu still gives ⌘K to Connect: ${keys["net.connect"]}`);
      if (commandsOn("Mod+K").map((c) => c.id).join() !== "view.hidden") throw new Error("⌘K doesn't run only Toggle hidden items");
      await setMenuKeys(keys);
      setKeys("net.connect", ["Mod+Shift+K", "F4"]);
      if (menuAccelerators()["net.connect"] !== "Cmd+Shift+K") throw new Error(JSON.stringify(menuAccelerators()));
      await setMenuKeys({ "net.connect": "Num+" }); // unparsable: no shortcut, no error
    } finally {
      resetKeys();
      settings.data.keyBindings = JSON.parse(before);
      await setMenuKeys(menuAccelerators());
    }
  });

  await check("copy into a subfolder (transfer engine)", async () => {
    const id = await transfers.submit({ kind: "copy", sources: [childUri(dir, "alpha.txt")], dest: childUri(dir, "sub") });
    const j = await job(id);
    if (j.state !== "done") throw new Error(`job ${j.state}: ${j.errors[0]?.message}`);
    const list: Entry[] = [];
    await listDir(childUri(dir, "sub"), (e) => e.type === "batch" && list.push(...e.entries));
    if (!list.some((e) => e.name === "alpha.txt")) throw new Error("copy missing");
  });

  await check("duplicate names the copy like Explorer", async () => {
    tab().selectOnly("alpha.txt");
    await ws.duplicate();
    await until("alpha - Copy.txt", () => has("alpha - Copy.txt"), 8000);
  });

  await check("move to trash and undo restores it", async () => {
    tab().selectOnly("alpha - Copy.txt");
    await tab().trashSelection();
    await until("gone", () => !has("alpha - Copy.txt"));
    await transfers.undo();
    await until("restored", () => has("alpha - Copy.txt"), 5000);
  });

  await check("text preview", async () => {
    const t = await previewText(childUri(dir, "alpha.txt"));
    if (!t.text.includes("hello")) throw new Error(t.text);
  });

  await check("cxfile:// serves bytes and ranges", async () => {
    const full = await fetch(fileUrl(childUri(dir, "alpha.txt")));
    if (!(await full.text()).startsWith("hello")) throw new Error("bad body");
    const part = await fetch(fileUrl(childUri(dir, "alpha.txt")), { headers: { Range: "bytes=6-10" } });
    const body = await part.text();
    if (part.status !== 206 || body !== "cross") throw new Error(`${part.status} ${body}`);
  });

  await check("cxthumb:// renders an image thumbnail", async () => {
    const r = await fetch(thumbUrl(childUri(dir, "photo.png"), 64, 1));
    if (!r.ok || !(r.headers.get("content-type") ?? "").startsWith("image/")) throw new Error(`${r.status} ${r.headers.get("content-type")}`);
  });

  await check("Office preview: CSV renders as a table in a sandboxed frame", async () => {
    const view = await previewOffice(childUri(dir, "sheet.csv"));
    if (view.kind !== "html" || !view.html.includes("Apples") || !view.html.includes("<table")) throw new Error(JSON.stringify(view).slice(0, 200));
    let blocked = "";
    const onViolation = (e: SecurityPolicyViolationEvent) => (blocked = `${e.violatedDirective} ${e.blockedURI}`);
    document.addEventListener("securitypolicyviolation", onViolation);
    try {
      tab().navigate(dir);
      await until("listing", () => tab().folder.status === "ready" && has("sheet.csv"));
      const entry = tab().folder.items.find((e) => e.name === "sheet.csv")!;
      tab().selectOnly(keyOf(entry));
      quicklook.open = true;
      const frame = await until("office frame", () => document.querySelector<HTMLIFrameElement>(".ql iframe.office"), 5000);
      await new Promise((r) => setTimeout(r, 400));
      if (!frame!.srcdoc.includes("Pears")) throw new Error("frame has no table");
      if (frame!.getAttribute("sandbox") !== "") throw new Error("frame not sandboxed");
      if (blocked) throw new Error(`CSP blocked the preview: ${blocked}`);
    } finally {
      quicklook.close();
      document.removeEventListener("securitypolicyviolation", onViolation);
    }
  });

  await check("PDF Quick Look draws pages with pdf.js and zooms", async () => {
    let blocked = "";
    const onViolation = (e: SecurityPolicyViolationEvent) => (blocked = `${e.violatedDirective} ${e.blockedURI}`);
    document.addEventListener("securitypolicyviolation", onViolation);
    try {
      await until("listing", () => tab().folder.status === "ready" && has("doc.pdf"));
      tab().selectOnly(keyOf(tab().folder.items.find((e) => e.name === "doc.pdf")!));
      quicklook.open = true;
      const canvas = await until("pdf page", () => document.querySelector<HTMLCanvasElement>(".ql .pdf canvas[data-page='1']")?.width ? document.querySelector<HTMLCanvasElement>(".ql .pdf canvas")! : null, 8000);
      // Something other than white was drawn (the text).
      const ctx = canvas!.getContext("2d")!;
      const px = ctx.getImageData(0, 0, canvas!.width, Math.floor(canvas!.height / 4)).data;
      let ink = 0;
      for (let i = 0; i < px.length; i += 4) if (px[i] < 128) ink++;
      if (!ink) throw new Error("page is blank");
      const w0 = parseFloat(canvas!.style.width);
      const box = document.querySelector(".ql .pdf")!.getBoundingClientRect();
      document.querySelector(".ql .pdf")!.dispatchEvent(new WheelEvent("wheel", { deltaY: -120, ctrlKey: true, clientX: box.left + 50, clientY: box.top + 50, bubbles: true, cancelable: true }));
      await until("zoomed", () => parseFloat(canvas!.style.width) > w0 * 1.5);
      if (blocked) throw new Error(`CSP blocked: ${blocked}`);
    } finally {
      quicklook.close();
      document.removeEventListener("securitypolicyviolation", onViolation);
    }
  });

  await check("closing Quick Look actually stops a playing video/audio", async () => {
    // Removing the element should already stop it per spec, but a custom
    // URI scheme handler (cxfile://) backing the media is exactly the kind
    // of thing that can make a webview's implicit stop-on-remove unreliable,
    // so Preview.svelte pauses explicitly on destroy. Exercise it for real.
    await until("listing", () => tab().folder.status === "ready" && has("tone.wav"));
    tab().selectOnly(keyOf(tab().folder.items.find((e) => e.name === "tone.wav")!));
    quicklook.open = true;
    const audio = await until("audio element", () => document.querySelector<HTMLAudioElement>(".ql audio"));
    await audio!.play().catch(() => {});
    await until("it's actually playing", () => !audio!.paused);
    quicklook.close();
    await until("ql closed", () => !document.querySelector(".ql"));
    if (!audio!.paused) throw new Error("audio kept playing after Quick Look closed");
  });

  await check("the (small) preview pane also draws a remote PDF, not a broken thumbnail", async () => {
    // A remote PDF has no fast native thumbnail (unlike images), so the
    // preview pane used to fall back to a thumbnail image that always
    // failed to load for non-local files, leaving the pane blank. A file
    // inside a zip is read like one on a server (not through local_path()).
    const zip = childUri(dir, "doc.zip");
    const c = await job(await transfers.submit({ kind: "compress", sources: [childUri(dir, "doc.pdf")], dest: zip }));
    if (c.state !== "done") throw new Error(`compress ${c.state}: ${c.errors[0]?.message}`);
    const wasOpen = settings.data.previewPane;
    settings.data.previewPane = true;
    try {
      tab().navigate(`archive://${zip}!/`);
      await until("archive listing", () => tab().folder.status === "ready" && has("doc.pdf"));
      tab().selectOnly(keyOf(tab().folder.items.find((e) => e.name === "doc.pdf")!));
      const canvas = await until("preview-pane pdf page", () => document.querySelector<HTMLCanvasElement>(".pane-preview .preview canvas[data-page='1']")?.width ? document.querySelector<HTMLCanvasElement>(".pane-preview .preview canvas")! : null, 8000);
      const ctx = canvas!.getContext("2d")!;
      const px = ctx.getImageData(0, 0, canvas!.width, Math.floor(canvas!.height / 4)).data;
      let ink = 0;
      for (let i = 0; i < px.length; i += 4) if (px[i] < 128) ink++;
      if (!ink) throw new Error("preview pane shows a blank page");
    } finally {
      settings.data.previewPane = wasOpen;
    }
  });

  await check("Copy to… folder browser lists subfolders and drives", async () => {
    tab().navigate(dir);
    await until("listing", () => tab().folder.status === "ready" && has("sub"));
    void dialogs.ask("destination", { uris: [childUri(dir, "alpha.txt")], mode: "copy" });
    try {
      const browse = await until("browse button", () => [...document.querySelectorAll<HTMLButtonElement>(".modal button")].find((b) => b.textContent?.includes("Choose another folder")));
      browse!.click();
      await until("subfolder 'sub' listed", () => [...document.querySelectorAll(".modal .blist .dir")].some((d) => d.textContent?.trim() === "sub"), 5000);
      // A big folder (large channel messages) lists completely too.
      const input = document.querySelector<HTMLInputElement>('.modal input[aria-label="Path"]')!;
      input.value = childUri(childUri(dir, "sub"), "wide");
      input.dispatchEvent(new Event("input"));
      input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
      await until("400 folders listed", () => document.querySelectorAll(".modal .blist .dir").length === 400, 5000);
      if ([...document.querySelectorAll(".modal .blist p")].some((p) => p.textContent?.includes("No subfolders"))) throw new Error("claimed no subfolders");
      document.querySelector<HTMLButtonElement>('.modal [aria-label="Drives"]')!.click();
      await until("drives listed", () => document.querySelectorAll(".modal .blist .dir").length >= 2);
    } finally {
      dialogs.close(null);
    }
  });

  await check("requests cancelled mid-flight don't crash the app (thumbnails, file reads)", async () => {
    // Images added and removed at once (fast scrolling), and aborted fetches:
    // WebKit stops these tasks while the backend is still producing them.
    const box = document.createElement("div");
    box.style.cssText = "position:fixed;left:-9999px;top:0;width:10px;height:10px;overflow:hidden";
    document.body.appendChild(box);
    for (let round = 0; round < 40; round++) {
      const imgs = Array.from({ length: 25 }, (_, i) => {
        const img = new Image();
        img.src = thumbUrl(childUri(dir, i % 2 ? "photo.png" : "doc.pdf"), 40 + round * 25 + i, round * 1000 + i);
        box.appendChild(img);
        return img;
      });
      await new Promise((r) => setTimeout(r, round % 3 === 0 ? 0 : 5));
      imgs.forEach((img) => ((img.src = ""), img.remove()));
      const ctl = new AbortController();
      const reads = Array.from({ length: 10 }, () => fetch(fileUrl(childUri(dir, "doc.pdf")), { signal: ctl.signal }).catch(() => null));
      ctl.abort();
      await Promise.all(reads);
    }
    box.remove();
    // Still alive and answering.
    const r = await fetch(fileUrl(childUri(dir, "alpha.txt")));
    if ((await r.text()) !== (await (await fetch(fileUrl(childUri(dir, "alpha.txt")))).text())) throw new Error("file reads broken after the storm");
  });

  await check("recursive name and content search", async () => {
    const byName: string[] = [];
    await search(dir, { text: "nested" }, (e) => e.type === "hits" && byName.push(...e.hits.map((h) => h.entry.name)));
    await until("name hits", () => byName.includes("nested.txt"));
    const byContent: string[] = [];
    await search(dir, { text: "", content: "line two" }, (e) => e.type === "hits" && byContent.push(...e.hits.map((h) => h.entry.name)));
    await until("content hits", () => byContent.includes("alpha.txt"));
  });

  await check("compress, browse the zip, extract", async () => {
    const zip = childUri(dir, "bundle.zip");
    const c = await job(await transfers.submit({ kind: "compress", sources: [childUri(dir, "alpha.txt"), childUri(dir, "sub")], dest: zip }));
    if (c.state !== "done") throw new Error(`compress ${c.state}: ${c.errors[0]?.message}`);
    const inside: string[] = [];
    await listDir(`archive://${zip}!/`, (e) => e.type === "batch" && inside.push(...e.entries.map((x) => x.name)));
    if (!inside.includes("alpha.txt") || !inside.includes("sub")) throw new Error(inside.join(","));
    const x = await job(await transfers.submit({ kind: "extract", sources: [zip], dest: dir }));
    if (x.state !== "done") throw new Error(`extract ${x.state}: ${x.errors[0]?.message}`);
    await until("bundle folder", () => has("bundle"));

    // Dragging a non-local file into another app hands over a real local
    // copy (a file inside a zip stands in for a server file here).
    const member = `archive://${zip}!/alpha.txt`;
    const [a, b] = await Promise.all([invoke<string[]>("stage_for_drag", { uris: [member] }), invoke<string[]>("stage_for_drag", { uris: [member] })]);
    if (a[0] !== b[0] || !a[0].endsWith("alpha.txt") || a[0].startsWith("archive:")) throw new Error(`staged ${a} / ${b}`);
    const staged = await (await fetch(fileUrl("file://" + (a[0].startsWith("/") ? "" : "/") + a[0].replaceAll("\\", "/")))).text();
    if (staged !== "hello cross explore\nline two\n") throw new Error(`staged copy has ${JSON.stringify(staged)}`);

    // The file changes on the "server": the next drag carries the new version.
    await invoke("selftest_touch", { name: "alpha.txt" });
    const del = await job(await transfers.submit({ kind: "delete", sources: [zip] }));
    if (del.state !== "done") throw new Error(`delete zip ${del.state}`);
    const c2 = await job(await transfers.submit({ kind: "compress", sources: [childUri(dir, "alpha.txt")], dest: zip }));
    if (c2.state !== "done") throw new Error(`recompress ${c2.state}: ${c2.errors[0]?.message}`);
    const [again] = await invoke<string[]>("stage_for_drag", { uris: [member] });
    const fresh = await (await fetch(fileUrl("file://" + (again.startsWith("/") ? "" : "/") + again.replaceAll("\\", "/")))).text();
    if (fresh !== "made outside the app\n") throw new Error(`after the change, the drag still carries ${JSON.stringify(fresh)}`);
  });

  await check("media from a server streams in ranges (shared chunk cache, read-ahead)", async () => {
    // A file inside a zip is read like one on a server: through the chunk cache.
    const mediaZip = childUri(dir, "media.zip");
    const c = await job(await transfers.submit({ kind: "compress", sources: [childUri(childUri(dir, "media"), "clip.bin")], dest: mediaZip }));
    if (c.state !== "done") throw new Error(`compress ${c.state}: ${c.errors[0]?.message}`);
    const remote = fileUrl(`archive://${mediaZip}!/clip.bin`);
    const local = new Uint8Array(await (await fetch(fileUrl(childUri(childUri(dir, "media"), "clip.bin")))).arrayBuffer());
    if (local.length !== 3_600_000) throw new Error(`local copy is ${local.length} bytes`);
    const ranges: [number, number][] = [[0, 1], [1048566, 1048585], [3_599_950, 3_599_999], [2_000_000, 2_600_000], [500, 2_500_000]];
    for (const [a, b] of ranges) {
      const r = await fetch(remote, { headers: { Range: `bytes=${a}-${b}` } });
      const got = new Uint8Array(await r.arrayBuffer());
      const cr = r.headers.get("content-range") ?? "";
      const m = /bytes (\d+)-(\d+)\/(\d+)/.exec(cr);
      if (r.status !== 206 || !m || +m[1] !== a || +m[3] !== 3_600_000) throw new Error(`range ${a}-${b}: ${r.status} ${cr}`);
      if (+m[2] - a + 1 !== got.length || got.length > 2 * 1048576) throw new Error(`range ${a}-${b}: ${got.length} bytes for ${cr}`);
      for (let i = 0; i < got.length; i++) if (got[i] !== local[a + i]) throw new Error(`range ${a}-${b}: byte ${a + i} differs`);
    }
    // A request with no Range header at all — how pdf.js's opening probe
    // fetches a document — must report the file's true size honestly. A
    // truncated 206 here (Content-Length = only what we felt like sending)
    // made pdf.js believe remote PDFs ended a couple of MB in, so it never
    // reached the real end of the file (where the xref table lives) and
    // rendered nothing.
    const whole = await fetch(remote);
    const wholeBytes = new Uint8Array(await whole.arrayBuffer());
    if (whole.status !== 200) throw new Error(`unranged request: expected 200, got ${whole.status}`);
    if (whole.headers.get("content-length") !== "3600000") throw new Error(`unranged request: Content-Length is ${whole.headers.get("content-length")}, not the true size`);
    if (wholeBytes.length !== 3_600_000) throw new Error(`unranged request: got ${wholeBytes.length} bytes`);
    for (let i = 0; i < wholeBytes.length; i += 99_991) if (wholeBytes[i] !== local[i]) throw new Error(`unranged request: byte ${i} differs`);
  });

  await check("tags round trip", async () => {
    const u = childUri(dir, "beta.md");
    await setTags(u, ["Red", "Work"]);
    const t = await getTags([u]);
    if (!t[u]?.includes("Red") || !t[u]?.includes("Work")) throw new Error(JSON.stringify(t));
  });

  await check("compare folders", async () => {
    const d = await compareDirs(childUri(dir, "sub"), childUri(childUri(dir, "bundle"), "sub"), false);
    if (!d.length || !d.every((i) => i.kind === "same")) throw new Error(JSON.stringify(d.map((i) => `${i.relPath}:${i.kind}`)));
  });

  await check("folder size", async () => {
    const n = await dirSize(dir, () => {});
    if (n <= 0) throw new Error(String(n));
  });

  await check("list outline: expand a folder in place, live", async () => {
    tab().view = "details";
    const sub = tab().visible.find((e) => e.name === "sub")!;
    tab().toggleExpand(sub);
    await until("nested rows", () => tab().visible.some((e) => e.name === "nested.txt" && e.depth === 1));
    const nested = tab().visible.find((e) => e.name === "nested.txt")!;
    if (tab().uriOf(nested) !== childUri(childUri(dir, "sub"), "nested.txt")) throw new Error(tab().uriOf(nested));
    tab().collapseAll();
    if (tab().visible.some((e) => e.depth)) throw new Error("collapse left rows");
  });

  await check("views render with real data", async () => {
    for (const v of ["icons", "columns", "gallery", "details"] as const) {
      tab().view = v;
      const cls = v === "details" ? ".details" : `.${v}`;
      await until(`${v} view`, () => document.querySelector(`.pane.active ${cls}`), 3000).catch((e) => {
        const pane = document.querySelector(".pane.active");
        throw new Error(`${e.message} (tab: ${tab().view} ${tab().folder.kind} ${tab().folder.status} ${tab().dirUri}; dialogs: ${dialogs.stack.map((d) => d.kind).join(",") || "none"}; dupes: ${JSON.stringify(Object.entries(tab().folder.items.reduce((m: Record<string, number>, x) => ((m[x.name] = (m[x.name] ?? 0) + 1), m), {})).filter(([, n]) => n > 1))}; errors: ${errors.join(" || ").slice(0, 200) || "none"}; pane: ${pane?.innerHTML.slice(0, 120)})`);
      });
    }
    tab().selectOnly(keyOf(tab().visible[0]));
  });

  await check("embedded terminal runs a shell in the folder", async () => {
    let out = "";
    const id = await termOpen(dir, 80, 24, (e) => {
      if (e.kind === "output") out += atob(e.data);
    });
    await termWrite(id, "echo cx-$((2+3)) && pwd\r");
    await until("shell output", () => out.includes("cx-5") && out.includes("cx-selftest"), 8000);
    await termClose(id);
  });

  await check("back / forward from menu commands (what Logi Options+ keystrokes reach)", async () => {
    const { run } = await import("./commands.svelte");
    tab().navigate(dir);
    await until("dir", () => tab().folder.status === "ready");
    tab().navigate(childUri(dir, "sub"));
    await until("sub", () => tab().folder.status === "ready" && tab().folder.info?.name === "sub");
    run("nav.back", true);
    await until("back", () => tab().folder.info?.uri.replace(/\/$/, "") === dir.replace(/\/$/, ""));
    run("nav.forward", true);
    run("nav.forward", true); // a duplicate within 250 ms is ignored
    await until("forward", () => tab().folder.info?.name === "sub");
    if (tab().index !== tab().history.length - 1) throw new Error("ran twice");
    run("nav.up", true);
    await until("up", () => tab().folder.info?.uri.replace(/\/$/, "") === dir.replace(/\/$/, ""));
  });

  await check("settings: every section opens", async () => {
    void dialogs.ask("settings");
    await until("settings dialog", () => document.querySelector(".modal nav"));
    for (const name of ["Sharing & devices", "Servers", "About", "General", "Servers"]) {
      const btn = [...document.querySelectorAll<HTMLButtonElement>(".modal nav button")].find((b) => b.textContent?.includes(name));
      if (!btn) throw new Error(`no ${name} button`);
      btn.click();
      await until(`${name} section`, () => document.querySelector(".modal nav button.active")?.textContent?.includes(name), 3000);
      await sleep(200);
    }
    dialogs.close(null);
  });

  await check("peer service and discovery respond", async () => {
    const p = await peerStatus();
    if (!p.deviceId) throw new Error("no device id");
    const d = await listDevices();
    if (!Array.isArray(d)) throw new Error("devices");
    await log(`  peer ${p.deviceId} · ${d.length} devices discovered so far`, "test");
  });

  if (cfg.remote) {
    const servers = [
      { name: "SFTP (OpenSSH)", uri: "sftp://127.0.0.1:2223/config", mode: "polling" },
      { name: "SMB (Samba)", uri: "smb://127.0.0.1:1445/private", mode: "live" },
      { name: "WebDAV (rclone)", uri: "dav://127.0.0.1:8088/", mode: "polling" },
      { name: "FTP (Pure-FTPd)", uri: "ftp://127.0.0.1:2121/", mode: "polling" },
      { name: "FTPS (Pure-FTPd)", uri: "ftps://127.0.0.1:2121/", mode: "polling" },
      { name: "S3 (MinIO)", uri: "s3://127.0.0.1:9900/cx-test", mode: "polling", user: "cxadmin", password: "cxsecret123" },
    ];
    for (const srv of servers) {
      await check(`${srv.name}: sign in, browse, upload, preview, rename, delete`, async () => {
        const s = srv as { user?: string; password?: string };
        const creds = { user: s.user ?? "cx", secret: { type: "password" as const, password: s.password ?? "cxpass" } };
        for (let i = 0; ; i++) {
          try {
            await connectServer(srv.uri, creds, false);
            break;
          } catch (e) {
            const err = asCxError(e);
            // What the host-key dialog does after the user accepts.
            if (err?.kind === "hostKeyUnknown" && i < 2) await trustHostKey(err.message.uri, err.message.keyType, err.message.fingerprint);
            else throw e;
          }
        }
        tab().navigate(srv.uri);
        await until("remote listing", () => tab().folder.status === "ready" || tab().folder.status === "error", 15000);
        if (tab().folder.status === "error") throw new Error(tab().folder.error ?? "listing failed");
        await until(`${srv.mode} watch`, () => tab().folder.live === srv.mode, 5000);
        const name = `cx-${Date.now()}.txt`;
        const up = await job(await transfers.submit({ kind: "copy", sources: [childUri(dir, "alpha.txt")], dest: srv.uri, conflict: "replace" }));
        if (up.state !== "done") throw new Error(`upload ${up.state}: ${up.errors[0]?.message}`);
        const remote = childUri(srv.uri, "alpha.txt");
        // Our own finished upload refreshes the polled folder immediately.
        await until("uploaded file listed", () => has("alpha.txt"), 5000);
        const t = await previewText(remote);
        if (!t.text.startsWith("hello")) throw new Error(`preview: ${t.text}`);
        const bytes = await (await fetch(fileUrl(remote), { headers: { Range: "bytes=0-4" } })).text();
        if (bytes !== "hello") throw new Error(`range read: ${bytes}`);
        await renameEntry(srv.uri, "alpha.txt", name);
        const del = await job(await transfers.submit({ kind: "delete", sources: [childUri(srv.uri, name)] }));
        if (del.state !== "done") throw new Error(`delete ${del.state}: ${del.errors[0]?.message}`);
      });
    }
    await check("opening an untrusted SFTP server from the sidebar: trust key, sign in, browse", async () => {
      // Answer the dialogs like a person would.
      const seen: string[] = [];
      const ask = dialogs.ask.bind(dialogs);
      dialogs.ask = (async (kind: string, props: Record<string, unknown>) => {
        seen.push(kind);
        if (kind === "hostKey") return true;
        if (kind === "signIn") {
          try {
            await connectServer(props.uri as string, { user: "cx", secret: { type: "password", password: "cxpass" } }, false);
          } catch (e) {
            seen.push(`signIn failed for ${props.uri}: ${JSON.stringify(e)}`);
            return false;
          }
          return true;
        }
        return ask(kind as never, props);
      }) as typeof dialogs.ask;
      try {
        // The IPv6 loopback is a host name we have never trusted.
        tab().navigate("sftp://[::1]:2223/config");
        await until("listing after trust + sign-in", () => tab().folder.status === "ready", 20000).catch((e) => {
          throw new Error(`${e.message}; dialogs: ${seen.join(",")}; state: ${tab().folder.status} ${tab().folder.error ?? ""}`);
        });
        if (!seen.includes("hostKey")) throw new Error(`dialogs: ${seen.join(",")}`);
        if (!tab().folder.info?.name) throw new Error("tab has no name");
      } finally {
        dialogs.ask = ask;
      }
    });
    tab().navigate(dir);
  }

  await log(`SELFTEST ${failed ? "FAILED" : "PASSED"}: ${passed} passed, ${failed} failed`, "result");
  await invoke("selftest_exit", { code: failed ? 1 : 0 });
}
