// Quick Look zoom: ctrl+wheel (trackpad pinch in Chromium), pan, keys, double-click.
//   npm run dev & node tests/ui-zoom.mjs
import { checker, launch, sleep } from "./cdp.mjs";

const { check, failures } = checker();
const t = await launch();
const tf = () => t.eval(`document.querySelector('.ql .zoom img')?.style.transform ?? ''`);
const scaleOf = async () => Number((await tf()).match(/scale\(([\d.]+)\)/)?.[1] ?? 0);

try {
  await t.open("?path=~/Pictures");
  await t.focusList();
  const rows = await t.rows();
  const i = rows.findIndex((n) => /\.(png|jpe?g|heic|webp)$/i.test(n));
  check("folder has an image", i >= 0, JSON.stringify(rows));
  await t.key("Home");
  for (let k = 0; k < i; k++) await t.key("ArrowDown");
  await t.key(" ");
  await sleep(400);
  check("Quick Look shows a zoomable image", await t.eval(`!!document.querySelector('.ql .zoom img')`));
  const c = await t.eval(`(() => { const r = document.querySelector('.ql .zoom').getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);

  for (let k = 0; k < 5; k++) await t.send("Input.dispatchMouseEvent", { type: "mouseWheel", x: c.x, y: c.y, deltaX: 0, deltaY: -40, modifiers: 2 });
  await sleep(100);
  const s1 = await scaleOf();
  check("pinch (ctrl+wheel) zooms in", s1 > 1.5, await tf());
  check("zoom badge shows percentage", /%$/.test(await t.eval(`document.querySelector('.ql .badge')?.textContent ?? ''`)));

  const before = await tf();
  await t.send("Input.dispatchMouseEvent", { type: "mouseWheel", x: c.x, y: c.y, deltaX: 30, deltaY: 30 });
  await sleep(100);
  check("two-finger scroll pans when zoomed", (await tf()) !== before, `${before} -> ${await tf()}`);
  check("Quick Look stays on the same file while panning", await t.eval(`!!document.querySelector('.ql .zoom img')`));

  await t.key("0");
  await sleep(250);
  check("0 resets to fit", (await scaleOf()) === 1, await tf());
  await t.key("+");
  await sleep(250);
  check("+ zooms in", (await scaleOf()) > 1.2, await tf());
  await t.key("-");
  await t.key("-");
  await sleep(250);
  check("- zooms out (not below fit)", (await scaleOf()) === 1, await tf());

  for (const type of ["mousePressed", "mouseReleased"]) await t.send("Input.dispatchMouseEvent", { type, x: c.x, y: c.y, button: "left", clickCount: 1 });
  for (const type of ["mousePressed", "mouseReleased"]) await t.send("Input.dispatchMouseEvent", { type, x: c.x, y: c.y, button: "left", clickCount: 2 });
  await sleep(300);
  check("double-click zooms in", (await scaleOf()) >= 2, await tf());

  await t.key("Escape");
  await sleep(150);
  check("Escape closes Quick Look", !(await t.eval(`!!document.querySelector('.ql')`)));
  // Office documents render in a sandboxed frame.
  await t.open("?path=~/Documents");
  await t.focusList();
  {
    const rows = await t.rows();
    await t.key("Home");
    for (let k = 0; k < rows.indexOf("Resume.docx"); k++) await t.key("ArrowDown");
  }
  await t.key(" ");
  await sleep(500);
  check("Quick Look renders a Word document", await t.eval(`(() => { const f = document.querySelector('.ql iframe.office'); return !!f && f.getAttribute('sandbox') === '' && f.srcdoc.includes('Resume.docx'); })()`));
  await t.key("Escape");

  check("no uncaught errors", t.errors.length === 0, t.errors.join("\n"));
} catch (e) {
  console.error(e);
  check("test run", false, String(e));
} finally {
  t.close();
}
const failed = failures();
console.log(failed ? `\n${failed} failed` : "\nall passed");
process.exit(failed ? 1 : 0);
