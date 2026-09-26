<script lang="ts">
  import { errorText, peerRespond, type IncomingOffer } from "../api";
  import { formatSize } from "../format";
  import { devices } from "../stores/devices.svelte";
  import { dialogs } from "../stores/dialogs.svelte";
  import { toasts } from "../toasts.svelte";
  import { ws } from "../workspace.svelte";
  import Modal from "./Modal.svelte";

  let { offer }: { offer: IncomingOffer } = $props();
  let dest = $state(ws.places?.favorites.find((f) => f.icon === "downloads")?.uri ?? ws.places?.home.uri ?? "");

  async function respond(accept: boolean) {
    try {
      await peerRespond(offer.id, accept, accept ? dest : null);
      devices.dropOffer(offer.id);
      if (accept) toasts.show(`Receiving from ${offer.from.name}…`);
      dialogs.close(accept);
    } catch (e) {
      toasts.show(errorText(e), "error");
    }
  }
</script>

<Modal title="{offer.from.name} wants to send you {offer.files.length === 1 ? 'a file' : `${offer.files.length} files`}" onsubmit={() => respond(true)}>
  <div class="files">
    {#each offer.files.slice(0, 8) as f}
      <div class="f"><span>{f.name}</span><span class="muted">{formatSize(f.size)}</span></div>
    {/each}
    {#if offer.files.length > 8}<div class="muted">and {offer.files.length - 8} more</div>{/if}
  </div>
  <p class="muted">Total {formatSize(offer.total)} · saved to {decodeURIComponent(dest.replace(/^file:\/\//, ""))}</p>
  {#snippet footer()}
    <button type="button" class="btn" onclick={() => respond(false)}>Decline</button>
    <button type="submit" class="btn primary">Accept</button>
  {/snippet}
</Modal>

<style>
  .files {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-bottom: 10px;
  }
  .f {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-size: 12.5px;
  }
</style>
