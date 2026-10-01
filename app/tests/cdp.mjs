// Tiny Chrome DevTools Protocol driver for UI tests against the browser
// preview (mock backend). Needs `npm run dev` running.
import { spawn } from "node:child_process";
import { writeFileSync } from "node:fs";

const CHROME = process.env.CHROME ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
export const BASE = process.env.UI_URL ?? "http://localhost:1420/";
export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const KEYCODES = { Enter: 13, Escape: 27, ArrowDown: 40, ArrowUp: 38, ArrowLeft: 37, ArrowRight: 39, Backspace: 8, Delete: 46, Tab: 9, F2: 113, F3: 114, F5: 116, F6: 117, F7: 118, F8: 119, F9: 120, End: 35, Home: 36, " ": 32 };
const CODES = { " ": "Space", Enter: "Enter", Escape: "Escape", Tab: "Tab", Backspace: "Backspace", Delete: "Delete", ",": "Comma", ".": "Period", "\\": "Backslash" };
export const MOD = { alt: 1, ctrl: 2, meta: 4, shift: 8 };

export async function launch({ width = 1280, height = 800, dark = true } = {}) {
  const port = 9300 + Math.floor(Math.random() * 500);
  const chrome = spawn(CHROME, ["--headless=new", `--remote-debugging-port=${port}`, `--window-size=${width},${height}`, `--user-data-dir=/tmp/cx-ui-${port}`, "--hide-scrollbars", "--autoplay-policy=no-user-gesture-required", "--mute-audio", ...(dark ? ["--force-dark-mode"] : []), "about:blank"], { stdio: "ignore" });
  let target;
  for (let i = 0; i < 80 && !target; i++) {
    await sleep(100);
    try {
      target = (await (await fetch(`http://127.0.0.1:${port}/json`)).json()).find((t) => t.type === "page");
    } catch {}
  }
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r) => (ws.onopen = r));
  let seq = 0;
  const pending = new Map();
  const errors = [];
  ws.onmessage = (m) => {
    const d = JSON.parse(m.data);
    if (d.id && pending.has(d.id)) {
      const p = pending.get(d.id);
      pending.delete(d.id);
      d.error ? p.rej(new Error(JSON.stringify(d.error))) : p.res(d.result);
    } else if (d.method === "Runtime.exceptionThrown") errors.push(d.params.exceptionDetails.exception?.description ?? d.params.exceptionDetails.text);
    else if (d.method === "Runtime.consoleAPICalled" && d.params.type === "error") errors.push(d.params.args.map((a) => a.value ?? a.description).join(" "));
  };
  const send = (method, params = {}) => {
    const id = ++seq;
    ws.send(JSON.stringify({ id, method, params }));
    return new Promise((res, rej) => pending.set(id, { res, rej }));
  };
  await send("Page.enable");
  await send("Runtime.enable");
  await send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
  if (dark) await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "dark" }] });

  const page = {
    errors,
    send,
    async open(query = "", clearStorage = true) {
      if (clearStorage) {
        // Clear from a page that doesn't run the app, so nothing re-saves settings.
        await send("Page.navigate", { url: BASE + "tests/blank.html" });
        await sleep(200);
        await page.eval(`localStorage.clear()`);
      }
      await send("Page.navigate", { url: BASE + query });
      await sleep(1200);
    },
    async eval(expr) {
      const r = await send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true });
      if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
      return r.result.value;
    },
    async key(k, mods = 0) {
      const text = k === "Enter" ? "\r" : k.length === 1 && !(mods & (MOD.ctrl | MOD.meta)) ? k : undefined;
      const code = CODES[k] ?? (k.length === 1 && /[a-z]/i.test(k) ? `Key${k.toUpperCase()}` : k.length === 1 && /\d/.test(k) ? `Digit${k}` : k);
      const base = { key: k, code, windowsVirtualKeyCode: KEYCODES[k] ?? k.toUpperCase().charCodeAt(0), modifiers: mods };
      await send("Input.dispatchKeyEvent", { type: text ? "keyDown" : "rawKeyDown", text, ...base });
      await send("Input.dispatchKeyEvent", { type: "keyUp", ...base });
      await sleep(40);
    },
    async type(s) {
      for (const c of s) await page.key(c);
    },
    async click(selector, { button = "left", double = false, mods = 0 } = {}) {
      const box = await page.eval(`(() => { const el = typeof ${JSON.stringify(selector)} === "string" ? document.querySelector(${JSON.stringify(selector)}) : null; if (!el) return null; const r = el.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
      if (!box) throw new Error(`no element ${selector}`);
      for (const type of ["mousePressed", "mouseReleased"]) await send("Input.dispatchMouseEvent", { type, x: box.x, y: box.y, button, clickCount: 1, modifiers: mods });
      if (double) for (const type of ["mousePressed", "mouseReleased"]) await send("Input.dispatchMouseEvent", { type, x: box.x, y: box.y, button, clickCount: 2, modifiers: mods });
      await sleep(80);
    },
    async clickText(selector, text) {
      const ok = await page.eval(`(() => { const el = [...document.querySelectorAll(${JSON.stringify(selector)})].find(e => e.textContent.includes(${JSON.stringify(text)})); if (!el) return false; el.click(); return true; })()`);
      if (!ok) throw new Error(`no ${selector} with text ${text}`);
      await sleep(120);
    },
    async shot(path) {
      const r = await send("Page.captureScreenshot", { format: "png" });
      writeFileSync(path, Buffer.from(r.data, "base64"));
    },
    rows: () => page.eval(`[...document.querySelectorAll('.pane.active .details .row .text')].map(e => e.textContent)`),
    selected: () => page.eval(`[...document.querySelectorAll('.pane.active .details .row.selected .text')].map(e => e.textContent)`),
    focusList: () => page.eval(`document.querySelector('.pane.active .file-view').focus()`),
    close() {
      ws.close();
      chrome.kill();
    },
  };
  return page;
}

export function checker() {
  let failures = 0;
  const check = (name, cond, detail = "") => {
    console.log(`${cond ? "✓" : "✗"} ${name}${cond ? "" : "  " + detail}`);
    if (!cond) failures++;
  };
  return { check, failures: () => failures };
}
