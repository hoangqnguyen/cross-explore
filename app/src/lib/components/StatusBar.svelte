<script lang="ts">
  import { freeSpace } from "../api";
  import { formatSize } from "../format";
  import { clipboard } from "../stores/clipboard.svelte";
  import { settings } from "../stores/settings.svelte";
  import { uriName } from "../api";
  import Icon from "./Icon.svelte";
  import { ws } from "../workspace.svelte";

  let tab = $derived(ws.activeTab);
  let folder = $derived(tab?.folder);
  let count = $derived(tab?.visible.length ?? 0);
  let sel = $derived(tab?.selectedEntries ?? []);
  let selBytes = $derived(sel.reduce((n, e) => n + (e.isDir ? 0 : e.size), 0));
  let space = $state<{ free: number; total: number } | null>(null);

  // A string, so a reload of the same folder (new `info` object) doesn't re-ask.
  let spaceUri = $derived(folder?.kind === "folder" ? (folder.info?.uri ?? null) : null);
  $effect(() => {
    const uri = spaceUri;
    space = null;
    if (!uri) return;
    let stale = false;
    freeSpace(uri)
      .then((s) => !stale && (space = s))
      .catch(() => {});
    return () => (stale = true);
  });

  /**
   * Finder's path bar: where the focused item lives. Rows inside expanded
   * folders extend the current folder's crumbs with the folders in between.
   */
  let path = $derived.by(() => {
    const info = folder?.info;
    if (!settings.data.pathBar || !info || folder?.kind !== "folder") return [];
    const crumbs: { label: string; uri: string; icon: string }[] = info.crumbs.map((c) => ({ ...c }));
    const cur = tab?.cursorEntry;
    if (cur?.parent && cur.depth) {
      const base = tab.dirUri.replace(/\/+$/, "");
      const rel = cur.parent.slice(base.length).split("/").filter(Boolean);
      let acc = base;
      for (const seg of rel) {
        acc += "/" + seg;
        crumbs.push({ label: decodeURIComponent(seg), uri: acc, icon: "folder" });
      }
    }
    if (cur) crumbs.push({ label: cur.name, uri: tab.uriOf(cur), icon: cur.isDir ? "folder" : "file" });
    return crumbs;
  });

  function openCrumb(uri: string, i: number) {
    // The last crumb is the focused item itself: reveal rather than open it.
    if (i === path.length - 1 && tab.cursorEntry && !tab.cursorEntry.isDir) return;
    tab.navigate(uri);
  }

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
  {#if path.length}
    <nav class="pathbar" aria-label="Path">
      {#each path as c, i (c.uri + i)}
        {#if i > 0}<Icon name="chevronRight" size={10} />{/if}
        <button class:last={i === path.length - 1} title={uriName(c.uri) || c.label} onclick={() => openCrumb(c.uri, i)}>
          {#if c.icon === "home"}<Icon name="home" size={12} />{:else if c.icon === "drive"}<Icon name="drive" size={12} />{:else if c.icon === "server"}<Icon name="server" size={12} />{/if}
          {c.label}
        </button>
      {/each}
    </nav>
    <span class="spacer"></span>
  {/if}
  {#if settings.data.keymap !== (ws.platform === "macos" ? "finder" : "explorer")}<span class="badge" title={settings.data.keymap === "commander" ? "Total Commander-style keys: F3 view, F5 copy, F6 move, F7 new folder, F8 delete, Tab switch pane" : `${settings.data.keymap === "finder" ? "Finder" : "Explorer"} keys`}>{settings.data.keymap === "commander" ? "Commander" : settings.data.keymap === "finder" ? "Finder" : "Explorer"} keys</span>{/if}
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
  .pathbar {
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
    overflow: hidden;
    color: var(--text-3);
  }
  .pathbar button {
    display: flex;
    align-items: center;
    gap: 4px;
    height: 20px;
    padding: 0 5px;
    border-radius: 4px;
    color: var(--text-2);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex: 0 1 auto;
    min-width: 0;
  }
  .pathbar button.last {
    color: var(--text);
    flex-shrink: 0;
  }
  .pathbar button:hover {
    background: var(--hover);
  }
  .badge {
    padding: 1px 8px;
    border-radius: 9px;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 11px;
  }
</style>
