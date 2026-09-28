<script lang="ts">
  // Highlighted code with line numbers and folding: chevrons in the gutter
  // collapse a block (Alt/Option-click also folds everything inside it), the
  // "⋯ N lines" marker opens it again, and Collapse all / Expand all sit in
  // the corner.
  import { foldRanges, splitHtmlLines, type FoldRange } from "../folding";
  import Icon from "./Icon.svelte";

  let { text, html, lang, large = false }: { text: string; html: string; lang: string | null; large?: boolean } = $props();

  // A final newline ends the last line; it doesn't start an empty one.
  let lines = $derived.by(() => {
    const l = splitHtmlLines(html);
    return l.length > 1 && l[l.length - 1] === "" ? l.slice(0, -1) : l;
  });
  let ranges = $derived(foldRanges(text, lang));
  let startOf = $derived(new Map(ranges.map((r) => [r.start, r])));
  let collapsed = $state(new Set<number>());

  // A new file starts fully expanded.
  $effect(() => {
    void text;
    collapsed = new Set();
  });

  /** Line numbers to draw, skipping the inside of collapsed blocks. */
  let visible = $derived.by(() => {
    const out: number[] = [];
    for (let i = 0; i < lines.length; i++) {
      out.push(i);
      const r = collapsed.has(i) ? startOf.get(i) : undefined;
      if (r) i = r.end;
    }
    return out;
  });

  const inside = (outer: FoldRange, r: FoldRange) => r.start >= outer.start && r.end <= outer.end;

  function toggle(n: number, e?: MouseEvent) {
    const r = startOf.get(n);
    if (!r) return;
    const next = new Set(collapsed);
    const closing = !next.has(n);
    if (e?.altKey) {
      // Fold or unfold this block and everything nested in it.
      for (const x of ranges) if (inside(r, x)) closing ? next.add(x.start) : next.delete(x.start);
    } else if (closing) next.add(n);
    else next.delete(n);
    collapsed = next;
  }

  const collapseAll = () => (collapsed = new Set(ranges.map((r) => r.start)));
  const expandAll = () => (collapsed = new Set());

  let digits = $derived(String(lines.length).length);
</script>

<div class="codeview" class:large>
  {#if ranges.length}
    <div class="folds">
      <button type="button" title="Collapse all blocks" aria-label="Collapse all" onclick={collapseAll}><Icon name="chevronRight" size={11} stroke={2} /> Collapse all</button>
      <button type="button" title="Expand all blocks" aria-label="Expand all" onclick={expandAll} disabled={!collapsed.size}><Icon name="chevronDown" size={11} stroke={2} /> Expand all</button>
    </div>
  {/if}
  <div class="scroll">
    <div class="lines" style:--gutter="{digits + 1.2}ch">
      {#each visible as n (n)}
        {@const r = startOf.get(n)}
        {@const closed = collapsed.has(n)}
        <div class="line" class:closed>
          <span class="no" aria-hidden="true">{n + 1}</span>
          {#if r}
            <button type="button" class="fold" class:closed aria-label={closed ? `Expand lines ${n + 2}–${r.end + 1}` : `Collapse lines ${n + 2}–${r.end + 1}`} aria-expanded={!closed} onclick={(e) => toggle(n, e)}>
              <Icon name="chevronDown" size={10} stroke={2.2} />
            </button>
          {:else}
            <span class="fold-space"></span>
          {/if}
          <span class="src">{@html lines[n] || " "}{#if closed && r}<button type="button" class="more" onclick={() => toggle(n)} title="Show {r.end - n} hidden lines">⋯ {r.end - n} lines</button>{/if}</span>
        </div>
      {/each}
    </div>
  </div>
</div>

<style>
  .codeview {
    position: relative;
    align-self: stretch;
    width: 100%;
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--layer-2);
    border-radius: 6px;
    box-shadow: inset 0 0 0 1px var(--stroke);
  }
  .scroll {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 10px 0;
  }
  .lines {
    display: table;
    min-width: 100%;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, "Cascadia Code", monospace;
    font-size: 11.5px;
    line-height: 1.55;
    tab-size: 4;
  }
  .large .lines {
    font-size: 13px;
  }
  .line {
    display: flex;
    align-items: flex-start;
    white-space: pre;
    /* Long files: off-screen lines skip layout and paint. */
    content-visibility: auto;
    contain-intrinsic-size: auto 1.55em;
  }
  .line.closed {
    background: color-mix(in srgb, var(--accent) 8%, transparent);
  }
  .no {
    flex: none;
    width: var(--gutter);
    padding-right: 0.4ch;
    text-align: right;
    color: var(--text-3, #8a94a3);
    opacity: 0.7;
    user-select: none;
  }
  .fold,
  .fold-space {
    flex: none;
    width: 16px;
    height: 1.55em;
  }
  .fold {
    display: grid;
    place-items: center;
    padding: 0;
    border: 0;
    background: none;
    color: var(--text-3, #8a94a3);
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.12s;
  }
  .lines:hover .fold,
  .fold.closed,
  .fold:focus-visible {
    opacity: 1;
  }
  .fold.closed :global(svg) {
    transform: rotate(-90deg);
  }
  .fold:hover {
    color: var(--text);
  }
  .src {
    flex: 1;
    padding-right: 14px;
    user-select: text;
  }
  .more {
    margin-left: 8px;
    padding: 0 6px;
    border: 0;
    border-radius: 4px;
    font: inherit;
    font-size: 0.85em;
    color: var(--text-2, inherit);
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    cursor: pointer;
  }
  .more:hover {
    background: color-mix(in srgb, var(--accent) 28%, transparent);
  }
  .folds {
    position: absolute;
    top: 6px;
    right: 10px;
    z-index: 1;
    display: flex;
    gap: 4px;
    opacity: 0;
    transition: opacity 0.15s;
  }
  .codeview:hover .folds,
  .folds:focus-within {
    opacity: 1;
  }
  .folds button {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 8px;
    border: 1px solid var(--stroke);
    border-radius: 5px;
    background: var(--layer-1, var(--layer-2));
    color: inherit;
    font-size: 11px;
    cursor: pointer;
  }
  .folds button:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
