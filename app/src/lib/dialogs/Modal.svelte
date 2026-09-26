<script lang="ts">
  import type { Snippet } from "svelte";
  import { dialogs } from "../stores/dialogs.svelte";

  let {
    title,
    width = 440,
    children,
    footer,
    onsubmit,
  }: { title: string; width?: number; children: Snippet; footer?: Snippet; onsubmit?: () => void } = $props();

  let box: HTMLFormElement | undefined = $state();

  $effect(() => {
    // Focus the first field (or the dialog) when it opens.
    const candidates = box?.querySelectorAll<HTMLElement>("input:not([type=checkbox]):not([type=radio]):not([disabled]), select, textarea, button.primary") ?? [];
    const first = [...candidates].find((el) => el.offsetParent !== null);
    (first ?? box)?.focus();
  });

  function onkeydown(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === "Escape") {
      e.preventDefault();
      dialogs.close(null);
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
<div class="backdrop" onclick={() => dialogs.close(null)}></div>
<!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
<form
  class="modal"
  style:width="min({width}px, calc(100vw - 32px))"
  role="dialog"
  aria-label={title}
  tabindex="-1"
  bind:this={box}
  {onkeydown}
  onsubmit={(e) => {
    e.preventDefault();
    onsubmit?.();
  }}
>
  <h2>{title}</h2>
  <div class="content">{@render children()}</div>
  {#if footer}<footer>{@render footer()}</footer>{/if}
</form>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 88;
    background: rgba(0, 0, 0, 0.28);
    animation: fade 0.12s;
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  .modal {
    position: fixed;
    z-index: 89;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    max-height: calc(100vh - 48px);
    display: flex;
    flex-direction: column;
    border-radius: 12px;
    background: var(--flyout);
    box-shadow: 0 24px 64px rgba(0, 0, 0, 0.35), 0 0 0 1px var(--stroke-strong);
    outline: none;
    animation: pop 0.16s var(--ease);
  }
  @keyframes pop {
    from {
      opacity: 0;
      transform: translate(-50%, -48%) scale(0.98);
    }
  }
  h2 {
    margin: 0;
    padding: 20px 22px 6px;
    font-size: 16px;
    font-weight: 600;
  }
  .content {
    padding: 8px 22px 16px;
    overflow-y: auto;
    min-height: 0;
  }
  footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 14px 22px;
    border-top: 1px solid var(--stroke);
    background: var(--hover);
    border-radius: 0 0 12px 12px;
  }
  .modal :global(.btn) {
    height: 32px;
    min-width: 88px;
    padding: 0 14px;
    border-radius: var(--radius);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke-strong);
  }
  .modal :global(.btn:hover:not(:disabled)) {
    background: var(--hover);
  }
  .modal :global(.btn.primary) {
    background: var(--accent);
    color: var(--accent-text);
    box-shadow: none;
  }
  .modal :global(.btn.primary:hover:not(:disabled)) {
    filter: brightness(1.08);
    background: var(--accent);
  }
  .modal :global(.btn.danger) {
    background: var(--danger);
    color: #fff;
    box-shadow: none;
  }
  .modal :global(.btn:disabled) {
    opacity: 0.5;
  }
  .modal :global(label.field) {
    display: flex;
    flex-direction: column;
    gap: 5px;
    margin-bottom: 12px;
    font-size: 12px;
    color: var(--text-2);
  }
  .modal :global(input[type="text"]),
  .modal :global(input[type="password"]),
  .modal :global(input[type="number"]),
  .modal :global(select),
  .modal :global(textarea) {
    height: 32px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--radius);
    background: var(--layer-2);
    box-shadow: inset 0 0 0 1px var(--stroke-strong);
    color: var(--text);
    font: inherit;
    outline: none;
    user-select: text;
    -webkit-user-select: text;
  }
  .modal :global(textarea) {
    height: auto;
    padding: 8px 10px;
  }
  .modal :global(input:focus),
  .modal :global(select:focus),
  .modal :global(textarea:focus) {
    box-shadow: inset 0 0 0 1px var(--stroke-strong), inset 0 -2px 0 var(--accent);
  }
  .modal :global(.row) {
    display: flex;
    gap: 10px;
  }
  .modal :global(.row > *) {
    flex: 1;
  }
  .modal :global(.check) {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 6px 0;
    font-size: 13px;
    color: var(--text);
  }
  .modal :global(.muted) {
    color: var(--text-3);
    font-size: 12px;
  }
  .modal :global(.error) {
    color: var(--danger);
    font-size: 12px;
    margin: 4px 0 8px;
  }
  .modal :global(p) {
    margin: 0 0 10px;
    color: var(--text-2);
    line-height: 1.5;
  }
</style>
