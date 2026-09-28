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

  // PDFs: drawn with pdf.js, so pinch/scroll/keys work like images.
  await t.open("?path=~/Documents/Invoices");
  await t.focusList();
  await t.key("Home");
  await t.key(" ");
  const pdfReady = async () => {
    for (let i = 0; i < 60; i++) {
      if (await t.eval(`(() => { const c = document.querySelector('.ql .pdf canvas'); return !!c && c.width > 0; })()`)) return true;
      await sleep(100);
    }
    return false;
  };
  check("Quick Look draws the PDF with pdf.js", await pdfReady());
  check("all pages laid out, with a page counter", (await t.eval(`document.querySelectorAll('.ql .pdf canvas').length`)) === 3 && (await t.eval(`document.querySelector('.ql .pageno')?.textContent`)) === "1 / 3");
  const cssW = () => t.eval(`parseFloat(document.querySelector('.ql .pdf canvas').style.width)`);
  const pxW = () => t.eval(`document.querySelector('.ql .pdf canvas').width`);
  const w0 = await cssW();
  const px0 = await pxW();
  const pc = await t.eval(`(() => { const r = document.querySelector('.ql .pdf').getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 3 }; })()`);
  for (let k = 0; k < 5; k++) await t.send("Input.dispatchMouseEvent", { type: "mouseWheel", x: pc.x, y: pc.y, deltaX: 0, deltaY: -40, modifiers: 2 });
  await sleep(500);
  check("pinch (ctrl+wheel) zooms the PDF", (await cssW()) > w0 * 1.5, `${w0} -> ${await cssW()}`);
  check("pages re-render sharp after zooming", (await pxW()) > px0 * 1.5, `${px0} -> ${await pxW()}`);
  const sx = await t.eval(`document.querySelector('.ql .pdf').scrollLeft`);
  check("zoom keeps the point under the cursor (scrolls sideways)", sx > 0, String(sx));
  await t.send("Input.dispatchMouseEvent", { type: "mouseWheel", x: pc.x, y: pc.y, deltaX: 0, deltaY: 400 });
  await sleep(200);
  check("two-finger scroll pans the PDF", (await t.eval(`document.querySelector('.ql .pdf').scrollTop`)) > 0);
  await t.key("0");
  await sleep(300);
  check("0 fits the page again", Math.abs((await cssW()) - w0) < 1, `${w0} vs ${await cssW()}`);
  await t.key("+");
  await sleep(300);
  check("+ zooms the PDF", (await cssW()) > w0 * 1.2);
  await t.key("Escape");
  await sleep(150);

  // Code previews fold: brackets for Rust, indentation for Python.
  await t.open("?path=~/Documents/Projects");
  await t.focusList();
  {
    const rows = await t.rows();
    await t.key("Home");
    for (let k = 0; k < rows.indexOf("main.rs"); k++) await t.key("ArrowDown");
    await t.key(" ");
    await sleep(400);
    const lineNos = () => t.eval(`[...document.querySelectorAll('.ql .codeview .line .no')].map(e => +e.textContent)`);
    check("code preview shows line numbers", JSON.stringify(await lineNos()) === "[1,2,3,4,5,6,7]", JSON.stringify(await lineNos()));
    const folds = await t.eval(`[...document.querySelectorAll('.ql .codeview .line')].filter(l => l.querySelector('.fold')).map(l => +l.querySelector('.no').textContent)`);
    check("fold markers on block starts (fn, for)", JSON.stringify(folds) === "[3,4]", JSON.stringify(folds));
    await t.eval(`document.querySelectorAll('.ql .codeview .fold')[0].click()`);
    await sleep(150);
    check("collapsing fn main hides its body, keeps the closing brace", JSON.stringify(await lineNos()) === "[1,2,3,7]", JSON.stringify(await lineNos()));
    check("collapsed block says how many lines are hidden", (await t.eval(`document.querySelector('.ql .codeview .more')?.textContent`)) === "⋯ 3 lines");
    await t.eval(`document.querySelector('.ql .codeview .more').click()`);
    await sleep(150);
    check("the ⋯ marker expands it again", (await lineNos()).length === 7);
    await t.eval(`document.querySelector('.ql .codeview .folds button').click()`);
    await sleep(150);
    check("Collapse all", JSON.stringify(await lineNos()) === "[1,2,3,7]", JSON.stringify(await lineNos()));
    await t.key("ArrowUp"); // previous file: greet.py
    await sleep(400);
    const pyFolds = await t.eval(`[...document.querySelectorAll('.ql .codeview .line')].filter(l => l.querySelector('.fold')).map(l => +l.querySelector('.no').textContent)`);
    check("Python folds by indentation (class, defs, if/else, main)", JSON.stringify(pyFolds) === "[4,5,8,9,11,15]", JSON.stringify(pyFolds));
    await t.eval(`[...document.querySelectorAll('.ql .codeview .line')].find(l => l.querySelector('.no').textContent === '4').querySelector('.fold').click()`);
    await sleep(150);
    check("collapsing a Python class hides its methods", JSON.stringify(await lineNos()) === "[1,2,3,4,13,14,15,16]", JSON.stringify(await lineNos()));
    await t.key("Escape");
  }
  const split = await t.eval(`import('/src/lib/folding.ts').then(m => JSON.stringify(m.splitHtmlLines('a<span class="c">/* x\\ny */</span>b')))`);
  check("highlight spans across lines are closed and reopened", split === JSON.stringify(['a<span class="c">/* x</span>', '<span class="c">y */</span>b']), split);

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
