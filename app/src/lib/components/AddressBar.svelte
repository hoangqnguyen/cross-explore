<script lang="ts">
  import { childUri, listDir, type Entry } from "../api";
  import { shortcut } from "../commands.svelte";
  import { isMac, primary } from "../keys";
  import { dropTarget } from "../listing";
  import { menu } from "../menu.svelte";
  import { searchUri } from "../search.svelte";
  import { ws, type Tab } from "../workspace.svelte";
  import Icon from "./Icon.svelte";

  let { tab, compact = false }: { tab: Tab; compact?: boolean } = $props();

  let info = $derived(tab.folder.info);
  let editing = $state(false);
  let draft = $state("");
  let pathInput: HTMLInputElement | undefined = $state();
  let searchInput: HTMLInputElement | undefined = $state();
  let crumbsEl: HTMLDivElement | undefined = $state();
  let mine = $derived(ws.activeTab === tab);

  // When the path is too long, keep its end (the current folder) in view.
  $effect(() => {
    void info?.uri;
    if (crumbsEl) crumbsEl.scrollLeft = crumbsEl.scrollWidth;
  });

  $effect(() => {
    const edit = () => mine && editPath();
    const find = () => {
      if (!mine) return;
      searchInput?.focus();
      searchInput?.select();
    };
    document.addEventListener("cx:edit-path", edit);
    document.addEventListener("cx:focus-search", find);
    return () => {
      document.removeEventListener("cx:edit-path", edit);
      document.removeEventListener("cx:focus-search", find);
    };
  });

  function editPath() {
    draft = info && tab.folder.kind === "folder" ? info.display : "";
    editing = true;
    queueMicrotask(() => pathInput?.select());
  }

  function commit() {
    editing = false;
    const target = draft.trim();
    if (target && target !== info?.display) tab.navigate(target);
    focusList();
  }

  function focusList() {
    requestAnimationFrame(() => (document.querySelector(".pane.active .file-view") as HTMLElement | null)?.focus());
  }

  function onPathKey(e: KeyboardEvent) {
    if (e.key === "Enter") commit();
    else if (e.key === "Escape") {
      editing = false;
      focusList();
    }
    e.stopPropagation();
  }

  function onSearchKey(e: KeyboardEvent) {
    if (e.key === "Enter" && primary(e) && tab.filter.trim()) {
      // ⌘/Ctrl+Enter: search this folder and everything below it.
      tab.navigate(searchUri(tab.dirUri, tab.filter.trim()));
      e.preventDefault();
    } else if (e.key === "Escape" || e.key === "Enter" || e.key === "ArrowDown") {
      if (e.key === "Escape") tab.filter = "";
      focusList();
      e.preventDefault();
    }
    e.stopPropagation();
  }

  /** Explorer's crumb chevrons: a menu of the folder's subfolders. */
  async function subfolders(uri: string, anchor: HTMLElement) {
    const dirs: Entry[] = [];
    try {
      await listDir(uri, (ev) => ev.type === "batch" && dirs.push(...ev.entries.filter((x) => x.isDir && !x.hidden)));
    } catch {
      return;
    }
    dirs.sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
    menu.showBelow(
      dirs.length ? dirs.slice(0, 60).map((d) => ({ label: d.name, icon: "folder" as const, action: () => tab.navigate(childUri(uri, d.name)) })) : [{ label: "No subfolders", disabled: true, action: () => {} }],
      anchor,
    );
  }
</script>

<div class="bar" class:compact>
  <div class="nav">
    <button title="Back ({shortcut('nav.back')})" aria-label="Back" disabled={!tab.canBack} onclick={() => tab.back()}><Icon name="back" /></button>
    <button title="Forward ({shortcut('nav.forward')})" aria-label="Forward" disabled={!tab.canForward} onclick={() => tab.forward()}><Icon name="forward" /></button>
    <button title="{isMac ? 'Enclosing folder' : 'Up'} ({shortcut('nav.up')})" aria-label="Up" disabled={!info?.parent} onclick={() => tab.up()}><Icon name="up" /></button>
  </div>

  <div class="address" class:editing>
    {#if editing}
      <input bind:this={pathInput} bind:value={draft} onkeydown={onPathKey} onblur={() => (editing = false)} spellcheck="false" aria-label="Path" placeholder="Path, ~/folder, smb://server/share, sftp://user@host…" />
    {:else}
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
      <div class="crumbs" bind:this={crumbsEl} onclick={(e) => e.target === e.currentTarget && editPath()} title="Click to type a path ({shortcut('nav.editPath')})">
        {#each info?.crumbs ?? [] as crumb, i (crumb.uri + i)}
          {#if i > 0}
            <button class="sep" aria-label="Subfolders" onclick={(e) => subfolders(info!.crumbs[i - 1].uri, e.currentTarget)}><Icon name="chevronRight" size={12} /></button>
          {/if}
          <button class="crumb" class:current={i === info!.crumbs.length - 1} onclick={() => tab.navigate(crumb.uri)} use:dropTarget={{ dest: () => (crumb.icon === "search" || crumb.icon === "tag" ? null : crumb.uri) }}>
            {#if crumb.icon !== "folder"}<Icon name={crumb.icon === "share" ? "folder" : crumb.icon === "drive" ? "drive" : crumb.icon} size={14} />{/if}
            {crumb.label}
          </button>
        {/each}
      </div>
      {#if tab.folder.refreshing || tab.folder.status === "loading"}
        <span class="state" title="Loading…"><span class="spinner"></span></span>
      {:else if tab.folder.live === "live"}
        <span class="state live" title="Live: changes to this folder appear automatically"><span class="dot"></span>{compact ? "" : "Live"}</span>
      {:else if tab.folder.live === "polling"}
        <span class="state polling" title="This server can't push changes, so the folder is checked every few seconds"><span class="dot"></span>{compact ? "" : "Auto"}</span>
      {/if}
    {/if}
  </div>

  <label class="search" class:active={!!tab.filter}>
    <Icon name="search" size={14} />
    <input bind:this={searchInput} bind:value={tab.filter} onkeydown={onSearchKey} placeholder="Filter {info?.name ?? ''}" spellcheck="false" aria-label="Filter" />
    {#if tab.filter}
      <button class="clear" title="Search subfolders too ({isMac ? '⌘' : 'Ctrl+'}Enter)" aria-label="Search subfolders" onclick={() => tab.navigate(searchUri(tab.dirUri, tab.filter.trim()))}><Icon name="radar" size={13} /></button>
      <button class="clear" aria-label="Clear filter" onclick={() => (tab.filter = "")}><Icon name="close" size={12} /></button>
    {/if}
  </label>
</div>

<style>
  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 44px;
    padding: 0 10px 0 6px;
    background: var(--layer);
    flex: none;
  }
  .bar.compact {
    height: 38px;
    gap: 6px;
  }
  .nav {
    display: flex;
    gap: 2px;
  }
  .nav button {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: var(--radius);
  }
  .compact .nav button {
    width: 28px;
    height: 28px;
  }
  .nav button:hover:not(:disabled) {
    background: var(--hover);
  }
  .nav button:disabled {
    color: var(--text-3);
    opacity: 0.6;
  }
  .address {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    height: 32px;
    padding: 0 4px;
    border-radius: var(--radius);
    background: var(--layer-2);
    box-shadow: inset 0 0 0 1px var(--stroke);
  }
  .compact .address,
  .compact .search {
    height: 28px;
  }
  .address.editing {
    box-shadow: inset 0 0 0 1px var(--stroke), inset 0 -2px 0 var(--accent);
  }
  .address input,
  .search input {
    flex: 1;
    min-width: 0;
    height: 100%;
    border: 0;
    outline: none;
    background: transparent;
    padding: 0 6px;
  }
  .crumbs {
    flex: 1;
    min-width: 0;
    height: 100%;
    display: flex;
    align-items: center;
    overflow: hidden;
  }
  .crumbs > * {
    flex: none;
  }
  .crumb {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 26px;
    padding: 0 6px;
    border-radius: 4px;
    white-space: nowrap;
    color: var(--text-2);
  }
  .compact .crumb {
    height: 22px;
  }
  .crumb.current {
    color: var(--text);
  }
  .crumb:hover,
  .sep:hover {
    background: var(--hover);
  }
  .crumb:global(.drop-hover) {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .sep {
    color: var(--text-3);
    display: grid;
    place-items: center;
    height: 22px;
    width: 16px;
    border-radius: 4px;
  }
  .state {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 8px;
    font-size: 12px;
    color: var(--text-2);
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--live);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--live) 22%, transparent);
    animation: breathe 3s ease-in-out infinite;
  }
  .polling .dot {
    background: var(--text-3);
    box-shadow: none;
    animation: none;
  }
  @keyframes breathe {
    50% {
      box-shadow: 0 0 0 5px color-mix(in srgb, var(--live) 8%, transparent);
    }
  }
  .spinner {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: 2px solid var(--stroke-strong);
    border-top-color: var(--accent);
    animation: spin 0.7s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(1turn);
    }
  }
  .search {
    display: flex;
    align-items: center;
    gap: 2px;
    width: clamp(140px, 22vw, 280px);
    height: 32px;
    padding: 0 6px 0 10px;
    border-radius: var(--radius);
    background: var(--layer-2);
    box-shadow: inset 0 0 0 1px var(--stroke);
    color: var(--text-2);
  }
  .compact .search {
    width: clamp(110px, 14vw, 200px);
  }
  .search:focus-within,
  .search.active {
    box-shadow: inset 0 0 0 1px var(--stroke), inset 0 -2px 0 var(--accent);
  }
  .search input {
    color: var(--text);
  }
  .search input::placeholder {
    color: var(--text-3);
  }
  .clear {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border-radius: 4px;
    flex: none;
  }
  .clear:hover {
    background: var(--hover);
  }
</style>
