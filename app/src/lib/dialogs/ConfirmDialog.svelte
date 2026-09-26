<script lang="ts">
  import { dialogs } from "../stores/dialogs.svelte";
  import Modal from "./Modal.svelte";

  let { title, message, ok = "OK", danger = false }: { title: string; message: string; ok?: string; danger?: boolean } = $props();
</script>

<Modal {title} onsubmit={() => dialogs.close(true)}>
  <p>{message}</p>
  {#snippet footer()}
    <button type="button" class="btn" onclick={() => dialogs.close(false)}>Cancel</button>
    <button type="submit" class="btn primary" class:danger>{ok}</button>
  {/snippet}
</Modal>
