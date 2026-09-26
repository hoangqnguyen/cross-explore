<script lang="ts">
  // Total Commander's "Synchronize dirs": compare two folders (any mix of
  // local, network and peer locations) and copy what's missing or newer.
  import { compareDirs, errorText, uriName, type DiffItem, type DiffKind } from "../api";
  import { formatDate, formatSize } from "../format";
  import { transfers } from "../stores/transfers.svelte";
  import { toasts } from "../toasts.svelte";
  import type { Tab } from "../workspace.svelte";
  import FileIcon from "./FileIcon.svelte";
  import Icon from "./Icon.svelte";

  let { tab }: { tab: Tab } = $props();

  let params = $derived(new URLSearchParams(tab.folder.uri.slice(tab.folder.uri.indexOf("?") + 1)));
  let left = $derived(params.get("left") ?? "");
  let right = $derived(params.get("right") ?? "");
  let items = $state<DiffItem[] | null>(null);
  let error = $state<string | null>(null);
  let byContent = $state(false);
  let showSame = $state(false);

  async function load() {
    items = null;
    error = null;
    try {
      items = await compareDirs(left, right, byContent);
    } catch (e) {
      error = errorText(e);
    }
  }

  $effect(() => {
    void left;
    void right;
    void byContent;
    void load();
  });

  let shown = $derived((items ?? []).filter((i) => showSame || i.kind !== "same"));
  let counts = $derived(
    (items ?? []).reduce(
      (c, i) => {
        c[i.kind] = (c[i.kind] ?? 0) + 1;
        return c;
      },
      {} as Partial<Record<DiffKind, number>>,
    ),
  );

  const symbols: Record<DiffKind, { icon: "forward" | "back" | "diff" | "check"; label: string; tone: string }> = {
    leftOnly: { icon: "forward", label: "Only on the left", tone: "left" },
    rightOnly: { icon: "back", label: "Only on the right", tone: "right" },
    leftNewer: { icon: "forward", label: "Newer on the left", tone: "left" },
    rightNewer: { icon: "back", label: "Newer on the right", tone: "right" },
    different: { icon: "diff", label: "Different", tone: "diff" },
    same: { icon: "check", label: "Identical", tone: "same" },
  };

  function join(base: string, rel: string) {
    return base.replace(/\/+$/, "") + "/" + rel.split("/").map(encodeURIComponent).join("/");
  }

  async function sync(direction: "right" | "left" | "both") {
    const plans: { from: string; to: string; kinds: DiffKind[] }[] = [];
    if (direction !== "left") plans.push({ from: left, to: right, kinds: ["leftOnly", "leftNewer"] });
    if (direction !== "right") plans.push({ from: right, to: left, kinds: ["rightOnly", "rightNewer"] });
    let jobs = 0;
    for (const p of plans) {
      // One copy job per destination folder.
      const byDir = new Map<string, string[]>();
      for (const i of items ?? []) {
        if (!p.kinds.includes(i.kind)) continue;
        const dir = i.relPath.includes("/") ? i.relPath.slice(0, i.relPath.lastIndexOf("/")) : "";
        const dest = dir ? join(p.to, dir) : p.to;
        byDir.set(dest, [...(byDir.get(dest) ?? []), join(p.from, i.relPath)]);
      }
      for (const [dest, sources] of byDir) {
        await transfers.submit({ kind: "copy", sources, dest, conflict: "replace" });
        jobs++;
      }
    }
    if (!jobs) toasts.show("Nothing to copy");
    else transfers.flyoutOpen = true;
  }
</script>

<div class="compare">
  <div class="bar">
    <div class="side"><Icon name="folder" size={14} /> {uriName(left) || left}</div>
    <div class="mid">
      <button onclick={() => sync("right")} title="Copy missing and newer files to the right"><Icon name="forward" size={14} /> Update right</button>
      <button onclick={() => sync("both")} title="Copy missing and newer files both ways"><Icon name="sync" size={14} /> Sync both</button>
      <button onclick={() => sync("left")} title="Copy missing and newer files to the left"><Icon name="back" size={14} /> Update left</button>
    </div>
    <div class="side right">{uriName(right) || right} <Icon name="folder" size={14} /></div>
  </div>
  <div class="filters">
    <label><input type="checkbox" bind:checked={showSame} /> Show identical</label>
    <label><input type="checkbox" bind:checked={byContent} /> Compare contents</label>
    <span class="counts">
      {#each Object.entries(counts) as [k, n]}<span class="count {symbols[k as DiffKind].tone}">{n} {symbols[k as DiffKind].label.toLowerCase()}</span>{/each}
    </span>
    <button onclick={load}><Icon name="reload" size={14} /></button>
  </div>
  <div class="rows">
    {#if error}
      <div class="msg">{error}</div>
    {:else if !items}
      <div class="msg">Comparing…</div>
    {:else if !shown.length}
      <div class="msg">The folders are identical</div>
    {:else}
      {#each shown as i (i.relPath)}
        {@const s = symbols[i.kind]}
        <div class="row">
          <div class="cell" class:missing={!i.left}>
            {#if i.left}<FileIcon name={i.left.name} isDir={i.left.isDir} size={16} /><span class="n">{i.relPath}</span><span class="meta">{i.left.isDir ? "" : formatSize(i.left.size)} · {formatDate(i.left.modified)}</span>{/if}
          </div>
          <div class="state {s.tone}" title={s.label}><Icon name={s.icon} size={14} /></div>
          <div class="cell" class:missing={!i.right}>
            {#if i.right}<FileIcon name={i.right.name} isDir={i.right.isDir} size={16} /><span class="n">{i.relPath}</span><span class="meta">{i.right.isDir ? "" : formatSize(i.right.size)} · {formatDate(i.right.modified)}</span>{/if}
          </div>
        </div>
      {/each}
    {/if}
  </div>
</div>

<style>
  .compare {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }
  .bar {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    gap: 12px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--stroke);
  }
  .side {
    display: flex;
    align-items: center;
    gap: 6px;
    font-weight: 600;
    overflow: hidden;
    white-space: nowrap;
  }
  .side.right {
    justify-content: flex-end;
  }
  .mid {
    display: flex;
    gap: 6px;
  }
  .mid button,
  .filters button {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 10px;
    border-radius: var(--radius);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke-strong);
  }
  .mid button:hover {
    background: var(--hover);
  }
  .filters {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 6px 16px;
    font-size: 12px;
    color: var(--text-2);
    border-bottom: 1px solid var(--stroke);
  }
  .counts {
    flex: 1;
    display: flex;
    gap: 10px;
  }
  .count.left {
    color: var(--accent);
  }
  .count.right {
    color: #c2410c;
  }
  .rows {
    flex: 1;
    overflow: auto;
    padding: 4px 12px;
  }
  .row {
    display: grid;
    grid-template-columns: 1fr 36px 1fr;
    align-items: center;
    height: 30px;
    border-radius: 4px;
  }
  .row:hover {
    background: var(--hover);
  }
  .cell {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    padding: 0 8px;
  }
  .cell.missing {
    background: repeating-linear-gradient(135deg, transparent 0 6px, var(--hover) 6px 12px);
    align-self: stretch;
    border-radius: 4px;
  }
  .n {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    font-size: 11.5px;
    color: var(--text-3);
    white-space: nowrap;
  }
  .state {
    display: grid;
    place-items: center;
    color: var(--text-3);
  }
  .state.left {
    color: var(--accent);
  }
  .state.right {
    color: #c2410c;
  }
  .state.diff {
    color: var(--danger);
  }
  .msg {
    padding: 40px;
    text-align: center;
    color: var(--text-3);
  }
</style>
