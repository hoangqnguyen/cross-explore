<script lang="ts">
  import { appWindow as win } from "../api";
  import { ws } from "../workspace.svelte";
  import { mod } from "../keys";
  import Icon from "./Icon.svelte";
  import FileIcon from "./FileIcon.svelte";

  // macOS keeps native traffic lights; elsewhere we draw caption buttons.
  let customCaption = $derived(ws.platform === "windows");
</script>

<header class="titlebar" class:mac={ws.platform === "macos"} data-tauri-drag-region>
  <div class="tabs" role="tablist" data-tauri-drag-region>
    {#each ws.tabs as tab (tab.id)}
      {@const active = tab.id === ws.activeId}
      <div
        class="tab"
        class:active
        role="tab"
        tabindex="-1"
        aria-selected={active}
        title={tab.folder.info?.display}
        onpointerdown={(e) => e.button === 0 && ws.activate(tab.id)}
        onauxclick={(e) => e.button === 1 && ws.closeTab(tab.id)}
      >
        {#if tab.folder.info?.crumbs.length === 1 && tab.folder.info.crumbs[0].icon === "home"}
          <Icon name="home" size={15} />
        {:else}
          <FileIcon name="" isDir size={16} />
        {/if}
        <span class="title">{tab.title || "Loading…"}</span>
        <button class="close" title="Close tab ({mod}W)" aria-label="Close tab" onpointerdown={(e) => e.stopPropagation()} onclick={() => ws.closeTab(tab.id)}>
          <Icon name="close" size={12} stroke={1.6} />
        </button>
      </div>
    {/each}
    <button class="new" title="New tab ({mod}T)" aria-label="New tab" onclick={() => ws.newTab()}>
      <Icon name="plus" size={14} />
    </button>
  </div>

  {#if customCaption}
    <div class="caption">
      <button aria-label="Minimize" onclick={() => win.minimize()}><Icon name="minimize" size={14} stroke={1} /></button>
      <button aria-label="Maximize" onclick={() => win.toggleMaximize()}><Icon name="maximize" size={12} stroke={1} /></button>
      <button class="close-win" aria-label="Close" onclick={() => win.close()}><Icon name="winClose" size={14} stroke={1} /></button>
    </div>
  {/if}
</header>

<style>
  .titlebar {
    display: flex;
    align-items: flex-end;
    height: var(--titlebar-h);
    padding-left: 8px;
    background: var(--chrome);
    flex: none;
  }
  .titlebar.mac {
    padding-left: 84px;
  }
  .tabs {
    display: flex;
    align-items: flex-end;
    flex: 1;
    min-width: 0;
    height: 100%;
    gap: 2px;
  }
  .tab {
    position: relative;
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 0 1 230px;
    min-width: 96px;
    height: 36px;
    padding: 0 6px 0 12px;
    border-radius: var(--radius-lg) var(--radius-lg) 0 0;
    color: var(--text-2);
    transition: background 0.1s;
  }
  .tab:hover:not(.active) {
    background: var(--hover);
  }
  .tab.active {
    background: var(--layer);
    color: var(--text);
    box-shadow: 0 0 0 1px var(--stroke);
    clip-path: inset(-1px -1px 0 -1px);
  }
  /* Separators between inactive tabs, as in Windows 11. */
  .tab:not(.active):not(:hover) + .tab:not(.active):not(:hover)::before {
    content: "";
    position: absolute;
    left: -1px;
    top: 10px;
    bottom: 10px;
    width: 1px;
    background: var(--stroke-strong);
  }
  .title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .close {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    border-radius: 4px;
    color: var(--text-2);
    opacity: 0;
  }
  .tab:hover .close,
  .tab.active .close {
    opacity: 1;
  }
  .close:hover {
    background: var(--hover);
  }
  .new {
    display: grid;
    place-items: center;
    width: 32px;
    height: 30px;
    margin: 0 0 3px 2px;
    border-radius: var(--radius);
    color: var(--text-2);
  }
  .new:hover {
    background: var(--hover);
  }
  .caption {
    display: flex;
    align-self: flex-start;
    height: 36px;
  }
  .caption button {
    display: grid;
    place-items: center;
    width: 46px;
    height: 100%;
  }
  .caption button:hover {
    background: var(--hover);
  }
  .caption .close-win:hover {
    background: #c42b1c;
    color: #fff;
  }
</style>
