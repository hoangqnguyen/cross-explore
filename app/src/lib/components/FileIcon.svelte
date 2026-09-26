<script lang="ts">
  // Colorful file-type icons: a Windows 11 style folder and a page with a
  // category-colored band, so files read at a glance without thumbnails.
  import { categoryOf, extOf, type Category } from "../format";

  let { name, isDir, size = 18 }: { name: string; isDir: boolean; size?: number } = $props();

  const colors: Record<Category, string> = {
    folder: "#f4b937",
    image: "#2fa66a",
    video: "#d8434f",
    audio: "#d6479b",
    archive: "#b7791f",
    code: "#3a78d8",
    doc: "#2b5fc4",
    sheet: "#1d8a4c",
    slides: "#d0602a",
    pdf: "#d33b2f",
    text: "#7a8594",
    app: "#6453d6",
    font: "#8e5bd0",
    disk: "#5d6b7c",
    file: "#8a94a3",
  };

  let cat = $derived(categoryOf({ name, isDir }));
  let color = $derived(colors[cat]);
  let label = $derived(size >= 32 ? extOf(name).slice(0, 4).toUpperCase() : "");
</script>

{#if cat === "folder"}
  <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
    <path d="M2 6.2C2 5 3 4 4.2 4h4.6c.6 0 1.1.2 1.5.6L12 6.3h7.8C21 6.3 22 7.3 22 8.5V9H2z" fill="#e8a520" />
    <path d="M2 8.8C2 7.8 2.8 7 3.8 7h16.4c1 0 1.8.8 1.8 1.8v9C22 19 21 20 19.8 20H4.2C3 20 2 19 2 17.8z" fill="#fcc94a" />
    <path d="M2 10.5h20" stroke="#ffe08a" stroke-width=".8" opacity=".8" />
  </svg>
{:else}
  <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
    <path d="M6 2.5h8l5 5v13a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-17a1 1 0 0 1 1-1z" fill="var(--file-page, #fff)" stroke="var(--file-edge, rgba(0,0,0,.28))" stroke-width=".9" />
    <path d="M14 2.5v4a1 1 0 0 0 1 1h4" fill="none" stroke="var(--file-edge, rgba(0,0,0,.28))" stroke-width=".9" />
    <rect x="5" y="14" width="14" height={label ? 5 : 4.5} rx=".6" fill={color} />
    {#if label}
      <text x="12" y="17.9" text-anchor="middle" font-size="3.6" font-weight="700" fill="#fff" font-family="system-ui">{label}</text>
    {/if}
  </svg>
{/if}

<style>
  svg {
    flex: none;
    display: block;
  }
  @media (prefers-color-scheme: dark) {
    svg {
      --file-page: #e9ecf1;
      --file-edge: rgba(0, 0, 0, 0.45);
    }
  }
</style>
