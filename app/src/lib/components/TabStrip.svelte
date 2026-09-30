<script lang="ts">
  import type { Snippet } from "svelte";
  import { shortcut } from "../commands.svelte";
  import { dropTarget } from "../listing";
  import { ws, type Pane } from "../workspace.svelte";
  import FileIcon from "./FileIcon.svelte";
  import Icon, { type IconName } from "./Icon.svelte";

  // `trailing`: extra buttons (the split-pane toggle) right after the "+" —
  // inside the same stretchy row, so they sit next to it instead of being
  // pushed away by whatever fills the rest of the bar.
  let { pane, compact = false, trailing }: { pane: Pane; compact?: boolean; trailing?: Snippet } = $props();

  let dragId: number | null = null;

  function iconFor(tab: Pane["tabs"][number]): IconName | null {
    const c = tab.folder.info?.crumbs;
    const kind = tab.folder.kind;
    if (kind === "home") return "home";
    if (kind === "search") return tab.folder.uri.startsWith("cx:tag") ? "tag" : "search";
    if (kind === "compare") return "sync";
    if (c?.length === 1 && c[0].icon === "home") return "home";
    const last = c?.at(-1)?.icon;
    if (last === "server" || last === "share") return "server";
    if (last === "archive") return "archive";
    return null;
  }
</script>

<div class="tabs" class:compact role="tablist" data-tauri-drag-region>
  {#each pane.tabs as tab (tab.id)}
    {@const active = tab.id === pane.activeId}
    {@const icon = iconFor(tab)}
    <div
      class="tab"
      class:active
      class:focused={active && (!ws.dual || ws.activePane === pane.id)}
      role="tab"
      tabindex="-1"
      aria-selected={active}
      title={tab.folder.info?.display}
      draggable="true"
      onpointerdown={(e) => {
        if (e.button === 0) {
          ws.focusPane(pane.id);
          pane.activate(tab.id);
        }
      }}
      onauxclick={(e) => e.button === 1 && pane.close(tab.id)}
      ondragstart={(e) => {
        dragId = tab.id;
        e.dataTransfer!.setData("application/x-cx-tab", String(tab.id));
        e.dataTransfer!.effectAllowed = "move";
      }}
      ondragover={(e) => {
        if (dragId != null && e.dataTransfer?.types.includes("application/x-cx-tab")) e.preventDefault();
      }}
      ondrop={(e) => {
        if (dragId == null) return;
        e.preventDefault();
        pane.move(dragId, pane.tabs.indexOf(tab));
        dragId = null;
      }}
      use:dropTarget={{ dest: () => (tab.writable ? tab.dirUri : null), spring: () => pane.activate(tab.id) }}
    >
      {#if icon}<Icon name={icon} size={15} />{:else}<FileIcon name="" isDir size={16} />{/if}
      <span class="title">{tab.title || "Loading…"}</span>
      <button class="close" title="Close tab ({shortcut('tab.close')})" aria-label="Close tab" onpointerdown={(e) => e.stopPropagation()} onclick={() => pane.close(tab.id)}>
        <Icon name="close" size={12} stroke={1.6} />
      </button>
    </div>
  {/each}
  <button
    class="new"
    title="New tab ({shortcut('tab.new')})"
    aria-label="New tab"
    onclick={() => {
      ws.focusPane(pane.id);
      pane.add(pane.active?.folder.uri ?? "cx:home");
    }}
  >
    <Icon name="plus" size={14} />
  </button>
  {@render trailing?.()}
</div>

<style>
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
    min-width: 90px;
    height: 36px;
    padding: 0 6px 0 12px;
    border-radius: var(--radius-lg) var(--radius-lg) 0 0;
    color: var(--text-2);
    transition: background 0.1s;
  }
  .compact .tab {
    height: 30px;
    flex-basis: 180px;
    min-width: 70px;
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
  .compact .tab.focused::after {
    content: "";
    position: absolute;
    left: 10px;
    right: 10px;
    top: 0;
    height: 2px;
    border-radius: 0 0 2px 2px;
    background: var(--accent);
  }
  .tab:global(.drop-hover) {
    background: var(--accent-soft);
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
    flex: none;
  }
  .compact .new {
    height: 26px;
    margin-bottom: 2px;
  }
  .new:hover {
    background: var(--hover);
  }
</style>
