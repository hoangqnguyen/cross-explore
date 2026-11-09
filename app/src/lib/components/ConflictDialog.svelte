<script lang="ts">
  // Side-by-side "which one do you keep?" card for copy/move conflicts.
  import { parentUri, uriName, type Resolution } from "../api";
  import { formatDate, formatDateFull, formatSize } from "../format";
  import { transfers } from "../stores/transfers.svelte";
  import Thumb from "./Thumb.svelte";

  let job = $derived(transfers.conflicts[0] ?? null);
  let c = $derived(job?.conflict ?? null);
  let all = $state(false);

  function pick(r: Resolution) {
    if (!job || !c) return;
    void transfers.resolve(job.id, c.id, r, all);
  }

  let newer = $derived(c ? ((c.source.modified ?? 0) > (c.dest.modified ?? 0) ? "source" : (c.dest.modified ?? 0) > (c.source.modified ?? 0) ? "dest" : null) : null);
  let bigger = $derived(c ? (c.source.size > c.dest.size ? "source" : c.dest.size > c.source.size ? "dest" : null) : null);
  /** Same size and the same modified time (to the second): almost surely the same file. */
  let same = $derived(!!c && !c.dest.isDir && c.source.size === c.dest.size && Math.floor((c.source.modified ?? 0) / 1000) === Math.floor((c.dest.modified ?? 0) / 1000));
  const folderOf = (uri: string) => uriName(parentUri(uri)) || uri;

  function onkeydown(e: KeyboardEvent) {
    if (!job || e.defaultPrevented) return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      transfers.cancel(job.id);
    }
  }
</script>

<svelte:window {onkeydown} />

{#if job && c}
  <div class="backdrop"></div>
  <div class="dialog" role="alertdialog" aria-label="File conflict">
    <h2>“{c.dest.name}” already exists</h2>
    <p class="sub">{c.dest.isDir ? "Folders are merged: files inside are compared one by one." : same ? "Both have the same size and date, so they're most likely the same file." : "Choose which file to keep."}</p>
    <div class="cards">
      {#each [{ side: "source", e: c.source, uri: c.sourceUri, label: "Keep the new one", where: `from ${folderOf(c.sourceUri)}`, r: "replace" }, { side: "dest", e: c.dest, uri: c.destUri, label: "Keep the existing one", where: `in ${folderOf(c.destUri)}`, r: "skip" }] as k (k.side)}
        <button class="card" onclick={() => pick(k.r as Resolution)}>
          <span class="tag">{k.label}</span>
          <Thumb entry={k.e} uri={k.uri} size={72} />
          <span class="meta">{k.e.isDir ? "Folder" : formatSize(k.e.size)}{#if bigger === k.side}<em>Larger</em>{/if}</span>
          <span class="meta" title={formatDateFull(k.e.modified)}>{formatDate(k.e.modified)}{#if newer === k.side}<em>Newer</em>{/if}</span>
          <span class="meta where" title={k.uri}>{k.where}</span>
        </button>
      {/each}
    </div>
    <div class="row">
      <label><input type="checkbox" bind:checked={all} /> Do this for all conflicts</label>
      <span class="spacer"></span>
      <button onclick={() => pick("replaceIfNewer")}>Keep newer</button>
      <button onclick={() => pick("keepBoth")}>Keep both</button>
      <button class="danger" onclick={() => transfers.cancel(job!.id)}>Stop</button>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 85;
    background: rgba(0, 0, 0, 0.25);
  }
  .dialog {
    position: fixed;
    z-index: 86;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    width: min(560px, calc(100vw - 32px));
    padding: 22px;
    border-radius: 12px;
    background: var(--flyout);
    box-shadow: 0 24px 64px rgba(0, 0, 0, 0.35), 0 0 0 1px var(--stroke-strong);
  }
  h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
    word-break: break-word;
  }
  .sub {
    margin: 4px 0 16px;
    color: var(--text-2);
  }
  .cards {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
  }
  .card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 14px;
    border-radius: 10px;
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke-strong);
  }
  .card:hover {
    box-shadow: 0 0 0 2px var(--accent);
  }
  .tag {
    font-weight: 600;
    margin-bottom: 4px;
  }
  .meta {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-2);
  }
  .meta em {
    font-style: normal;
    font-size: 10.5px;
    font-weight: 600;
    padding: 1px 6px;
    border-radius: 999px;
    color: var(--accent);
    background: var(--accent-soft);
  }
  .meta.where {
    max-width: 100%;
    color: var(--text-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    display: block;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 16px;
  }
  label {
    font-size: 12px;
    color: var(--text-2);
  }
  .spacer {
    flex: 1;
  }
  .row button {
    height: 30px;
    padding: 0 12px;
    border-radius: var(--radius);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke-strong);
  }
  .row button:hover {
    background: var(--hover);
  }
  .row .danger {
    color: var(--danger);
  }
</style>
