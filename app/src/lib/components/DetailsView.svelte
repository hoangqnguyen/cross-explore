<script lang="ts">
  // Virtualized Details view: only the rows in the viewport exist in the DOM,
  // so a 100k-item folder scrolls as smoothly as a 10-item one.
  import type { Entry } from "../api";
  import { formatDate, formatDateFull, formatSize, stemRange, typeLabel } from "../format";
  import { isMac, isTextInput, primary } from "../keys";
  import { menu, type MenuItem } from "../menu.svelte";
  import type { SortKey } from "../sort";
  import { ws, type Tab } from "../workspace.svelte";
  import FileIcon from "./FileIcon.svelte";
  import Icon from "./Icon.svelte";

  let { tab }: { tab: Tab } = $props();

  const OVERSCAN = 8;
  let scroller: HTMLDivElement | undefined = $state();
  let scrollTop = $state(0);
  let viewportH = $state(600);
  let now = $state(Date.now());
  let showSkeleton = $state(false);

  let rowH = $derived(ws.settings.compact ? 24 : 30);
  let rows = $derived(tab.visible);
  let start = $derived(Math.max(0, Math.floor(scrollTop / rowH) - OVERSCAN));
  let end = $derived(Math.min(rows.length, Math.ceil((scrollTop + viewportH) / rowH) + OVERSCAN));
  let slice = $derived(rows.slice(start, end));
  let folder = $derived(tab.folder);

  // Relative dates ("Just now", "5 min ago") stay current.
  $effect(() => {
    const t = setInterval(() => (now = Date.now()), 30_000);
    return () => clearInterval(t);
  });

  // Skeleton rows only if a listing is slow enough to notice.
  $effect(() => {
    if (folder.status !== "loading") {
      showSkeleton = false;
      return;
    }
    const t = setTimeout(() => (showSkeleton = true), 150);
    return () => clearTimeout(t);
  });

  // Restore the scroll position (or reveal the selected row) once per navigation.
  let restoredFor: object | null = null;
  $effect(() => {
    if (!scroller || folder.status !== "ready" || restoredFor === folder) return;
    restoredFor = folder;
    if (tab.restoreScroll >= 0) scroller.scrollTop = tab.restoreScroll;
    else reveal(true);
  });

  function indexOf(name: string | null) {
    return name == null ? -1 : rows.findIndex((e) => e.name === name);
  }

  /** Scroll so the cursor row is visible. */
  function reveal(center = false) {
    const i = indexOf(tab.cursor);
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

  function onRowDown(e: PointerEvent, entry: Entry) {
    if (e.button !== 0 && !(e.button === 2 && !tab.selection.has(entry.name))) return;
    if (e.button === 2) return tab.selectOnly(entry.name);
    if (e.shiftKey) tab.selectRange(entry.name, primary(e));
    else if (primary(e)) tab.toggle(entry.name);
    else if (!tab.selection.has(entry.name) || tab.selection.size === 1) tab.selectOnly(entry.name);
    else tab.cursor = entry.name;
  }

  function onRowUp(e: PointerEvent, entry: Entry) {
    // Clicking one row of a multi-selection narrows to it on release.
    if (e.button === 0 && !e.shiftKey && !primary(e) && tab.selection.size > 1) tab.selectOnly(entry.name);
  }

  function openSelection() {
    const sel = tab.selectedEntries;
    if (sel.length === 1 || sel.every((e) => !e.isDir)) sel.forEach((e) => tab.open(e));
    else if (sel.length) tab.open(sel[0]);
  }

  function rowMenu(e: MouseEvent, entry: Entry) {
    e.preventDefault();
    const many = tab.selection.size > 1;
    const items: MenuItem[] = [
      { label: "Open", icon: "open", shortcut: isMac ? "⌘O" : "Enter", action: openSelection },
      { label: "Open in new tab", disabled: !entry.isDir || many, action: () => ws.newTab(childUriOf(entry)) },
      { separator: true },
      { label: "Rename", icon: "rename", shortcut: isMac ? "↩" : "F2", disabled: many, action: () => (tab.renaming = entry.name) },
      { label: many ? "Copy paths" : "Copy path", icon: "copy", action: () => tab.copyPath() },
      { separator: true },
      { label: isMac ? "Move to Trash" : "Delete", icon: "trash", danger: true, shortcut: isMac ? "⌘⌫" : "Del", action: () => tab.trashSelection() },
    ];
    menu.show(items, e.clientX, e.clientY);
  }

  function blankMenu(e: MouseEvent) {
    e.preventDefault();
    tab.selectOnly(null);
    menu.show(
      [
        { label: "New folder", icon: "newFolder", shortcut: isMac ? "⌘⇧N" : "Ctrl+Shift+N", action: () => tab.newFolder() },
        { label: "Copy path", icon: "copy", action: () => tab.copyPath() },
        { separator: true },
        { label: "Hidden items", checked: ws.settings.showHidden, action: () => ws.toggle("showHidden") },
        { label: "Compact view", checked: ws.settings.compact, action: () => ws.toggle("compact") },
      ],
      e.clientX,
      e.clientY,
    );
  }

  function childUriOf(entry: Entry) {
    return (folder.info?.uri ?? folder.uri).replace(/\/+$/, "") + "/" + encodeURIComponent(entry.name);
  }

  function move(to: number, e: KeyboardEvent) {
    if (!rows.length) return;
    const i = Math.max(0, Math.min(rows.length - 1, to));
    const name = rows[i].name;
    if (e.shiftKey) tab.selectRange(name);
    else if (primary(e) && !isMac) tab.cursor = name; // Ctrl+arrows move focus only, like Explorer
    else tab.selectOnly(name);
    reveal();
  }

  function onkeydown(e: KeyboardEvent) {
    if (isTextInput(e.target)) return;
    const cur = indexOf(tab.cursor);
    const page = Math.max(1, Math.floor(viewportH / rowH) - 1);
    const k = e.key;
    if (k === "ArrowDown" && !(isMac && e.metaKey)) move(cur < 0 ? 0 : cur + 1, e);
    else if (k === "ArrowUp" && !(isMac && e.metaKey)) move(cur < 0 ? 0 : cur - 1, e);
    else if (k === "Home") move(0, e);
    else if (k === "End") move(rows.length - 1, e);
    else if (k === "PageDown") move(cur + page, e);
    else if (k === "PageUp") move(cur - page, e);
    else if (k === "Enter" && isMac && tab.selection.size === 1) tab.renaming = [...tab.selection][0];
    else if (k === "Enter" || (isMac && e.metaKey && (k === "o" || k === "ArrowDown"))) openSelection();
    else if (k === "F2" && tab.selection.size === 1) tab.renaming = [...tab.selection][0];
    else if ((k === "Delete" && !isMac) || (isMac && e.metaKey && k === "Backspace")) tab.trashSelection();
    else if (primary(e) && k.toLowerCase() === "a") tab.selectAll();
    else if (k === "Escape") {
      if (tab.filter) tab.filter = "";
      else tab.selectOnly(null);
    } else if (k === "Backspace" && tab.filter) tab.filter = tab.filter.slice(0, -1);
    else if (k === "Backspace" && !isMac) tab.back();
    else if (k.length === 1 && k !== " " && !e.ctrlKey && !e.metaKey && !e.altKey) {
      // Type to filter, Total Commander style.
      tab.filter += k;
      scroller!.scrollTop = 0;
    } else return;
    e.preventDefault();
  }

  function renameInput(el: HTMLInputElement, entry: Entry) {
    const [a, b] = stemRange(entry.name, entry.isDir);
    el.focus();
    el.setSelectionRange(a, b);
    let done = false;
    const commit = (save: boolean) => {
      if (done) return;
      done = true;
      if (save) tab.rename(entry.name, el.value);
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

  const columns: { key: SortKey; label: string }[] = [
    { key: "name", label: "Name" },
    { key: "modified", label: "Date modified" },
    { key: "type", label: "Type" },
    { key: "size", label: "Size" },
  ];
</script>

<div class="view">
  <div class="header" role="row">
    {#each columns as col (col.key)}
      {@const sorted = ws.settings.sort.key === col.key}
      <button class="col {col.key}" class:sorted onclick={() => ws.sortBy(col.key)} role="columnheader" aria-sort={sorted ? (ws.settings.sort.desc ? "descending" : "ascending") : "none"}>
        {col.label}
        {#if sorted}
          <span class="arrow" class:desc={ws.settings.sort.desc}><Icon name="chevronDown" size={11} stroke={1.8} /></span>
        {/if}
      </button>
    {/each}
  </div>

  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    class="details"
    role="grid"
    tabindex="0"
    aria-rowcount={rows.length}
    bind:this={scroller}
    bind:clientHeight={viewportH}
    {onscroll}
    {onkeydown}
    onpointerdown={(e) => e.target === e.currentTarget && e.button === 0 && tab.selectOnly(null)}
    oncontextmenu={(e) => e.target === e.currentTarget && blankMenu(e)}
  >
    <div class="spacer" style:height="{rows.length * rowH}px"></div>

    {#each slice as entry, k (entry.name)}
      {@const i = start + k}
      {@const selected = tab.selection.has(entry.name)}
      <div
        class="row"
        class:selected
        class:cursor={tab.cursor === entry.name}
        class:fresh={folder.fresh.has(entry.name)}
        class:dim={entry.hidden}
        role="row"
        tabindex="-1"
        aria-selected={selected}
        style:transform="translateY({i * rowH}px)"
        style:height="{rowH}px"
        onpointerdown={(e) => onRowDown(e, entry)}
        onpointerup={(e) => onRowUp(e, entry)}
        ondblclick={() => tab.open(entry)}
        oncontextmenu={(e) => rowMenu(e, entry)}
      >
        <span class="cell name">
          <FileIcon name={entry.name} isDir={entry.isDir} size={ws.settings.compact ? 16 : 18} />
          {#if tab.renaming === entry.name}
            <input class="rename" value={entry.name} use:renameInput={entry} spellcheck="false" />
          {:else}
            <span class="text">{entry.name}</span>
            {#if entry.kind === "symlink"}<span class="badge" title="Symbolic link">↗</span>{/if}
          {/if}
        </span>
        <span class="cell modified" title={formatDateFull(entry.modified)}>{formatDate(entry.modified, now)}</span>
        <span class="cell type">{typeLabel(entry)}</span>
        <span class="cell size">{entry.isDir ? "" : formatSize(entry.size)}</span>
      </div>
    {/each}

    {#if folder.status === "loading" && showSkeleton}
      {#each { length: 8 } as _, i}
        <div class="row skeleton" style:transform="translateY({i * rowH}px)" style:height="{rowH}px" style:animation-delay="{i * 60}ms">
          <span class="cell name"><span class="bone" style:width="{40 + ((i * 37) % 40)}%"></span></span>
        </div>
      {/each}
    {:else if folder.status === "error"}
      <div class="empty">
        <Icon name="folder" size={40} stroke={1} />
        <p>{folder.error}</p>
        <div class="actions">
          {#if tab.canBack}<button onclick={() => tab.back()}>Go back</button>{/if}
          <button onclick={() => folder.load()}>Try again</button>
        </div>
      </div>
    {:else if folder.status === "ready" && rows.length === 0}
      <div class="empty">
        {#if tab.filter}
          <Icon name="search" size={36} stroke={1} />
          <p>No items match “{tab.filter}”</p>
          <div class="actions"><button onclick={() => (tab.filter = "")}>Clear filter</button></div>
        {:else}
          <Icon name="folder" size={40} stroke={1} />
          <p>This folder is empty</p>
          {#if folder.items.length}<p class="sub">{folder.items.length} hidden {folder.items.length === 1 ? "item" : "items"}</p>{/if}
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .view {
    --cols: minmax(180px, 1fr) 190px 150px 100px;
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
    background: var(--layer);
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
    will-change: transform;
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
  .details:focus-visible .row.cursor {
    box-shadow: inset 0 0 0 1px var(--sel-stroke);
  }
  .details:focus .row.selected.cursor {
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
  .skeleton {
    animation: pulse 1.2s ease-in-out infinite;
  }
  .bone {
    display: block;
    height: 10px;
    border-radius: 5px;
    background: var(--stroke-strong);
    margin-left: 28px;
  }
  @keyframes pulse {
    50% {
      opacity: 0.45;
    }
  }
  .empty {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    color: var(--text-3);
    pointer-events: none;
    text-align: center;
    padding: 24px;
  }
  .empty p {
    margin: 4px 0 0;
    color: var(--text-2);
    max-width: 420px;
  }
  .empty .sub {
    color: var(--text-3);
    font-size: 12px;
  }
  .actions {
    display: flex;
    gap: 8px;
    margin-top: 10px;
    pointer-events: auto;
  }
  .actions button {
    height: 30px;
    padding: 0 14px;
    border-radius: var(--radius);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke-strong);
    color: var(--text);
  }
  .actions button:hover {
    background: var(--hover);
  }
</style>
