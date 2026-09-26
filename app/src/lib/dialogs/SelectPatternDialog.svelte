<script lang="ts">
  import { dialogs } from "../stores/dialogs.svelte";
  import { globToRegExp, ws } from "../workspace.svelte";
  import Modal from "./Modal.svelte";

  let { select }: { select: boolean } = $props();
  let pattern = $state("*.*");
  const presets = ["*.jpg;*.jpeg;*.png;*.heic", "*.mp4;*.mov;*.mkv", "*.pdf", "*.zip;*.7z;*.tar.gz", "*.txt;*.md"];
  let count = $derived.by(() => {
    try {
      const re = globToRegExp(pattern);
      return ws.activeTab.visible.filter((e) => re.test(e.name)).length;
    } catch {
      return 0;
    }
  });
</script>

<Modal title={select ? "Select by pattern" : "Deselect by pattern"} onsubmit={() => dialogs.close(pattern)}>
  <label class="field">Wildcards: * matches anything, ? one character, ; separates patterns<input type="text" bind:value={pattern} spellcheck="false" /></label>
  <div class="presets">
    {#each presets as p}<button type="button" onclick={() => (pattern = p)}>{p}</button>{/each}
  </div>
  <p class="muted">{count} matching {count === 1 ? "item" : "items"}</p>
  {#snippet footer()}
    <button type="button" class="btn" onclick={() => dialogs.close(null)}>Cancel</button>
    <button type="submit" class="btn primary">{select ? "Select" : "Deselect"}</button>
  {/snippet}
</Modal>

<style>
  .presets {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 10px;
  }
  .presets button {
    padding: 3px 8px;
    border-radius: 10px;
    background: var(--hover);
    font-size: 11.5px;
  }
  .presets button:hover {
    background: var(--accent-soft);
    color: var(--accent);
  }
</style>
