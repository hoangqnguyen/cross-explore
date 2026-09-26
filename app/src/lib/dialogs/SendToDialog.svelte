<script lang="ts">
  import { errorText, peerSend, uriName } from "../api";
  import DeviceIcon from "../components/DeviceIcon.svelte";
  import { devices } from "../stores/devices.svelte";
  import { dialogs } from "../stores/dialogs.svelte";
  import { toasts } from "../toasts.svelte";
  import Modal from "./Modal.svelte";

  let { uris }: { uris: string[] } = $props();
  let targets = $derived(devices.nearby.filter((d) => d.services.some((s) => s.scheme === "peer")));
  let busy = $state<string | null>(null);

  async function send(id: string, name: string) {
    busy = id;
    try {
      await peerSend(id, uris);
      toasts.show(`Offered ${uris.length === 1 ? `“${uriName(uris[0])}”` : `${uris.length} items`} to ${name}`);
      dialogs.close(true);
    } catch (e) {
      toasts.show(errorText(e), "error");
      busy = null;
    }
  }
</script>

<Modal title="Send to device">
  <p>{uris.length === 1 ? `“${uriName(uris[0])}”` : `${uris.length} items`} will be offered to the device. It arrives once they accept.</p>
  <div class="list">
    {#each targets as d (d.id)}
      <button type="button" class="dev" disabled={!!busy || (d.tailnet ? !d.tailnet.online : false)} onclick={() => send(d.id, d.name)}>
        <DeviceIcon kind={d.kind} size={22} />
        <span class="name">{d.name}</span>
        <span class="muted">{busy === d.id ? "Sending…" : d.tailnet ? "tailnet" : "nearby"}</span>
      </button>
    {:else}
      <p class="muted">No devices running Cross Explore were found. Turn on sharing on your other device (Settings → Sharing & devices) and pair it.</p>
    {/each}
  </div>
  {#snippet footer()}
    <button type="button" class="btn" onclick={() => dialogs.ask("pair")}>Pair a device…</button>
    <button type="button" class="btn" onclick={() => dialogs.close(null)}>Cancel</button>
  {/snippet}
</Modal>

<style>
  .list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .dev {
    display: flex;
    align-items: center;
    gap: 12px;
    height: 48px;
    padding: 0 12px;
    border-radius: var(--radius-lg);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke-strong);
    color: var(--accent);
    text-align: left;
  }
  .dev:hover:not(:disabled) {
    box-shadow: 0 0 0 2px var(--accent);
  }
  .name {
    flex: 1;
    color: var(--text);
    font-weight: 500;
  }
</style>
