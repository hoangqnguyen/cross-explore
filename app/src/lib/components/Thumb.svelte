<script lang="ts">
  // A file's picture: a real thumbnail for media (and anything QuickLook /
  // the shell can render), the colored type icon otherwise or on failure.
  import { thumbUrl, type Item } from "../api";
  import { hasOsIcon } from "../fileTypes";
  import { categoryOf } from "../format";
  import { ws } from "../workspace.svelte";
  import FileIcon from "./FileIcon.svelte";

  let { entry, uri, size, fit = "contain", iconScale = 0.78 }: { entry: Item; uri: string; size: number; fit?: "contain" | "cover"; iconScale?: number } = $props();

  const thumbable = new Set(["image", "video", "pdf", "doc", "slides", "sheet", "font"]);
  // Thumbnails come in a few fixed sizes, scaled down to fit by CSS: zooming
  // the icons in 10% steps then reuses cached thumbnails instead of making
  // every visible one again at each step.
  const SIZES = [32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024];
  const bucket = (px: number) => SIZES.find((s) => s >= px) ?? Math.round(px);
  let wanted = $derived((!entry.isDir && thumbable.has(categoryOf(entry))) || hasOsIcon(entry.name, entry.isDir, uri, ws.platform) ? thumbUrl(uri, bucket(size * (window.devicePixelRatio || 1)), entry.modified) : "");

  // Double-buffered: when the file changes (new mtime → new URL) the old
  // picture stays up until the new one has loaded, so refreshes don't blink.
  let shown = $state("");
  let failedSrc = $state("");
  let shownUri = "";
  let pending = $derived(wanted && wanted !== shown && wanted !== failedSrc ? wanted : "");
  let layers = $derived([...new Set([shown, pending].filter(Boolean))]);

  // A different file entirely (the component was reused): start over.
  $effect(() => {
    if (uri !== shownUri) {
      shownUri = uri;
      shown = "";
      failedSrc = "";
    }
  });
</script>

<div class="thumb" style:width="{size}px" style:height="{size}px">
  {#each layers as layer (layer)}
    <img
      src={layer}
      alt=""
      loading="lazy"
      decoding="async"
      draggable="false"
      class:loaded={layer === shown}
      class:cover={fit === "cover"}
      onload={() => layer === pending && (shown = layer)}
      onerror={() => {
        failedSrc = layer;
        if (layer === shown) shown = "";
      }}
    />
  {/each}
  {#if !shown}
    <span class="icon"><FileIcon name={entry.name} isDir={entry.isDir} executable={entry.executable} size={Math.round(size * iconScale)} /></span>
  {/if}
</div>

<style>
  .thumb {
    position: relative;
    display: grid;
    place-items: center;
    flex: none;
  }
  img {
    position: absolute;
    max-width: 100%;
    max-height: 100%;
    border-radius: 4px;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.18), 0 0 0 1px rgba(0, 0, 0, 0.06);
    opacity: 0;
    transition: opacity 0.15s;
  }
  img.cover {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  img.loaded {
    opacity: 1;
  }
  .icon {
    display: grid;
    place-items: center;
  }
</style>
