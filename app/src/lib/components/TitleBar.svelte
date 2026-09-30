<script lang="ts">
  import { appWindow } from "../api";
  import { run, shortcut } from "../commands.svelte";
  import { transfers } from "../stores/transfers.svelte";
  import { ws } from "../workspace.svelte";
  import Icon from "./Icon.svelte";
  import TabStrip from "./TabStrip.svelte";
  import TransferCapsule from "./TransferCapsule.svelte";

  // macOS keeps native traffic lights; Windows gets drawn caption buttons.
  let customCaption = $derived(ws.platform === "windows");
</script>

<header class="titlebar" class:mac={ws.platform === "macos"} data-tauri-drag-region>
  {#if ws.dual}
    <div class="dual-tabs">
      <div class="pane-tabs" class:active={ws.activePane === 0}><TabStrip pane={ws.panes[0]} compact /></div>
      <div class="pane-tabs" class:active={ws.activePane === 1}><TabStrip pane={ws.panes[1]} compact /></div>
    </div>
  {:else}
    <TabStrip pane={ws.panes[0]} />
  {/if}

  <div class="tools">
    {#if transfers.jobs.length}<TransferCapsule />{/if}
    <button class="palette" title="Command palette ({shortcut('app.palette')})" onclick={() => run("app.palette")}>
      <Icon name="search" size={14} />
      <span>Search commands</span>
      <kbd>{shortcut("app.palette")}</kbd>
    </button>
    <button class="icon-btn" title="Settings ({shortcut('app.settings')})" aria-label="Settings" onclick={() => run("app.settings")}><Icon name="settings" size={16} /></button>
  </div>

  {#if customCaption}
    <div class="caption">
      <button aria-label="Minimize" onclick={() => appWindow.minimize()}><Icon name="minimize" size={14} stroke={1} /></button>
      <button aria-label="Maximize" onclick={() => appWindow.toggleMaximize()}><Icon name="maximize" size={12} stroke={1} /></button>
      <button class="close-win" aria-label="Close" onclick={() => appWindow.close()}><Icon name="winClose" size={14} stroke={1} /></button>
    </div>
  {/if}
</header>

<style>
  .titlebar {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    height: var(--titlebar-h);
    padding-left: 8px;
    background: var(--chrome);
    flex: none;
  }
  .titlebar.mac {
    padding-left: 84px;
  }
  .dual-tabs {
    display: flex;
    align-items: flex-end;
    flex: 1;
    min-width: 0;
    height: 100%;
  }
  .pane-tabs {
    display: flex;
    flex: 1;
    min-width: 0;
    height: 100%;
  }
  .pane-tabs.active {
    /* The tab strip of the pane that has focus reads slightly stronger. */
    --text-2: var(--text);
  }
  .pane-tabs + .pane-tabs {
    border-left: 1px solid var(--stroke-strong);
  }
  .tools {
    display: flex;
    align-items: center;
    gap: 4px;
    align-self: center;
    padding-right: 8px;
  }
  .palette {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 28px;
    padding: 0 6px 0 10px;
    border-radius: var(--radius);
    color: var(--text-3);
    background: var(--hover);
    font-size: 12px;
  }
  .palette:hover {
    color: var(--text-2);
    background: var(--pressed);
  }
  kbd {
    font-family: inherit;
    font-size: 11px;
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--layer);
    color: var(--text-3);
  }
  .icon-btn {
    display: grid;
    place-items: center;
    width: 30px;
    height: 28px;
    border-radius: var(--radius);
    color: var(--text-2);
  }
  .icon-btn:hover {
    background: var(--hover);
  }
  @media (max-width: 900px) {
    .palette span,
    .palette kbd {
      display: none;
    }
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
