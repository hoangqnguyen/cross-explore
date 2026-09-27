<script lang="ts">
  // Virtualized icon grid with thumbnails. Rows of cells are rendered only
  // around the viewport, like the Details view.
  import type { Item } from "../api";
  import { keyOf } from "../folder.svelte";
  import { stemRange } from "../format";
  import { blankMenu, dropTarget, handleNavKey, itemMenu, onDragEnd, onDragStart, onItemPointerDown, onItemPointerUp, startMarquee, type MarqueeRect } from "../listing";
  import { clipboard } from "../stores/clipboard.svelte";
  import { settings } from "../stores/settings.svelte";
  import { ui } from "../stores/ui.svelte";
  import { ws, type Tab } from "../workspace.svelte";
  import Thumb from "./Thumb.svelte";
  import ViewStates from "./ViewStates.svelte";

  let { tab }: { tab: Tab } = $props();

  let scroller: HTMLDivElement | undefined = $state();
  let scrollTop = $state(0);
  let viewportH = $state(600);
  let width = $state(800);

  let icon = $derived(settings.data.iconSize);
  let cellW = $derived(icon + 36);
  let cellH = $derived(icon + 48);
  let cols = $derived(Math.max(1, Math.floor((width - 24) / cellW)));
  let rows = $derived(tab.visible);
  let rowCount = $derived(Math.ceil(rows.length / cols));
  let firstRow = $derived(Math.max(0, Math.floor(scrollTop / cellH) - 2));
  let lastRow = $derived(Math.min(rowCount, Math.ceil((scrollTop + viewportH) / cellH) + 2));
  let slice = $derived(rows.slice(firstRow * cols, lastRow * cols));
  let folder = $derived(tab.folder);

  let marquee = $state<MarqueeRect | null>(null);

  /** Cells a selection rectangle overlaps. */
  function cellsIn(r: MarqueeRect) {
    const keys: string[] = [];
    const rowFrom = Math.max(0, Math.floor((r.y - 8) / cellH));
    const rowTo = Math.min(rowCount - 1, Math.floor((r.y + r.h - 8) / cellH));
    for (let row = rowFrom; row <= rowTo; row++) {
      for (let col = 0; col < cols; col++) {
        const i = row * cols + col;
        if (i >= rows.length) break;
        const x = 12 + col * cellW;
        const y = 8 + row * cellH;
        if (x < r.x + r.w && x + cellW - 8 > r.x && y < r.y + r.h && y + cellH - 8 > r.y) keys.push(keyOf(rows[i]));
      }
    }
    return keys;
  }

  let restoredFor: object | null = null;
  $effect(() => {
    if (!scroller || folder.status !== "ready" || restoredFor === folder || !rows.length) return;
    restoredFor = folder;
    if (tab.restoreScroll >= 0) scroller.scrollTop = tab.restoreScroll;
    else reveal();
  });

  function reveal() {
    const i = tab.cursor == null ? -1 : rows.findIndex((e) => keyOf(e) === tab.cursor);
    if (!scroller || i < 0) return;
    const top = Math.floor(i / cols) * cellH;
    if (top < scroller.scrollTop) scroller.scrollTop = top;
    else if (top + cellH > scroller.scrollTop + viewportH) scroller.scrollTop = top + cellH - viewportH;
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.target !== scroller) return;
    handleNavKey(e, tab, { cols, page: cols * Math.max(1, Math.floor(viewportH / cellH)), reveal: () => requestAnimationFrame(reveal) });
  }

  function renameInput(el: HTMLTextAreaElement, entry: Item) {
    const [a, b] = stemRange(entry.name, entry.isDir);
    el.focus();
    el.setSelectionRange(a, b);
    let done = false;
    const commit = (save: boolean) => {
      if (done) return;
      done = true;
      if (save) tab.rename(keyOf(entry), el.value.replace(/\n/g, ""));
      else tab.renaming = null;
      scroller?.focus();
    };
    el.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter") {
        e.preventDefault();
        commit(true);
      } else if (e.key === "Escape") commit(false);
    });
    el.addEventListener("blur", () => commit(true));
  }

  // Ctrl/⌘ + wheel zooms the icons, like Explorer and Finder.
  function onwheel(e: WheelEvent) {
    if (!e.ctrlKey && !e.metaKey) return;
    e.preventDefault();
    settings.data.iconSize = Math.max(48, Math.min(256, Math.round(icon * (e.deltaY < 0 ? 1.1 : 0.9))));
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div
  class="icons file-view"
  role="grid"
  tabindex="0"
  bind:this={scroller}
  bind:clientHeight={viewportH}
  bind:clientWidth={width}
  onscroll={() => {
    scrollTop = scroller!.scrollTop;
    tab.scrollTop = scrollTop;
  }}
  {onkeydown}
  {onwheel}
  onpointerdown={(e) => {
    ws.focusPane(tab.pane.id);
    if (e.target === e.currentTarget && e.button === 0 && scroller) startMarquee(e, scroller, tab, cellsIn, (r) => (marquee = r), () => tab.selectOnly(null));
  }}
  oncontextmenu={(e) => e.target === e.currentTarget && blankMenu(e, tab)}
  use:dropTarget={{ dest: () => (tab.writable ? tab.dirUri : null) }}
>
  <div class="spacer" style:height="{rowCount * cellH + 12}px"></div>
  {#each slice as entry, k (keyOf(entry))}
    {@const i = firstRow * cols + k}
    {@const key = keyOf(entry)}
    {@const selected = tab.selection.has(key)}
    <div
      class="cell"
      class:selected
      class:cursor={tab.cursor === key}
      class:fresh={folder.fresh.has(key)}
      class:dim={entry.hidden || clipboard.isCut(tab.uriOf(entry))}
      role="gridcell"
      tabindex="-1"
      aria-selected={selected}
      draggable={tab.renaming !== key}
      style:width="{cellW - 8}px"
      style:height="{cellH - 8}px"
      style:transform="translate({12 + (i % cols) * cellW}px, {8 + Math.floor(i / cols) * cellH}px)"
      title={entry.name}
      onpointerdown={(e) => onItemPointerDown(e, tab, entry)}
      onpointerup={(e) => onItemPointerUp(e, tab, entry)}
      ondblclick={() => !ui.phone && tab.open(entry)}
      oncontextmenu={(e) => itemMenu(e, tab, entry)}
      ondragstart={(e) => onDragStart(e, tab, entry)}
      ondragend={onDragEnd}
      use:dropTarget={{ dest: () => (entry.isDir ? tab.uriOf(entry) : null), spring: () => tab.open(entry) }}
    >
      <Thumb {entry} uri={tab.uriOf(entry)} size={icon} />
      {#if tab.renaming === key}
        <textarea class="rename" value={entry.name} use:renameInput={entry} spellcheck="false" rows="2"></textarea>
      {:else}
        <span class="name">{entry.name}</span>
      {/if}
    </div>
  {/each}
  {#if marquee}<div class="marquee" style:left="{marquee.x}px" style:top="{marquee.y}px" style:width="{marquee.w}px" style:height="{marquee.h}px"></div>{/if}
  <ViewStates {tab} rowH={cellH} />
</div>

<style>
  .icons {
    position: relative;
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    outline: none;
    contain: strict;
  }
  .spacer {
    width: 1px;
    pointer-events: none;
  }
  .marquee {
    position: absolute;
    z-index: 3;
    border: 1px solid var(--accent);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    border-radius: 2px;
    pointer-events: none;
  }
  .cell {
    position: absolute;
    top: 0;
    left: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 8px 4px 4px;
    border-radius: var(--radius);
    contain: layout style;
  }
  .cell:hover {
    background: var(--hover);
  }
  .cell.selected {
    background: var(--sel);
  }
  .icons:focus .cell.cursor {
    box-shadow: inset 0 0 0 1px var(--sel-stroke);
  }
  .cell.fresh {
    animation: fresh 1.6s var(--ease);
  }
  @keyframes fresh {
    0% {
      background: var(--fresh);
    }
  }
  .cell.dim {
    opacity: 0.55;
  }
  .cell:global(.drop-hover) {
    background: var(--accent-soft);
    box-shadow: inset 0 0 0 1.5px var(--accent);
  }
  .name {
    max-width: 100%;
    text-align: center;
    font-size: 12px;
    line-height: 16px;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    word-break: break-word;
  }
  .selected .name {
    color: var(--text);
  }
  .rename {
    width: 100%;
    resize: none;
    font: inherit;
    font-size: 12px;
    text-align: center;
    border: 0;
    border-radius: 4px;
    outline: none;
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke-strong), inset 0 -2px 0 var(--accent);
    user-select: text;
    -webkit-user-select: text;
  }
</style>
