<script lang="ts">
  import { uriName } from "../api";
  import { commands, enabled, keysFor } from "../commands.svelte";
  import { fuzzy, markMatches } from "../fuzzy";
  import { formatCombo } from "../keys";
  import { tagUri } from "../search.svelte";
  import { devices } from "../stores/devices.svelte";
  import { settings } from "../stores/settings.svelte";
  import { TAG_COLORS } from "../tags";
  import { ws } from "../workspace.svelte";
  import Icon, { type IconName } from "./Icon.svelte";

  interface Entry {
    label: string;
    detail?: string;
    icon: IconName;
    kind: string;
    keys?: string;
    run: () => void;
  }

  let open = $state(false);
  let query = $state("");
  let active = $state(0);
  let input: HTMLInputElement | undefined = $state();
  let list: HTMLDivElement | undefined = $state();

  $effect(() => {
    const show = () => {
      open = true;
      query = "";
      active = 0;
      queueMicrotask(() => input?.focus());
    };
    document.addEventListener("cx:palette", show);
    return () => document.removeEventListener("cx:palette", show);
  });

  const go = (uri: string) => () => ws.activeTab.navigate(uri);
  const pretty = (uri: string) => (uri.startsWith("file://") ? decodeURIComponent(uri.slice(7)) : uri);

  let entries = $derived.by((): Entry[] => {
    if (!open) return [];
    const out: Entry[] = [];
    for (const c of commands) {
      if (!enabled(c.id)) continue;
      const k = keysFor(c)[0];
      out.push({ label: c.label, detail: c.group, icon: c.icon ?? "command", kind: "Command", keys: k ? formatCombo(k) : undefined, run: () => void c.run() });
    }
    for (const p of ws.panes) for (const t of p.tabs) out.push({ label: t.title || "Tab", detail: t.folder.info?.display, icon: "folder", kind: "Tab", run: () => (ws.focusPane(p.id), p.activate(t.id)) });
    for (const f of ws.places?.favorites ?? []) out.push({ label: f.name, detail: pretty(f.uri), icon: f.icon as IconName, kind: "Favorite", run: go(f.uri) });
    for (const b of settings.data.bookmarks) out.push({ label: b.name, detail: pretty(b.uri), icon: "star", kind: "Favorite", run: go(b.uri) });
    for (const r of settings.data.recent) out.push({ label: uriName(r) || r, detail: pretty(r), icon: "folder", kind: "Recent", run: go(r) });
    for (const c of ws.places?.cloud ?? []) out.push({ label: c.name, detail: c.account ?? pretty(c.uri), icon: "cloud", kind: "Cloud", run: go(c.uri) });
    for (const v of ws.places?.volumes ?? []) out.push({ label: v.name, detail: pretty(v.uri), icon: "drive", kind: "Drive", run: go(v.uri) });
    for (const d of devices.nearby) for (const s of d.services) out.push({ label: `${d.name} — ${s.label}`, detail: s.uri, icon: "server", kind: "Device", run: go(s.uri) });
    for (const s of settings.data.servers) out.push({ label: s.name, detail: s.uri, icon: "server", kind: "Server", run: go(s.uri) });
    for (const w of settings.data.workspaces) out.push({ label: `Open workspace “${w.name}”`, icon: "columns", kind: "Workspace", run: () => ws.restore(w) });
    for (const tag of Object.keys(TAG_COLORS)) out.push({ label: `Tag: ${tag}`, icon: "tag", kind: "Tag", run: go(tagUri(tag)) });
    return out;
  });

  let isPath = $derived(/^(~|\/|\\\\|[A-Za-z]:[\\/]|\w+:\/\/)/.test(query.trim()));

  let results = $derived.by(() => {
    const q = query.trim();
    const scored = entries
      .map((e) => ({ e, m: fuzzy(q, e.label) ?? (e.detail ? fuzzy(q, e.detail) && { score: (fuzzy(q, e.detail)!.score - 8), positions: [] } : null) }))
      .filter((x): x is { e: Entry; m: { score: number; positions: number[] } } => !!x.m);
    if (q) scored.sort((a, b) => b.m.score - a.m.score);
    const top = scored.slice(0, 60);
    if (isPath) top.unshift({ e: { label: `Go to ${q}`, icon: "forward", kind: "Path", run: go(q) }, m: { score: 1e9, positions: [] } });
    return top;
  });

  $effect(() => {
    void query;
    active = 0;
  });

  function close() {
    open = false;
    requestAnimationFrame(() => (document.querySelector(".pane.active .file-view") as HTMLElement | null)?.focus());
  }

  function choose(i: number) {
    const r = results[i];
    if (!r) return;
    close();
    r.e.run();
  }

  function onkeydown(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === "ArrowDown") active = Math.min(results.length - 1, active + 1);
    else if (e.key === "ArrowUp") active = Math.max(0, active - 1);
    else if (e.key === "Enter") choose(active);
    else if (e.key === "Escape") close();
    else return;
    e.preventDefault();
    requestAnimationFrame(() => list?.querySelector(".active")?.scrollIntoView({ block: "nearest" }));
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
  <div class="backdrop" onclick={close}></div>
  <div class="palette" role="dialog" aria-label="Command palette">
    <div class="field">
      <Icon name="search" size={16} />
      <input bind:this={input} bind:value={query} {onkeydown} placeholder="Type a command, folder, device or path…" spellcheck="false" aria-label="Search" />
    </div>
    <div class="results" bind:this={list} role="listbox">
      {#each results as r, i (r.e.kind + r.e.label + (r.e.detail ?? "") + i)}
        <button class="res" class:active={i === active} role="option" aria-selected={i === active} onpointermove={() => (active = i)} onclick={() => choose(i)}>
          <span class="ico"><Icon name={r.e.icon} size={16} /></span>
          <span class="label">{@html markMatches(r.e.label, r.m.positions)}</span>
          {#if r.e.detail}<span class="detail">{r.e.detail}</span>{/if}
          {#if r.e.keys}<kbd>{r.e.keys}</kbd>{:else}<span class="kind">{r.e.kind}</span>{/if}
        </button>
      {:else}
        <div class="none">No matches</div>
      {/each}
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 95;
    background: rgba(0, 0, 0, 0.12);
  }
  .palette {
    position: fixed;
    z-index: 96;
    top: 72px;
    left: 50%;
    transform: translateX(-50%);
    width: min(640px, calc(100vw - 32px));
    border-radius: 12px;
    background: var(--flyout);
    backdrop-filter: blur(40px) saturate(1.5);
    -webkit-backdrop-filter: blur(40px) saturate(1.5);
    box-shadow: 0 24px 64px rgba(0, 0, 0, 0.3), 0 0 0 1px var(--stroke-strong);
    overflow: hidden;
    animation: drop 0.14s var(--ease);
  }
  @keyframes drop {
    from {
      opacity: 0;
      transform: translate(-50%, -8px);
    }
  }
  .field {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 16px;
    height: 50px;
    border-bottom: 1px solid var(--stroke);
    color: var(--text-3);
  }
  input {
    flex: 1;
    height: 100%;
    border: 0;
    outline: none;
    background: transparent;
    font-size: 15px;
    color: var(--text);
  }
  .results {
    max-height: min(420px, 60vh);
    overflow-y: auto;
    padding: 6px;
  }
  .res {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    height: 36px;
    padding: 0 10px;
    border-radius: var(--radius);
    text-align: left;
  }
  .res.active {
    background: var(--sel);
  }
  .ico {
    color: var(--text-2);
    display: grid;
  }
  .label {
    white-space: nowrap;
  }
  .label :global(mark) {
    background: none;
    color: var(--accent);
    font-weight: 600;
  }
  .detail {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    color: var(--text-3);
  }
  .kind {
    font-size: 11px;
    color: var(--text-3);
    margin-left: auto;
  }
  kbd {
    margin-left: auto;
    font-family: inherit;
    font-size: 11px;
    padding: 2px 6px;
    border-radius: 4px;
    background: var(--hover);
    color: var(--text-2);
  }
  .none {
    padding: 20px;
    text-align: center;
    color: var(--text-3);
  }
</style>
