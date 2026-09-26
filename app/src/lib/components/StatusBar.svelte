<script lang="ts">
  import { freeSpace } from "../api";
  import { formatSize } from "../format";
  import { ws } from "../workspace.svelte";

  let tab = $derived(ws.active);
  let folder = $derived(tab?.folder);
  let count = $derived(tab?.visible.length ?? 0);
  let sel = $derived(tab?.selectedEntries ?? []);
  let selBytes = $derived(sel.reduce((n, e) => n + (e.isDir ? 0 : e.size), 0));
  let space = $state<{ free: number; total: number } | null>(null);

  $effect(() => {
    const uri = folder?.info?.uri;
    if (!uri) return;
    let stale = false;
    freeSpace(uri)
      .then((s) => !stale && (space = s))
      .catch(() => {});
    return () => (stale = true);
  });

  let timingText = $derived(
    folder?.timing
      ? `First rows on screen in ${folder.timing.firstRowsMs.toFixed(0)} ms` + (folder.timing.totalMs ? ` · ${folder.timing.count.toLocaleString()} items listed in ${folder.timing.totalMs.toFixed(0)} ms` : "")
      : "",
  );
</script>

<footer class="status">
  <span title={timingText}>{count.toLocaleString()} {count === 1 ? "item" : "items"}</span>
  {#if tab?.filter}<span class="muted">filtered from {folder.items.length.toLocaleString()}</span>{/if}
  {#if sel.length}
    <span class="sep"></span>
    <span>{sel.length.toLocaleString()} selected{selBytes ? ` · ${formatSize(selBytes)}` : ""}</span>
  {/if}
  <span class="spacer"></span>
  {#if space}<span class="muted">{formatSize(space.free)} free</span>{/if}
</footer>

<style>
  .status {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 28px;
    padding: 0 16px;
    font-size: 12px;
    color: var(--text-2);
    background: var(--layer);
    border-top: 1px solid var(--stroke);
    flex: none;
  }
  .muted {
    color: var(--text-3);
  }
  .sep {
    width: 1px;
    height: 12px;
    background: var(--stroke-strong);
  }
  .spacer {
    flex: 1;
  }
</style>
