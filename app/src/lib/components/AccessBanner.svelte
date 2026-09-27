<script lang="ts">
  // macOS asks separately for Desktop, Documents, Downloads, drives… A file
  // manager is expected to have Full Disk Access instead: one grant, no more
  // prompts. This banner offers it until granted (or dismissed).
  import { fullDiskAccess, openFullDiskAccessSettings } from "../api";
  import { settings } from "../stores/settings.svelte";
  import { ws } from "../workspace.svelte";
  import Icon from "./Icon.svelte";

  let needed = $state(false);
  let waiting = $state(false);

  async function check() {
    try {
      needed = !(await fullDiskAccess());
    } catch {
      needed = false;
    }
  }

  $effect(() => {
    if (ws.platform !== "macos" || settings.data.fdaDismissed) return;
    void check();
    // Re-check when the user comes back from System Settings.
    const onFocus = () => void check();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  });

  async function grant() {
    waiting = true;
    await openFullDiskAccessSettings();
  }
</script>

{#if needed && !settings.data.fdaDismissed}
  <div class="banner" role="status">
    <Icon name="lock" size={16} />
    <span class="text">
      {#if waiting}
        In System Settings, turn on <strong>Cross Explore</strong> under Full Disk Access (use <strong>+</strong> if it isn't listed), then come back. Restart the app if macOS asks.
      {:else}
        <strong>Stop the “would like to access” prompts.</strong> Give Cross Explore Full Disk Access once, like other file managers.
      {/if}
    </span>
    <button class="primary" onclick={grant}>{waiting ? "Open Settings again" : "Allow access…"}</button>
    <button class="icon" aria-label="Dismiss" title="Don't show again" onclick={() => (settings.data.fdaDismissed = true)}><Icon name="close" size={12} /></button>
  </div>
{/if}

<style>
  .banner {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    background: var(--accent-soft);
    border-bottom: 1px solid var(--stroke);
    font-size: 12.5px;
    flex: none;
  }
  .banner :global(svg) {
    color: var(--accent);
    flex: none;
  }
  .text {
    flex: 1;
    min-width: 0;
  }
  .primary {
    height: 28px;
    padding: 0 12px;
    border-radius: var(--radius);
    background: var(--accent);
    color: var(--accent-text);
    white-space: nowrap;
  }
  .icon {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    border-radius: 4px;
  }
  .icon:hover {
    background: var(--hover);
  }
</style>
