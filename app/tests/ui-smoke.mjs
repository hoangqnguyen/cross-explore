// UI smoke test over the browser preview (mock backend): drives the real
// UI with keyboard and mouse the way a person would.
//   npm run dev & node tests/ui-smoke.mjs
import { checker, launch, MOD, sleep } from "./cdp.mjs";

const mac = process.platform === "darwin";
const M = mac ? MOD.meta : MOD.ctrl;
const { check, failures } = checker();
const t = await launch();

try {
  await t.open("?path=~/Downloads");
  const initial = await t.rows();
  check("lists the folder", initial.length === 10, JSON.stringify(initial));
  check("natural sort, case-insensitive", initial[0] === "cross-explore-0.1.0.dmg" && initial.indexOf("IMG_2041.HEIC") < initial.indexOf("Inter-4.0.zip"));
  check("breadcrumb shows path", (await t.eval(`[...document.querySelectorAll('.crumb')].map(c => c.textContent.trim()).join(' > ')`)) === "demo > Downloads");
  check("live indicator", (await t.eval(`document.querySelector('.state.live')?.textContent ?? ''`)).includes("Live"));

  await t.focusList();
  await t.key("ArrowDown");
  await t.key("ArrowDown");
  check("arrow keys move selection", JSON.stringify(await t.selected()) === JSON.stringify([initial[1]]));
  await t.key("ArrowDown", MOD.shift);
  check("shift+arrow extends selection", (await t.selected()).length === 2);

  await t.type("img");
  check("type-to-filter", JSON.stringify(await t.rows()) === JSON.stringify(["IMG_2041.HEIC", "IMG_2042.HEIC"]), JSON.stringify(await t.rows()));
  await t.key("Escape");
  check("escape clears filter", (await t.rows()).length === 10);

  // New folder via command bar → inline rename → commit.
  await t.clickText(".commands button", "New folder");
  await sleep(150);
  check("new folder enters rename mode", await t.eval(`document.activeElement?.classList.contains('rename') && document.activeElement.value === 'New folder'`));
  await t.eval(`document.activeElement.value = 'Receipts'`);
  await t.key("Enter");
  await sleep(200);
  check("renamed folder sorts first", (await t.rows())[0] === "Receipts", JSON.stringify(await t.rows()));

  // Undo the rename, then the folder creation.
  await t.focusList();
  await t.key("z", M);
  await sleep(200);
  check("undo rename", (await t.rows()).includes("New folder"), JSON.stringify(await t.rows()));

  // Live update from "someone else".
  let fresh = [];
  for (let i = 0; i < 90 && !fresh.some((n) => n.startsWith("Shared note")); i++) {
    await sleep(100);
    fresh = await t.eval(`[...document.querySelectorAll('.pane.active .row.fresh .text')].map(e => e.textContent)`);
  }
  check("live update appears with highlight", fresh.some((n) => n.startsWith("Shared note")), JSON.stringify(fresh));

  // Quick Look.
  await t.focusList();
  await t.key("End");
  await t.key(" ");
  await sleep(200);
  check("space opens Quick Look", await t.eval(`!!document.querySelector('.ql')`));
  const qlFirst = await t.eval(`document.querySelector('.ql .title strong')?.textContent`);
  await t.key("ArrowUp");
  await sleep(100);
  const qlTitle = await t.eval(`document.querySelector('.ql .title strong')?.textContent`);
  check("arrows walk items in Quick Look", !!qlTitle && qlTitle !== qlFirst, `${qlFirst} → ${qlTitle}`);
  await t.key("Escape");
  await sleep(100);
  check("Quick Look closes", !(await t.eval(`!!document.querySelector('.ql')`)));

  // Copy + paste duplicates through the transfer engine.
  await t.focusList();
  await t.key("Home");
  await t.key("ArrowDown");
  const picked = (await t.selected())[0];
  await t.key("c", M);
  await t.key("v", M);
  await sleep(1500);
  check("paste into same folder makes a copy", (await t.rows()).some((n) => n.includes(" - Copy")), JSON.stringify(await t.rows()));

  // Views.
  await t.key("1", mac ? MOD.meta : MOD.ctrl | MOD.shift);
  await t.key("2", mac ? MOD.meta : MOD.ctrl | MOD.shift);
  for (const [cmd, sel] of [["view.icons", ".icons"], ["view.columns", ".columns"], ["view.gallery", ".gallery"], ["view.details", ".details"]]) {
    await t.eval(`document.dispatchEvent(new CustomEvent('cx:palette'))`);
    await sleep(80);
    await t.type(cmd === "view.icons" ? "icons view" : cmd === "view.columns" ? "columns view" : cmd === "view.gallery" ? "gallery view" : "details view");
    await t.key("Enter");
    await sleep(250);
    check(`palette switches to ${cmd}`, await t.eval(`!!document.querySelector('.pane.active ${sel}')`));
  }

  // Dual pane + F5 in commander mode.
  await t.eval(`document.dispatchEvent(new CustomEvent('cx:palette'))`);
  await t.type("dual pane");
  await t.key("Enter");
  await sleep(300);
  check("dual pane shows two panes", (await t.eval(`document.querySelectorAll('.pane').length`)) === 2);
  check("each pane has tabs", (await t.eval(`document.querySelectorAll('.phead .tab').length`)) >= 2);

  // Navigate with Enter into a folder, then up.
  await t.open("?path=~/Documents");
  await t.focusList();
  await t.key("Home");
  await t.key(mac ? "ArrowDown" : "Enter", mac ? MOD.meta : 0);
  await sleep(300);
  check("open folder", (await t.eval(`document.querySelector('.crumb.current').textContent.trim()`)) === "Invoices");
  await t.key("ArrowUp", mac ? MOD.meta : MOD.alt);
  await sleep(300);
  check("up selects the folder we came from", JSON.stringify(await t.selected()) === '["Invoices"]', JSON.stringify(await t.selected()));

  // Copy to many destinations, move to one.
  await t.open("?path=~/Downloads");
  await t.eval(`(() => { const tab = window.__cx.ws.activeTab; tab.selection = new Set(['dataset.csv', 'setup.sh']); tab.cursor = 'dataset.csv'; })()`);
  await t.eval(`(() => { const r = [...document.querySelectorAll('.pane.active .row')].find(r => r.textContent.includes('dataset.csv')).getBoundingClientRect(); [...document.querySelectorAll('.pane.active .row')].find(r => r.textContent.includes('dataset.csv')).dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: r.left + 60, clientY: r.top + 10 })); })()`);
  await sleep(150);
  check("context menu has Copy to… and Move to…", await t.eval(`['Copy to…', 'Move to…'].every(l => [...document.querySelectorAll('.menu button')].some(b => b.textContent.includes(l)))`));
  await t.clickText(".menu button", "Copy to…");
  await sleep(200);
  check("destination picker opens", (await t.eval(`document.querySelector('.modal h2')?.textContent ?? ''`)).includes("Copy 2 items to"));
  await t.clickText(".modal .place", "Documents");
  await t.clickText(".modal .place", "Desktop");
  check("button says copy to 2 places", (await t.eval(`document.querySelector('.modal footer .btn.primary').textContent`)).includes("2 places"));
  await t.clickText(".modal footer .btn.primary", "Copy to 2 places");
  let both = false;
  for (let i = 0; i < 60 && !both; i++) {
    await sleep(200);
    both = await t.eval(`(async () => { const { listDir } = await import('/src/lib/api.ts'); const names = async (u) => { const n = []; await listDir(u, (e) => e.type === 'batch' && n.push(...e.entries.map(x => x.name))); return n; }; const d = await names('~/Documents'); const k = await names('~/Desktop'); return ['dataset.csv','setup.sh'].every(f => d.includes(f) && k.includes(f)); })()`);
  }
  check("both files copied to both destinations", both);
  await t.eval(`(() => { const tab = window.__cx.ws.activeTab; tab.selectOnly('podcast-ep12.mp3'); })()`);
  await t.eval(`window.__cx.ws.activeTab && document.dispatchEvent(new CustomEvent('cx:palette'))`);
  await sleep(100);
  await t.type("move to");
  await t.key("Enter");
  await sleep(200);
  await t.clickText(".modal .place", "Music");
  await t.clickText(".modal .place", "Movies");
  check("move allows a single destination", (await t.eval(`document.querySelectorAll('.modal .place.on').length`)) === 1);
  await t.clickText(".modal footer .btn.primary", "Move");
  for (let i = 0; i < 40 && (await t.rows()).includes("podcast-ep12.mp3"); i++) await sleep(200);
  check("moved file left the folder", !(await t.rows()).includes("podcast-ep12.mp3"));
  await t.open("?path=~/Documents");

  // Finder-style outline: expand folders in place.
  await t.eval(`document.querySelector('.pane.active .row .disclosure').click()`);
  await sleep(300);
  check("disclosure expands a folder in place", (await t.rows()).includes("Invoice-2026-001.pdf"));
  await t.focusList();
  await t.key("ArrowDown");
  await t.key("ArrowLeft");
  check("← jumps to the containing folder", JSON.stringify(await t.selected()) === '["Invoices"]', JSON.stringify(await t.selected()));
  await t.key("ArrowLeft");
  check("← collapses it", !(await t.rows()).includes("Invoice-2026-001.pdf"));
  await t.key("ArrowRight");
  await sleep(300);
  check("→ expands it again", (await t.rows()).includes("Invoice-2026-001.pdf"));
  await t.key("ArrowLeft");

  // Search subfolders.
  await t.eval(`document.dispatchEvent(new CustomEvent('cx:focus-search'))`);
  await t.type("invoice-2026-00");
  await t.key("Enter", M);
  await sleep(400);
  check("recursive search finds nested files", (await t.rows()).length === 9, JSON.stringify(await t.rows()));

  // Tabs.
  await t.focusList();
  await t.key("t", M);
  await sleep(200);
  check("new tab", (await t.eval(`document.querySelectorAll('.titlebar .tab').length`)) === 2);
  await t.key("w", M);
  await sleep(200);
  check("close tab", (await t.eval(`document.querySelectorAll('.titlebar .tab').length`)) === 1);

  // NAS needs a password.
  await t.open("?path=smb://nas.local/Media");
  await sleep(300);
  check("sign-in dialog for protected share", await t.eval(`document.querySelector('.modal h2')?.textContent.includes('Sign in')`));
  await t.eval(`(() => { const [u, p] = document.querySelectorAll('.modal input'); u.value = 'demo'; u.dispatchEvent(new Event('input')); p.value = 'demo'; p.dispatchEvent(new Event('input')); })()`);
  await t.key("Enter");
  await sleep(600);
  check("share lists after sign in", (await t.rows()).includes("Movies"), JSON.stringify(await t.rows()));
  check("remote folder shows auto-refresh", (await t.eval(`document.querySelector('.state.polling')?.textContent ?? ''`)).includes("Auto"));

  // Drag a selection rectangle across row whitespace.
  await t.open("?path=~/Downloads");
  {
    const pts = await t.eval(`(() => { const r = [...document.querySelectorAll('.pane.active .details .row')]; const a = r[1].getBoundingClientRect(), b = r[4].getBoundingClientRect(); return { x: a.right - 20, y1: a.top + a.height / 2, y2: b.top + b.height / 2 }; })()`);
    await t.send("Input.dispatchMouseEvent", { type: "mousePressed", x: pts.x, y: pts.y1, button: "left", buttons: 1, clickCount: 1 });
    for (let i = 1; i <= 6; i++) await t.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: pts.x - i * 30, y: pts.y1 + ((pts.y2 - pts.y1) * i) / 6, button: "left", buttons: 1 });
    check("marquee is drawn while dragging", await t.eval(`!!document.querySelector('.pane.active .marquee')`));
    await t.send("Input.dispatchMouseEvent", { type: "mouseReleased", x: pts.x - 180, y: pts.y2, button: "left", buttons: 0, clickCount: 1 });
    await sleep(100);
    const sel = await t.selected();
    check("marquee selects the rows it crosses", sel.length === 4, JSON.stringify(sel));
    check("marquee disappears on release", !(await t.eval(`!!document.querySelector('.pane.active .marquee')`)));
  }

  // Finder and Explorer keymaps differ: Enter renames in Finder, opens in Explorer.
  await t.eval(`window.__cx.settings.data.keymap = 'finder'`);
  await t.focusList();
  await t.key("Home");
  await t.key("Enter");
  await sleep(150);
  check("Finder: Enter renames", await t.eval(`!!document.activeElement?.classList.contains('rename')`));
  await t.key("Escape");
  await t.eval(`window.__cx.settings.data.keymap = 'explorer'`);
  await t.focusList();
  await t.key("F2");
  await sleep(150);
  check("Explorer: F2 renames", await t.eval(`!!document.activeElement?.classList.contains('rename')`));
  await t.key("Escape");

  // Cloud account: paste a client ID, sign in through the (mock) browser, lands in the sidebar.
  await t.eval(`void window.__cx.dialogs.ask("cloud", { service: "dropbox", navigate: false })`);
  await sleep(300);
  check("cloud dialog asks for the App key", (await t.eval(`document.querySelector('.modal')?.textContent ?? ''`)).includes("App key"));
  await t.eval(`(() => { const i = document.querySelector('.modal input[type=text]'); i.value = 'demo-key'; i.dispatchEvent(new Event('input')); })()`);
  await t.clickText(".modal button", "Sign in with browser");
  await sleep(900);
  check("signed-in account is added to the sidebar", (await t.eval(`[...document.querySelectorAll('.sidebar .item .label')].map(e => e.textContent).join('|')`)).includes("Dropbox (demo@example.com)"));
  check("cloud dialog closes after sign-in", !(await t.eval(`!!document.querySelector('.modal')`)));

  // Trash.
  await t.open("?path=~/Downloads");
  await t.focusList();
  await t.key("Home");
  await t.key(mac ? "Backspace" : "Delete", mac ? MOD.meta : 0);
  await sleep(300);
  check("move to trash removes the row", !(await t.rows()).includes("cross-explore-0.1.0.dmg"));
  check("toast confirms", (await t.eval(`document.querySelector('.toast')?.textContent ?? ''`)).includes("Moved"));

  // Right-click → Delete permanently… → confirm.
  {
    const victim = (await t.rows())[1];
    await t.eval(`(() => { const r = [...document.querySelectorAll('.pane.active .details .row')].find(r => r.querySelector('.text')?.textContent === ${JSON.stringify(victim)}); r.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, button: 0 })); r.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 300, clientY: 300 })); })()`);
    await sleep(150);
    const labels = await t.eval(`[...document.querySelectorAll('.menu [role=menuitem], .menu button')].map(b => b.textContent.trim())`);
    check("context menu offers Delete permanently", labels.some((l) => l.startsWith("Delete permanently")), JSON.stringify(labels));
    await t.clickText(".menu button, .menu [role=menuitem]", "Delete permanently");
    await sleep(200);
    check("permanent delete asks first", (await t.eval(`document.querySelector('.modal')?.textContent ?? ''`)).includes("can't undo"));
    await t.clickText(".modal button", "Delete");
    await sleep(400);
    check("permanent delete removes the row", !(await t.rows()).includes(victim), victim);
  }

  // Icons view: two-line names are never cut off, at any icon size.
  await t.open("?path=~/Downloads");
  await t.eval(`window.__cx.ws.activeTab.view = "icons"`);
  for (const size of [48, 64, 96, 128, 256]) {
    await t.eval(`window.__cx.settings.data.iconSize = ${size}`);
    await sleep(200);
    const clipped = await t.eval(`[...document.querySelectorAll(".pane.active .cell")].filter((c) => { const n = c.querySelector(".name"); return n.getBoundingClientRect().bottom > c.getBoundingClientRect().bottom + 0.5 || (n.scrollHeight > 20 && n.clientHeight < 31); }).length`);
    check(`icons view names fit at ${size}px`, clipped === 0, `${clipped} clipped`);
  }

  // Copy to… → browse → Drives lists network devices; sign in inline and browse a share.
  await t.open("?path=~");
  await sleep(1000);
  await t.eval(`void window.__cx.dialogs.ask("destination", { uris: ["file:///Users/demo/Downloads"], mode: "copy" })`);
  await sleep(300);
  await t.clickText(".modal button", "Choose another folder");
  await sleep(300);
  await t.click('.modal [aria-label="Drives"]');
  await sleep(300);
  {
    const sections = await t.eval(`[...document.querySelectorAll('.modal .bsection')].map(e => e.textContent)`);
    check("browser Drives view has Drives, Cloud and Network", JSON.stringify(sections) === '["Drives","Cloud","Network"]', JSON.stringify(sections));
    check("network devices are listed", (await t.eval(`[...document.querySelectorAll('.modal .dir')].map(e => e.textContent).join('|')`)).includes("synology"));
    await t.clickText(".modal .dir", "synology");
    await sleep(600);
    check("server that needs a password asks inline", await t.eval(`!!document.querySelector('.modal .signin input[type=password]')`));
    await t.eval(`(() => { const [u, p] = document.querySelectorAll('.modal .signin input'); u.value = 'demo'; u.dispatchEvent(new Event('input')); p.value = 'demo'; p.dispatchEvent(new Event('input')); })()`);
    await t.clickText(".modal .signin button", "Sign in");
    await sleep(900);
    const listed = await t.eval(`[...document.querySelectorAll('.modal .blist .dir')].map(e => e.textContent.trim())`);
    check("after signing in, the server's folders list", listed.includes("Media"), JSON.stringify(listed));
    await t.eval(`window.__cx.dialogs.close(null)`);
  }

  // File icons: programs get app tiles, documents get pages with their own look.
  await t.open("?path=~/Code/icon-gallery");
  {
    const shape = async (name) => {
      await t.eval(`window.__cx.ws.activeTab.filter = ${JSON.stringify(name)}`);
      await sleep(100);
      return shapeOf(name);
    };
    const shapeOf = (name) => t.eval(`(() => { const r = [...document.querySelectorAll('.pane.active .details .row')].find((r) => r.querySelector('.text')?.textContent === ${JSON.stringify(name)}); const svg = r?.querySelector('svg:not(.disclosure svg)'); return !svg ? 'none' : svg.classList.contains('tile') ? 'tile' : 'page'; })()`);
    check("an .exe gets an app tile", (await shape("PawnIO_setup.exe")) === "tile");
    check("an executable without extension gets a program tile", (await shape("cx-helper")) === "tile");
    check("a PDF is a page", (await shape("invoice.pdf")) === "page");
    await t.eval(`window.__cx.ws.activeTab.filter = ""`);
    await sleep(100);
    const looks = await t.eval(`new Set([...document.querySelectorAll('.pane.active .details .row')].map((r) => r.querySelector('.cell.name svg')?.innerHTML ?? '')).size`);
    const count = (await t.rows()).length;
    check("file types look different from each other", looks > count * 0.8, `${looks} distinct icons for ${count} files`);
  }

  // Right-click a folder → Add to Favorites → it shows in the sidebar; again → removed.
  await t.open("?path=~");
  {
    const menuOn = async (name) => {
      await t.eval(`(() => { const r = [...document.querySelectorAll('.pane.active .details .row')].find(r => r.querySelector('.text')?.textContent === ${JSON.stringify(name)}); r.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, button: 0 })); r.dispatchEvent(new MouseEvent('pointerup', { bubbles: true, button: 0 })); r.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 300, clientY: 300 })); })()`);
      await sleep(150);
      return t.eval(`[...document.querySelectorAll('.menu button, .menu [role=menuitem]')].map(b => b.textContent.trim())`);
    };
    const favs = () => t.eval(`[...document.querySelectorAll('.sidebar .item .label')].map(e => e.textContent)`);
    const labels = await menuOn("Code");
    check("folder context menu offers Add to Favorites", labels.some((l) => l.startsWith("Add to Favorites")), JSON.stringify(labels));
    await t.clickText(".menu button, .menu [role=menuitem]", "Add to Favorites");
    await sleep(150);
    check("the folder appears in the sidebar", (await favs()).includes("Code"), JSON.stringify(await favs()));
    // Give it an alias from the sidebar's own menu.
    await t.eval(`(() => { const el = [...document.querySelectorAll('.sidebar .item')].find(e => e.querySelector('.label')?.textContent === 'Code'); el.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 60, clientY: 300 })); })()`);
    await sleep(150);
    await t.clickText(".menu button, .menu [role=menuitem]", "Rename…");
    await sleep(200);
    check("renaming a favorite asks for a name", (await t.eval(`document.querySelector('.modal input')?.value`)) === "Code");
    await t.eval(`(() => { const i = document.querySelector('.modal input'); i.value = 'My code'; i.dispatchEvent(new Event('input')); })()`);
    await t.key("Enter");
    await sleep(200);
    check("the favorite shows its alias", (await favs()).includes("My code") && !(await favs()).includes("Code"), JSON.stringify(await favs()));
    check("the alias still opens the same folder", (await t.eval(`window.__cx.settings.data.bookmarks.find(b => b.name === 'My code')?.uri`))?.endsWith("/Code"));
    const again = await menuOn("Code");
    check("then offers Remove from Favorites", again.some((l) => l.startsWith("Remove from Favorites")), JSON.stringify(again));
    await t.clickText(".menu button, .menu [role=menuitem]", "Remove from Favorites");
    await sleep(150);
    check("and removing takes it off the sidebar", !(await favs()).includes("My code"));
  }

  // Column view: right-click works in every column, not just the focused one.
  await t.open("?path=~/Documents");
  await t.eval(`window.__cx.ws.activeTab.view = "columns"`);
  await sleep(500);
  {
    const menuVisible = () => t.eval(`!!document.querySelector('.menu')`);
    const closeMenu = async () => { await t.key("Escape"); await sleep(100); };
    const rclick = async (sel, text) => {
      const box = await t.eval(`(() => { const el = [...document.querySelectorAll(${JSON.stringify(sel)})].find(e => ${text ? `(e.querySelector('span')?.textContent ?? e.textContent).trim() === ${JSON.stringify(text)}` : "true"}); if (!el) return null; el.scrollIntoView({ block: 'nearest' }); const r = el.getBoundingClientRect(); return { x: r.left + Math.min(40, r.width / 2), y: r.top + Math.min(10, r.height / 2) }; })()`);
      if (!box) return false;
      for (const type of ["mousePressed", "mouseReleased"]) await t.send("Input.dispatchMouseEvent", { type, x: box.x, y: box.y, button: "right", buttons: type === "mousePressed" ? 2 : 0, clickCount: 1 });
      await sleep(400);
      return true;
    };
    await rclick(".columns .column.current .item", "Resume.docx");
    check("right-click in the focused column shows the menu", await menuVisible());
    await closeMenu();
    await rclick(".columns .column:not(.current):not(.preview) .item", "Desktop");
    check("right-click in a parent column shows the menu", await menuVisible());
    check("…for the item under the pointer, now selected", JSON.stringify(await t.eval(`[...document.querySelectorAll('.columns .column.current .item.selected span')].map(e => e.textContent)`)) === '["Desktop"]');
    await closeMenu();
    await t.eval(`window.__cx.ws.activeTab.navigate(window.__cx.ws.activeTab.dirUri, "Documents")`);
    await sleep(300);
    await rclick(".columns .column.current ~ .column:not(.preview) .item", "Invoices");
    check("right-click in the next-folder column shows the menu", await menuVisible());
    await closeMenu();
    await rclick(".columns .filler");
    check("right-click on the empty area shows the folder menu", await menuVisible());
    await closeMenu();
  }

  // Background refreshes are seamless: no spinner, rows never disappear
  // (a normal reload of the same remote folder does show the spinner).
  await t.open("?path=smb://nas.local/Media");
  await sleep(400);
  await t.eval(`(() => { const [u, p] = document.querySelectorAll('.modal input'); u.value = 'demo'; u.dispatchEvent(new Event('input')); p.value = 'demo'; p.dispatchEvent(new Event('input')); })()`);
  await t.key("Enter");
  await sleep(800);
  {
    const watchFlash = async (quiet) => {
      await t.eval(`(() => { window.__flash = { spinner: 0, empty: 0 }; window.__watching = true; const tick = () => { if (!window.__watching) return; if (document.querySelector('.state .spinner')) window.__flash.spinner++; if (!document.querySelector('.pane.active .details .row')) window.__flash.empty++; requestAnimationFrame(tick); }; requestAnimationFrame(tick); })()`);
      await t.eval(`window.__cx.ws.activeTab.folder.load({ quiet: ${quiet} })`);
      await sleep(400);
      await t.eval(`window.__watching = false`);
      return t.eval(`window.__flash`);
    };
    const loud = await watchFlash(false);
    const quiet = await watchFlash(true);
    check("a normal reload shows the spinner (detector works)", loud.spinner > 0, JSON.stringify(loud));
    check("a background refresh shows no spinner", quiet.spinner === 0, JSON.stringify(quiet));
    check("…and the rows never disappear", quiet.empty === 0, JSON.stringify(quiet));
  }

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
