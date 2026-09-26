<script lang="ts">
  // Bottom navigation for phones; turns into a selection toolbar when items
  // are selected with a long press.
  import { run } from "../commands.svelte";
  import { clipboard } from "../stores/clipboard.svelte";
  import { dialogs } from "../stores/dialogs.svelte";
  import { transfers } from "../stores/transfers.svelte";
  import { ui } from "../stores/ui.svelte";
  import { HOME_URI, ws } from "../workspace.svelte";
  import Icon, { type IconName } from "./Icon.svelte";

  let tab = $derived(ws.activeTab);
  let count = $derived(tab?.selection.size ?? 0);

  $effect(() => {
    if (ui.selecting && count === 0) ui.selecting = false;
  });

  function done() {
    ui.selecting = false;
    tab.selectOnly(null);
  }
</script>

{#snippet btn(icon: IconName, label: string, action: () => void, opts: { active?: boolean; badge?: number; danger?: boolean; disabled?: boolean } = {})}
  <button class:active={opts.active} class:danger={opts.danger} disabled={opts.disabled} onclick={action}>
    <span class="i"><Icon name={icon} size={22} />{#if opts.badge}<span class="badge">{opts.badge}</span>{/if}</span>
    <span class="l">{label}</span>
  </button>
{/snippet}

<nav class="mobile-bar" class:selecting={ui.selecting}>
  {#if ui.selecting}
    {@render btn("close", `${count} selected`, done)}
    {@render btn("copy", "Copy", () => (run("edit.copy"), done()))}
    {@render btn("cut", "Move", () => (run("edit.cut"), done()))}
    {@render btn("send", "Send", () => run("file.sendTo"), { disabled: !tab.folder.info?.local })}
    {@render btn("trash", "Delete", () => (run("file.trash"), done()), { danger: true })}
  {:else}
    {@render btn("sidebarRight", "Places", () => (ui.drawerOpen = true))}
    {@render btn("home", "Home", () => tab.navigate(HOME_URI), { active: tab.folder.kind === "home" })}
    {#if clipboard.uris.length}
      {@render btn("paste", "Paste here", () => run("edit.paste"), { disabled: !tab.writable })}
    {:else}
      {@render btn("newFolder", "New folder", () => run("file.newFolder"), { disabled: !tab.writable })}
    {/if}
    {@render btn("transfer", "Transfers", () => (transfers.flyoutOpen = !transfers.flyoutOpen), { badge: transfers.active.length })}
    {@render btn("more", "More", () => dialogs.ask("settings"))}
  {/if}
</nav>

<style>
  .mobile-bar {
    display: flex;
    justify-content: space-around;
    padding: 6px 4px calc(6px + env(safe-area-inset-bottom));
    background: var(--chrome);
    backdrop-filter: blur(20px);
    -webkit-backdrop-filter: blur(20px);
    border-top: 1px solid var(--stroke);
    flex: none;
  }
  button {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    min-height: 48px;
    padding: 4px 0;
    border-radius: 10px;
    color: var(--text-2);
    font-size: 11px;
  }
  button.active {
    color: var(--accent);
  }
  button.danger {
    color: var(--danger);
  }
  button:disabled {
    opacity: 0.4;
  }
  .i {
    position: relative;
    display: grid;
  }
  .badge {
    position: absolute;
    top: -4px;
    right: -8px;
    min-width: 16px;
    height: 16px;
    padding: 0 4px;
    border-radius: 8px;
    background: var(--accent);
    color: var(--accent-text);
    font-size: 10px;
    line-height: 16px;
    text-align: center;
  }
</style>
