<script lang="ts">
  // "Copy to…" / "Move to…": pick destinations from everywhere the app knows
  // about (other pane, tabs, favorites, recents, servers, devices) or browse
  // to one. Copy can go to several places at once; one job per destination.
  import { asCxError, connectServer, errorText, isCloudUri, listDir, trustHostKey, type CxError, uriName, type ConflictPolicy, type Device, type Entry } from "../api";
  import FileIcon from "../components/FileIcon.svelte";
  import Icon, { type IconName } from "../components/Icon.svelte";
  import { deviceTarget, devices } from "../stores/devices.svelte";
  import DeviceIcon from "../components/DeviceIcon.svelte";
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
  let browseProblem = $state<CxError | null>(null);
  let retry = $state(0);
  let signUser = $state("");
  let signPass = $state("");
  let signing = $state(false);
  let pathInput = $state("");

  /** Pseudo location: the list of drives (plus home and cloud folders). */
  const DRIVES = "cx:drives";
  interface Spot {
    uri: string | null;
    label: string;
    detail: string;
    icon?: IconName;
    device?: Device;
    offline?: boolean;
  }
  let drives = $derived.by(() => {
    const sections: { title: string; spots: Spot[] }[] = [];
    const local: Spot[] = [
      ...(ws.places ? [{ uri: ws.places.home.uri, label: ws.places.home.name, detail: "Home", icon: "home" as IconName }] : []),
      ...(ws.places?.volumes ?? []).map((v) => ({ uri: v.uri, label: v.name, detail: pretty(v.uri), icon: (v.removable ? "external" : "drive") as IconName })),
    ];
    sections.push({ title: "Drives", spots: local });
    const cloud = (ws.places?.cloud ?? []).map((c) => ({ uri: c.uri, label: c.name, detail: c.account ?? "Synced folder", icon: "cloud" as IconName }));
    if (cloud.length) sections.push({ title: "Cloud", spots: cloud });
    // Everything the sidebar's Network section shows: saved servers, other
    // connected servers, nearby and tailnet devices (and their shares).
    const seen = new Set<string>();
    const net: Spot[] = [];
    const add = (s: Spot) => {
      const k = s.uri ? norm(s.uri) : s.label;
      if (seen.has(k)) return;
      seen.add(k);
      net.push(s);
    };
    for (const srv of settings.data.servers) add({ uri: srv.uri, label: srv.name, detail: srv.uri.replace(/^(\w+):\/\/.*/, "$1").toUpperCase(), icon: isCloudUri(srv.uri) ? "cloud" : "server" });
    for (const c of devices.connected) add({ uri: c, label: c.replace(/^\w+:\/\//, "").replace(/\/$/, ""), detail: "Connected", icon: "server" });
    for (const d of devices.nearby) {
      const target = deviceTarget(d);
      const offline = !!d.tailnet && !d.tailnet.online;
      const svc = d.services.find((s) => s.uri === target);
      add({ uri: target, label: d.name, detail: offline ? "Offline" : (svc?.label ?? "No file sharing found"), device: d, offline: offline || !target });
      for (const sh of d.shares) add({ uri: sh.uri, label: `${d.name} › ${sh.name}`, detail: "Share", icon: "folder" });
    }
    sections.push({ title: "Network", spots: net });
    return sections;
  });

  $effect(() => {
    if (!browsing) return;
    void retry;
    const uri = browseUri;
    browseProblem = null;
    if (uri === DRIVES) {
      browseDirs = [];
      browseError = null;
      return;
    }
    browseDirs = null;
    browseError = null;
    const dirs: Entry[] = [];
    listDir(uri, (ev) => {
      if (ev.type === "meta") browseUri = ev.info.uri;
      else if (ev.type === "batch") dirs.push(...ev.entries.filter((e) => e.isDir && (settings.data.showHidden || !e.hidden)));
    })
      .then(() => (browseDirs = dirs.sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }))))
      .catch((e) => {
        browseError = errorText(e);
        browseProblem = asCxError(e);
        if (browseProblem?.kind === "authRequired") signUser = browseProblem.message.user ?? "";
      });
  });

  /** Sign in (or trust the SSH key) right here, then list again. */
  async function fixAccess() {
    const p = browseProblem;
    if (!p || signing) return;
    signing = true;
    try {
      if (p.kind === "authRequired") {
        await connectServer(p.message.uri, { user: signUser, secret: signPass ? { type: "password", password: signPass } : { type: "none" } }, true);
        signPass = "";
      } else if (p.kind === "hostKeyUnknown") {
        await trustHostKey(p.message.uri, p.message.keyType, p.message.fingerprint);
      }
      retry++;
    } catch (e) {
      browseError = errorText(e);
      browseProblem = asCxError(e) ?? p;
    } finally {
      signing = false;
    }
  }

  const upOf = (uri: string) => {
    if (uri === DRIVES) return null;
    const n = norm(uri);
    const i = n.lastIndexOf("/");
    // Above a drive's root (or a server's) come the drives.
    return i > n.indexOf("://") + 2 ? n.slice(0, i) || n : DRIVES;
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
        <button type="button" class="icon" class:on={browseUri === DRIVES} aria-label="Drives" title="Drives" onclick={() => (browseUri = DRIVES)}><Icon name="drive" size={15} /></button>
        <input type="text" bind:value={pathInput} placeholder={browseUri === DRIVES ? "Drives — or type a path / server URL" : pretty(browseUri)} onkeydown={(e) => e.key === "Enter" && (e.preventDefault(), goToTyped())} spellcheck="false" aria-label="Path" />
      </div>
      <div class="blist">
        {#if browseUri === DRIVES}
          {#each drives as sec (sec.title)}
            <div class="bsection">{sec.title}</div>
            {#each sec.spots as d, i (d.uri ?? `${d.label}-${i}`)}
              <button type="button" class="dir" disabled={!d.uri || d.offline} onclick={() => d.uri && (browseUri = d.uri)} title={d.uri ?? d.label}>
                {#if d.device}<DeviceIcon kind={d.device.kind} size={16} />{:else}<Icon name={d.icon ?? "folder"} size={16} />{/if}
                <span>{d.label}</span>
                <span class="muted ddetail">{d.detail}</span>
                <Icon name="chevronRight" size={11} />
              </button>
            {/each}
            {#if sec.title === "Network" && !sec.spots.length}
              <p class="muted">No servers or devices yet — type a server URL above, or connect with ⌘K.</p>
            {/if}
          {/each}
        {:else if browseProblem?.kind === "authRequired"}
          <div class="signin">
            <p>{browseError}</p>
            <div class="row">
              <label class="field">User<input type="text" bind:value={signUser} autocomplete="username" spellcheck="false" /></label>
              <label class="field">Password<input type="password" bind:value={signPass} autocomplete="current-password" onkeydown={(e) => e.key === "Enter" && (e.preventDefault(), fixAccess())} /></label>
            </div>
            <button type="button" class="btn primary" disabled={signing} onclick={fixAccess}>{signing ? "Signing in…" : "Sign in"}</button>
          </div>
        {:else if browseProblem?.kind === "hostKeyUnknown"}
          <div class="signin">
            <p>{browseProblem.message.changed ? "⚠️ This server's key has CHANGED since you last connected." : "First connection to this server."} Key {browseProblem.message.keyType}:</p>
            <code>{browseProblem.message.fingerprint}</code>
            <button type="button" class="btn primary" disabled={signing} onclick={fixAccess}>Trust and connect</button>
          </div>
        {:else if browseError}
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
        <span class="muted where">{browseUri === DRIVES ? "Drives" : pretty(browseUri)}</span>
        <button type="button" class="btn" onclick={() => (browsing = false)}>Back</button>
        <button type="button" class="btn primary" disabled={!!browseError || browseUri === DRIVES} onclick={() => addCustom(browseUri)}>Choose this folder</button>
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
  .signin {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
    padding: 12px;
  }
  .signin .row {
    display: flex;
    gap: 8px;
    width: 100%;
  }
  .signin .field {
    flex: 1;
  }
  .signin code {
    font-size: 12px;
    word-break: break-all;
  }
  .bsection {
    padding: 8px 8px 2px;
    font-size: 11px;
    font-weight: 600;
    color: var(--text-2, inherit);
    opacity: 0.7;
  }
  .dir:disabled {
    opacity: 0.45;
  }
  .dir .ddetail {
    flex: none;
    max-width: 45%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
  }
  .icon.on {
    background: var(--accent-soft);
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
