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

  // Trash.
  await t.open("?path=~/Downloads");
  await t.focusList();
  await t.key("Home");
  await t.key(mac ? "Backspace" : "Delete", mac ? MOD.meta : 0);
  await sleep(300);
  check("move to trash removes the row", !(await t.rows()).includes("cross-explore-0.1.0.dmg"));
  check("toast confirms", (await t.eval(`document.querySelector('.toast')?.textContent ?? ''`)).includes("Moved"));

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
