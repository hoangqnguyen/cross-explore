// Rendering performance in the real UI (browser preview, mock backend):
// time to first rows for a 100k-item folder and scrolling frame times.
//   npm run dev & node tests/ui-perf.mjs [--check]
import { launch, sleep } from "./cdp.mjs";

const t = await launch({ width: 1280, height: 800 });
await t.open("?path=~/Downloads");
// Warm the mock's generated 100k folder, then measure a fresh navigation.
await t.eval(`(async () => { window.__cx.ws.activeTab.navigate('~/Big'); while (window.__cx.ws.activeTab.visible.length < 100000) await new Promise(r => setTimeout(r, 20)); window.__cx.ws.activeTab.navigate('~/Downloads'); await new Promise(r => setTimeout(r, 300)); })()`);
const firstRows = await t.eval(`(async () => {
  const t0 = performance.now();
  window.__cx.ws.activeTab.navigate('~/Big');
  while (!document.querySelector('.pane.active .details .row .text')?.textContent?.startsWith('file-')) await new Promise(r => requestAnimationFrame(r));
  const first = performance.now() - t0;
  while (window.__cx.ws.activeTab.folder.status !== 'ready' || window.__cx.ws.activeTab.folder.refreshing || window.__cx.ws.activeTab.visible.length < 100000) await new Promise(r => setTimeout(r, 10));
  return { first, all: performance.now() - t0, count: window.__cx.ws.activeTab.visible.length };
})()`);
await sleep(300);
const frames = await t.eval(`(async () => {
  const el = document.querySelector('.pane.active .details');
  const times = [];
  let last = performance.now();
  for (let i = 0; i < 180; i++) {
    el.scrollTop += 900;
    await new Promise(r => requestAnimationFrame(r));
    const now = performance.now();
    times.push(now - last);
    last = now;
  }
  times.sort((a, b) => a - b);
  return { p50: times[90], p95: times[171], max: times[179], rowsInDom: document.querySelectorAll('.pane.active .details .row').length };
})()`);
const sortMs = await t.eval(`(async () => { const t0 = performance.now(); window.__cx.ws.sortBy('size'); await new Promise(r => requestAnimationFrame(r)); return performance.now() - t0; })()`);
const filterMs = await t.eval(`(async () => { const t0 = performance.now(); window.__cx.ws.activeTab.filter = '0421'; await new Promise(r => requestAnimationFrame(r)); await new Promise(r => requestAnimationFrame(r)); return performance.now() - t0; })()`);
t.close();

const budgets = [
  ["first rows on screen (100k folder)", firstRows.first, 50],
  ["whole 100k listing sorted & shown", firstRows.all, 1500],
  // Frame times are quantized to the 60 Hz refresh; under 25 ms = no dropped frames.
  ["scroll frame p95 (no dropped frames)", frames.p95, 25],
  ["re-sort 100k by size", sortMs, 400],
  ["type-to-filter 100k", filterMs, 100],
];
let failed = 0;
for (const [name, v, budget] of budgets) {
  const ok = v <= budget;
  failed += ok ? 0 : 1;
  console.log(`${ok ? "✓" : "✗"} ${name.padEnd(36)} ${v.toFixed(1).padStart(8)} ms  (budget ${budget.toFixed(0)} ms)`);
}
console.log(`  ${firstRows.count.toLocaleString()} items · ${frames.rowsInDom} rows in the DOM · frame p50 ${frames.p50.toFixed(1)} ms, max ${frames.max.toFixed(1)} ms`);
process.exit(process.argv.includes("--check") && failed ? 1 : 0);
