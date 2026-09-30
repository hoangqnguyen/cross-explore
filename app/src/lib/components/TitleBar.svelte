<script lang="ts">
  import { appWindow } from "../api";
  import { run, shortcut } from "../commands.svelte";
  import { settings } from "../stores/settings.svelte";
  import { transfers } from "../stores/transfers.svelte";
  import { ws } from "../workspace.svelte";
  import Icon from "./Icon.svelte";
  import TabStrip from "./TabStrip.svelte";
  import TransferCapsule from "./TransferCapsule.svelte";

  // macOS keeps native traffic lights. Windows has no system title bar (it is
  // removed so the tabs can sit in it, over Mica), so these are the minimize /
  // maximize / close buttons. Detect Windows from the web view too: until
  // places load, `platform` defaults to "macos", which would hide them.
  let customCaption = $derived(ws.platform === "windows" || /Windows/i.test(navigator.userAgent));
  // The title bar's own left padding (room for traffic lights) isn't the
  // sidebar: pad the rest of the way so the tab split lines up with where
  // the sidebar actually ends, below.
  let leftPad = $derived(ws.platform === "macos" ? 84 : 8);
  // .titlebar's flex `gap` adds 8px between the spacer and the tab clusters
  // that .panes doesn't have, so it comes out of the spacer.
  let sidebarGap = $derived(Math.max(0, settings.data.sidebarWidth - leftPad - 8));
  // The search box is taken out of the flex row (below) so it can't shrink
  // the tab strip. When it — or the Windows caption buttons — reaches back
  // over the panes, pad the right cluster by that much. Padding is inside
  // the cluster, so the split between the two tab rows stays on the split
  // between the panes.
  let toolsWidth = $state(0);
  let toolsInset = $derived(customCaption ? 138 : 8);
  let previewW = $derived(settings.data.previewPane ? settings.data.previewWidth : 0);
  let rightInset = $derived(Math.max(0, toolsWidth + toolsInset - previewW));
</script>

{#snippet splitBtn()}
  <button class="icon-btn split-btn" class:on={ws.dual} title="Dual pane ({shortcut('pane.dual')})" aria-label="Dual pane" onclick={() => run('pane.dual')}>
    <Icon name="columns" size={16} />
  </button>
{/snippet}

<header class="titlebar" class:mac={ws.platform === "macos"} class:win={customCaption} style:padding-right={!ws.dual ? `${toolsWidth + toolsInset}px` : undefined} data-tauri-drag-region>
  {#if ws.dual}
    <div class="sidebar-gap" style:width="{sidebarGap}px" data-tauri-drag-region></div>
    <!-- Exactly as wide as .panes, and not allowed to shrink. A max-width
         left this at the tabs' own width (the search box's auto margin ate
         the free space), so the split between the clusters landed left of
         the split between the panes. -->
    <div class="dual-tabs" class:measured={ws.panesWidth > 0} style:width={ws.panesWidth > 0 ? `${ws.panesWidth}px` : undefined}>
      <div class="pane-tabs" class:active={ws.activePane === 0}><TabStrip pane={ws.panes[0]} compact /></div>
      <div class="pane-tabs" class:active={ws.activePane === 1}>
        <!-- Padding lives inside the cluster. On the cluster itself it
             widened that flex item and pulled the divider off the panes. -->
        <div class="pane-tabs-in" style:padding-right={rightInset > 0 ? `${rightInset}px` : undefined}>
          <TabStrip pane={ws.panes[1]} compact trailing={splitBtn} />
        </div>
      </div>
    </div>
  {:else}
    <TabStrip pane={ws.panes[0]} trailing={splitBtn} />
  {/if}

  <div class="tools" bind:clientWidth={toolsWidth}>
    {#if transfers.jobs.length}<TransferCapsule />{/if}
    <button class="palette" title="Command palette ({shortcut('app.palette')})" onclick={() => run("app.palette")}>
      <Icon name="search" size={14} />
      <span>Search commands</span>
      <kbd>{shortcut("app.palette")}</kbd>
    </button>
    <button class="icon-btn" title="Settings ({shortcut('app.settings')})" aria-label="Settings" onclick={() => run("app.settings")}><Icon name="settings" size={16} /></button>
  </div>

  {#if customCaption}
    <!-- Out of the flex row, pinned to the window corner, so tabs and the
         search box can never push them off-screen. -->
    <div class="caption" data-tauri-drag-region="false">
      <button aria-label="Minimize" title="Minimize" onclick={() => appWindow.minimize()}><Icon name="minimize" size={10} stroke={1} /></button>
      <button aria-label="Maximize" title="Maximize" onclick={() => appWindow.toggleMaximize()}><Icon name="maximize" size={10} stroke={1} /></button>
      <button class="close-win" aria-label="Close" title="Close" onclick={() => appWindow.close()}><Icon name="winClose" size={10} stroke={1} /></button>
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
    min-width: 0;
    position: relative;
  }
  .titlebar.mac {
    padding-left: 84px;
  }
  /* Room for the three caption buttons (46px each), which are taken out of flow. */
  .titlebar.win {
    padding-right: 138px;
  }
  .sidebar-gap {
    flex: none;
    height: 100%;
  }
  .dual-tabs {
    display: flex;
    align-items: flex-end;
    flex: 1 1 auto;
    min-width: 0;
    height: 100%;
  }
  .dual-tabs.measured {
    flex: none;
  }
  .pane-tabs {
    display: flex;
    flex: 1;
    min-width: 0;
    height: 100%;
  }
  .pane-tabs-in {
    display: flex;
    flex: 1;
    min-width: 0;
    height: 100%;
    box-sizing: border-box;
  }
  .pane-tabs.active {
    /* The tab strip of the pane that has focus reads slightly stronger. */
    --text-2: var(--text);
  }
  .pane-tabs + .pane-tabs {
    border-left: 1px solid var(--stroke-strong);
  }
  /* Rendered inside TabStrip's .tabs (as its `trailing` snippet), right after
     the "+" button, so it sits next to it instead of being pushed away by
     whatever stretch space .tabs picks up. Margin mirrors .new's own
     bottom-alignment within that flex-end row. */
  .split-btn {
    flex: none;
    margin: 0 0 3px 4px;
  }
  :global(.tabs.compact) .split-btn {
    margin-bottom: 2px;
  }
  .split-btn.on {
    background: var(--accent-soft);
  }
  .split-btn.on :global(svg) {
    color: var(--accent);
  }
  .tools {
    position: absolute;
    top: 0;
    right: 8px;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 4px;
    height: 100%;
  }
  .titlebar.win .tools {
    right: 138px;
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
    position: absolute;
    top: 0;
    right: 0;
    z-index: 2;
    display: flex;
    height: 100%;
  }
  .caption button {
    display: grid;
    place-items: center;
    width: 46px;
    height: 100%;
    color: var(--text);
  }
  .caption button:hover {
    background: var(--hover);
  }
  .caption button:active {
    background: var(--pressed);
  }
  .caption .close-win:hover {
    background: #e81123;
    color: #fff;
  }
  .caption .close-win:active {
    background: #c50f1f;
    color: #fff;
  }
</style>
