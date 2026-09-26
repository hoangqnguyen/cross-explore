<script lang="ts">
  import { ws, type Pane } from "../workspace.svelte";
  import AddressBar from "./AddressBar.svelte";
  import ColumnsView from "./ColumnsView.svelte";
  import CompareView from "./CompareView.svelte";
  import DetailsView from "./DetailsView.svelte";
  import GalleryView from "./GalleryView.svelte";
  import HomeView from "./HomeView.svelte";
  import IconsView from "./IconsView.svelte";
  import TabStrip from "./TabStrip.svelte";

  let { pane }: { pane: Pane } = $props();
  let tab = $derived(pane.active);
  let active = $derived(!ws.dual || ws.activePane === pane.id);
</script>

<section class="pane" class:active class:dual={ws.dual} onpointerdowncapture={() => ws.focusPane(pane.id)} aria-label="Pane {pane.id + 1}">
  {#if ws.dual}
    <div class="phead"><TabStrip {pane} compact /></div>
    {#if tab}<AddressBar {tab} compact />{/if}
  {/if}
  {#if tab}
    {#key tab.id}
      {#if tab.folder.kind === "home"}
        <HomeView {tab} />
      {:else if tab.folder.kind === "compare"}
        <CompareView {tab} />
      {:else if tab.view === "icons"}
        <IconsView {tab} />
      {:else if tab.view === "columns" && tab.folder.kind === "folder"}
        <ColumnsView {tab} />
      {:else if tab.view === "gallery"}
        <GalleryView {tab} />
      {:else}
        <DetailsView {tab} />
      {/if}
    {/key}
  {/if}
</section>

<style>
  .pane {
    position: relative;
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
    min-height: 0;
    background: var(--layer);
  }
  .pane.dual + :global(.pane.dual) {
    border-left: 1px solid var(--stroke-strong);
  }
  .phead {
    display: flex;
    height: 34px;
    padding: 0 6px;
    background: var(--chrome);
    flex: none;
  }
  .pane.dual:not(.active) {
    --sel: var(--hover);
    --sel-hover: var(--pressed);
  }
  .pane.dual:not(.active) :global(.bar) {
    opacity: 0.85;
  }
</style>
