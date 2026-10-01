<script lang="ts">
  import { menu } from "../menu.svelte";
  import MenuList from "./MenuList.svelte";
</script>

{#if menu.open}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="scrim" onpointerdown={() => menu.close()} oncontextmenu={(e) => { e.preventDefault(); menu.close(); }}></div>
  <MenuList items={menu.items} x={menu.x} y={menu.y} onClose={() => menu.close()} />
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 90;
  }
</style>
