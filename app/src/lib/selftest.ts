// End-to-end self test against the real backend. Runs only when the app was
// started with CX_SELFTEST=1: drives the UI state layer through a realistic
// session and reports each check to the terminal, then exits.
import { invoke } from "@tauri-apps/api/core";
import { termClose, termOpen, termWrite, asCxError, connectServer, trustHostKey, childUri, dirSize, renameEntry, fileUrl, getTags, previewText, search, setTags, thumbUrl, compareDirs, peerStatus, devices as listDevices, listDir, type Entry } from "./api";
import { keyOf } from "./folder.svelte";
import { transfers } from "./stores/transfers.svelte";
import { ws } from "./workspace.svelte";

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

  await check("views render with real data", async () => {
    for (const v of ["icons", "columns", "gallery", "details"] as const) {
      tab().view = v;
      const cls = v === "details" ? ".details" : `.${v}`;
      await until(`${v} view`, () => document.querySelector(`.pane.active ${cls}`), 3000);
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
    tab().navigate(dir);
  }

  await log(`SELFTEST ${failed ? "FAILED" : "PASSED"}: ${passed} passed, ${failed} failed`, "result");
  await invoke("selftest_exit", { code: failed ? 1 : 0 });
}
