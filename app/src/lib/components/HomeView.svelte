<script lang="ts">
  // The Home page: pinned and recent folders, drives, nearby devices and
  // recent transfers — Explorer's Home and Finder's Recents in one place.
  import { uriName, type Device, type Service } from "../api";
  import { formatSize } from "../format";
  import { devices } from "../stores/devices.svelte";
  import { dialogs } from "../stores/dialogs.svelte";
  import { settings } from "../stores/settings.svelte";
  import { jobTitle, transfers } from "../stores/transfers.svelte";
  import { ws, type Tab } from "../workspace.svelte";
  import DeviceIcon from "./DeviceIcon.svelte";
  import FileIcon from "./FileIcon.svelte";
  import Icon, { type IconName } from "./Icon.svelte";

  let { tab }: { tab: Tab } = $props();

  let pinned = $derived([...(ws.places?.favorites ?? []).map((p) => ({ name: p.name, uri: p.uri, icon: p.icon as IconName })), ...settings.data.bookmarks.map((b) => ({ name: b.name, uri: b.uri, icon: "star" as IconName }))]);
  let recent = $derived(settings.data.recent.filter((u) => !pinned.some((p) => p.uri === u)).slice(0, 8));
  let finished = $derived(transfers.jobs.filter((j) => j.state === "done").slice(0, 4));

  function pretty(uri: string) {
    if (!uri.startsWith("file://")) return uri.replace(/^(\w+):\/\//, "$1 · ");
    return decodeURIComponent(uri.slice(7)).replace(/^\/Users\/[^/]+/, "~");
  }

  function open(uri: string, e?: MouseEvent) {
    if (e && (e.metaKey || e.ctrlKey || e.button === 1)) ws.newTab(uri, false);
    else tab.navigate(uri);
  }

  function primaryService(d: Device): Service | undefined {
    const order = ["peer", "smb", "sftp", "davs", "dav", "ftps", "ftp"];
    return [...d.services].sort((a, b) => order.indexOf(a.scheme) - order.indexOf(b.scheme))[0];
  }

  async function openDevice(d: Device) {
    const s = primaryService(d);
    if (s) tab.navigate(s.uri);
    else if (d.tailnet) await dialogs.ask("connect", { host: d.hostname ?? d.addresses[0] });
  }
</script>

<div class="home file-view" tabindex="-1" role="region" aria-label="Home">
  <section>
    <h2>Quick access</h2>
    <div class="tiles">
      {#each pinned as p (p.uri)}
        <button class="tile" onclick={(e) => open(p.uri, e)} onauxclick={(e) => open(p.uri, e)} title={pretty(p.uri)}>
          <FileIcon name="" isDir size={40} />
          <span class="name">{p.name}</span>
          <span class="sub">{pretty(p.uri)}</span>
        </button>
      {/each}
    </div>
  </section>

  {#if recent.length}
    <section>
      <h2>Recent folders</h2>
      <div class="list">
        {#each recent as uri (uri)}
          <button class="row" onclick={(e) => open(uri, e)} onauxclick={(e) => open(uri, e)}>
            <FileIcon name="" isDir size={18} />
            <span class="name">{uriName(uri) || uri}</span>
            <span class="sub">{pretty(uri)}</span>
          </button>
        {/each}
      </div>
    </section>
  {/if}

  <section>
    <h2>
      Devices & network
      <span class="h-actions">
        <button onclick={() => devices.scan()} disabled={devices.scanning}><Icon name="radar" size={14} /> {devices.scanning ? "Scanning…" : "Scan"}</button>
        <button onclick={() => dialogs.ask("connect")}><Icon name="server" size={14} /> Connect to server</button>
      </span>
    </h2>
    <div class="tiles">
      {#each ws.places?.volumes ?? [] as v (v.uri)}
        <button class="tile device" onclick={(e) => open(v.uri, e)}>
          <span class="dicon"><Icon name={v.removable ? "external" : "drive"} size={22} /></span>
          <span class="name">{v.name}</span>
          <span class="meter"><span style:width="{v.total ? Math.round((1 - v.free / v.total) * 100) : 0}%"></span></span>
          <span class="sub">{formatSize(v.free)} free of {formatSize(v.total)}</span>
        </button>
      {/each}
      {#each devices.nearby as d (d.id)}
        {@const offline = d.tailnet && !d.tailnet.online}
        <div class="tile device" class:offline>
          <button class="main" onclick={() => openDevice(d)} disabled={!!offline}>
            <span class="dicon"><DeviceIcon kind={d.kind} size={22} /></span>
            <span class="name">{d.name}</span>
            <span class="sub">{offline ? "Offline" : d.services.map((s) => s.label).join(" · ") || (d.tailnet ? "Tailnet device" : "Nearby")}</span>
          </button>
          {#if d.shares.length}
            <div class="shares">
              {#each d.shares.slice(0, 4) as s (s.uri)}
                <button class="chip" onclick={(e) => open(s.uri, e)}>{s.name}</button>
              {/each}
            </div>
          {/if}
          {#if d.tailnet}<span class="badge" title="On your tailnet">tailnet</span>{/if}
        </div>
      {/each}
      {#each settings.data.servers as s (s.uri)}
        <button class="tile device" onclick={(e) => open(s.uri, e)}>
          <span class="dicon"><Icon name="server" size={22} /></span>
          <span class="name">{s.name}</span>
          <span class="sub">{pretty(s.uri)}</span>
        </button>
      {/each}
    </div>
  </section>

  {#if finished.length}
    <section>
      <h2>Recent transfers</h2>
      <div class="list">
        {#each finished as j (j.id)}
          <button class="row" onclick={() => j.dest && open(j.dest)}>
            <Icon name="transfer" size={16} />
            <span class="name">{jobTitle(j)}</span>
            <span class="sub">{j.dest ? `to ${pretty(j.dest)}` : ""}</span>
          </button>
        {/each}
      </div>
    </section>
  {/if}
</div>

<style>
  .home {
    flex: 1;
    overflow-y: auto;
    padding: 12px 24px 32px;
    outline: none;
  }
  section {
    margin-top: 14px;
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 0 10px;
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
  }
  .h-actions {
    display: flex;
    gap: 4px;
    margin-left: auto;
  }
  .h-actions button {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 10px;
    border-radius: var(--radius);
    font-weight: 400;
    color: var(--text-2);
  }
  .h-actions button:hover {
    background: var(--hover);
    color: var(--text);
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(170px, 1fr));
    gap: 8px;
  }
  .tile {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 4px;
    min-width: 0;
    padding: 12px;
    border-radius: var(--radius-lg);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke);
    text-align: left;
    transition: background 0.1s, box-shadow 0.1s;
  }
  .tile:hover {
    box-shadow: 0 0 0 1px var(--stroke-strong), 0 2px 8px rgba(0, 0, 0, 0.06);
  }
  .tile .main {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 4px;
    width: 100%;
    text-align: left;
  }
  .tile.offline {
    opacity: 0.55;
  }
  .dicon {
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    border-radius: 10px;
    background: var(--accent-soft);
    color: var(--accent);
    margin-bottom: 2px;
  }
  .name {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 500;
  }
  .sub {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 11.5px;
    color: var(--text-3);
  }
  .meter {
    display: block;
    width: 100%;
    height: 4px;
    border-radius: 2px;
    background: var(--stroke-strong);
    overflow: hidden;
  }
  .meter span {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  .shares {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 4px;
  }
  .chip {
    padding: 2px 8px;
    border-radius: 10px;
    font-size: 11.5px;
    background: var(--hover);
  }
  .chip:hover {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .badge {
    position: absolute;
    top: 10px;
    right: 10px;
    font-size: 10px;
    padding: 1px 6px;
    border-radius: 8px;
    background: var(--accent-soft);
    color: var(--accent);
  }
  .list {
    display: flex;
    flex-direction: column;
  }
  .row {
    display: grid;
    grid-template-columns: 22px minmax(120px, 280px) 1fr;
    align-items: center;
    gap: 10px;
    height: 32px;
    padding: 0 10px;
    border-radius: var(--radius);
    text-align: left;
  }
  .row:hover {
    background: var(--hover);
  }
</style>
