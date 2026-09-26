<script lang="ts">
  import { ws } from "../workspace.svelte";
  import { isMac, mod } from "../keys";
  import Icon from "./Icon.svelte";

  let tab = $derived(ws.active);
  let info = $derived(tab?.folder.info);
  let editing = $state(false);
  let draft = $state("");
  let pathInput: HTMLInputElement | undefined = $state();
  let searchInput: HTMLInputElement | undefined = $state();
  let crumbsEl: HTMLDivElement | undefined = $state();

  // When the path is too long, keep its end (the current folder) in view.
  $effect(() => {
    void info?.uri;
    if (crumbsEl) crumbsEl.scrollLeft = crumbsEl.scrollWidth;
  });

  export function editPath() {
    draft = info?.display ?? "";
    editing = true;
    queueMicrotask(() => pathInput?.select());
  }

  export function focusSearch() {
    searchInput?.focus();
    searchInput?.select();
  }

  function commit() {
    editing = false;
    const target = draft.trim();
    if (target && target !== info?.display) tab?.navigate(target);
  }

  function onPathKey(e: KeyboardEvent) {
    if (e.key === "Enter") commit();
    else if (e.key === "Escape") editing = false;
  }

  function onSearchKey(e: KeyboardEvent) {
    if (e.key === "Escape" || e.key === "Enter" || e.key === "ArrowDown") {
      if (e.key === "Escape") tab.filter = "";
      (document.querySelector(".details") as HTMLElement | null)?.focus();
      e.preventDefault();
    }
  }
</script>

<div class="bar">
  <div class="nav">
    <button title={isMac ? "Back (⌘[)" : "Back (Alt+Left)"} aria-label="Back" disabled={!tab?.canBack} onclick={() => tab.back()}><Icon name="back" /></button>
    <button title={isMac ? "Forward (⌘])" : "Forward (Alt+Right)"} aria-label="Forward" disabled={!tab?.canForward} onclick={() => tab.forward()}><Icon name="forward" /></button>
    <button title={isMac ? "Enclosing folder (⌘↑)" : "Up (Alt+Up)"} aria-label="Up" disabled={!info?.parent} onclick={() => tab.up()}><Icon name="up" /></button>
  </div>

  <div class="address" class:editing>
    {#if editing}
      <input bind:this={pathInput} bind:value={draft} onkeydown={onPathKey} onblur={() => (editing = false)} spellcheck="false" aria-label="Path" />
    {:else}
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
      <div class="crumbs" bind:this={crumbsEl} onclick={(e) => e.target === e.currentTarget && editPath()} title="Click to type a path ({mod}L)">
        {#each info?.crumbs ?? [] as crumb, i (crumb.uri)}
          {#if i > 0}<span class="sep"><Icon name="chevronRight" size={12} /></span>{/if}
          <button class="crumb" class:current={i === info!.crumbs.length - 1} onclick={() => tab.navigate(crumb.uri)}>
            {#if crumb.icon === "home"}<Icon name="home" size={14} />{:else if crumb.icon === "drive"}<Icon name="drive" size={14} />{/if}
            {crumb.label}
          </button>
        {/each}
      </div>
      <!-- The live dot: changes here appear without refreshing. -->
      {#if tab?.folder.refreshing || tab?.folder.status === "loading"}
        <span class="state loading" title="Loading…"><span class="spinner"></span></span>
      {:else if tab?.folder.live}
        <span class="state live" title="Live: changes to this folder appear automatically"><span class="dot"></span>Live</span>
      {/if}
    {/if}
  </div>

  <label class="search" class:active={!!tab?.filter}>
    <Icon name="search" size={14} />
    <input bind:this={searchInput} bind:value={tab.filter} onkeydown={onSearchKey} placeholder="Filter {info?.name ?? ''}" spellcheck="false" aria-label="Filter" />
    {#if tab?.filter}
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
  .crumb.current {
    color: var(--text);
  }
  .crumb:hover {
    background: var(--hover);
  }
  .sep {
    color: var(--text-3);
    display: grid;
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
    width: clamp(160px, 24vw, 280px);
    height: 32px;
    padding: 0 6px 0 10px;
    border-radius: var(--radius);
    background: var(--layer-2);
    box-shadow: inset 0 0 0 1px var(--stroke);
    color: var(--text-2);
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
  }
  .clear:hover {
    background: var(--hover);
  }
</style>
