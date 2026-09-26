<script lang="ts">
  import { menu, type MenuItem } from "../menu.svelte";
  import Icon from "./Icon.svelte";

  let el: HTMLDivElement | undefined = $state();
  let active = $state(-1);
  let pos = $state({ left: 0, top: 0 });

  const actionable = (i: MenuItem) => !("separator" in i) && !i.disabled;

  // Keep the menu inside the window.
  $effect(() => {
    if (!menu.open || !el) return;
    const r = el.getBoundingClientRect();
    pos = {
      left: Math.max(4, Math.min(menu.x, window.innerWidth - r.width - 4)),
      top: menu.y + r.height > window.innerHeight - 4 ? Math.max(4, menu.y - r.height) : menu.y,
    };
    active = -1;
    el.focus();
  });

  function run(item: MenuItem) {
    if ("separator" in item || item.disabled) return;
    menu.close();
    item.action();
  }

  function onkeydown(e: KeyboardEvent) {
    const items = menu.items;
    const step = (d: number) => {
      for (let n = 1; n <= items.length; n++) {
        const i = (active + d * n + items.length * 2) % items.length;
        if (actionable(items[i])) return i;
      }
      return active;
    };
    if (e.key === "ArrowDown") active = step(1);
    else if (e.key === "ArrowUp") active = step(-1);
    else if (e.key === "Enter" && active >= 0) run(items[active]);
    else if (e.key === "Escape" || e.key === "Tab") menu.close();
    else return;
    e.preventDefault();
    e.stopPropagation();
  }
</script>

{#if menu.open}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="scrim" onpointerdown={() => menu.close()} oncontextmenu={(e) => { e.preventDefault(); menu.close(); }}></div>
  <div class="menu" role="menu" tabindex="-1" bind:this={el} style:left="{pos.left}px" style:top="{pos.top}px" {onkeydown}>
    {#each menu.items as item, i}
      {#if "separator" in item}
        <div class="sep" role="separator"></div>
      {:else}
        <button
          role="menuitem"
          class:active={i === active}
          class:danger={item.danger}
          disabled={item.disabled}
          onpointerenter={() => (active = item.disabled ? -1 : i)}
          onclick={() => run(item)}
        >
          <span class="icon">
            {#if item.checked}<Icon name="check" />{:else if item.icon}<Icon name={item.icon} />{/if}
          </span>
          <span class="label">{item.label}</span>
          {#if item.shortcut}<span class="shortcut">{item.shortcut}</span>{/if}
        </button>
      {/if}
    {/each}
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 90;
  }
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
