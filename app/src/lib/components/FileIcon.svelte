<script lang="ts">
  // File-type icons: a Windows 11 style folder; for files a page with a type
  // glyph (image, video, archive zipper, code…), Office-style letter tiles,
  // language badges, and app tiles for programs. See fileTypes.ts.
  import { iconSpec, type Mark } from "../fileTypes";
  import { extOf } from "../format";

  let { name, isDir, size = 18, executable = false }: { name: string; isDir: boolean; size?: number; executable?: boolean } = $props();

  let spec = $derived(isDir ? (extOf(name) === "app" ? iconSpec(name) : null) : iconSpec(name, executable));
  // The extension under the glyph, once there is room to read it.
  let label = $derived(spec?.shape === "page" && !spec.badge && size >= 48 ? extOf(name).slice(0, 5).toUpperCase() : "");
  let badgeSize = $derived(spec?.badge ? (spec.badge.text.length === 1 ? 7.6 : spec.badge.text.length === 2 ? 5.6 : 4.3) : 0);

  const isText = (m: Mark): m is Extract<Mark, { text: string }> => "text" in m;
  // List sizes: the glyph fills more of the page and strokes get bolder, so
  // types still read at 16–20px.
  let small = $derived(size <= 24);
  let glyphTransform = $derived(small ? "translate(12 14.8) scale(1.28) translate(-12 -14.8)" : undefined);
  let strokeBoost = $derived(small ? 1.25 : 1);
</script>

{#snippet marks(list: Mark[], ink: string, knock: string, faint = false)}
  {#each list as m, i (i)}
    {#if isText(m)}
      <text x="12" y={m.y ?? 16} text-anchor="middle" font-size={m.size} font-weight={m.weight ?? 600} fill={ink} font-family="system-ui, -apple-system, 'Segoe UI', sans-serif">{m.text}</text>
    {:else if m.fill}
      <path d={m.d} fill={m.knock ? knock : ink} opacity={faint ? 0.35 : 1} />
    {:else}
      <path d={m.d} fill="none" stroke={ink} stroke-width={(m.width ?? 1.2) * strokeBoost} stroke-linecap="round" stroke-linejoin="round" opacity={faint ? 0.35 : 1} />
    {/if}
  {/each}
{/snippet}

{#if !spec}
  <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
    <path d="M2 6.2C2 5 3 4 4.2 4h4.6c.6 0 1.1.2 1.5.6L12 6.3h7.8C21 6.3 22 7.3 22 8.5V9H2z" fill="#e8a520" />
    <path d="M2 8.8C2 7.8 2.8 7 3.8 7h16.4c1 0 1.8.8 1.8 1.8v9C22 19 21 20 19.8 20H4.2C3 20 2 19 2 17.8z" fill="#fcc94a" />
    <path d="M2 10.5h20" stroke="#ffe08a" stroke-width=".8" opacity=".8" />
  </svg>
{:else if spec.shape === "tile"}
  <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true" class="tile">
    <rect x="3" y="3.2" width="18" height="17.6" rx="4.2" fill={spec.color} />
    <path d="M3 7.4a4.2 4.2 0 0 1 4.2-4.2h9.6A4.2 4.2 0 0 1 21 7.4V11H3z" fill="#fff" opacity=".13" />
    <rect x="3.4" y="3.6" width="17.2" height="16.8" rx="3.9" fill="none" stroke="#000" stroke-opacity=".12" stroke-width=".8" />
    <g transform={small ? "translate(12 12) scale(1.12) translate(-12 -12)" : undefined}>{@render marks(spec.marks ?? [], "#fff", spec.color)}</g>
  </svg>
{:else}
  <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
    <path d="M6 2.5h8l5 5v13a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-17a1 1 0 0 1 1-1z" fill={spec.tint ?? "var(--file-page, #fff)"} stroke="var(--file-edge, rgba(0,0,0,.28))" stroke-width=".9" />
    <path d="M14 2.5v4a1 1 0 0 0 1 1h4" fill="var(--file-fold, #eef0f3)" stroke="var(--file-edge, rgba(0,0,0,.28))" stroke-width=".9" stroke-linejoin="round" />
    {#if spec.badge}
      {@render marks(spec.marks ?? [], "#8b95a3", spec.tint ?? "#fff", true)}
      {#if small}
        <rect x="2" y="9.6" width="14" height="12" rx="2.2" fill={spec.badge.bg} />
      {:else}
        <rect x="2.6" y="11.2" width="11.4" height="9.8" rx="1.9" fill={spec.badge.bg} />
      {/if}
      <text x={small ? 9 : 8.3} y={(small ? 15.6 : 16.1) + badgeSize * (small ? 0.44 : 0.36)} transform={small ? `translate(${9} ${15.6}) scale(1.22) translate(-9 -15.6)` : undefined} text-anchor="middle" font-size={badgeSize} font-weight="800" fill={spec.badge.fg ?? "#fff"} font-family="system-ui, -apple-system, 'Segoe UI', sans-serif" letter-spacing="-.2">{spec.badge.text}</text>
    {:else}
      <g transform={glyphTransform}>{@render marks(spec.marks ?? [], spec.color, spec.tint ?? "var(--file-page, #fff)")}</g>
      {#if label}
        <text x="12" y="20.6" text-anchor="middle" font-size="2.3" font-weight="700" fill={spec.color} font-family="system-ui, -apple-system, 'Segoe UI', sans-serif" letter-spacing=".15">{label}</text>
      {/if}
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
      --file-fold: #d5dae1;
      --file-edge: rgba(0, 0, 0, 0.45);
    }
  }
</style>
