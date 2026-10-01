<script lang="ts">
  // The actual popup (positioning, keyboard nav, submenus). Recursive: a
  // submenu is just another MenuList, opened to the side of its parent row.
  import type { MenuItem } from "../menu.svelte";
  import Icon from "./Icon.svelte";
  import MenuList from "./MenuList.svelte";

  let { items, x, y, onClose, onLeft }: { items: MenuItem[]; x: number; y: number; onClose: () => void; onLeft?: () => void } = $props();

  let el: HTMLDivElement | undefined = $state();
  let active = $state(-1);
  let pos = $state({ left: 0, top: 0 });

  type SubItem = Extract<MenuItem, { items: MenuItem[] | (() => MenuItem[] | Promise<MenuItem[]>) }>;
  const isSub = (i: MenuItem): i is SubItem => "items" in i;
  const actionable = (i: MenuItem) => !("separator" in i) && !i.disabled;

  let sub = $state<{ index: number; x: number; y: number; items: MenuItem[]; loading: boolean } | null>(null);
  let hoverTimer: ReturnType<typeof setTimeout> | undefined;

  $effect(() => {
    if (!el) return;
    const r = el.getBoundingClientRect();
    pos = {
      left: Math.max(4, Math.min(x, window.innerWidth - r.width - 4)),
      top: y + r.height > window.innerHeight - 4 ? Math.max(4, y - r.height) : y,
    };
    el.focus();
    return () => clearTimeout(hoverTimer);
  });

  function run(item: MenuItem) {
    if (!("action" in item)) return;
    onClose();
    item.action();
  }

  async function openSub(i: number, item: MenuItem) {
    if (!isSub(item) || item.disabled) return;
    const r = el!.querySelectorAll("button")[i]!.getBoundingClientRect();
    const at = { x: r.right - 2, y: r.top - 4 };
    if (typeof item.items === "function") {
      sub = { index: i, ...at, items: [], loading: true };
      const list = await item.items();
      if (sub?.index === i) sub = { index: i, ...at, items: list, loading: false };
    } else {
      sub = { index: i, ...at, items: item.items, loading: false };
    }
  }

  function hover(i: number, item: Exclude<MenuItem, { separator: true }>) {
    active = item.disabled ? -1 : i;
    clearTimeout(hoverTimer);
    if (isSub(item) && !item.disabled) hoverTimer = setTimeout(() => openSub(i, item), 120);
    else sub = null;
  }

  function onkeydown(e: KeyboardEvent) {
    const step = (d: number) => {
      for (let n = 1; n <= items.length; n++) {
        const i = (active + d * n + items.length * 2) % items.length;
        if (actionable(items[i])) return i;
      }
      return active;
    };
    if (e.key === "ArrowDown") {
      active = step(1);
      sub = null;
    } else if (e.key === "ArrowUp") {
      active = step(-1);
      sub = null;
    } else if (e.key === "ArrowRight" && active >= 0 && isSub(items[active])) {
      void openSub(active, items[active]);
    } else if (e.key === "ArrowLeft" && onLeft) {
      onLeft();
    } else if (e.key === "Enter" && active >= 0) {
      const item = items[active];
      if (isSub(item)) void openSub(active, item);
      else run(item);
    } else if (e.key === "Escape") {
      (onLeft ?? onClose)();
    } else if (e.key === "Tab") {
      onClose();
    } else return;
    e.preventDefault();
    e.stopPropagation();
  }
</script>

<div class="menu" role="menu" tabindex="-1" bind:this={el} style:left="{pos.left}px" style:top="{pos.top}px" {onkeydown}>
  {#each items as item, i}
    {#if "separator" in item}
      <div class="sep" role="separator"></div>
    {:else}
      <button
        role="menuitem"
        class:active={i === active}
        class:danger={"danger" in item && !!item.danger}
        disabled={item.disabled}
        aria-haspopup={isSub(item) ? "true" : undefined}
        onpointerenter={() => hover(i, item)}
        onclick={() => (isSub(item) ? openSub(i, item) : run(item))}
      >
        <span class="icon">
          {#if "checked" in item && item.checked}<Icon name="check" />{:else if item.icon}<Icon name={item.icon} />{/if}
        </span>
        <span class="label">{item.label}</span>
        {#if "shortcut" in item && item.shortcut}<span class="shortcut">{item.shortcut}</span>{/if}
        {#if isSub(item)}<Icon name="chevronRight" size={12} />{/if}
      </button>
    {/if}
  {/each}
</div>

{#if sub}
  {#if sub.loading}
    <div class="menu status" style:left="{sub.x}px" style:top="{sub.y}px">Loading…</div>
  {:else if sub.items.length}
    <MenuList items={sub.items} x={sub.x} y={sub.y} {onClose} onLeft={() => (sub = null)} />
  {:else}
    <div class="menu status" style:left="{sub.x}px" style:top="{sub.y}px">No apps found</div>
  {/if}
{/if}

<style>
  .menu {
    position: fixed;
    z-index: 91;
    min-width: 220px;
    padding: 4px;
    border-radius: var(--radius-lg);
    background: var(--flyout);
    backdrop-filter: blur(30px) saturate(1.4);
    -webkit-backdrop-filter: blur(30px) saturate(1.4);
    box-shadow: var(--shadow-flyout);
    outline: none;
    animation: pop 0.14s var(--ease);
    transform-origin: top left;
  }
  .menu.status {
    padding: 8px 12px;
    color: var(--text-3);
    font-size: 13px;
  }
  @keyframes pop {
    from {
      opacity: 0;
      transform: translateY(-4px) scale(0.98);
    }
  }
  button {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    height: 30px;
    padding: 0 10px 0 8px;
    border-radius: var(--radius);
    text-align: left;
  }
  button.active {
    background: var(--hover);
  }
  button:disabled {
    color: var(--text-3);
  }
  button.danger {
    color: var(--danger);
  }
  .icon {
    width: 16px;
    color: var(--text-2);
  }
  .label {
    flex: 1;
    white-space: nowrap;
  }
  .shortcut {
    color: var(--text-3);
    font-size: 12px;
    margin-left: 24px;
  }
  .sep {
    height: 1px;
    margin: 4px 6px;
    background: var(--stroke);
  }
</style>
