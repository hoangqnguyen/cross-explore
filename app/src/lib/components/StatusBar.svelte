<script lang="ts">
  import { freeSpace } from "../api";
  import { formatSize } from "../format";
  import { clipboard } from "../stores/clipboard.svelte";
  import { settings } from "../stores/settings.svelte";
  import { ws } from "../workspace.svelte";

  let tab = $derived(ws.activeTab);
  let folder = $derived(tab?.folder);
  let count = $derived(tab?.visible.length ?? 0);
  let sel = $derived(tab?.selectedEntries ?? []);
  let selBytes = $derived(sel.reduce((n, e) => n + (e.isDir ? 0 : e.size), 0));
  let space = $state<{ free: number; total: number } | null>(null);

  $effect(() => {
    const uri = folder?.kind === "folder" ? folder.info?.uri : null;
    space = null;
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
  {#if folder?.kind !== "home"}
    <span title={timingText}>{count.toLocaleString()} {count === 1 ? "item" : "items"}</span>
    {#if tab?.filter}<span class="muted">filtered from {folder.items.length.toLocaleString()}</span>{/if}
    {#if folder?.kind === "search" && folder.refreshing}<span class="muted">searching…</span>{/if}
    {#if sel.length}
      <span class="sep"></span>
      <span>{sel.length.toLocaleString()} selected{selBytes ? ` · ${formatSize(selBytes)}` : ""}</span>
    {/if}
  {/if}
  {#if clipboard.uris.length}
    <span class="sep"></span>
    <span class="muted">{clipboard.uris.length} {clipboard.mode === "cut" ? "cut" : "copied"} — paste to {clipboard.mode === "cut" ? "move" : "copy"}</span>
  {/if}
  <span class="spacer"></span>
  {#if settings.data.keymap === "commander"}<span class="badge" title="Total Commander-style keys: F3 view, F5 copy, F6 move, F7 new folder, F8 delete, Tab switch pane">Commander keys</span>{/if}
  {#if folder?.info && !folder.info.local}<span class="muted">{folder.info.scheme.toUpperCase()}</span>{/if}
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
    white-space: nowrap;
    overflow: hidden;
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
  .badge {
    padding: 1px 8px;
    border-radius: 9px;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 11px;
  }
</style>
