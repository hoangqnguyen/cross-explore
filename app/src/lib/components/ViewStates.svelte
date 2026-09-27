<script lang="ts">
  // Loading skeleton, errors (with sign-in / trust actions) and empty states,
  // shared by all views.
  import { errorText, isCloudUri, trustHostKey } from "../api";
  import { dialogs } from "../stores/dialogs.svelte";
  import { toasts } from "../toasts.svelte";
  import type { Tab } from "../workspace.svelte";
  import Icon from "./Icon.svelte";

  let { tab, rowH = 30 }: { tab: Tab; rowH?: number } = $props();
  let folder = $derived(tab.folder);
  let showSkeleton = $state(false);

  // Skeleton rows only if a listing is slow enough to notice.
  $effect(() => {
    if (folder.status !== "loading" || folder.items.length) {
      showSkeleton = false;
      return;
    }
    const t = setTimeout(() => (showSkeleton = true), 150);
    return () => clearTimeout(t);
  });

  let detail = $derived(folder.errorDetail);

  async function signIn() {
    if (detail?.kind === "authRequired") {
      const u = detail.message.uri;
      const cloud = isCloudUri(u);
      const ok = cloud
        ? await dialogs.ask("cloud", { service: u.slice(0, u.indexOf(":")), navigate: false })
        : await dialogs.ask("signIn", { uri: u, user: detail.message.user, reason: detail.message.reason });
      if (ok) tab.reload();
    } else if (detail?.kind === "hostKeyUnknown") {
      const key = detail.message;
      const ok = await dialogs.ask("hostKey", { ...key });
      if (!ok) return;
      try {
        // Remember the key, then connect again (which may now ask to sign in).
        await trustHostKey(key.uri, key.keyType, key.fingerprint);
        tab.reload();
      } catch (e) {
        toasts.show(errorText(e), "error");
      }
    }
  }

  // Ask right away instead of making people click through an error first:
  // once per folder and kind of problem (trusting a key can lead to a
  // sign-in prompt next).
  let asked = "";
  $effect(() => {
    if (folder.status === "error" && (detail?.kind === "authRequired" || detail?.kind === "hostKeyUnknown")) {
      const key = `${folder.uri}|${detail.kind}`;
      if (asked !== key) {
        asked = key;
        void signIn();
      }
    }
  });
</script>

{#if folder.status === "loading" && showSkeleton}
  {#each { length: 8 } as _, i}
    <div class="skeleton" style:top="{i * rowH}px" style:height="{rowH}px" style:animation-delay="{i * 60}ms">
      <span class="bone" style:width="{30 + ((i * 37) % 40)}%"></span>
    </div>
  {/each}
{:else if folder.status === "error"}
  <div class="empty">
    <Icon name={detail?.kind === "authRequired" || detail?.kind === "hostKeyUnknown" ? "lock" : "warning"} size={40} stroke={1} />
    <p>{folder.error}</p>
    <div class="actions">
      {#if detail?.kind === "authRequired"}<button class="primary" onclick={signIn}>Sign in…</button>{/if}
      {#if detail?.kind === "hostKeyUnknown"}<button class="primary" onclick={signIn}>Review…</button>{/if}
      {#if tab.canBack}<button onclick={() => tab.back()}>Go back</button>{/if}
      <button onclick={() => tab.reload()}>Try again</button>
    </div>
  </div>
{:else if folder.status === "ready" && tab.visible.length === 0 && !folder.refreshing}
  <div class="empty">
    {#if tab.filter}
      <Icon name="search" size={36} stroke={1} />
      <p>No items match “{tab.filter}”</p>
      <div class="actions"><button onclick={() => (tab.filter = "")}>Clear filter</button></div>
    {:else if folder.kind === "search"}
      <Icon name="search" size={36} stroke={1} />
      <p>No results</p>
    {:else}
      <Icon name="folder" size={40} stroke={1} />
      <p>This folder is empty</p>
      {#if folder.items.length}<p class="sub">{folder.items.length} hidden {folder.items.length === 1 ? "item" : "items"}</p>{/if}
    {/if}
  </div>
{/if}

<style>
  .skeleton {
    position: absolute;
    left: 12px;
    right: 12px;
    display: flex;
    align-items: center;
    animation: pulse 1.2s ease-in-out infinite;
  }
  .bone {
    display: block;
    height: 10px;
    border-radius: 5px;
    background: var(--stroke-strong);
    margin-left: 36px;
  }
  @keyframes pulse {
    50% {
      opacity: 0.45;
    }
  }
  .empty {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    color: var(--text-3);
    pointer-events: none;
    text-align: center;
    padding: 24px;
  }
  .empty p {
    margin: 4px 0 0;
    color: var(--text-2);
    max-width: 420px;
  }
  .empty .sub {
    color: var(--text-3);
    font-size: 12px;
  }
  .actions {
    display: flex;
    gap: 8px;
    margin-top: 10px;
    pointer-events: auto;
  }
  .actions button {
    height: 30px;
    padding: 0 14px;
    border-radius: var(--radius);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke-strong);
    color: var(--text);
  }
  .actions button:hover {
    background: var(--hover);
  }
  .actions button.primary {
    background: var(--accent);
    color: var(--accent-text);
    box-shadow: none;
  }
</style>
