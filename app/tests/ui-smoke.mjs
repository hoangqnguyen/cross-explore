// UI smoke test: drives the browser preview (mock backend) through headless
// Chrome over the DevTools protocol. Needs `npm run dev` running.
//   node tests/ui-smoke.mjs
import { spawn } from "node:child_process";

const CHROME = process.env.CHROME ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const URL = process.env.UI_URL ?? "http://localhost:1420/?path=~/Downloads";
const port = 9333;
const chrome = spawn(CHROME, ["--headless=new", `--remote-debugging-port=${port}`, "--window-size=1180,740", "--user-data-dir=/tmp/cx-ui-smoke", "about:blank"], { stdio: "ignore" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let ws, seq = 0;
const pending = new Map();
function send(method, params = {}) {
  const id = ++seq;
  ws.send(JSON.stringify({ id, method, params }));
  return new Promise((res, rej) => pending.set(id, { res, rej }));
}
async function evaluate(expr) {
  const r = await send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.text + " " + JSON.stringify(r.exceptionDetails.exception?.description));
  return r.result.value;
}
async function key(k, opts = {}) {
  const text = k.length === 1 ? k : undefined;
  const code = { Enter: 13, Escape: 27, ArrowDown: 40, ArrowUp: 38, Backspace: 8, F2: 113, End: 35, Home: 36 }[k] ?? k.toUpperCase().charCodeAt(0);
  const base = { key: k, windowsVirtualKeyCode: code, modifiers: opts.modifiers ?? 0 };
  await send("Input.dispatchKeyEvent", { type: text ? "keyDown" : "rawKeyDown", text, ...base });
  await send("Input.dispatchKeyEvent", { type: "keyUp", ...base });
  await sleep(30);
}
const rows = () => evaluate(`[...document.querySelectorAll('.details .row:not(.skeleton) .text')].map(e => e.textContent)`);
const selected = () => evaluate(`[...document.querySelectorAll('.details .row.selected .text')].map(e => e.textContent)`);

let failures = 0;
function check(name, cond, detail = "") {
  console.log(`${cond ? "✓" : "✗"} ${name}${cond ? "" : "  " + detail}`);
  if (!cond) failures++;
}

try {
  let target;
  for (let i = 0; i < 50 && !target; i++) {
    await sleep(100);
    try {
      target = (await (await fetch(`http://127.0.0.1:${port}/json`)).json()).find((t) => t.type === "page");
    } catch {}
  }
  ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r) => (ws.onopen = r));
  ws.onmessage = (m) => {
    const d = JSON.parse(m.data);
    if (d.id && pending.has(d.id)) (d.error ? pending.get(d.id).rej(d.error) : pending.get(d.id).res(d.result)), pending.delete(d.id);
  };
  await send("Page.enable");
  await send("Runtime.enable");
  await send("Page.navigate", { url: URL });
  await sleep(1500);

  const initial = await rows();
  check("lists the folder", initial.length === 10, JSON.stringify(initial));
  check("natural sort, case-insensitive", initial[0] === "cross-explore-0.1.0.dmg" && initial.indexOf("IMG_2041.HEIC") < initial.indexOf("Inter-4.0.zip"), JSON.stringify(initial));
  check("breadcrumb shows path", (await evaluate(`[...document.querySelectorAll('.crumb')].map(c => c.textContent.trim()).join(' > ')`)) === "demo > Downloads");

  await evaluate(`document.querySelector('.details').focus()`);
  await key("ArrowDown");
  await key("ArrowDown");
  check("arrow keys move selection", JSON.stringify(await selected()) === JSON.stringify([initial[1]]), JSON.stringify(await selected()));
  await key("ArrowDown", { modifiers: 8 /* shift */ });
  check("shift+arrow extends selection", (await selected()).length === 2);

  for (const c of "img") await key(c);
  check("type-to-filter", JSON.stringify(await rows()) === JSON.stringify(["IMG_2041.HEIC", "IMG_2042.HEIC"]), JSON.stringify(await rows()));
  check("status bar shows filtered count", (await evaluate(`document.querySelector('.status').textContent`)).includes("filtered from 10"));
  await key("Escape");
  check("escape clears filter", (await rows()).length === 10);

  // New folder → inline rename → commit.
  await evaluate(`[...document.querySelectorAll('.commands button')].find(b => b.textContent.includes('New folder')).click()`);
  await sleep(200);
  check("new folder enters rename mode", await evaluate(`document.activeElement?.classList.contains('rename') && document.activeElement.value === 'New folder'`));
  await evaluate(`document.activeElement.value = 'Receipts'`);
  await key("Enter");
  await sleep(200);
  const afterRename = await rows();
  check("renamed folder sorts first (folders first)", afterRename[0] === "Receipts", JSON.stringify(afterRename));
  check("renamed folder is selected", JSON.stringify(await selected()) === '["Receipts"]');

  // The mock backend adds a file every 6 s to watched folders.
  let fresh = [];
  for (let i = 0; i < 70 && !fresh.length; i++) {
    await sleep(100);
    fresh = await evaluate(`[...document.querySelectorAll('.details .row.fresh .text')].map(e => e.textContent)`);
  }
  check("live update appears with highlight, no refresh", fresh.some((n) => n.startsWith("Shared note")), JSON.stringify(fresh));

  // Navigate into a folder with Enter and back up.
  await evaluate(`document.querySelector('.details').focus()`);
  await key("Home");
  await key("Enter", { modifiers: 0 });
  await sleep(300);
  const macRename = await evaluate(`!!document.querySelector('.rename')`);
  if (macRename) await key("Escape"); // Enter renames on macOS, like Finder
  await key("ArrowDown", { modifiers: 4 /* meta */ });
  await sleep(300);
  check("open folder", (await evaluate(`document.querySelector('.crumb.current').textContent.trim()`)) === "Receipts");
  check("empty state", (await evaluate(`document.querySelector('.empty')?.textContent ?? ''`)).includes("This folder is empty"));
  await key("ArrowUp", { modifiers: 4 });
  await sleep(300);
  check("up selects the folder we came from", JSON.stringify(await selected()) === '["Receipts"]', JSON.stringify(await selected()));

  // Tabs.
  await key("t", { modifiers: 4 });
  await sleep(300);
  check("new tab", (await evaluate(`document.querySelectorAll('.tab').length`)) === 2);
  await key("w", { modifiers: 4 });
  await sleep(200);
  check("close tab", (await evaluate(`document.querySelectorAll('.tab').length`)) === 1);

  // Trash.
  await evaluate(`document.querySelector('.details').focus()`);
  await key("Home");
  await key("Backspace", { modifiers: 4 });
  await sleep(300);
  check("move to trash removes the row", !(await rows()).includes("Receipts"), JSON.stringify(await rows()));
  check("toast confirms", (await evaluate(`document.querySelector('.toast')?.textContent ?? ''`)).includes("Moved"));
} catch (e) {
  console.error(e);
  failures++;
} finally {
  chrome.kill();
}
console.log(failures ? `\n${failures} failed` : "\nall passed");
process.exit(failures ? 1 : 0);
