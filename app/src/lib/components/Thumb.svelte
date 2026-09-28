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
  let failed = $state(false);
  let loaded = $state(false);
  let wants = $derived(!failed && ((!entry.isDir && thumbable.has(categoryOf(entry))) || hasOsIcon(entry.name, entry.isDir, uri, ws.platform)));
  let src = $derived(wants ? thumbUrl(uri, Math.round(size * (window.devicePixelRatio || 1)), entry.modified) : "");

  $effect(() => {
    void src;
    failed = false;
    loaded = false;
  });
</script>

<div class="thumb" style:width="{size}px" style:height="{size}px">
  {#if src && !failed}
    <img {src} alt="" loading="lazy" decoding="async" draggable="false" class:loaded class:cover={fit === "cover"} onload={() => (loaded = true)} onerror={() => (failed = true)} />
  {/if}
  {#if !src || failed || !loaded}
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
