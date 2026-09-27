<script lang="ts">
  import { disconnectServer, errorText, type Device } from "../api";
  import { formatSize } from "../format";
  import { dropTarget } from "../listing";
  import { menu } from "../menu.svelte";
  import { tagUri } from "../search.svelte";
  import { devices } from "../stores/devices.svelte";
  import { dialogs } from "../stores/dialogs.svelte";
  import { settings } from "../stores/settings.svelte";
  import { ui } from "../stores/ui.svelte";
  import { TAG_COLORS } from "../tags";
  import { toasts } from "../toasts.svelte";
  import { HOME_URI, ws } from "../workspace.svelte";
  import DeviceIcon from "./DeviceIcon.svelte";
  import Icon, { type IconName } from "./Icon.svelte";

  let current = $derived(norm(ws.activeTab?.folder.info?.uri ?? ws.activeTab?.folder.uri ?? ""));
  let collapsed = $state<Record<string, boolean>>({ tags: true });
  let expanded = $state<Record<string, boolean>>({});

  function norm(uri: string) {
    return uri.replace(/\/+$/, "");
  }

  function go(uri: string, e: MouseEvent) {
    ui.drawerOpen = false;
    // Cmd/Ctrl-click or middle-click opens in a new tab, like a browser.
    if (e.metaKey || e.ctrlKey || e.button === 1) ws.newTab(uri, false);
    else ws.activeTab?.navigate(uri);
  }

  let dragging = false;
  function resize(e: PointerEvent) {
    if (dragging) settings.data.sidebarWidth = Math.round(Math.min(360, Math.max(170, e.clientX)));
  }

  function bookmarkMenu(e: MouseEvent, uri: string) {
    e.preventDefault();
    menu.show(
      [
        { label: "Open in new tab", icon: "plus", action: () => ws.newTab(uri) },
        { label: "Remove from Favorites", icon: "close", action: () => (settings.data.bookmarks = settings.data.bookmarks.filter((b) => b.uri !== uri)) },
      ],
      e.clientX,
      e.clientY,
    );
  }

  function serverMenu(e: MouseEvent, uri: string, saved: boolean) {
    e.preventDefault();
    menu.show(
      [
        { label: "Open in new tab", icon: "plus", action: () => ws.newTab(uri) },
        ...(devices.connected.some((c) => uri.startsWith(c)) ? [{ label: "Disconnect", icon: "close" as const, action: () => disconnect(uri) }] : []),
        ...(saved ? [{ label: "Remove", icon: "trash" as const, action: () => (settings.data.servers = settings.data.servers.filter((s) => s.uri !== uri)) }] : []),
      ],
      e.clientX,
      e.clientY,
    );
  }

  async function disconnect(uri: string) {
    try {
      await disconnectServer(uri);
      await devices.refreshConnections();
    } catch (e) {
      toasts.show(errorText(e), "error");
    }
  }

  function deviceTarget(d: Device): string | null {
    const order = ["peer", "smb", "sftp", "davs", "dav", "ftps", "ftp"];
    return [...d.services].sort((a, b) => order.indexOf(a.scheme) - order.indexOf(b.scheme))[0]?.uri ?? null;
  }

  let savedUris = $derived(new Set(settings.data.servers.map((s) => norm(s.uri))));
  let loose = $derived(devices.connected.filter((c) => !savedUris.has(norm(c)) && !devices.nearby.some((d) => d.services.some((s) => norm(s.uri) === norm(c)))));
</script>

{#snippet item(label: string, uri: string, icon: IconName, opts: { meter?: number | null; title?: string; menu?: (e: MouseEvent) => void; dim?: boolean; indent?: boolean } = {})}
  <button
    class="item"
    class:current={current === norm(uri)}
    class:dim={opts.dim}
    class:indent={opts.indent}
    onclick={(e) => go(uri, e)}
    onauxclick={(e) => go(uri, e)}
    oncontextmenu={opts.menu}
    title={opts.title ?? label}
    use:dropTarget={{ dest: () => (uri.startsWith("cx:") ? null : uri) }}
  >
    <span class="icon"><Icon name={icon} size={16} /></span>
    <span class="text">
      <span class="label">{label}</span>
      {#if opts.meter != null}
        <span class="meter"><span style:width="{Math.round(opts.meter * 100)}%" class:full={opts.meter > 0.9}></span></span>
      {/if}
    </span>
  </button>
{/snippet}

{#snippet section(id: string, title: string, action?: { icon: IconName; label: string; run: () => void })}
  <div class="section">
    <button class="stitle" onclick={() => (collapsed[id] = !collapsed[id])} aria-expanded={!collapsed[id]}>
      <span class="chev" class:closed={collapsed[id]}><Icon name="chevronDown" size={11} stroke={1.8} /></span>
      {title}
    </button>
    {#if action}<button class="saction" title={action.label} aria-label={action.label} onclick={action.run}><Icon name={action.icon} size={13} /></button>{/if}
  </div>
{/snippet}

<nav class="sidebar" style:width="{settings.data.sidebarWidth}px">
  {#if ws.places}
    <div class="group">
      {@render item("Home", HOME_URI, "home")}
      {@render item(ws.places.home.name, ws.places.home.uri, "user")}
    </div>

    {@render section("fav", "Favorites")}
    {#if !collapsed.fav}
      <div class="group">
        {#each ws.places.favorites as p (p.uri)}
          {@render item(p.name, p.uri, p.icon as IconName)}
        {/each}
        {#each settings.data.bookmarks as b (b.uri)}
          {@render item(b.name, b.uri, "star", { title: b.uri, menu: (e) => bookmarkMenu(e, b.uri) })}
        {/each}
      </div>
    {/if}

    {@render section("net", "Network", { icon: "plus", label: "Connect to server…", run: () => dialogs.ask("connect") })}
    {#if !collapsed.net}
      <div class="group">
        {#each settings.data.servers as s (s.uri)}
          {@render item(s.name, s.uri, "server", { title: s.uri, menu: (e) => serverMenu(e, s.uri, true) })}
        {/each}
        {#each loose as c (c)}
          {@render item(c.replace(/^\w+:\/\//, ""), c, "server", { title: c, menu: (e) => serverMenu(e, c, false) })}
        {/each}
        {#each devices.nearby as d (d.id)}
          {@const target = deviceTarget(d)}
          {@const offline = !!d.tailnet && !d.tailnet.online}
          <div class="device" class:offline>
            <button
              class="item"
              class:current={!!target && current === norm(target)}
              disabled={offline}
              title="{d.name}{d.tailnet ? ' · tailnet' : ''}{offline ? ' · offline' : ''}"
              onclick={(e) => (target ? go(target, e) : dialogs.ask("connect", { host: d.hostname ?? d.addresses[0] }))}
            >
              <span class="icon"><DeviceIcon kind={d.kind} size={16} /></span>
              <span class="text"><span class="label">{d.name}</span></span>
              {#if d.tailnet?.online}<span class="tdot" title="Online on your tailnet"></span>{/if}
            </button>
            {#if d.shares.length}
              <button class="exp" aria-label="Show shares" onclick={() => (expanded[d.id] = !expanded[d.id])}>
                <span class="chev" class:closed={!expanded[d.id]}><Icon name="chevronDown" size={11} stroke={1.8} /></span>
              </button>
            {/if}
          </div>
          {#if expanded[d.id]}
            {#each d.shares as s (s.uri)}
              {@render item(s.name, s.uri, "folder", { indent: true })}
            {/each}
          {/if}
        {/each}
        {#if !devices.nearby.length && !settings.data.servers.length && !loose.length}
          <button class="hint" onclick={() => devices.scan()}>{devices.scanning ? "Looking for devices…" : "No devices found yet — scan"}</button>
        {/if}
      </div>
    {/if}

    {@render section("tags", "Tags")}
    {#if !collapsed.tags}
      <div class="group">
        {#each Object.entries(TAG_COLORS) as [name, color] (name)}
          <button class="item" class:current={current === norm(tagUri(name))} onclick={(e) => go(tagUri(name), e)}>
            <span class="icon"><span class="tagdot" style:background={color}></span></span>
            <span class="text"><span class="label">{name}</span></span>
          </button>
        {/each}
      </div>
    {/if}

    {#if ws.places.cloud?.length}
      {@render section("cloud", "Cloud")}
      {#if !collapsed.cloud}
        <div class="group">
          {#each ws.places.cloud as c (c.uri)}
            {@render item(c.name, c.uri, "cloud", { title: c.account ? `${c.name} — ${c.account}` : c.name })}
          {/each}
        </div>
      {/if}
    {/if}

    {@render section("drives", "Drives")}
    {#if !collapsed.drives}
      <div class="group">
        {#each ws.places.volumes as v (v.uri)}
          {@render item(v.name, v.uri, v.removable ? "external" : "drive", { meter: v.total ? 1 - v.free / v.total : null, title: `${formatSize(v.free)} free of ${formatSize(v.total)}` })}
        {/each}
      </div>
    {/if}
  {/if}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="grip"
    onpointerdown={(e) => {
      dragging = true;
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    }}
    onpointermove={resize}
    onpointerup={() => (dragging = false)}
  ></div>
</nav>

<style>
  .sidebar {
    position: relative;
    flex: none;
    overflow-y: auto;
    overflow-x: hidden;
    padding: 8px 6px 12px;
  }
  .group {
    display: flex;
    flex-direction: column;
    gap: 1px;
    margin-bottom: 6px;
  }
  .section {
    display: flex;
    align-items: center;
  }
  .stitle {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 8px 8px 4px;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-3);
    text-align: left;
  }
  .stitle:hover {
    color: var(--text-2);
  }
  .saction {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    margin: 4px 4px 0 0;
    border-radius: 4px;
    color: var(--text-3);
    opacity: 0;
  }
  .section:hover .saction {
    opacity: 1;
  }
  .saction:hover {
    background: var(--hover);
    color: var(--text);
  }
  .chev {
    display: grid;
    transition: transform 0.15s var(--ease);
  }
  .chev.closed {
    transform: rotate(-90deg);
  }
  .item {
    position: relative;
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    min-height: 30px;
    padding: 4px 10px;
    border-radius: var(--radius);
    text-align: left;
  }
  .item.indent {
    padding-left: 34px;
  }
  .item:hover:not(:disabled) {
    background: var(--hover);
  }
  .item.current {
    background: var(--pressed);
  }
  .item:global(.drop-hover) {
    background: var(--accent-soft);
    box-shadow: inset 0 0 0 1.5px var(--accent);
  }
  /* Windows 11's accent pill on the selected navigation item. */
  .item.current::before {
    content: "";
    position: absolute;
    left: 0;
    top: 8px;
    bottom: 8px;
    width: 3px;
    border-radius: 2px;
    background: var(--accent);
  }
  .icon {
    display: grid;
    place-items: center;
    width: 16px;
    color: var(--accent);
    flex: none;
  }
  .text {
    flex: 1;
    min-width: 0;
  }
  .label {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meter {
    display: block;
    height: 4px;
    margin: 4px 0 2px;
    border-radius: 2px;
    background: var(--stroke-strong);
    overflow: hidden;
  }
  .meter span {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  .meter span.full {
    background: var(--danger);
  }
  .device {
    display: flex;
    align-items: center;
  }
  .device.offline .item {
    opacity: 0.5;
  }
  .exp {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    margin-right: 4px;
    border-radius: 4px;
    color: var(--text-3);
    flex: none;
  }
  .exp:hover {
    background: var(--hover);
  }
  .tdot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--live);
    flex: none;
  }
  .tagdot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
  }
  .hint {
    padding: 4px 10px 4px 36px;
    font-size: 12px;
    color: var(--text-3);
    text-align: left;
  }
  .hint:hover {
    color: var(--accent);
  }
  .grip {
    position: absolute;
    top: 0;
    right: 0;
    bottom: 0;
    width: 6px;
    cursor: col-resize;
  }
</style>
