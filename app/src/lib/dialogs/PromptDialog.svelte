<script lang="ts">
  import { untrack } from "svelte";
  import { dialogs } from "../stores/dialogs.svelte";
  import Modal from "./Modal.svelte";

  let { title, label, value = "", ok = "OK", secret = false, selectAll = false }: { title: string; label: string; value?: string; ok?: string; secret?: boolean; selectAll?: boolean } = $props();
  // svelte-ignore state_referenced_locally
  let text = $state(value);
  let input: HTMLInputElement | undefined = $state();

  // Preselect the name without its extension, like a rename, once when the
  // dialog opens (not on every keystroke, which would select what you typed).
  $effect(() => {
    if (!input) return;
    const start = untrack(() => text);
    // File names select up to the extension. A user name (or a secret) is the
    // whole value — a dot in "jane.doe" is not an extension.
    const dot = start.lastIndexOf(".");
    input.setSelectionRange(0, selectAll || secret || dot <= 0 ? start.length : dot);
  });
</script>

<Modal {title} onsubmit={() => text.trim() && dialogs.close(text.trim())}>
  <label class="field">{label}<input type={secret ? "password" : "text"} bind:this={input} bind:value={text} spellcheck="false" autocomplete={secret ? "current-password" : "off"} /></label>
  {#snippet footer()}
    <button type="button" class="btn" onclick={() => dialogs.close(null)}>Cancel</button>
    <button type="submit" class="btn primary" disabled={!text.trim()}>{ok}</button>
  {/snippet}
</Modal>
