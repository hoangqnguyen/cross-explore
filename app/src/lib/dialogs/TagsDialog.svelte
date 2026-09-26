<script lang="ts">
  import { errorText, getTags, setTags } from "../api";
  import { dialogs } from "../stores/dialogs.svelte";
  import { TAG_COLORS, tagColor } from "../tags";
  import { toasts } from "../toasts.svelte";
  import Modal from "./Modal.svelte";

  let { uris }: { uris: string[] } = $props();
  let current = $state<Record<string, string[]>>({});
  let custom = $state("");
  let all = $derived([...new Set([...Object.keys(TAG_COLORS), ...Object.values(current).flat()])]);

  $effect(() => {
    getTags(uris)
      .then((m) => (current = m))
      .catch(() => {});
  });

  const tagState = (t: string) => {
    const n = uris.filter((u) => current[u]?.includes(t)).length;
    return n === 0 ? "off" : n === uris.length ? "on" : "mixed";
  };

  function toggle(t: string) {
    const on = tagState(t) !== "on";
    current = Object.fromEntries(uris.map((u) => [u, on ? [...new Set([...(current[u] ?? []), t])] : (current[u] ?? []).filter((x) => x !== t)]));
  }

  async function save() {
    try {
      await Promise.all(uris.map((u) => setTags(u, current[u] ?? [])));
      dialogs.close(current[uris[0]] ?? []);
    } catch (e) {
      toasts.show(errorText(e), "error");
    }
  }
</script>

<Modal title={uris.length === 1 ? "Tags" : `Tags for ${uris.length} items`} onsubmit={save}>
  <div class="tags">
    {#each all as t (t)}
      {@const s = tagState(t)}
      <button type="button" class="tag {s}" onclick={() => toggle(t)}><i style:background={tagColor(t)}></i>{t}{#if s === "mixed"}<span class="muted">–</span>{/if}</button>
    {/each}
  </div>
  <div class="row add">
    <input type="text" bind:value={custom} placeholder="New tag" spellcheck="false" onkeydown={(e) => {
      if (e.key === "Enter" && custom.trim()) {
        e.preventDefault();
        toggle(custom.trim());
        custom = "";
      }
    }} />
  </div>
  {#snippet footer()}
    <button type="button" class="btn" onclick={() => dialogs.close(null)}>Cancel</button>
    <button type="submit" class="btn primary">Save</button>
  {/snippet}
</Modal>

<style>
  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 12px;
  }
  .tag {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 12px 0 10px;
    border-radius: 14px;
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke-strong);
  }
  .tag.on {
    box-shadow: 0 0 0 2px var(--accent);
  }
  .tag i {
    width: 10px;
    height: 10px;
    border-radius: 50%;
  }
</style>
