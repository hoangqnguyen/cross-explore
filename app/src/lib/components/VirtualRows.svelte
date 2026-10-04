<script lang="ts" generics="T">
  // Fixed-height rows inside a scrolling parent (a Columns-view column):
  // only the rows in or near view exist in the DOM, the rest is spacer.
  import type { Snippet } from "svelte";

  let { items, rowH, key, row }: { items: T[]; rowH: number; key: (item: T) => string; row: Snippet<[T]> } = $props();

  const OVERSCAN = 10;
  let el: HTMLDivElement | undefined = $state();
  let scrollTop = $state(0);
  let viewH = $state(800);
  let offset = $state(0);

  $effect(() => {
    const scroller = el?.parentElement;
    if (!el || !scroller) return;
    const update = () => {
      scrollTop = scroller.scrollTop;
      viewH = scroller.clientHeight;
      offset = el!.offsetTop;
    };
    update();
    scroller.addEventListener("scroll", update, { passive: true });
    const ro = new ResizeObserver(update);
    ro.observe(scroller);
    return () => {
      scroller.removeEventListener("scroll", update);
      ro.disconnect();
    };
  });

  let first = $derived(Math.max(0, Math.floor((scrollTop - offset) / rowH) - OVERSCAN));
  let last = $derived(Math.min(items.length, Math.ceil((scrollTop - offset + viewH) / rowH) + OVERSCAN));
  let slice = $derived(items.slice(first, last));
</script>

<div bind:this={el} style:height="{items.length * rowH}px">
  <div style:transform="translateY({first * rowH}px)">
    {#each slice as item (key(item))}{@render row(item)}{/each}
  </div>
</div>
