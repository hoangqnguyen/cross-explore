// UI test for sharing with my devices, over the browser preview (mock
// backend): Share… from the context menu, the share dialog, the Sharing page
// (outbound, inbound, devices, activity) and stopping a share.
//   npm run dev & node tests/ui-sharing.mjs [screenshot-dir] [light]
import { checker, launch, sleep } from "./cdp.mjs";
const out = process.argv[2];
const dark = process.argv[3] !== "light";
const { check, failures } = checker();
const t = await launch({ width: 1360, height: 860, dark });
const rclick = async (sel, text) => {
  const box = await t.eval(`(() => { const el = [...document.querySelectorAll(${JSON.stringify(sel)})].find(e => e.textContent.trim().includes(${JSON.stringify(text)})); if (!el) return null; const r = el.getBoundingClientRect(); return { x: r.left + Math.min(60, r.width / 2), y: r.top + r.height / 2 }; })()`);
  if (!box) throw new Error("no " + text);
  for (const type of ["mousePressed", "mouseReleased"]) await t.send("Input.dispatchMouseEvent", { type, x: box.x, y: box.y, button: "right", buttons: type === "mousePressed" ? 2 : 0, clickCount: 1 });
  await sleep(300);
};
const menuLabels = () => t.eval(`[...document.querySelectorAll('.menu [role=menuitem]')].map(e => e.textContent.trim())`);
try {
  await t.open("?path=~");
  await t.eval(`window.__cx.ws.activeTab.view = "details"`);
  await sleep(300);
  check("shared folder has a badge in Details", await t.eval(`[...document.querySelectorAll('.pane.active .details .row')].some(r => r.textContent.includes('Downloads') && r.querySelector('.sbadge'))`));
  check("unshared folder has no badge", !(await t.eval(`[...document.querySelectorAll('.pane.active .details .row')].some(r => r.textContent.includes('Documents') && r.querySelector('.sbadge'))`)));
  await rclick(".pane.active .details .row", "Downloads");
  check("context menu offers 'Sharing options…' on a shared folder", (await menuLabels()).some((l) => l.startsWith("Sharing options")), JSON.stringify(await menuLabels()));
  await t.key("Escape");
  await rclick(".pane.active .details .row", "Documents");
  check("context menu offers 'Share…'", (await menuLabels()).some((l) => l.startsWith("Share…")));
  await t.clickText(".menu [role=menuitem]", "Share…");
  await sleep(300);
  check("share dialog opens for the folder", (await t.eval(`document.querySelector('.modal h2')?.textContent`)) === "Share “Documents”");
  check("warns that sharing is off", await t.eval(`!!document.querySelector('.modal .banner')`));
  check("link preview uses this device id", (await t.eval(`document.querySelector('.modal .linkrow input')?.value`)) === "peer://k7d2-mq4x/Documents/");
  await t.clickText(".modal .card", "Can edit");
  await t.clickText(".modal .opt", "Only the devices I choose");
  check("must pick a device", await t.eval(`document.querySelector('.modal button[type=submit]').disabled`));
  await t.clickText(".modal .dev", "hpc");
  await t.eval(`(() => { const i = document.querySelector('.modal .field input'); i.value = 'Downloads'; i.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  await sleep(100);
  check("duplicate share names are refused", (await t.eval(`document.querySelector('.modal .error')?.textContent ?? ''`)).includes("already"));
  await t.eval(`(() => { const i = document.querySelector('.modal .field input'); i.value = 'Docs'; i.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  await sleep(100);
  if (out) await t.shot(out + "/share-dialog.png");
  await t.click(".modal button[type=submit]");
  await sleep(400);
  check("dialog closes after sharing", !(await t.eval(`!!document.querySelector('.modal')`)));
  const st = await t.eval(`import('/src/lib/stores/devices.svelte.ts').then(m => JSON.parse(JSON.stringify(m.devices.peer)))`);
  const docs = st.shares.find((s) => s.name === "Docs");
  check("saved: name, edit access, only hpc, sharing turned on", !!docs && docs.readOnly === false && JSON.stringify(docs.devices) === '["hpc"]' && st.enabled === true, JSON.stringify(st.shares));
  check("toast confirms", (await t.eval(`document.body.textContent`)).includes("Sharing “Docs” with hpc"));
  check("newly shared folder gets a badge", await t.eval(`[...document.querySelectorAll('.pane.active .details .row')].some(r => r.textContent.includes('Documents') && r.querySelector('.sbadge'))`));

  await t.clickText(".sidebar .item", "Sharing");
  await sleep(1200);
  check("Sharing page opens from the sidebar", await t.eval(`!!document.querySelector('.sharing .hero')`));
  check("lists both shares", (await t.eval(`document.querySelectorAll('.sharing .srow').length`)) === 2);
  check("audience shows the chosen device", await t.eval(`[...document.querySelectorAll('.sharing .srow')].some(r => r.textContent.includes('Docs') && r.querySelector('.aud').textContent.includes('hpc'))`));
  check("connected device shown on the shares it can use", await t.eval(`[...document.querySelectorAll('.sharing .srow')].every(r => r.querySelector('.live')?.textContent.includes('hpc'))`));
  check("inbound: hpc's shares listed", JSON.stringify(await t.eval(`[...document.querySelectorAll('.sharing .dcard')].find(c => c.textContent.includes('hpc'))?.querySelectorAll('.chip').length`)) === "2");
  check("inbound: unreachable device says so", await t.eval(`[...document.querySelectorAll('.sharing .dcard')].some(c => c.textContent.includes("Joy") && c.querySelector('.derr'))`));
  const acts = await t.eval(`[...document.querySelectorAll('.sharing .arow .atext')].map(e => e.textContent.replace(/\\s+/g, ' ').trim())`);
  check("activity reads naturally, browsing collapsed", acts[0]?.startsWith("hpc opened /Downloads/invoice.pdf") && acts.some((a) => a.includes("browsed /Downloads ×2")), JSON.stringify(acts));
  check("failed access shows in activity", acts.some((a) => a.includes("couldn't saved") === false && a.includes("couldn't save")), JSON.stringify(acts));
  check("tab shows Sharing", (await t.eval(`document.querySelector('.tab.active, .tabs .active')?.textContent ?? ''`)).includes("Sharing"));
  if (out) await t.shot(out + "/sharing-page.png");
  // Open an inbound share.
  await t.clickText(".sharing .dcard .chip", "Projects");
  await sleep(800);
  check("clicking an inbound share opens it", JSON.stringify(await t.rows()).includes("thesis"), JSON.stringify(await t.rows()));
  await t.eval(`window.__cx.ws.activeTab.back()`);
  await sleep(600);
  // Show in folder from activity.
  await t.eval(`document.querySelector('.sharing .arow button').click()`);
  await sleep(800);
  check("activity 'show in folder' opens the folder with the file selected", (await t.eval(`JSON.stringify([window.__cx.ws.activeTab.folder.uri, [...window.__cx.ws.activeTab.selection]])`)) === '["file:///Users/demo/Downloads",["invoice.pdf"]]');
  await t.eval(`window.__cx.ws.activeTab.back()`);
  await sleep(600);
  // Stop sharing (hpc is connected → confirm).
  await t.eval(`document.querySelector('.sharing .srow .ibtn.danger').scrollIntoView({ block: 'center' })`);
  await t.click(".sharing .srow .ibtn.danger");
  await sleep(300);
  check("stopping asks first when a device is connected", (await t.eval(`document.querySelector('.modal h2')?.textContent`)) === "Stop sharing?");
  await t.clickText(".modal button", "Stop sharing");
  await sleep(400);
  check("share removed", (await t.eval(`document.querySelectorAll('.sharing .srow').length`)) === 1);
  check("command palette has the commands", await t.eval(`import('/src/lib/commands.svelte.ts').then(m => m.byId.has('file.share') && m.byId.has('go.sharing'))`));
  check("no uncaught errors", t.errors.length === 0, JSON.stringify(t.errors));
} finally {
  t.close();
  console.log(failures() ? `${failures()} FAILED` : "all passed");
  process.exit(failures() ? 1 : 0);
}
