<script lang="ts">
  import { toasts } from "../toasts.svelte";
  import Icon from "./Icon.svelte";
</script>

<div class="toasts" aria-live="polite">
  {#each toasts.list as t (t.id)}
    <div class="toast" class:error={t.tone === "error"}>
      <span>{t.text}</span>
      <button aria-label="Dismiss" onclick={() => toasts.dismiss(t.id)}><Icon name="close" size={12} /></button>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 16px;
    bottom: 40px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    z-index: 80;
    pointer-events: none;
  }
  .toast {
    display: flex;
    align-items: center;
    gap: 12px;
    max-width: 420px;
    padding: 10px 10px 10px 14px;
    border-radius: var(--radius-lg);
    background: var(--flyout);
    backdrop-filter: blur(30px);
    -webkit-backdrop-filter: blur(30px);
    box-shadow: var(--shadow-flyout);
    pointer-events: auto;
    animation: slide 0.2s var(--ease);
  }
  .toast.error {
    box-shadow: var(--shadow-flyout), inset 3px 0 0 var(--danger);
  }
  @keyframes slide {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
  }
  button {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border-radius: 4px;
    color: var(--text-2);
  }
  button:hover {
    background: var(--hover);
  }
</style>
