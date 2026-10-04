<script lang="ts">
  // Finder's column (Miller) view. The tab's folder is the focused column;
  // columns to the left are its ancestors, and the column to the right
  // previews the selected folder (or file).
  import { childUri, uriName, type Item } from "../api";
  import { Folder, keyOf } from "../folder.svelte";
  import { stemRange } from "../format";
  import { blankMenu, dropTarget, handleNavKey, itemMenu, onDragEnd, onDragStart, onItemPointerDown, onItemPointerUp } from "../listing";
  import { settings } from "../stores/settings.svelte";
  import { ws, type Tab } from "../workspace.svelte";
  import FileIcon from "./FileIcon.svelte";
  import Icon from "./Icon.svelte";
  import Preview from "./Preview.svelte";
  import VirtualRows from "./VirtualRows.svelte";

  let { tab }: { tab: Tab } = $props();

  const MAX_ANCESTORS = 3;
  const ROW_H = 26;
  /** Folders kept listed (and watched) for the side columns, least recent first. */
  const CACHE_SIZE = 8;
  const cache = new Map<string, Folder>();
  let wrap: HTMLDivElement | undefined = $state();
  let current: HTMLDivElement | undefined = $state();

  function folderFor(uri: string): Folder {
    let f = cache.get(uri);
    if (f) {
      cache.delete(uri); // most recent goes last
    } else {
      f = new Folder(uri, settings.data.sort);
      void f.load();
    }
    cache.set(uri, f);
    return f;
  }

  // Drop folders no column shows any more, oldest first: walking through
  // many folders used to keep every listing it passed in memory.
  $effect(() => {
    const keep = new Set([...ancestors.map((a) => a.uri), ...(next ? [next] : [])]);
    for (const [uri, f] of cache) {
      if (cache.size <= CACHE_SIZE) break;
      if (keep.has(uri)) continue;
      f.dispose();
      cache.delete(uri);
    }
  });

  $effect(() => () => cache.forEach((f) => f.dispose()));

  let crumbs = $derived(tab.folder.info?.crumbs ?? []);
  let ancestors = $derived(crumbs.slice(Math.max(0, crumbs.length - 1 - MAX_ANCESTORS), -1).map((c, i, all) => ({ uri: c.uri, label: c.label, child: (all[i + 1] ?? crumbs.at(-1))!.label })));
  let focusEntry = $derived(tab.cursorEntry);
  let wantNext = $derived(focusEntry?.isDir && tab.selection.size <= 1 ? childUri(tab.dirUri, focusEntry.name) : null);
  // Holding an arrow key over folders shouldn't list each one it passes:
  // folders not listed yet wait for the cursor to settle.
  let next = $state<string | null>(null);
  $effect(() => {
    const want = wantNext;
    if (want == null || cache.has(want)) {
      next = want;
      return;
    }
    const t = setTimeout(() => (next = want), 100);
    return () => clearTimeout(t);
  });

  const visibleOf = (items: Item[]) => (settings.data.showHidden ? items : items.filter((e) => !e.hidden));

  // Keep the focused column in view.
  $effect(() => {
    void tab.folder.uri;
    requestAnimationFrame(() => wrap && (wrap.scrollLeft = wrap.scrollWidth));
  });

  function reveal() {
    const i = tab.cursor == null ? -1 : tab.indexOf(tab.cursor);
    if (!current || i < 0) return;
    // Rows start below the column's top padding.
    const top = (current.firstElementChild as HTMLElement | null)?.offsetTop ?? 0;
    const y = top + i * ROW_H;
    if (y < current.scrollTop) current.scrollTop = y;
    else if (y + ROW_H > current.scrollTop + current.clientHeight) current.scrollTop = y + ROW_H - current.clientHeight;
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.target !== current) return;
    if (e.key === "ArrowRight" && focusEntry?.isDir) {
      tab.navigate(childUri(tab.dirUri, focusEntry.name));
      e.preventDefault();
      return;
    }
    if (e.key === "ArrowLeft" && tab.folder.info?.parent) {
      tab.up();
      e.preventDefault();
      return;
    }
    handleNavKey(e, tab, { cols: 1, page: 15, reveal: () => requestAnimationFrame(reveal) });
  }

  /**
   * Right-click in a column other than the focused one: focus that column
   * (like Finder), select the item under the pointer, then show the same
   * menu the focused column would.
   */
  async function menuIn(ev: MouseEvent, uri: string, name: string | null) {
    ev.preventDefault();
    ev.stopPropagation();
    const at = { x: ev.clientX, y: ev.clientY };
    tab.navigate(uri, name);
    const end = Date.now() + 3000;
    const same = () => tab.folder.uri === uri || tab.folder.info?.uri === uri;
    while (Date.now() < end && !(same() && tab.folder.status === "ready" && (!name || tab.visible.some((x) => x.name === name)))) {
      await new Promise((r) => setTimeout(r, 30));
    }
    if (!same()) return;
    const fake = new MouseEvent("contextmenu", { clientX: at.x, clientY: at.y });
    const entry = name ? tab.visible.find((x) => x.name === name) : undefined;
    current?.focus();
    if (entry) itemMenu(fake, tab, entry);
    else blankMenu(fake, tab);
  }

  // Every folder column (ancestors, the focused one, the next-folder preview)
  // shares one width, like Finder: drag any column's right edge to resize all.
  let resizeStartX = 0;
  let resizeStartWidth = 0;
  function startResize(e: PointerEvent) {
    e.preventDefault();
    e.stopPropagation();
    resizeStartX = e.clientX;
    resizeStartWidth = settings.data.columnWidth;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function onResizeMove(e: PointerEvent) {
    if (e.buttons !== 1) return;
    settings.data.columnWidth = Math.round(Math.min(480, Math.max(140, resizeStartWidth + (e.clientX - resizeStartX))));
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
      current?.focus();
    };
    el.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter") commit(true);
      else if (e.key === "Escape") commit(false);
    });
    el.addEventListener("blur", () => commit(true));
  }
</script>

<div class="columns" bind:this={wrap}>
  {#each ancestors as a (a.uri)}
    {@const f = folderFor(a.uri)}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="column" style:width="{settings.data.columnWidth}px" use:dropTarget={{ dest: () => a.uri }} oncontextmenu={(ev) => ev.target === ev.currentTarget && menuIn(ev, a.uri, null)}>
      <VirtualRows items={visibleOf(f.items)} rowH={ROW_H} key={(e) => e.name}>
        {#snippet row(e)}
          <button class="item" class:trail={e.name === a.child} oncontextmenu={(ev) => menuIn(ev, a.uri, e.name)} ondblclick={() => tab.open({ ...e, uri: childUri(a.uri, e.name) })} onclick={() => (e.isDir ? tab.navigate(childUri(a.uri, e.name)) : tab.navigate(a.uri, e.name))}>
            <FileIcon name={e.name} isDir={e.isDir} executable={e.executable} size={16} />
            <span>{e.name}</span>
            {#if e.isDir}<Icon name="chevronRight" size={11} />{/if}
          </button>
        {/snippet}
      </VirtualRows>
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="resize" onpointerdown={startResize} onpointermove={onResizeMove}></div>
    </div>
  {/each}

  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    class="column current file-view"
    style:width="{settings.data.columnWidth}px"
    tabindex="0"
    role="listbox"
    bind:this={current}
    {onkeydown}
    onpointerdown={(e) => {
      ws.focusPane(tab.pane.id);
      if (e.target === e.currentTarget) tab.selectOnly(null);
    }}
    oncontextmenu={(e) => e.target === e.currentTarget && blankMenu(e, tab)}
    use:dropTarget={{ dest: () => (tab.writable ? tab.dirUri : null) }}
  >
    <VirtualRows items={tab.visible} rowH={ROW_H} key={keyOf}>
      {#snippet row(e)}
        {@const key = keyOf(e)}
        <div
          class="item"
          class:selected={tab.selection.has(key)}
          class:cursor={tab.cursor === key}
          class:fresh={tab.folder.fresh.has(key)}
          class:dim={e.hidden}
          role="option"
          tabindex="-1"
          aria-selected={tab.selection.has(key)}
          draggable="true"
          onpointerdown={(ev) => onItemPointerDown(ev, tab, e)}
          onpointerup={(ev) => onItemPointerUp(ev, tab, e)}
          ondblclick={() => tab.open(e)}
          oncontextmenu={(ev) => itemMenu(ev, tab, e)}
          ondragstart={(ev) => onDragStart(ev, tab, e)}
          ondragend={onDragEnd}
          use:dropTarget={{ dest: () => (e.isDir ? tab.uriOf(e) : null), spring: () => tab.open(e) }}
        >
          <FileIcon name={e.name} isDir={e.isDir} executable={e.executable} size={16} />
          {#if tab.renaming === key}
            <input class="rename" value={e.name} use:renameInput={e} spellcheck="false" />
          {:else}
            <span>{e.name}</span>
          {/if}
          {#if e.isDir}<Icon name="chevronRight" size={11} />{/if}
        </div>
      {/snippet}
    </VirtualRows>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="resize" onpointerdown={startResize} onpointermove={onResizeMove}></div>
  </div>

  {#if next}
    {@const f = folderFor(next)}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="column" style:width="{settings.data.columnWidth}px" oncontextmenu={(ev) => ev.target === ev.currentTarget && menuIn(ev, next!, null)}>
      <VirtualRows items={visibleOf(f.items)} rowH={ROW_H} key={(e) => e.name}>
        {#snippet row(e)}
          <button class="item" oncontextmenu={(ev) => menuIn(ev, next!, e.name)} onclick={() => tab.navigate(next!, e.name)} ondblclick={() => tab.open({ ...e, uri: childUri(next!, e.name) })}>
            <FileIcon name={e.name} isDir={e.isDir} executable={e.executable} size={16} />
            <span>{e.name}</span>
            {#if e.isDir}<Icon name="chevronRight" size={11} />{/if}
          </button>
        {/snippet}
      </VirtualRows>
      {#if f.status === "ready" && !visibleOf(f.items).length}<div class="hint">Empty folder</div>{/if}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="resize" onpointerdown={startResize} onpointermove={onResizeMove}></div>
    </div>
  {:else if focusEntry && !focusEntry.isDir}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="column preview" oncontextmenu={(ev) => focusEntry && !window.getSelection()?.toString() && itemMenu(ev, tab, focusEntry)}>
      {#key tab.uriOf(focusEntry)}
        <Preview entry={focusEntry} uri={tab.uriOf(focusEntry)} />
      {/key}
      <div class="pname">{focusEntry.name}</div>
    </div>
  {/if}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  {#if next || !focusEntry || focusEntry.isDir}
    <div class="filler" title={uriName(tab.dirUri)} oncontextmenu={(ev) => blankMenu(ev, tab)}></div>
  {/if}
</div>

<style>
  .columns {
    display: flex;
    flex: 1;
    min-height: 0;
    overflow-x: auto;
    overflow-y: hidden;
  }
  .column {
    position: relative;
    flex: none;
    overflow-y: auto;
    padding: 6px;
    border-right: 1px solid var(--stroke);
    outline: none;
  }
  /* Sits just inside the column's own box (not straddling the border): the
     column clips overflow, so a handle poking outside it would be unclickable
     right where it matters, at the edge. */
  .resize {
    position: absolute;
    right: 0;
    top: 0;
    bottom: 0;
    width: 6px;
    cursor: col-resize;
    z-index: 2;
    touch-action: none;
  }
  .resize:hover,
  .resize:active {
    background: var(--accent-soft);
  }
  /* The file preview takes whatever width is left, instead of a blank strip. */
  .column.preview {
    flex: 1 0 300px;
    width: auto;
    min-width: 300px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 16px;
  }
  .pname {
    text-align: center;
    font-size: 12px;
    color: var(--text-2);
    word-break: break-word;
  }
  .filler {
    flex: 1;
    min-width: 40px;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 26px;
    padding: 0 8px;
    border-radius: 4px;
    text-align: left;
    white-space: nowrap;
  }
  .item span {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .item :global(svg:last-child) {
    color: var(--text-3);
  }
  .item:hover {
    background: var(--hover);
  }
  .item.trail {
    background: var(--pressed);
  }
  .item.selected {
    background: var(--sel);
  }
  .current:focus .item.cursor {
    box-shadow: inset 0 0 0 1px var(--sel-stroke);
  }
  .current:focus .item.selected {
    background: var(--accent);
    color: var(--accent-text);
  }
  .current:focus .item.selected :global(svg:last-child) {
    color: inherit;
  }
  .item.fresh {
    animation: fresh 1.6s var(--ease);
  }
  @keyframes fresh {
    0% {
      background: var(--fresh);
    }
  }
  .item.dim {
    opacity: 0.55;
  }
  .item:global(.drop-hover),
  .column:global(.drop-hover) {
    background: var(--accent-soft);
  }
  .hint {
    padding: 12px;
    font-size: 12px;
    color: var(--text-3);
  }
  .rename {
    flex: 1;
    min-width: 0;
    border: 0;
    border-radius: 4px;
    outline: none;
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke-strong), inset 0 -2px 0 var(--accent);
    color: var(--text);
    user-select: text;
    -webkit-user-select: text;
  }
</style>
