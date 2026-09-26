<script lang="ts">
  import { errorText, peerPair, peerPairCode, peerStatus } from "../api";
  import { devices } from "../stores/devices.svelte";
  import { dialogs } from "../stores/dialogs.svelte";
  import { toasts } from "../toasts.svelte";
  import Modal from "./Modal.svelte";

  let mode = $state<"enter" | "show">("enter");
  let address = $state("");
  let code = $state("");
  let myCode = $state<string | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let peers = $derived(devices.nearby.filter((d) => d.services.some((s) => s.scheme === "peer")));

  async function showCode() {
    mode = "show";
    try {
      myCode = await peerPairCode();
    } catch (e) {
      error = errorText(e);
    }
  }

  async function pair() {
    busy = true;
    error = null;
    try {
      const d = await peerPair(address.trim(), code.replace(/\s/g, ""));
      devices.peer = await peerStatus();
      toasts.show(`Paired with ${d.name}`);
      dialogs.close(true);
    } catch (e) {
      error = errorText(e);
      busy = false;
    }
  }
</script>

<Modal title="Pair a device" onsubmit={() => mode === "enter" && pair()}>
  <div class="tabs">
    <button type="button" class:on={mode === "enter"} onclick={() => (mode = "enter")}>Enter a code</button>
    <button type="button" class:on={mode === "show"} onclick={showCode}>Show my code</button>
  </div>
  {#if mode === "enter"}
    <p>On the other device, open Settings → Sharing & devices → Show pairing code.</p>
    <label class="field">Device
      <input type="text" bind:value={address} list="cx-peers" placeholder="Name, IP or host.tailnet.ts.net" spellcheck="false" />
      <datalist id="cx-peers">{#each peers as p}<option value={p.hostname ?? p.addresses[0]}>{p.name}</option>{/each}</datalist>
    </label>
    <label class="field">Pairing code<input type="text" bind:value={code} placeholder="123 456" inputmode="numeric" /></label>
    {#if error}<div class="error">{error}</div>{/if}
  {:else}
    <p>Enter this code on your other device. It's valid for 2 minutes.</p>
    <div class="code">{myCode ?? "…"}</div>
    {#if devices.peer && !devices.peer.enabled}<p class="error">Sharing is off on this device — turn it on in Settings so the other device can reach it.</p>{/if}
  {/if}
  {#snippet footer()}
    <button type="button" class="btn" onclick={() => dialogs.close(null)}>{mode === "show" ? "Done" : "Cancel"}</button>
    {#if mode === "enter"}<button type="submit" class="btn primary" disabled={busy || !address.trim() || code.replace(/\s/g, "").length < 6}>{busy ? "Pairing…" : "Pair"}</button>{/if}
  {/snippet}
</Modal>

<style>
  .tabs {
    display: flex;
    gap: 4px;
    padding: 3px;
    margin-bottom: 14px;
    border-radius: var(--radius-lg);
    background: var(--hover);
  }
  .tabs button {
    flex: 1;
    height: 28px;
    border-radius: var(--radius);
  }
  .tabs button.on {
    background: var(--layer-2);
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.1);
  }
  .code {
    font-size: 40px;
    font-weight: 600;
    letter-spacing: 0.15em;
    text-align: center;
    padding: 16px;
    font-variant-numeric: tabular-nums;
  }
</style>
