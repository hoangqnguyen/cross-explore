<script lang="ts">
  import { dialogs } from "../stores/dialogs.svelte";
  import Modal from "./Modal.svelte";

  let { title, label, value = "", ok = "OK" }: { title: string; label: string; value?: string; ok?: string } = $props();
  // svelte-ignore state_referenced_locally
  let text = $state(value);
  let input: HTMLInputElement | undefined = $state();

  $effect(() => {
    // Preselect the name without its extension, like a rename.
    const dot = text.lastIndexOf(".");
    input?.setSelectionRange(0, dot > 0 ? dot : text.length);
  });
</script>

<Modal {title} onsubmit={() => text.trim() && dialogs.close(text.trim())}>
  <label class="field">{label}<input type="text" bind:this={input} bind:value={text} spellcheck="false" /></label>
  {#snippet footer()}
    <button type="button" class="btn" onclick={() => dialogs.close(null)}>Cancel</button>
    <button type="submit" class="btn primary" disabled={!text.trim()}>{ok}</button>
  {/snippet}
</Modal>
