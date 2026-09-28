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

  let { tab }: { tab: Tab } = $props();

  const MAX_ANCESTORS = 3;
  const cache = new Map<string, Folder>();
  let wrap: HTMLDivElement | undefined = $state();
  let current: HTMLDivElement | undefined = $state();

  function folderFor(uri: string): Folder {
    let f = cache.get(uri);
    if (!f) {
      f = new Folder(uri, settings.data.sort);
      void f.load();
      cache.set(uri, f);
    }
    return f;
  }

  $effect(() => () => cache.forEach((f) => f.dispose()));

  let crumbs = $derived(tab.folder.info?.crumbs ?? []);
  let ancestors = $derived(crumbs.slice(Math.max(0, crumbs.length - 1 - MAX_ANCESTORS), -1).map((c, i, all) => ({ uri: c.uri, label: c.label, child: (all[i + 1] ?? crumbs.at(-1))!.label })));
  let focusEntry = $derived(tab.cursorEntry);
  let next = $derived(focusEntry?.isDir && tab.selection.size <= 1 ? childUri(tab.dirUri, focusEntry.name) : null);

  const visibleOf = (items: Item[]) => (settings.data.showHidden ? items : items.filter((e) => !e.hidden));

  // Keep the focused column in view.
  $effect(() => {
    void tab.folder.uri;
    requestAnimationFrame(() => wrap && (wrap.scrollLeft = wrap.scrollWidth));
  });

  function reveal() {
    const el = current?.querySelector(".item.cursor") as HTMLElement | null;
    el?.scrollIntoView({ block: "nearest" });
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
    <div class="column" use:dropTarget={{ dest: () => a.uri }}>
      {#each visibleOf(f.items) as e (e.name)}
        <button class="item" class:trail={e.name === a.child} ondblclick={() => tab.open({ ...e, uri: childUri(a.uri, e.name) })} onclick={() => (e.isDir ? tab.navigate(childUri(a.uri, e.name)) : tab.navigate(a.uri, e.name))}>
          <FileIcon name={e.name} isDir={e.isDir} executable={e.executable} size={16} />
          <span>{e.name}</span>
          {#if e.isDir}<Icon name="chevronRight" size={11} />{/if}
        </button>
      {/each}
    </div>
  {/each}

  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    class="column current file-view"
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
    {#each tab.visible as e (keyOf(e))}
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
    {/each}
  </div>

  {#if next}
    {@const f = folderFor(next)}
    <div class="column">
      {#each visibleOf(f.items) as e (e.name)}
        <button class="item" onclick={() => tab.navigate(next!, e.name)} ondblclick={() => tab.open({ ...e, uri: childUri(next!, e.name) })}>
          <FileIcon name={e.name} isDir={e.isDir} executable={e.executable} size={16} />
          <span>{e.name}</span>
          {#if e.isDir}<Icon name="chevronRight" size={11} />{/if}
        </button>
      {/each}
      {#if f.status === "ready" && !visibleOf(f.items).length}<div class="hint">Empty folder</div>{/if}
    </div>
  {:else if focusEntry && !focusEntry.isDir}
    <div class="column preview">
      {#key tab.uriOf(focusEntry)}
        <Preview entry={focusEntry} uri={tab.uriOf(focusEntry)} />
      {/key}
      <div class="pname">{focusEntry.name}</div>
    </div>
  {/if}
  <div class="filler" title={uriName(tab.dirUri)}></div>
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
    flex: none;
    width: 240px;
    overflow-y: auto;
    padding: 6px;
    border-right: 1px solid var(--stroke);
    outline: none;
  }
  .column.preview {
    width: 300px;
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
