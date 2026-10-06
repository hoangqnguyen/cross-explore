<script lang="ts">
  import { formatSize } from "../format";
  import { openings } from "../opening.svelte";
  import { toasts } from "../toasts.svelte";
  import FileIcon from "./FileIcon.svelte";
  import Icon from "./Icon.svelte";
</script>

<div class="toasts" aria-live="polite">
  <!-- A file from a server downloads before it opens: show it's happening. -->
  {#each openings.list as o (o.id)}
    {@const pct = o.total ? Math.min(100, (o.done / o.total) * 100) : 0}
    <div class="toast opening" role="status">
      <FileIcon name={o.name} isDir={false} size={28} />
      <div class="body">
        <div class="title">Downloading <strong>{o.name}</strong> to open it</div>
        <div class="bar" class:indeterminate={!o.total} role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={o.total ? Math.round(pct) : undefined}>
          <div style:width="{o.total ? pct : 35}%"></div>
        </div>
        <div class="detail">
          {o.total ? `${formatSize(o.done)} of ${formatSize(o.total)}${o.speed > 0 ? ` · ${formatSize(o.speed)}/s` : ""}` : "Connecting…"}
        </div>
      </div>
      <button class="cancel" onclick={() => openings.cancel(o.id)}>Cancel</button>
    </div>
  {/each}
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
  .opening {
    width: 360px;
    align-items: center;
  }
  .opening .body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .opening .title {
    font-size: 12.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .opening .title strong {
    font-weight: 600;
  }
  .opening .detail {
    font-size: 11.5px;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }
  .bar {
    height: 4px;
    border-radius: 2px;
    background: var(--stroke-strong);
    overflow: hidden;
  }
  .bar > div {
    height: 100%;
    border-radius: 2px;
    background: var(--accent);
    transition: width 0.15s linear;
  }
  .bar.indeterminate > div {
    animation: slide-bar 1.1s ease-in-out infinite;
  }
  @keyframes slide-bar {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(300%);
    }
  }
  .opening .cancel {
    width: auto;
    height: 26px;
    padding: 0 10px;
    font-size: 12px;
    color: var(--text);
    box-shadow: inset 0 0 0 1px var(--stroke-strong);
    border-radius: var(--radius);
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
