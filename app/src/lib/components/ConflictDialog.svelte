<script lang="ts">
  // Side-by-side "which one do you keep?" card for copy/move conflicts.
  import type { Resolution } from "../api";
  import { formatDateFull, formatSize } from "../format";
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
</script>

{#if job && c}
  <div class="backdrop"></div>
  <div class="dialog" role="alertdialog" aria-label="File conflict">
    <h2>“{c.dest.name}” already exists</h2>
    <p class="sub">{c.dest.isDir ? "Folders are merged: files inside are compared one by one." : "Choose which file to keep."}</p>
    <div class="cards">
      <button class="card" onclick={() => pick("replace")}>
        <span class="tag">Keep the new one</span>
        <Thumb entry={c.source} uri={c.sourceUri} size={72} />
        <span class="meta">{c.source.isDir ? "Folder" : formatSize(c.source.size)}{bigger === "source" ? " · larger" : ""}</span>
        <span class="meta">{formatDateFull(c.source.modified)}{newer === "source" ? " · newer" : ""}</span>
      </button>
      <button class="card" onclick={() => pick("skip")}>
        <span class="tag">Keep the existing one</span>
        <Thumb entry={c.dest} uri={c.destUri} size={72} />
        <span class="meta">{c.dest.isDir ? "Folder" : formatSize(c.dest.size)}{bigger === "dest" ? " · larger" : ""}</span>
        <span class="meta">{formatDateFull(c.dest.modified)}{newer === "dest" ? " · newer" : ""}</span>
      </button>
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
    font-size: 12px;
    color: var(--text-2);
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
