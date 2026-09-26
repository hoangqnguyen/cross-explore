<script lang="ts">
  // Virtualized Details view: only the rows in the viewport exist in the DOM,
  // so a 100k-item folder scrolls as smoothly as a 10-item one.
  import type { Item } from "../api";
  import { keyOf } from "../folder.svelte";
  import { formatDate, formatDateFull, formatSize, stemRange, typeLabel } from "../format";
  import { blankMenu, dropTarget, handleNavKey, itemMenu, onDragEnd, onDragStart, onItemPointerDown, onItemPointerUp } from "../listing";
  import type { SortKey } from "../sort";
  import { clipboard } from "../stores/clipboard.svelte";
  import { settings } from "../stores/settings.svelte";
  import { sizes } from "../stores/sizes.svelte";
  import { ws, type Tab } from "../workspace.svelte";
  import FileIcon from "./FileIcon.svelte";
  import Icon from "./Icon.svelte";
  import ViewStates from "./ViewStates.svelte";

  let { tab }: { tab: Tab } = $props();

  const OVERSCAN = 8;
  let scroller: HTMLDivElement | undefined = $state();
  let scrollTop = $state(0);
  let viewportH = $state(600);
  let now = $state(Date.now());

  let rowH = $derived(settings.data.compact ? 24 : 30);
  let rows = $derived(tab.visible);
  let start = $derived(Math.max(0, Math.floor(scrollTop / rowH) - OVERSCAN));
  let end = $derived(Math.min(rows.length, Math.ceil((scrollTop + viewportH) / rowH) + OVERSCAN));
  let slice = $derived(rows.slice(start, end));
  let folder = $derived(tab.folder);
  let isSearch = $derived(folder.kind === "search");

  // Relative dates ("Just now", "5 min ago") stay current.
  $effect(() => {
    const t = setInterval(() => (now = Date.now()), 30_000);
    return () => clearInterval(t);
  });

  // Restore the scroll position (or reveal the selected row) once per navigation.
  let restoredFor: object | null = null;
  $effect(() => {
    if (!scroller || folder.status !== "ready" || restoredFor === folder || !rows.length) return;
    restoredFor = folder;
    if (tab.restoreScroll >= 0) scroller.scrollTop = tab.restoreScroll;
    else reveal(true);
  });

  // A new filter starts at the top.
  $effect(() => {
    void tab.filter;
    if (scroller) scroller.scrollTop = 0;
  });

  /** Scroll so the cursor row is visible. */
  function reveal(center = false) {
    const i = tab.cursor == null ? -1 : rows.findIndex((e) => keyOf(e) === tab.cursor);
    if (!scroller || i < 0) return;
    const top = i * rowH;
    if (center) scroller.scrollTop = top - viewportH / 2 + rowH;
    else if (top < scroller.scrollTop) scroller.scrollTop = top;
    else if (top + rowH > scroller.scrollTop + viewportH) scroller.scrollTop = top + rowH - viewportH;
  }

  function onscroll() {
    scrollTop = scroller!.scrollTop;
    tab.scrollTop = scrollTop;
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.target !== scroller) return;
    handleNavKey(e, tab, { cols: 1, page: Math.max(1, Math.floor(viewportH / rowH) - 1), reveal: () => requestAnimationFrame(() => reveal()) });
  }

  function renameInput(el: HTMLInputElement, entry: Item) {
    const [a, b] = stemRange(entry.name, entry.isDir);
    el.focus();
    el.setSelectionRange(a, b);
    let done = false;
    const commit = (save: boolean) => {
      if (done) return;
      done = true;
      if (save) tab.rename(keyOf(entry), el.value);
      else tab.renaming = null;
      scroller?.focus();
    };
    el.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter") commit(true);
      else if (e.key === "Escape") commit(false);
    });
    el.addEventListener("blur", () => commit(true));
  }

  function sizeText(entry: Item) {
    if (!entry.isDir) return formatSize(entry.size);
    const s = sizes.get(tab.uriOf(entry));
    return s ? (s.done ? formatSize(s.bytes) : `${formatSize(s.bytes)}…`) : "";
  }

  let columns = $derived<{ key: SortKey | "where"; label: string }[]>([
    { key: "name", label: "Name" },
    { key: "modified", label: "Date modified" },
    isSearch ? { key: "where", label: "Folder" } : { key: "type", label: "Type" },
    { key: "size", label: "Size" },
  ]);
</script>

<div class="view">
  <div class="header" role="row">
    {#each columns as col (col.key)}
      {@const sorted = settings.data.sort.key === col.key}
      <button class="col {col.key}" class:sorted onclick={() => col.key !== "where" && ws.sortBy(col.key)} role="columnheader" aria-sort={sorted ? (settings.data.sort.desc ? "descending" : "ascending") : "none"}>
        {col.label}
        {#if sorted}
          <span class="arrow" class:desc={settings.data.sort.desc}><Icon name="chevronDown" size={11} stroke={1.8} /></span>
        {/if}
      </button>
    {/each}
  </div>

  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    class="details file-view"
    role="grid"
    tabindex="0"
    aria-rowcount={rows.length}
    bind:this={scroller}
    bind:clientHeight={viewportH}
    {onscroll}
    {onkeydown}
    onpointerdown={(e) => {
      ws.focusPane(tab.pane.id);
      if (e.target === e.currentTarget && e.button === 0) tab.selectOnly(null);
    }}
    oncontextmenu={(e) => e.target === e.currentTarget && blankMenu(e, tab)}
    use:dropTarget={{ dest: () => (tab.writable ? tab.dirUri : null) }}
  >
    <div class="spacer" style:height="{rows.length * rowH}px"></div>

    {#each slice as entry, k (keyOf(entry))}
      {@const i = start + k}
      {@const key = keyOf(entry)}
      {@const selected = tab.selection.has(key)}
      <div
        class="row"
        class:selected
        class:cursor={tab.cursor === key}
        class:fresh={folder.fresh.has(key)}
        class:dim={entry.hidden || clipboard.isCut(tab.uriOf(entry))}
        role="row"
        tabindex="-1"
        aria-selected={selected}
        draggable={tab.renaming !== key}
        style:transform="translateY({i * rowH}px)"
        style:height="{rowH}px"
        onpointerdown={(e) => onItemPointerDown(e, tab, entry)}
        onpointerup={(e) => onItemPointerUp(e, tab, entry)}
        ondblclick={() => tab.open(entry)}
        oncontextmenu={(e) => itemMenu(e, tab, entry)}
        ondragstart={(e) => onDragStart(e, tab, entry)}
        ondragend={onDragEnd}
        use:dropTarget={{ dest: () => (entry.isDir ? tab.uriOf(entry) : null), spring: () => tab.open(entry) }}
      >
        <span class="cell name">
          <FileIcon name={entry.name} isDir={entry.isDir} size={settings.data.compact ? 16 : 18} />
          {#if tab.renaming === key}
            <input class="rename" value={entry.name} use:renameInput={entry} spellcheck="false" />
          {:else}
            <span class="text">{entry.name}</span>
            {#if entry.kind === "symlink"}<span class="badge" title="Symbolic link">↗</span>{/if}
            {#if entry.snippet}<span class="snippet">{entry.line ? `${entry.line}: ` : ""}{entry.snippet}</span>{/if}
          {/if}
        </span>
        <span class="cell modified" title={formatDateFull(entry.modified)}>{formatDate(entry.modified, now)}</span>
        {#if isSearch}
          <span class="cell type" title={entry.relPath}>{entry.relPath?.includes("/") ? entry.relPath.slice(0, entry.relPath.lastIndexOf("/")) : "—"}</span>
        {:else}
          <span class="cell type">{typeLabel(entry)}</span>
        {/if}
        <span class="cell size">{sizeText(entry)}</span>
      </div>
    {/each}

    <ViewStates {tab} {rowH} />
  </div>
</div>

<style>
  .view {
    --cols: minmax(180px, 1fr) 190px 150px 100px;
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
    min-height: 0;
  }
  .header {
    display: grid;
    grid-template-columns: var(--cols);
    height: 32px;
    padding: 0 12px;
    flex: none;
  }
  .col {
    position: relative;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 0 10px;
    font-size: 12px;
    color: var(--text-2);
    border-radius: 4px;
    margin: 3px 0;
    white-space: nowrap;
    overflow: hidden;
  }
  .col.name {
    padding-left: 38px;
  }
  .col:hover {
    background: var(--hover);
  }
  .col:not(:first-child)::before {
    content: "";
    position: absolute;
    left: 0;
    top: 6px;
    bottom: 6px;
    width: 1px;
    background: var(--stroke);
  }
  .col.size {
    justify-content: flex-end;
  }
  .arrow {
    display: grid;
    color: var(--text-3);
    transform: rotate(180deg);
  }
  .arrow.desc {
    transform: none;
  }
  .details {
    position: relative;
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    outline: none;
    contain: strict;
    padding: 0 12px;
  }
  .spacer {
    width: 1px;
    pointer-events: none;
  }
  .row {
    position: absolute;
    top: 0;
    left: 12px;
    right: 12px;
    display: grid;
    grid-template-columns: var(--cols);
    align-items: center;
    border-radius: 4px;
    contain: layout style;
  }
  .row:hover {
    background: var(--hover);
  }
  .row.selected {
    background: var(--sel);
  }
  .row.selected:hover {
    background: var(--sel-hover);
  }
  .details:focus .row.cursor {
    box-shadow: inset 0 0 0 1px var(--sel-stroke);
  }
  .row.fresh {
    animation: fresh 1.6s var(--ease);
  }
  @keyframes fresh {
    0% {
      background: var(--fresh);
      box-shadow: inset 3px 0 0 var(--live);
    }
    60% {
      box-shadow: inset 3px 0 0 var(--live);
    }
  }
  .row.dim .name {
    opacity: 0.55;
  }
  .row:global(.drop-hover) {
    background: var(--accent-soft);
    box-shadow: inset 0 0 0 1.5px var(--accent);
  }
  .cell {
    padding: 0 10px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    color: var(--text-2);
  }
  .cell.name {
    display: flex;
    align-items: center;
    gap: 10px;
    padding-left: 8px;
    color: var(--text);
  }
  .cell.size {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .text {
    overflow: hidden;
    text-overflow: ellipsis;
    flex: none;
    max-width: 100%;
  }
  .snippet {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--text-3);
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 11.5px;
  }
  .badge {
    font-size: 11px;
    color: var(--text-3);
  }
  .rename {
    flex: 1;
    min-width: 0;
    height: calc(100% - 4px);
    padding: 0 6px;
    margin-left: -6px;
    border: 0;
    border-radius: 4px;
    outline: none;
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke-strong), inset 0 -2px 0 var(--accent);
    user-select: text;
    -webkit-user-select: text;
  }
</style>
