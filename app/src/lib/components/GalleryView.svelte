<script lang="ts">
  // Finder-style gallery: a large preview of the focused item over a
  // filmstrip of everything in the folder.
  import { keyOf } from "../folder.svelte";
  import { formatDate, formatSize, typeLabel } from "../format";
  import { blankMenu, dropTarget, handleNavKey, itemMenu, onDragEnd, onDragStart, onItemPointerDown, onItemPointerUp } from "../listing";
  import { ws, type Tab } from "../workspace.svelte";
  import Preview from "./Preview.svelte";
  import Thumb from "./Thumb.svelte";
  import ViewStates from "./ViewStates.svelte";

  let { tab }: { tab: Tab } = $props();

  const CELL = 88;
  let strip: HTMLDivElement | undefined = $state();
  let root: HTMLDivElement | undefined = $state();
  let scrollLeft = $state(0);
  let stripW = $state(800);
  let rows = $derived(tab.visible);
  let first = $derived(Math.max(0, Math.floor(scrollLeft / CELL) - 4));
  let last = $derived(Math.min(rows.length, Math.ceil((scrollLeft + stripW) / CELL) + 4));
  let current = $derived(tab.cursorEntry ?? rows[0] ?? null);

  // Always have something focused so the big preview isn't empty.
  $effect(() => {
    if (!tab.cursorEntry && rows.length && tab.folder.status === "ready") tab.selectOnly(keyOf(rows[0]));
  });

  function reveal() {
    const i = tab.cursor == null ? -1 : tab.indexOf(tab.cursor);
    if (!strip || i < 0) return;
    strip.scrollTo({ left: i * CELL - stripW / 2 + CELL / 2, behavior: "smooth" });
  }

  $effect(() => {
    void tab.cursor;
    requestAnimationFrame(reveal);
  });

  function onkeydown(e: KeyboardEvent) {
    if (e.target !== root) return;
    handleNavKey(e, tab, { cols: 1, page: 10, reveal, horizontal: true });
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div
  class="gallery file-view"
  tabindex="0"
  role="grid"
  bind:this={root}
  {onkeydown}
  onpointerdown={() => ws.focusPane(tab.pane.id)}
  oncontextmenu={(e) => e.target === e.currentTarget && blankMenu(e, tab)}
  use:dropTarget={{ dest: () => (tab.writable ? tab.dirUri : null) }}
>
  <div class="stage">
    {#if current}
      {#key tab.uriOf(current)}
        <Preview entry={current} uri={tab.uriOf(current)} />
      {/key}
    {/if}
    <ViewStates {tab} />
  </div>
  {#if current}
    <div class="caption">
      <strong>{current.name}</strong>
      <span>{typeLabel(current)}{current.isDir ? "" : ` · ${formatSize(current.size)}`} · {formatDate(current.modified)}</span>
    </div>
  {/if}
  <div class="strip" bind:this={strip} bind:clientWidth={stripW} onscroll={() => (scrollLeft = strip!.scrollLeft)}>
    <div class="track" style:width="{rows.length * CELL}px">
      {#each rows.slice(first, last) as entry, k (keyOf(entry))}
        {@const key = keyOf(entry)}
        <div
          class="frame"
          class:selected={tab.selection.has(key)}
          class:cursor={tab.cursor === key}
          style:transform="translateX({(first + k) * CELL}px)"
          role="gridcell"
          tabindex="-1"
          draggable="true"
          title={entry.name}
          onpointerdown={(e) => onItemPointerDown(e, tab, entry)}
          onpointerup={(e) => onItemPointerUp(e, tab, entry)}
          ondblclick={() => tab.open(entry)}
          oncontextmenu={(e) => itemMenu(e, tab, entry)}
          ondragstart={(e) => onDragStart(e, tab, entry)}
          ondragend={onDragEnd}
        >
          <Thumb {entry} uri={tab.uriOf(entry)} size={64} fit="cover" />
        </div>
      {/each}
    </div>
  </div>
</div>

<style>
  .gallery {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    outline: none;
  }
  .stage {
    position: relative;
    flex: 1;
    min-height: 0;
    padding: 20px 24px 8px;
    display: flex;
  }
  .caption {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    padding: 4px 16px 10px;
    font-size: 12px;
    color: var(--text-2);
  }
  .caption strong {
    font-size: 13px;
    color: var(--text);
    font-weight: 600;
  }
  .strip {
    height: 96px;
    flex: none;
    overflow-x: auto;
    overflow-y: hidden;
    border-top: 1px solid var(--stroke);
  }
  .track {
    position: relative;
    height: 100%;
  }
  .frame {
    position: absolute;
    top: 8px;
    left: 8px;
    width: 76px;
    height: 76px;
    display: grid;
    place-items: center;
    border-radius: var(--radius);
  }
  .frame:hover {
    background: var(--hover);
  }
  .frame.selected {
    background: var(--sel);
  }
  .frame.cursor {
    box-shadow: inset 0 0 0 2px var(--accent);
  }
</style>
