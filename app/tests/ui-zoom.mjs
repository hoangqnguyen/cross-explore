// Quick Look zoom: ctrl+wheel (trackpad pinch in Chromium), pan, keys, double-click.
//   npm run dev & node tests/ui-zoom.mjs
import { checker, launch, sleep } from "./cdp.mjs";

const { check, failures } = checker();
const t = await launch();
const tf = () => t.eval(`document.querySelector('.ql .zoom img')?.style.transform ?? ''`);
const scaleOf = async () => Number((await tf()).match(/scale\(([\d.]+)\)/)?.[1] ?? 0);
// Pinch (ctrl+wheel) at the middle of `sel`, as Chromium reports a trackpad pinch.
const pinchAt = async (sel, steps = 5, deltaY = -40) => {
  const p = await t.eval(`(() => { const r = document.querySelector(${JSON.stringify(sel)}).getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 3 }; })()`);
  for (let k = 0; k < steps; k++) await t.send("Input.dispatchMouseEvent", { type: "mouseWheel", x: p.x, y: p.y, deltaX: 0, deltaY, modifiers: 2 });
  await sleep(150);
};
const frameZoom = (sel) => t.eval(`Number(document.querySelector(${JSON.stringify(sel)})?.contentDocument?.body?.style.zoom || 1)`);
const badge = () => t.eval(`document.querySelector('.ql .zoom-badge')?.textContent ?? ''`);
const sandboxOk = (sel) => t.eval(`(() => { const f = document.querySelector(${JSON.stringify(sel)}); return !!f && f.getAttribute('sandbox') === 'allow-same-origin'; })()`);

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
  check("Quick Look renders a Word document", await t.eval(`(() => { const f = document.querySelector('.ql iframe.office'); return !!f && f.srcdoc.includes('Resume.docx'); })()`));
  check("…sandboxed with no scripts (same origin only, for gestures)", await sandboxOk(".ql iframe.office"));
  await pinchAt(".ql iframe.office");
  check("pinch zooms the document", (await frameZoom(".ql iframe.office")) > 1.5, String(await frameZoom(".ql iframe.office")));
  check("…with a zoom badge", /%$/.test(await badge()), await badge());
  await t.key("0");
  await sleep(100);
  check("0 fits the document again (Quick Look focused)", (await frameZoom(".ql iframe.office")) === 1);
  await t.key("=");
  await sleep(100);
  check("+ zooms the document", (await frameZoom(".ql iframe.office")) > 1.2);
  await pinchAt(".ql iframe.office", 20, 60);
  check("pinch out stops at 50%", (await frameZoom(".ql iframe.office")) === 0.5, String(await frameZoom(".ql iframe.office")));
  // Click into the document: its keys are its own now, but Quick Look's still work.
  {
    const r = await t.eval(`(() => { const r = document.querySelector('.ql iframe.office').getBoundingClientRect(); return { x: r.left + 40, y: r.top + 40 }; })()`);
    for (const type of ["mousePressed", "mouseReleased"]) await t.send("Input.dispatchMouseEvent", { type, x: r.x, y: r.y, button: "left", clickCount: 1 });
    await sleep(100);
    check("clicking the document focuses it", await t.eval(`document.activeElement?.tagName === 'IFRAME'`));
    await t.key("0");
    await sleep(100);
    check("0 still resets from inside the document", (await frameZoom(".ql iframe.office")) === 1);
    await t.key("ArrowUp");
    await sleep(400);
    check("arrow keys still move to another file", await t.eval(`document.querySelector('.ql header strong')?.textContent === 'Pitch deck.pptx'`), await t.eval(`document.querySelector('.ql header strong')?.textContent`));
  }
  {
    const r = await t.eval(`(() => { const r = document.querySelector('.ql iframe.office').getBoundingClientRect(); return { x: r.left + 40, y: r.top + 40 }; })()`);
    for (const type of ["mousePressed", "mouseReleased"]) await t.send("Input.dispatchMouseEvent", { type, x: r.x, y: r.y, button: "left", clickCount: 1 });
    await sleep(100);
  }
  await t.key("Escape");
  await sleep(150);
  check("Escape inside the document closes Quick Look", !(await t.eval(`!!document.querySelector('.ql')`)));
  {
    const rows = await t.rows();
    await t.key("Home");
    for (let k = 0; k < rows.indexOf("Pitch deck.pptx"); k++) await t.key("ArrowDown");
  }
  await t.key(" ");
  await sleep(500);
  await pinchAt(".ql iframe.office");
  check("pinch zooms a PowerPoint deck", (await frameZoom(".ql iframe.office")) > 1.5, String(await frameZoom(".ql iframe.office")));
  await t.key("Escape");
  await sleep(150);
  // The preview pane's (half-size) document zooms too.
  await t.eval(`void (window.__cx.settings.data.previewPane = true)`);
  await sleep(600);
  check("the preview pane shows the deck", await t.eval(`!!document.querySelector('.preview-pane iframe.office.small, iframe.office.small')`));
  await pinchAt("iframe.office.small");
  check("pinch zooms the preview pane's document", (await frameZoom("iframe.office.small")) > 1.5, String(await frameZoom("iframe.office.small")));
  await t.eval(`void (window.__cx.settings.data.previewPane = false)`);
  await sleep(150);


  // HTML files: rendered by default, with a Code toggle for the source.
  await t.open("?path=~/Documents/Projects");
  await t.focusList();
  await t.key("Home");
  for (let k = 0; k < (await t.rows()).indexOf("report.html"); k++) await t.key("ArrowDown");
  await t.key(" ");
  await sleep(400);
  check("Quick Look shows the rendered page by default", await t.eval(`(() => { const f = document.querySelector('.ql .htmlview iframe'); return !!f && f.srcdoc.includes('<h1>Full Run Report</h1>'); })()`));
  check("…sandboxed with no scripts (same origin only, for gestures)", await sandboxOk(".ql .htmlview iframe"));
  await pinchAt(".ql .htmlview iframe");
  check("pinch zooms the web page", (await frameZoom(".ql .htmlview iframe")) > 1.5, String(await frameZoom(".ql .htmlview iframe")));
  check("…and scrolls to keep the point under the fingers", await t.eval(`document.querySelector('.ql .htmlview iframe').contentDocument.scrollingElement.scrollTop > 0`));
  check("…with a Preview/Code switch, Preview active", (await t.eval(`document.querySelector('.ql .mode-switch button.active')?.textContent.trim()`) ?? "").includes("Preview"));
  await t.clickText(".ql .mode-switch button", "Code");
  await sleep(150);
  check("Code shows the source as text, not rendered", await t.eval(`!document.querySelector('.ql .htmlview iframe') && document.querySelector('.ql .htmlview .codeview')?.textContent.includes('<h1>Full Run Report</h1>')`));
  await t.clickText(".ql .mode-switch button", "Preview");
  await sleep(150);
  check("switching back to Preview restores the iframe", await t.eval(`!!document.querySelector('.ql .htmlview iframe')`));
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
    const codeZoom = () => t.eval(`Number(document.querySelector('.ql .codeview .lines').style.zoom || 1)`);
    await pinchAt(".ql .codeview .scroll");
    check("pinch makes code bigger", (await codeZoom()) > 1.5, String(await codeZoom()));
    await t.key("0");
    await sleep(100);
    check("0 puts code back to normal size", (await codeZoom()) === 1);
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
  // Markdown renders formatted; pinch makes it bigger too.
  await t.focusList();
  await t.key("Home");
  for (let k = 0; k < (await t.rows()).indexOf("README.md"); k++) await t.key("ArrowDown");
  await t.key(" ");
  await sleep(400);
  const md = await t.eval(`(() => { const m = document.querySelector('.ql .md'); const tb = m?.querySelector('table'); const ol = m?.querySelector('ol'); return m && { cols: tb ? tb.querySelectorAll('thead th').length : 0, rows: tb ? tb.querySelectorAll('tbody tr').length : 0, center: tb?.querySelector('th:nth-child(2)')?.style.textAlign ?? '', bordered: tb ? getComputedStyle(tb.querySelector('td')).borderTopStyle : '', pipes: m.textContent.includes('|---'), items: ol ? ol.children.length : 0, wrapped: ol?.children[0]?.textContent ?? '' }; })()`);
  check("Markdown tables render as tables", md?.cols === 3 && md.rows === 2 && md.center === "center" && md.bordered === "solid" && !md.pipes, JSON.stringify(md));
  check("a numbered item's wrapped line stays in that item", md?.items === 2 && md.wrapped === "Open a folder, or a server from the sidebar.", JSON.stringify(md));
  await pinchAt(".ql .md");
  check("pinch zooms Markdown", (await t.eval(`Number(document.querySelector('.ql .md > div').style.zoom || 1)`)) > 1.5);
  await t.key("Escape");
  await sleep(150);

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
