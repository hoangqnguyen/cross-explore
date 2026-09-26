<script lang="ts">
  import { formatSize } from "../format";
  import { transfers } from "../stores/transfers.svelte";
  import Icon from "./Icon.svelte";

  let active = $derived(transfers.active.length);
  let conflicts = $derived(transfers.conflicts.length);
  let pct = $derived(Math.round(transfers.progress * 100));
</script>

<button class="capsule" class:busy={active > 0} class:attention={conflicts > 0} onclick={() => (transfers.flyoutOpen = !transfers.flyoutOpen)} title="Transfers">
  {#if active}
    <svg class="ring" viewBox="0 0 20 20" width="16" height="16" aria-hidden="true">
      <circle cx="10" cy="10" r="8" fill="none" stroke="var(--stroke-strong)" stroke-width="2.4" />
      <circle cx="10" cy="10" r="8" fill="none" stroke="var(--accent)" stroke-width="2.4" stroke-linecap="round" stroke-dasharray="{(pct / 100) * 50.3} 50.3" transform="rotate(-90 10 10)" />
    </svg>
    <span>{conflicts ? "Needs attention" : `${active} · ${formatSize(transfers.totalSpeed)}/s`}</span>
  {:else}
    <Icon name="transfer" size={15} />
  {/if}
</button>

<style>
  .capsule {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 10px;
    border-radius: 14px;
    font-size: 12px;
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }
  .capsule:hover {
    background: var(--hover);
  }
  .capsule.busy {
    background: var(--accent-soft);
    color: var(--text);
  }
  .capsule.attention {
    background: color-mix(in srgb, #ff9500 22%, transparent);
  }
</style>
