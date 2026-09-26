<script lang="ts">
  import { ws } from "../workspace.svelte";
  import { formatSize } from "../format";
  import Icon, { type IconName } from "./Icon.svelte";

  let current = $derived(norm(ws.active?.folder.info?.uri ?? ""));
  let collapsed = $state<Record<string, boolean>>({});

  function norm(uri: string) {
    return uri.replace(/\/+$/, "");
  }

  function go(uri: string, e: MouseEvent) {
    // Cmd/Ctrl-click or middle-click opens in a new tab, like a browser.
    if (e.metaKey || e.ctrlKey || e.button === 1) ws.newTab(uri);
    else ws.active?.navigate(uri);
  }

  let dragging = false;
  function startResize(e: PointerEvent) {
    dragging = true;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function resize(e: PointerEvent) {
    if (dragging) ws.settings.sidebarWidth = Math.round(Math.min(360, Math.max(160, e.clientX)));
  }
  function endResize() {
    if (dragging) ws.save();
    dragging = false;
  }
</script>

{#snippet item(label: string, uri: string, icon: IconName, extra?: { used: number } | null)}
  <button class="item" class:current={current === norm(uri)} onclick={(e) => go(uri, e)} onauxclick={(e) => go(uri, e)} title={label}>
    <span class="icon"><Icon name={icon} size={16} /></span>
    <span class="text">
      <span class="label">{label}</span>
      {#if extra}
        <span class="meter"><span style:width="{Math.round(extra.used * 100)}%" class:full={extra.used > 0.9}></span></span>
      {/if}
    </span>
  </button>
{/snippet}

{#snippet section(id: string, title: string)}
  <button class="section" onclick={() => (collapsed[id] = !collapsed[id])} aria-expanded={!collapsed[id]}>
    <span class="chev" class:closed={collapsed[id]}><Icon name="chevronDown" size={11} stroke={1.8} /></span>
    {title}
  </button>
{/snippet}

<nav class="sidebar" style:width="{ws.settings.sidebarWidth}px">
  {#if ws.places}
    <div class="group">
      {@render item(ws.places.home.name, ws.places.home.uri, "home")}
    </div>

    {@render section("fav", "Favorites")}
    {#if !collapsed.fav}
      <div class="group">
        {#each ws.places.favorites as p (p.uri)}
          {@render item(p.name, p.uri, p.icon as IconName)}
        {/each}
      </div>
    {/if}

    {@render section("drives", "Drives")}
    {#if !collapsed.drives}
      <div class="group">
        {#each ws.places.volumes as v (v.uri)}
          <div title="{formatSize(v.free)} free of {formatSize(v.total)}">
            {@render item(v.name, v.uri, v.removable ? "external" : "drive", v.total ? { used: 1 - v.free / v.total } : null)}
          </div>
        {/each}
      </div>
    {/if}
  {/if}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="grip" onpointerdown={startResize} onpointermove={resize} onpointerup={endResize}></div>
</nav>

<style>
  .sidebar {
    position: relative;
    flex: none;
    overflow-y: auto;
    padding: 8px 6px 12px;
  }
  .group {
    display: flex;
    flex-direction: column;
    gap: 1px;
    margin-bottom: 6px;
  }
  .section {
    display: flex;
    align-items: center;
    gap: 4px;
    width: 100%;
    padding: 8px 8px 4px;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-3);
  }
  .section:hover {
    color: var(--text-2);
  }
  .chev {
    transition: transform 0.15s var(--ease);
  }
  .chev.closed {
    transform: rotate(-90deg);
  }
  .item {
    position: relative;
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    min-height: 32px;
    padding: 4px 10px;
    border-radius: var(--radius);
    text-align: left;
  }
  .item:hover {
    background: var(--hover);
  }
  .item.current {
    background: var(--pressed);
  }
  /* Windows 11's accent pill on the selected navigation item. */
  .item.current::before {
    content: "";
    position: absolute;
    left: 0;
    top: 9px;
    bottom: 9px;
    width: 3px;
    border-radius: 2px;
    background: var(--accent);
  }
  .icon {
    color: var(--accent);
  }
  .text {
    flex: 1;
    min-width: 0;
  }
  .label {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meter {
    display: block;
    height: 4px;
    margin: 4px 0 2px;
    border-radius: 2px;
    background: var(--stroke-strong);
    overflow: hidden;
  }
  .meter span {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  .meter span.full {
    background: var(--danger);
  }
  .grip {
    position: absolute;
    top: 0;
    right: 0;
    bottom: 0;
    width: 6px;
    cursor: col-resize;
  }
</style>
