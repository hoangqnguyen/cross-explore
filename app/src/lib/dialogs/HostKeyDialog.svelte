<script lang="ts">
  import { dialogs } from "../stores/dialogs.svelte";
  import Modal from "./Modal.svelte";

  let { host, keyType, fingerprint, changed }: { uri: string; host: string; keyType: string; fingerprint: string; changed: boolean } = $props();
</script>

<Modal title={changed ? `Warning: ${host} has a different identity` : `Trust ${host}?`} width={500} onsubmit={() => dialogs.close(true)}>
  {#if changed}
    <p class="error">The server's key has changed since you last connected. This can mean someone is intercepting the connection — or that the server was reinstalled. Only continue if you know why.</p>
  {:else}
    <p>This is the first time you connect to this server. Check that the fingerprint below matches the one your server reports (for example with <code>ssh-keygen -lf</code>).</p>
  {/if}
  <div class="fp">
    <span class="muted">{keyType}</span>
    <code>{fingerprint}</code>
  </div>
  {#snippet footer()}
    <button type="button" class="btn" onclick={() => dialogs.close(false)}>Cancel</button>
    <button type="submit" class="btn primary" class:danger={changed}>{changed ? "Trust new key" : "Trust and connect"}</button>
  {/snippet}
</Modal>

<style>
  .fp {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 12px;
    border-radius: var(--radius);
    background: var(--layer-2);
    box-shadow: inset 0 0 0 1px var(--stroke);
  }
  code {
    font-family: ui-monospace, Menlo, Consolas, monospace;
    font-size: 12px;
    word-break: break-all;
    user-select: text;
    -webkit-user-select: text;
  }
</style>
