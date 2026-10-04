<script lang="ts">
  // Space-bar preview, floating over the window. Arrow keys keep walking the
  // list underneath, so you can flip through photos without closing it.
  import { errorText, revealEntry } from "../api";
  import { keyOf } from "../folder.svelte";
  import { formatSize, typeLabel } from "../format";
  import { handleNavKey } from "../listing";
  import { quicklook } from "../stores/quicklook.svelte";
  import { toasts } from "../toasts.svelte";
  import { ws } from "../workspace.svelte";
  import Icon from "./Icon.svelte";
  import Preview from "./Preview.svelte";

  let tab = $derived(ws.activeTab);
  let entry = $derived(tab?.cursorEntry ?? tab?.selectedEntries[0] ?? null);
  let index = $derived(entry ? tab.indexOf(keyOf(entry!)) : -1);
  let box: HTMLDivElement | undefined = $state();

  $effect(() => {
    if (quicklook.open && !entry) quicklook.close();
  });

  $effect(() => {
    if (quicklook.open) box?.focus();
  });

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape" || e.key === " " || e.key === "F3") {
      quicklook.close();
      e.preventDefault();
      (document.querySelector(".pane.active .file-view") as HTMLElement | null)?.focus();
      return;
    }
    if (e.key === "Enter" && entry) {
      tab.open(entry);
      quicklook.close();
      e.preventDefault();
      return;
    }
    handleNavKey(e, tab, { cols: 1, page: 10, reveal: () => {}, horizontal: true });
    e.stopPropagation();
  }
</script>

{#if quicklook.open && entry}
  <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
  <div class="backdrop" onclick={() => quicklook.close()}></div>
  <div class="ql" role="dialog" aria-label="Quick Look" tabindex="-1" bind:this={box} {onkeydown}>
    <header>
      <div class="title">
        <strong>{entry.name}</strong>
        <span>{typeLabel(entry)}{entry.isDir ? "" : ` · ${formatSize(entry.size)}`}{index >= 0 ? ` · ${index + 1} of ${tab.visible.length}` : ""}</span>
      </div>
      <button title="Open" onclick={() => tab.open(entry!)}><Icon name="open" size={16} /></button>
      {#if tab.folder.info?.local}
        <button title="Show in folder" onclick={() => revealEntry(tab.uriOf(entry!)).catch((e) => toasts.show(errorText(e), "error"))}><Icon name="folder" size={16} /></button>
      {/if}
      <button title="Close (Space)" onclick={() => quicklook.close()}><Icon name="close" size={14} /></button>
    </header>
    <div class="body">
      {#key tab.uriOf(entry)}
        <Preview {entry} uri={tab.uriOf(entry)} large />
      {/key}
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 70;
    background: rgba(0, 0, 0, 0.18);
    animation: fade 0.12s;
  }
  .ql {
    position: fixed;
    z-index: 71;
    inset: 6% 10%;
    display: flex;
    flex-direction: column;
    border-radius: 12px;
    background: var(--flyout);
    backdrop-filter: blur(40px) saturate(1.5);
    -webkit-backdrop-filter: blur(40px) saturate(1.5);
    box-shadow: 0 24px 64px rgba(0, 0, 0, 0.35), 0 0 0 1px var(--stroke-strong);
    outline: none;
    animation: zoom 0.16s var(--ease);
  }
  @keyframes zoom {
    from {
      opacity: 0;
      transform: scale(0.96);
    }
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  header {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 8px 8px 8px 16px;
    border-bottom: 1px solid var(--stroke);
  }
  .title {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .title strong {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .title span {
    font-size: 11.5px;
    color: var(--text-3);
  }
  header button {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: var(--radius);
    color: var(--text-2);
  }
  header button:hover {
    background: var(--hover);
  }
  .body {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
    padding: 16px;
  }
</style>
