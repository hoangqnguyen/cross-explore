// Screenshots of the main screens for design review.
//   node tests/ui-shots.mjs [outdir]
import { launch, sleep, MOD } from "./cdp.mjs";

const out = process.argv[2] ?? "/tmp/cx-shots";
await import("node:fs").then((fs) => fs.mkdirSync(out, { recursive: true }));
const mac = process.platform === "darwin";
const t = await launch({ width: 1320, height: 820 });
const palette = async (q) => {
  await t.eval(`document.dispatchEvent(new CustomEvent('cx:palette'))`);
  await sleep(100);
  await t.type(q);
  await t.key("Enter");
  await sleep(400);
};

await t.open("?path=~/Downloads");
await t.focusList();
await t.key("ArrowDown");
await t.key("ArrowDown");
await t.shot(`${out}/01-details.png`);

await palette("toggle preview pane");
await t.shot(`${out}/02-preview-pane.png`);
await palette("toggle preview pane");

await t.open("?path=~/Pictures");
await palette("icons view");
await t.shot(`${out}/03-icons.png`);
await palette("gallery view");
await t.shot(`${out}/04-gallery.png`);

await t.open("?path=~/Documents/Projects");
await palette("columns view");
await t.focusList();
await t.key("ArrowDown");
await t.shot(`${out}/05-columns.png`);

await palette("details view");
await t.focusList();
await t.key("Home");
await t.key(" ");
await sleep(400);
await t.shot(`${out}/06-quicklook-markdown.png`);
await t.key("Escape");

await t.open("?path=~/Downloads");
await palette("toggle dual pane");
await t.shot(`${out}/07-dual.png`);
await palette("toggle dual pane");

await t.open("?path=cx:home");
await sleep(900);
await t.shot(`${out}/08-home.png`);

await t.open("?path=~/Downloads");
await t.eval(`document.dispatchEvent(new CustomEvent('cx:palette'))`);
await sleep(100);
await t.type("co");
await sleep(200);
await t.shot(`${out}/09-palette.png`);
await t.key("Escape");

// A copy with a conflict, then the transfers flyout.
await t.focusList();
await t.key("Home");
await t.key("c", mac ? MOD.meta : MOD.ctrl);
await t.open("?path=~/Downloads", false);
await t.eval(`(async () => { const { transfers } = window.__cx; await transfers.submit({ kind: 'copy', sources: ['file:///Users/demo/Downloads/lecture-07.mp4', 'file:///Users/demo/Downloads/holiday-photos.zip'], dest: 'file:///Users/demo/Desktop' }); await transfers.submit({ kind: 'copy', sources: ['file:///Users/demo/Desktop/todo.md'], dest: 'file:///Users/demo/Documents' }); transfers.flyoutOpen = true; })()`);
await sleep(900);
await t.shot(`${out}/10-transfers.png`);

await t.open("?path=~/Desktop");
await t.eval(`(async () => { const { transfers } = window.__cx; await transfers.submit({ kind: 'copy', sources: ['file:///Users/demo/Downloads/setup.sh'], dest: 'file:///Users/demo/Desktop' }); })()`);
await sleep(2500);
await t.eval(`(async () => { const { transfers } = window.__cx; await transfers.submit({ kind: 'copy', sources: ['file:///Users/demo/Downloads/setup.sh'], dest: 'file:///Users/demo/Desktop' }); })()`);
await sleep(600);
await t.shot(`${out}/11-conflict.png`);

await t.open("?path=~/Downloads");
await palette("settings");
await t.shot(`${out}/12-settings.png`);
await t.key("Escape");

await palette("connect to server");
await t.shot(`${out}/13-connect.png`);
await t.key("Escape");

await t.focusList();
await t.key("a", mac ? MOD.meta : MOD.ctrl);
await palette("rename multiple");
await t.eval(`(() => { const i = document.querySelector('.modal input'); i.value = 'Download [C] [N]'; i.dispatchEvent(new Event('input')); })()`);
await sleep(200);
await t.shot(`${out}/14-multirename.png`);
await t.key("Escape");

await t.open("?path=~/Downloads");
await t.focusList();
await t.key("ArrowDown");
await t.eval(`(() => { const r = document.querySelectorAll('.pane.active .row')[2].getBoundingClientRect(); document.querySelectorAll('.pane.active .row')[2].dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: r.left + 120, clientY: r.top + 10 })); })()`);
await sleep(200);
await t.shot(`${out}/15-context-menu.png`);

await t.open("?path=~/Downloads", true);
await t.eval(`localStorage.clear()`);
await t.send("Page.navigate", { url: "http://localhost:1420/" });
await sleep(1200);
await t.shot(`${out}/16-onboarding.png`);

console.log("errors:", t.errors);
t.close();
