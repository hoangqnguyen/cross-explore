<script lang="ts">
  // Total Commander's multi-rename tool: masks with tokens, search & replace,
  // case changes and a counter, with a live preview before anything changes.
  import { errorText, renameEntry, type Item, type UndoOp } from "../api";
  import { keyOf } from "../folder.svelte";
  import { dialogs } from "../stores/dialogs.svelte";
  import { transfers } from "../stores/transfers.svelte";
  import { toasts } from "../toasts.svelte";
  import type { Tab } from "../workspace.svelte";
  import Modal from "./Modal.svelte";

  let { tab }: { tab: Tab } = $props();

  // svelte-ignore state_referenced_locally
  const items: Item[] = tab.selectedEntries.length > 1 ? tab.selectedEntries : tab.visible;
  let nameMask = $state("[N]");
  let extMask = $state("[E]");
  let search = $state("");
  let replace = $state("");
  let regex = $state(false);
  let caseMode = $state<"keep" | "lower" | "upper" | "title">("keep");
  let start = $state(1);
  let step = $state(1);
  let digits = $state(2);
  let busy = $state(false);

  const pad = (n: number, d: number) => String(n).padStart(d, "0");

  function split(name: string, isDir: boolean): [string, string] {
    const i = name.lastIndexOf(".");
    return !isDir && i > 0 ? [name.slice(0, i), name.slice(i + 1)] : [name, ""];
  }

  function applyMask(mask: string, stem: string, ext: string, i: number, e: Item) {
    const d = e.modified ? new Date(e.modified) : null;
    return mask.replace(/\[(N|E)(\d+)?(?:-(\d+))?\]|\[C\]|\[YMD\]|\[hms\]|\[P\]/g, (m, t, a, b) => {
      if (m === "[C]") return pad(start + i * step, digits);
      if (m === "[YMD]") return d ? `${d.getFullYear()}-${pad(d.getMonth() + 1, 2)}-${pad(d.getDate(), 2)}` : "";
      if (m === "[hms]") return d ? `${pad(d.getHours(), 2)}.${pad(d.getMinutes(), 2)}.${pad(d.getSeconds(), 2)}` : "";
      if (m === "[P]") return tab.title;
      const src = t === "N" ? stem : ext;
      if (!a) return src;
      const from = Number(a) - 1;
      return b ? src.slice(from, Number(b)) : src.charAt(from);
    });
  }

  function transform(e: Item, i: number): string {
    const [stem, ext] = split(e.name, e.isDir);
    let name = applyMask(nameMask, stem, ext, i, e);
    const newExt = e.isDir ? "" : applyMask(extMask, stem, ext, i, e);
    let full = newExt ? `${name}.${newExt}` : name;
    if (search) {
      try {
        full = regex ? full.replace(new RegExp(search, "g"), replace) : full.split(search).join(replace);
      } catch {
        /* incomplete regex while typing */
      }
    }
    if (caseMode === "lower") full = full.toLowerCase();
    else if (caseMode === "upper") full = full.toUpperCase();
    else if (caseMode === "title") full = full.toLowerCase().replace(/(^|[\s_\-.(])(\p{L})/gu, (_, p, c) => p + c.toUpperCase());
    return full;
  }

  let preview = $derived(items.map((e, i) => ({ e, to: transform(e, i) })));
  let problems = $derived.by(() => {
    const counts = new Map<string, number>();
    for (const p of preview) counts.set(p.to.toLowerCase(), (counts.get(p.to.toLowerCase()) ?? 0) + 1);
    const renaming = new Set(items.map((e) => e.name.toLowerCase()));
    const others = new Set(tab.folder.items.map((e) => e.name.toLowerCase()).filter((n) => !renaming.has(n)));
    return new Map(preview.map((p) => [p.e.name, !p.to || /[/\0]/.test(p.to) ? "invalid" : (counts.get(p.to.toLowerCase()) ?? 0) > 1 || others.has(p.to.toLowerCase()) ? "duplicate" : ""]));
  });
  let changes = $derived(preview.filter((p) => p.to !== p.e.name));
  let blocked = $derived([...problems.values()].some(Boolean));

  async function apply() {
    busy = true;
    const done: UndoOp[] = [];
    const dirOf = (e: Item) => e.parent ?? tab.dirUri;
    try {
      // Two phases so swaps (a→b, b→a) never collide.
      const temp = changes.map((p, i) => ({ ...p, tmp: `.cx-rename-${Date.now()}-${i}` }));
      for (const p of temp) {
        await renameEntry(dirOf(p.e), p.e.name, p.tmp);
      }
      for (const p of temp) {
        await renameEntry(dirOf(p.e), p.tmp, p.to);
        done.push({ type: "rename", dir: dirOf(p.e), from: p.e.name, to: p.to });
      }
      transfers.pushUndo(`Rename ${done.length} items`, { type: "batch", ops: done });
      toasts.show(`Renamed ${done.length} ${done.length === 1 ? "item" : "items"}`);
      tab.selection = new Set(changes.map((p) => (p.e.uri ? p.e.uri.replace(/[^/]*$/, encodeURIComponent(p.to)) : p.to)));
      if (tab.folder.kind !== "folder") tab.reload();
      dialogs.close(true);
    } catch (e) {
      toasts.show(`Rename stopped: ${errorText(e)}`, "error");
      busy = false;
    }
  }
</script>

<Modal title="Rename {items.length} items" width={760} onsubmit={() => !blocked && changes.length && apply()}>
  <div class="grid">
    <label class="field">Name<input type="text" bind:value={nameMask} spellcheck="false" /></label>
    <label class="field">Extension<input type="text" bind:value={extMask} spellcheck="false" /></label>
    <label class="field">Find<input type="text" bind:value={search} spellcheck="false" /></label>
    <label class="field">Replace with<input type="text" bind:value={replace} spellcheck="false" /></label>
  </div>
  <div class="opts">
    <label class="check"><input type="checkbox" bind:checked={regex} /> Regular expression</label>
    <label class="field inline">Case
      <select bind:value={caseMode}>
        <option value="keep">Unchanged</option>
        <option value="lower">lowercase</option>
        <option value="upper">UPPERCASE</option>
        <option value="title">Title Case</option>
      </select>
    </label>
    <label class="field inline">Counter from<input type="number" bind:value={start} min="0" /></label>
    <label class="field inline">step<input type="number" bind:value={step} min="1" /></label>
    <label class="field inline">digits<input type="number" bind:value={digits} min="1" max="9" /></label>
  </div>
  <div class="tokens">
    {#each [["[N]", "name"], ["[N1-3]", "name chars 1–3"], ["[E]", "extension"], ["[C]", "counter"], ["[YMD]", "date"], ["[hms]", "time"], ["[P]", "folder"]] as [t, label]}
      <button type="button" title={label} onclick={() => (nameMask += t)}>{t} <span>{label}</span></button>
    {/each}
  </div>
  <div class="table">
    {#each preview as p (keyOf(p.e))}
      {@const problem = problems.get(p.e.name)}
      <div class="tr" class:changed={p.to !== p.e.name} class:bad={!!problem}>
        <span class="old">{p.e.name}</span>
        <span class="arrow">→</span>
        <span class="new">{p.to}</span>
        {#if problem}<span class="why">{problem === "duplicate" ? "name taken" : "invalid"}</span>{/if}
      </div>
    {/each}
  </div>
  {#snippet footer()}
    <span class="muted summary">{changes.length} will change</span>
    <button type="button" class="btn" onclick={() => dialogs.close(null)}>Cancel</button>
    <button type="submit" class="btn primary" disabled={busy || blocked || !changes.length}>{busy ? "Renaming…" : "Rename"}</button>
  {/snippet}
</Modal>

<style>
  .grid {
    display: grid;
    grid-template-columns: 2fr 1fr 1.5fr 1.5fr;
    gap: 10px;
  }
  .opts {
    display: flex;
    align-items: center;
    gap: 14px;
    flex-wrap: wrap;
  }
  .inline {
    flex-direction: row !important;
    align-items: center;
    margin: 0 !important;
  }
  .inline input {
    width: 64px;
  }
  .tokens {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 10px 0;
  }
  .tokens button {
    padding: 3px 8px;
    border-radius: 10px;
    background: var(--hover);
    font-size: 11.5px;
    font-family: ui-monospace, Menlo, monospace;
  }
  .tokens button span {
    font-family: var(--font);
    color: var(--text-3);
  }
  .table {
    max-height: 280px;
    overflow: auto;
    border-radius: var(--radius);
    box-shadow: inset 0 0 0 1px var(--stroke);
    padding: 4px;
  }
  .tr {
    display: grid;
    grid-template-columns: 1fr 24px 1fr auto;
    align-items: center;
    height: 26px;
    padding: 0 8px;
    font-size: 12.5px;
    border-radius: 4px;
  }
  .tr span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .old {
    color: var(--text-2);
  }
  .arrow {
    color: var(--text-3);
    text-align: center;
  }
  .changed .new {
    color: var(--accent);
    font-weight: 500;
  }
  .bad {
    background: color-mix(in srgb, var(--danger) 12%, transparent);
  }
  .why {
    color: var(--danger);
    font-size: 11px;
    padding-left: 8px;
  }
  .summary {
    margin-right: auto;
    align-self: center;
  }
</style>
