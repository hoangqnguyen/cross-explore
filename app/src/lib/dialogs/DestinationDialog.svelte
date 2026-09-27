<script lang="ts">
  // "Copy to…" / "Move to…": pick destinations from everywhere the app knows
  // about (other pane, tabs, favorites, recents, servers, devices) or browse
  // to one. Copy can go to several places at once; one job per destination.
  import { errorText, listDir, uriName, type ConflictPolicy, type Entry } from "../api";
  import FileIcon from "../components/FileIcon.svelte";
  import Icon, { type IconName } from "../components/Icon.svelte";
  import { devices } from "../stores/devices.svelte";
  import { dialogs } from "../stores/dialogs.svelte";
  import { settings } from "../stores/settings.svelte";
  import { transfers } from "../stores/transfers.svelte";
  import { toasts } from "../toasts.svelte";
  import { ws } from "../workspace.svelte";
  import Modal from "./Modal.svelte";

  let { uris, mode }: { uris: string[]; mode: "copy" | "move" } = $props();

  interface Place {
    uri: string;
    label: string;
    detail: string;
    icon: IconName;
  }

  const norm = (u: string) => u.replace(/\/+$/, "");
  const pretty = (uri: string) => {
    const home = ws.places?.home.uri && norm(ws.places.home.uri);
    if (!uri.startsWith("file://")) return uri;
    if (home && (norm(uri) === home || uri.startsWith(home + "/"))) return "~" + decodeURIComponent(uri.slice(home.length));
    return decodeURIComponent(uri.slice(7));
  };
  // The folder(s) the items are in: copying there is "duplicate", not this.
  // svelte-ignore state_referenced_locally
  const sourceDirs = new Set(uris.map((u) => norm(u.replace(/\/[^/]*\/?$/, ""))));

  let groups = $derived.by(() => {
    const seen = new Set<string>();
    const out: { title: string; places: Place[] }[] = [];
    const group = (title: string, places: Place[]) => {
      const fresh = places.filter((p) => {
        const k = norm(p.uri);
        if (seen.has(k) || uris.some((u) => norm(u) === k)) return false;
        seen.add(k);
        return true;
      });
      if (fresh.length) out.push({ title, places: fresh });
    };
    const other = ws.otherTab;
    if (other && other.folder.kind === "folder") group("Other pane", [{ uri: other.dirUri, label: other.title, detail: pretty(other.dirUri), icon: "columns" }]);
    group("Recent destinations", settings.data.recentDestinations.map((u) => ({ uri: u, label: uriName(u) || u, detail: pretty(u), icon: "transfer" as IconName })));
    group(
      "Open tabs",
      ws.allTabs.filter((t) => t.folder.kind === "folder" && !sourceDirs.has(norm(t.dirUri))).map((t) => ({ uri: t.dirUri, label: t.title, detail: pretty(t.dirUri), icon: "folder" as IconName })),
    );
    group("Favorites", [
      ...(ws.places?.favorites ?? []).map((f) => ({ uri: f.uri, label: f.name, detail: pretty(f.uri), icon: f.icon as IconName })),
      ...settings.data.bookmarks.map((b) => ({ uri: b.uri, label: b.name, detail: pretty(b.uri), icon: "star" as IconName })),
    ]);
    group("Recent folders", settings.data.recent.slice(0, 8).map((u) => ({ uri: u, label: uriName(u) || u, detail: pretty(u), icon: "folder" as IconName })));
    group("Network", [
      ...settings.data.servers.map((s) => ({ uri: s.uri, label: s.name, detail: s.uri, icon: "server" as IconName })),
      ...devices.nearby.flatMap((d) => d.services.filter((s) => s.scheme === "peer" || s.scheme === "smb").map((s) => ({ uri: s.uri, label: `${d.name}`, detail: s.label, icon: "server" as IconName }))),
    ]);
    group("Cloud", (ws.places?.cloud ?? []).map((c) => ({ uri: c.uri, label: c.name, detail: c.account ?? pretty(c.uri), icon: "cloud" as IconName })));
    group("Drives", (ws.places?.volumes ?? []).map((v) => ({ uri: v.uri, label: v.name, detail: pretty(v.uri), icon: "drive" as IconName })));
    return out;
  });

  let chosen = $state<string[]>([]);
  let custom = $state<Place[]>([]);
  let conflict = $state<ConflictPolicy>("ask");
  let busy = $state(false);

  function toggle(uri: string) {
    if (mode === "move") chosen = chosen.includes(uri) ? [] : [uri];
    else chosen = chosen.includes(uri) ? chosen.filter((u) => u !== uri) : [...chosen, uri];
  }

  // ---- inline folder browser ----
  let browsing = $state(false);
  let browseUri = $state(ws.activeTab.folder.kind === "folder" ? ws.activeTab.dirUri : (ws.places?.home.uri ?? "~"));
  let browseDirs = $state<Entry[] | null>(null);
  let browseError = $state<string | null>(null);
  let pathInput = $state("");

  $effect(() => {
    if (!browsing) return;
    const uri = browseUri;
    browseDirs = null;
    browseError = null;
    const dirs: Entry[] = [];
    listDir(uri, (ev) => {
      if (ev.type === "meta") browseUri = ev.info.uri;
      else if (ev.type === "batch") dirs.push(...ev.entries.filter((e) => e.isDir && (settings.data.showHidden || !e.hidden)));
    })
      .then(() => (browseDirs = dirs.sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }))))
      .catch((e) => (browseError = errorText(e)));
  });

  const upOf = (uri: string) => {
    const n = norm(uri);
    const i = n.lastIndexOf("/");
    return i > n.indexOf("://") + 2 ? n.slice(0, i) || n : null;
  };

  function addCustom(uri: string) {
    const place = { uri, label: uriName(uri) || uri, detail: pretty(uri), icon: "folder" as IconName };
    if (!custom.some((c) => norm(c.uri) === norm(uri)) && !groups.some((g) => g.places.some((p) => norm(p.uri) === norm(uri)))) custom = [...custom, place];
    if (!chosen.includes(uri)) toggle(uri);
    browsing = false;
  }

  function goToTyped() {
    const v = pathInput.trim();
    if (v) browseUri = v;
  }

  async function submit() {
    if (!chosen.length) return;
    busy = true;
    let ok = 0;
    for (const dest of chosen) {
      const id = await transfers.submit({ kind: mode, sources: uris, dest, conflict });
      if (id != null) ok++;
    }
    settings.data.recentDestinations = [...chosen, ...settings.data.recentDestinations.filter((u) => !chosen.includes(u))].slice(0, 8);
    busy = false;
    if (ok) {
      transfers.flyoutOpen = true;
      toasts.show(`${mode === "copy" ? "Copying" : "Moving"} ${what} to ${chosen.length === 1 ? `“${uriName(chosen[0]) || chosen[0]}”` : `${chosen.length} places`}`);
      dialogs.close(true);
    }
  }

  let what = $derived(uris.length === 1 ? `“${uriName(uris[0])}”` : `${uris.length} items`);
  let title = $derived(`${mode === "copy" ? "Copy" : "Move"} ${what} to…`);
</script>

{#snippet row(p: Place)}
  {@const on = chosen.includes(p.uri)}
  <button type="button" class="place" class:on onclick={() => toggle(p.uri)} title={p.uri}>
    <span class="mark" class:radio={mode === "move"}>{#if on}<Icon name="check" size={12} stroke={2.2} />{/if}</span>
    <Icon name={p.icon} size={15} />
    <span class="label">{p.label}</span>
    <span class="detail">{p.detail}</span>
  </button>
{/snippet}

<Modal {title} width={620} onsubmit={submit}>
  {#if browsing}
    <div class="browser">
      <div class="bbar">
        <button type="button" class="icon" aria-label="Up" disabled={!upOf(browseUri)} onclick={() => (browseUri = upOf(browseUri)!)}><Icon name="up" size={15} /></button>
        <input type="text" bind:value={pathInput} placeholder={pretty(browseUri)} onkeydown={(e) => e.key === "Enter" && (e.preventDefault(), goToTyped())} spellcheck="false" aria-label="Path" />
      </div>
      <div class="blist">
        {#if browseError}
          <p class="error">{browseError}</p>
        {:else if !browseDirs}
          <p class="muted">Loading…</p>
        {:else if !browseDirs.length}
          <p class="muted">No subfolders</p>
        {:else}
          {#each browseDirs as d (d.name)}
            <button type="button" class="dir" ondblclick={() => (browseUri = norm(browseUri) + "/" + encodeURIComponent(d.name))} onclick={() => (browseUri = norm(browseUri) + "/" + encodeURIComponent(d.name))}>
              <FileIcon name={d.name} isDir size={16} />
              <span>{d.name}</span>
              <Icon name="chevronRight" size={11} />
            </button>
          {/each}
        {/if}
      </div>
      <div class="bfoot">
        <span class="muted where">{pretty(browseUri)}</span>
        <button type="button" class="btn" onclick={() => (browsing = false)}>Back</button>
        <button type="button" class="btn primary" disabled={!!browseError} onclick={() => addCustom(browseUri)}>Choose this folder</button>
      </div>
    </div>
  {:else}
    <p class="hint">{mode === "copy" ? "Tick one or more destinations. Each gets its own copy." : "Choose where to move them."}</p>
    <div class="places">
      {#if custom.length}
        <div class="gtitle">Chosen</div>
        {#each custom as p (p.uri)}{@render row(p)}{/each}
      {/if}
      {#each groups as g (g.title)}
        <div class="gtitle">{g.title}</div>
        {#each g.places as p (p.uri)}{@render row(p)}{/each}
      {/each}
    </div>
    <button type="button" class="browse" onclick={() => (browsing = true)}><Icon name="folder" size={15} /> Choose another folder or type a path / server URL…</button>
  {/if}
  {#snippet footer()}
    <label class="policy">
      If a name exists
      <select bind:value={conflict}>
        <option value="ask">Ask me</option>
        <option value="keepBoth">Keep both</option>
        <option value="replace">Replace</option>
        <option value="replaceIfNewer">Replace if newer</option>
        <option value="skip">Skip</option>
      </select>
    </label>
    <button type="button" class="btn" onclick={() => dialogs.close(null)}>Cancel</button>
    <button type="submit" class="btn primary" disabled={!chosen.length || busy || browsing}>
      {mode === "copy" ? (chosen.length > 1 ? `Copy to ${chosen.length} places` : "Copy") : "Move"}
    </button>
  {/snippet}
</Modal>

<style>
  .hint {
    margin: 0 0 8px;
  }
  .places {
    max-height: min(52vh, 440px);
    overflow-y: auto;
    border-radius: var(--radius);
    box-shadow: inset 0 0 0 1px var(--stroke);
    padding: 4px;
  }
  .gtitle {
    padding: 8px 8px 3px;
    font-size: 11px;
    font-weight: 600;
    color: var(--text-3);
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }
  .place {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    height: 34px;
    padding: 0 8px;
    border-radius: var(--radius);
    text-align: left;
  }
  .place:hover {
    background: var(--hover);
  }
  .place.on {
    background: var(--sel);
  }
  .place :global(svg) {
    color: var(--accent);
    flex: none;
  }
  .mark {
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    border-radius: 4px;
    box-shadow: inset 0 0 0 1.5px var(--stroke-strong);
    flex: none;
  }
  .mark.radio {
    border-radius: 50%;
  }
  .on .mark {
    background: var(--accent);
    box-shadow: none;
  }
  .on .mark :global(svg) {
    color: var(--accent-text);
  }
  .label {
    white-space: nowrap;
    font-weight: 500;
  }
  .detail {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-3);
    font-size: 12px;
    text-align: right;
  }
  .browse {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 10px;
    color: var(--accent);
  }
  .browse:hover {
    text-decoration: underline;
  }
  .browser {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .bbar {
    display: flex;
    gap: 6px;
  }
  .bbar input {
    flex: 1;
  }
  .icon {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: var(--radius);
    box-shadow: inset 0 0 0 1px var(--stroke-strong);
  }
  .blist {
    height: min(44vh, 360px);
    overflow-y: auto;
    border-radius: var(--radius);
    box-shadow: inset 0 0 0 1px var(--stroke);
    padding: 4px;
  }
  .dir {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 30px;
    padding: 0 8px;
    border-radius: 4px;
    text-align: left;
  }
  .dir span {
    flex: 1;
  }
  .dir:hover {
    background: var(--hover);
  }
  .bfoot {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .where {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .policy {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-right: auto;
    font-size: 12px;
    color: var(--text-2);
  }
  .policy select {
    height: 30px;
  }
</style>
