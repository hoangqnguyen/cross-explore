<script lang="ts">
  // Right-hand details pane: a preview of the focused item plus its info
  // (Explorer's details pane and Finder's preview column in one).
  import { fileUriToPath, getTags, openEntry, errorText, type Item } from "../api";
  import { formatDateFull, formatSize, typeLabel } from "../format";
  import { dialogs } from "../stores/dialogs.svelte";
  import { settings } from "../stores/settings.svelte";
  import { sizes } from "../stores/sizes.svelte";
  import { tagColor } from "../tags";
  import { toasts } from "../toasts.svelte";
  import { ws } from "../workspace.svelte";
  import Icon from "./Icon.svelte";
  import Preview from "./Preview.svelte";

  let tab = $derived(ws.activeTab);
  let sel = $derived(tab?.selectedEntries ?? []);
  let entry = $derived<Item | null>(tab?.cursorEntry ?? (sel.length === 1 ? sel[0] : null));
  let uri = $derived(entry && tab ? tab.uriOf(entry) : null);
  let tags = $state<string[]>([]);

  $effect(() => {
    const u = uri;
    tags = [];
    if (!u) return;
    let stale = false;
    getTags([u])
      .then((m) => !stale && (tags = m[u] ?? []))
      .catch(() => {});
    return () => (stale = true);
  });

  let size = $derived(entry?.isDir && uri ? sizes.get(uri) : null);
  let multiBytes = $derived(sel.reduce((n, e) => n + (e.isDir ? 0 : e.size), 0));

  function where(e: Item) {
    const p = e.parent ?? tab.dirUri;
    return fileUriToPath(p);
  }

  let dragging = false;
  function resize(e: PointerEvent) {
    if (dragging) settings.data.previewWidth = Math.round(Math.min(600, Math.max(220, window.innerWidth - e.clientX)));
  }

  async function editTags() {
    if (!uri) return;
    const updated = await dialogs.ask<string[]>("tags", { uris: [uri] });
    if (updated) tags = updated;
  }
</script>

<aside class="pane-preview" style:width="{settings.data.previewWidth}px">
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="grip"
    onpointerdown={(e) => {
      dragging = true;
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    }}
    onpointermove={resize}
    onpointerup={() => (dragging = false)}
  ></div>
  {#if entry && uri}
    <div class="stage">
      {#key uri}<Preview {entry} {uri} />{/key}
    </div>
    <h3 title={entry.name}>{entry.name}</h3>
    <div class="kind">{typeLabel(entry)}</div>
    <div class="actions">
      <button onclick={() => tab.open(entry!)}><Icon name="open" size={14} /> Open</button>
      <button onclick={editTags}><Icon name="tag" size={14} /> Tags</button>
    </div>
    {#if tags.length}
      <div class="tags">
        {#each tags as t (t)}<span class="tag"><i style:background={tagColor(t)}></i>{t}</span>{/each}
      </div>
    {/if}
    <dl>
      <dt>Size</dt>
      <dd>
        {#if entry.isDir}
          {#if size}{formatSize(size.bytes)}{size.done ? "" : "…"} <span class="muted">({size.files.toLocaleString()} files)</span>
          {:else}<button class="link" onclick={() => sizes.compute(uri!)}>Calculate</button>{/if}
        {:else}{formatSize(entry.size)} <span class="muted">({entry.size.toLocaleString()} bytes)</span>{/if}
      </dd>
      <dt>Modified</dt>
      <dd>{formatDateFull(entry.modified)}</dd>
      {#if entry.created}
        <dt>Created</dt>
        <dd>{formatDateFull(entry.created)}</dd>
      {/if}
      <dt>Where</dt>
      <dd class="path" title={where(entry)}>{where(entry)}</dd>
      {#if entry.readonly}
        <dt>Access</dt>
        <dd>Read only</dd>
      {/if}
    </dl>
  {:else if sel.length > 1}
    <div class="multi">
      <Icon name="copy" size={48} stroke={1} />
      <h3>{sel.length} items selected</h3>
      <div class="kind">{formatSize(multiBytes)} in files</div>
    </div>
  {:else if tab?.folder.info}
    <div class="multi">
      <Icon name="folder" size={48} stroke={1} />
      <h3>{tab.folder.info.name}</h3>
      <div class="kind">{tab.visible.length.toLocaleString()} items</div>
      {#if tab.folder.info.local && tab.folder.kind === "folder"}
        <button class="link" onclick={() => openEntry(tab.dirUri).catch((e) => toasts.show(errorText(e), "error"))}>Open with default app</button>
      {/if}
    </div>
  {/if}
</aside>

<style>
  .pane-preview {
    position: relative;
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
    padding: 16px;
    border-left: 1px solid var(--stroke);
    overflow-y: auto;
    background: var(--layer);
  }
  .grip {
    position: absolute;
    left: -3px;
    top: 0;
    bottom: 0;
    width: 6px;
    cursor: col-resize;
    z-index: 2;
  }
  .stage {
    position: relative;
    height: 220px;
    flex: none;
    display: flex;
    margin-bottom: 6px;
  }
  h3 {
    margin: 4px 0 0;
    font-size: 14px;
    font-weight: 600;
    word-break: break-word;
  }
  .kind {
    color: var(--text-2);
    font-size: 12px;
  }
  .actions {
    display: flex;
    gap: 6px;
    margin: 6px 0;
  }
  .actions button {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 10px;
    border-radius: var(--radius);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke-strong);
  }
  .actions button:hover {
    background: var(--hover);
  }
  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .tag {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 12px;
    padding: 2px 8px 2px 6px;
    border-radius: 10px;
    background: var(--hover);
  }
  .tag i {
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 6px 12px;
    margin: 8px 0 0;
    font-size: 12px;
  }
  dt {
    color: var(--text-3);
  }
  dd {
    margin: 0;
    min-width: 0;
    color: var(--text);
    user-select: text;
    -webkit-user-select: text;
  }
  .path {
    word-break: break-all;
  }
  .muted {
    color: var(--text-3);
  }
  .link {
    color: var(--accent);
    font-size: 12px;
  }
  .link:hover {
    text-decoration: underline;
  }
  .multi {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    margin-top: 40px;
    color: var(--text-3);
    text-align: center;
  }
  .multi h3 {
    color: var(--text);
  }
</style>
