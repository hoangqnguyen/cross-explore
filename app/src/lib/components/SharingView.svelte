<script lang="ts">
  // The Sharing page: what this device shares (and with whom), what my other
  // devices share with me, the devices themselves, and what they did here.
  import { onMount } from "svelte";
  import { errorText, listDir, peerForget, peerPairCode, peerSetAutoTrust, type DeviceKind, type Entry, type PeerShare } from "../api";
  import { activity, ago, audienceLabel, osLabel, copyText, localTarget, openShareDialog, peerKind, peerOnline, setSharingOn, shareLink, stopSharing, summarize } from "../sharing.svelte";
  import { devices } from "../stores/devices.svelte";
  import { dialogs } from "../stores/dialogs.svelte";
  import { toasts } from "../toasts.svelte";
  import { ws, type Tab } from "../workspace.svelte";
  import DeviceIcon from "./DeviceIcon.svelte";
  import FileIcon from "./FileIcon.svelte";
  import Icon from "./Icon.svelte";
  import Toggle from "./Toggle.svelte";

  let { tab }: { tab: Tab } = $props();

  let peer = $derived(devices.peer);
  let shares = $derived(peer?.shares ?? []);
  let trusted = $derived(peer?.trusted ?? []);
  let connected = $derived(trusted.filter((t) => t.connected));
  let thisKind = $derived<DeviceKind>(ws.platform === "macos" ? "mac" : ws.platform === "windows" ? "pc" : "linux");
  // Cross Explore devices nearby that aren't paired yet.
  let unpaired = $derived(devices.nearby.filter((d) => d.services.some((s) => s.scheme === "peer" && !trusted.some((t) => s.uri.replace(/\/+$/, "") === `peer://${t.id}`))));
  let lines = $derived(summarize(activity.records, 40));

  // A clock for "5 min ago".
  let now = $state(Date.now());
  onMount(() => {
    if (!activity.loaded) void activity.load();
    const t = setInterval(() => (now = Date.now()), 30_000);
    return () => clearInterval(t);
  });

  // ---- shared with me: each paired device's share list ----
  type Inbound = { state: "loading" | "ok" | "error"; shares: Entry[]; error?: string };
  let inbound = $state<Record<string, Inbound>>({});

  async function loadInbound(id: string) {
    inbound[id] = { state: "loading", shares: inbound[id]?.shares ?? [] };
    const got: Entry[] = [];
    try {
      const timeout = new Promise<never>((_, reject) => setTimeout(() => reject(new Error("Not reachable right now")), 12_000));
      await Promise.race([listDir(`peer://${id}/`, (e) => e.type === "batch" && got.push(...e.entries)), timeout]);
      inbound[id] = { state: "ok", shares: got.sort((a, b) => a.name.localeCompare(b.name)) };
    } catch (e) {
      inbound[id] = { state: "error", shares: [], error: errorText(e) };
    }
  }

  // List each paired device once (and newly paired ones as they appear).
  $effect(() => {
    for (const t of trusted) if (!(t.id in inbound)) void loadInbound(t.id);
  });

  function refreshAll() {
    for (const t of trusted) void loadInbound(t.id);
    void activity.load();
    void devices.scan();
  }

  function open(uri: string, e?: MouseEvent) {
    if (e && (e.metaKey || e.ctrlKey || e.button === 1)) ws.newTab(uri, false);
    else tab.navigate(uri);
  }

  function openLocal(sh: PeerShare, e?: MouseEvent) {
    const t = localTarget(`/${sh.name}`);
    if (t) open(t.dir, e);
  }

  function reveal(path: string | null) {
    const t = localTarget(path);
    if (t) tab.navigate(t.dir, t.name || undefined);
  }

  async function forget(id: string, name: string) {
    if (!(await dialogs.confirm(`Forget ${name}?`, `${name} won't be able to open your shared folders until you pair again, and you won't see its folders here.`, "Forget", true))) return;
    try {
      devices.peer = await peerForget(id);
      delete inbound[id];
      toasts.show(`Forgot ${name}`);
    } catch (e) {
      toasts.show(errorText(e), "error");
    }
  }

  let code = $state<string | null>(null);
  async function showCode() {
    try {
      code = await peerPairCode();
      setTimeout(() => (code = null), 120_000);
    } catch (e) {
      toasts.show(errorText(e), "error");
    }
  }

  async function setAutoTrust(on: boolean) {
    try {
      devices.peer = await peerSetAutoTrust(on);
    } catch (e) {
      toasts.show(errorText(e), "error");
    }
  }

  /** What a paired device may open here. */
  function accessOf(id: string) {
    const n = shares.filter((s) => !s.devices || s.devices.includes(id)).length;
    if (!shares.length) return "Nothing shared yet";
    if (n === shares.length) return shares.length === 1 ? "Can open your shared folder" : `Can open all ${n} shared folders`;
    return n ? `Can open ${n} of ${shares.length} shared folders` : "Can't open any shared folder";
  }

  // ---- drop folders here to share them ----
  const MIME = "application/x-cx-uris";
  let dropping = $state(false);
  function dragover(e: DragEvent) {
    if (!e.dataTransfer?.types.includes(MIME)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "link";
    dropping = true;
  }
  async function drop(e: DragEvent) {
    dropping = false;
    let uris: string[] = [];
    try {
      uris = JSON.parse(e.dataTransfer?.getData(MIME) || "[]");
    } catch {
      /* not ours */
    }
    uris = uris.filter((u) => u.startsWith("file://"));
    if (!uris.length) return;
    e.preventDefault();
    for (const u of uris) if (!(await openShareDialog(u))) break;
  }

  let statusLine = $derived.by(() => {
    if (!peer) return "";
    if (!peer.enabled) return "Sharing is off — your other devices can't open this device's folders.";
    const parts = [shares.length ? `${shares.length} shared ${shares.length === 1 ? "folder" : "folders"}` : "No folders shared yet"];
    if (connected.length) parts.push(`${connected.map((t) => t.name).join(", ")} connected now`);
    else parts.push(`${trusted.length} paired ${trusted.length === 1 ? "device" : "devices"}`);
    return parts.join(" · ");
  });
</script>

<div class="sharing file-view" tabindex="-1" role="region" aria-label="Sharing">
  {#if !peer}
    <div class="unavailable">
      <Icon name="share" size={40} stroke={1} />
      <p>Sharing with your devices isn't available in this build.</p>
    </div>
  {:else}
    <header class="hero" class:on={peer.enabled}>
      <span class="hicon"><DeviceIcon kind={thisKind} size={26} /><span class="hdot" class:on={peer.enabled}></span></span>
      <div class="htext">
        <h1>{peer.name}</h1>
        <p>{statusLine}</p>
      </div>
      <div class="hactions">
        <div class="switch">
          <Toggle checked={peer.enabled} label="Sharing" onchange={(v) => setSharingOn(v)} />
          <span>{peer.enabled ? "Sharing on" : "Sharing off"}</span>
        </div>
        <button class="btn primary" onclick={() => openShareDialog()}><Icon name="plus" size={14} /> Share a folder…</button>
        <button class="btn icon" title="Refresh" aria-label="Refresh" onclick={refreshAll}><Icon name="reload" size={15} /></button>
      </div>
    </header>

    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <section class="drop" class:dropping ondragover={dragover} ondragleave={() => (dropping = false)} ondrop={drop}>
      <h2>Shared from this device <span class="count">{shares.length || ""}</span></h2>
      {#if shares.length}
        <div class="table" role="list">
          {#each shares as sh (sh.name)}
            {@const users = connected.filter((t) => !sh.devices || sh.devices.includes(t.id))}
            <div class="srow" role="listitem">
              <button class="sname" title="Open {sh.path}" onclick={(e) => openLocal(sh, e)} onauxclick={(e) => openLocal(sh, e)}>
                <span class="ficon"><FileIcon name="" isDir size={28} /><span class="badge"><Icon name="share" size={9} stroke={2} /></span></span>
                <span class="stext">
                  <span class="title">{sh.name}</span>
                  <span class="sub">{sh.path}</span>
                </span>
              </button>
              <span class="pill" class:edit={!sh.readOnly}><Icon name={sh.readOnly ? "eye" : "rename"} size={12} /> {sh.readOnly ? "View only" : "Can edit"}</span>
              <span class="aud" title={audienceLabel(sh)}><Icon name={sh.devices ? "user" : "laptop"} size={13} /> {audienceLabel(sh)}</span>
              <span class="who">
                {#if users.length}<span class="live"><span class="dot"></span>{users.map((u) => u.name).join(", ")}</span>{/if}
              </span>
              <span class="acts">
                <button class="ibtn" title="Copy link" aria-label="Copy link for {sh.name}" onclick={() => copyText(shareLink(sh))}><Icon name="link" size={15} /></button>
                <button class="ibtn" title="Sharing options" aria-label="Sharing options for {sh.name}" onclick={() => openShareDialog(localTarget(`/${sh.name}`)?.dir)}><Icon name="settings" size={15} /></button>
                <button class="ibtn danger" title="Stop sharing" aria-label="Stop sharing {sh.name}" onclick={() => stopSharing(sh)}><Icon name="close" size={14} /></button>
              </span>
            </div>
          {/each}
        </div>
        <p class="hint">Drop folders here to share them, or right-click a folder and choose <b>Share…</b></p>
      {:else}
        <button class="empty-drop" onclick={() => openShareDialog()}>
          <Icon name="share" size={28} stroke={1.2} />
          <span class="etitle">Share a folder with your devices</span>
          <span class="esub">Drop a folder here, or right-click any folder and choose Share…. Your paired devices can then browse it live, open files and save changes if you allow it.</span>
        </button>
      {/if}
      {#if dropping}<div class="drop-hint"><Icon name="share" size={22} /> Drop to share</div>{/if}
    </section>

    <section>
      <h2>Shared with you</h2>
      {#if trusted.length}
        <div class="devices">
          {#each trusted as t (t.id)}
            {@const st = inbound[t.id]}
            {@const online = peerOnline(t.id)}
            <div class="dcard">
              <div class="dhead">
                <span class="dicon"><DeviceIcon kind={peerKind(t.id, t.os)} size={20} /></span>
                <span class="dtext">
                  <span class="title">{t.name}</span>
                  <span class="sub">{st?.state === "loading" ? "Connecting…" : st?.state === "error" ? "Not reachable" : online === false ? "Offline" : `${st?.shares.length ?? 0} shared ${st?.shares.length === 1 ? "folder" : "folders"}`}</span>
                </span>
                <button class="ibtn" title="Open {t.name}" aria-label="Open {t.name}" onclick={(e) => open(`peer://${t.id}/`, e)}><Icon name="open" size={14} /></button>
                <button class="ibtn" title="Refresh" aria-label="Refresh {t.name}" onclick={() => loadInbound(t.id)}><Icon name="reload" size={14} /></button>
              </div>
              {#if st?.state === "loading" && !st.shares.length}
                <div class="chips"><span class="chip skel"></span><span class="chip skel short"></span></div>
              {:else if st?.state === "error"}
                <p class="derr" title={st.error}>Couldn't reach it. Is it on, with Cross Explore running and sharing turned on?</p>
              {:else if st?.shares.length}
                <div class="chips">
                  {#each st.shares as s (s.name)}
                    <button class="chip" onclick={(e) => open(`peer://${t.id}/${encodeURIComponent(s.name)}/`, e)} onauxclick={(e) => open(`peer://${t.id}/${encodeURIComponent(s.name)}/`, e)} title={s.readonly ? `${s.name} — view only` : s.name}>
                      <FileIcon name="" isDir size={16} />
                      {s.name}
                      {#if s.readonly}<Icon name="lock" size={11} />{/if}
                    </button>
                  {/each}
                </div>
              {:else if st?.state === "ok"}
                <p class="dnone">Nothing shared with you yet.</p>
              {/if}
            </div>
          {/each}
        </div>
      {:else}
        <div class="empty-row">
          <Icon name="laptop" size={22} stroke={1.2} />
          <span>Pair your other devices to see the folders they share with you.</span>
          <button class="btn" onclick={() => dialogs.ask("pair")}>Pair a device…</button>
        </div>
      {/if}
      {#if unpaired.length}
        <div class="nearby">
          <span class="muted">Nearby, not paired:</span>
          {#each unpaired as d (d.id)}
            <button class="chip" onclick={() => dialogs.ask("pair")} title="Pair with {d.name}"><DeviceIcon kind={d.kind} size={14} /> {d.name}</button>
          {/each}
        </div>
      {/if}
    </section>

    <section>
      <h2>
        Devices
        <span class="h-actions">
          <button onclick={showCode}><Icon name="command" size={14} /> Show pairing code</button>
          <button onclick={() => dialogs.ask("pair")}><Icon name="plus" size={14} /> Pair a device…</button>
        </span>
      </h2>
      {#if code}
        <div class="code-box">
          <span class="code">{code}</span>
          <span class="muted">Enter this code on your other device (Pair a device… → Enter a code). It works for 2 minutes.</span>
          <button class="ibtn" aria-label="Hide code" onclick={() => (code = null)}><Icon name="close" size={13} /></button>
        </div>
      {/if}
      <div class="table">
        {#each trusted as t (t.id)}
          {@const online = peerOnline(t.id)}
          <div class="drow">
            <span class="dicon small"><DeviceIcon kind={peerKind(t.id, t.os)} size={16} /></span>
            <span class="stext">
              <span class="title">{t.name}</span>
              <span class="sub">{t.os ? `${osLabel(t.os)} · ` : ""}paired {new Date(t.addedAt).toLocaleDateString()}</span>
            </span>
            <span class="state" class:live={t.connected} class:off={online === false}>{t.connected ? "Connected now" : online === true ? "Online" : online === false ? "Offline" : "Not seen nearby"}</span>
            <span class="sub access">{accessOf(t.id)}</span>
            <button class="ibtn danger" title="Forget {t.name}" aria-label="Forget {t.name}" onclick={() => forget(t.id, t.name)}><Icon name="trash" size={14} /></button>
          </div>
        {:else}
          <p class="hint">No paired devices yet. Pair one with a 6-digit code — the devices then trust each other by key.</p>
        {/each}
      </div>
      <div class="opt">
        <Toggle checked={peer.tailnetAutoTrust} label="Trust my Tailscale devices" onchange={setAutoTrust} />
        <span>
          Trust my own devices on Tailscale automatically
          <span class="muted">They can open folders shared with all your devices without pairing.</span>
        </span>
      </div>
    </section>

    <section>
      <h2>Recent activity</h2>
      {#if lines.length}
        <div class="activity">
          {#each lines as a (a.key)}
            {@const target = localTarget(a.path)}
            <div class="arow" class:failed={!a.ok}>
              <span class="dicon small"><DeviceIcon kind={peerKind(a.deviceId, trusted.find((t) => t.id === a.deviceId)?.os ?? "")} size={14} /></span>
              <span class="atext" title={a.error ?? ""}>
                <b>{a.device}</b>
                {a.verb.charAt(0).toLowerCase() + a.verb.slice(1)}
                {#if a.path}<span class="path">{a.path}</span>{/if}
                {#if a.count > 1}<span class="times">×{a.count}</span>{/if}
                {#if !a.ok && a.error}<span class="aerr">— {a.error}</span>{/if}
              </span>
              <span class="when" title={new Date(a.time).toLocaleString()}>{ago(a.time, now)}</span>
              {#if target}<button class="ibtn" title="Show in folder" aria-label="Show in folder" onclick={() => reveal(a.path)}><Icon name="folder" size={14} /></button>{:else}<span class="ibtn-space"></span>{/if}
            </div>
          {/each}
        </div>
      {:else}
        <p class="hint">Nothing yet. When your devices open or change files in your shared folders, it shows up here.</p>
      {/if}
    </section>
  {/if}
</div>

<style>
  .sharing {
    flex: 1;
    overflow-y: auto;
    padding: 16px 24px 40px;
    outline: none;
  }
  .unavailable {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    margin-top: 80px;
    color: var(--text-3);
  }
  .hero {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 16px 18px;
    border-radius: 14px;
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke);
  }
  .hero.on {
    background: linear-gradient(120deg, var(--accent-soft), var(--layer-2) 70%);
  }
  .hicon {
    position: relative;
    display: grid;
    place-items: center;
    width: 52px;
    height: 52px;
    border-radius: 14px;
    background: var(--accent-soft);
    color: var(--accent);
    flex: none;
  }
  .hdot {
    position: absolute;
    right: -2px;
    bottom: -2px;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--text-3);
    box-shadow: 0 0 0 3px var(--layer-2);
  }
  .hdot.on {
    background: var(--live);
  }
  .htext {
    flex: 1;
    min-width: 0;
  }
  h1 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
  }
  .htext p {
    margin: 3px 0 0;
    color: var(--text-2);
    font-size: 12.5px;
  }
  .hactions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    justify-content: flex-end;
  }
  .switch {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-right: 6px;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 32px;
    padding: 0 14px;
    border-radius: var(--radius);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke-strong);
    white-space: nowrap;
  }
  .btn:hover {
    background: var(--hover);
  }
  .btn.primary {
    background: var(--accent);
    color: var(--accent-text);
    box-shadow: none;
  }
  .btn.primary:hover {
    filter: brightness(1.08);
  }
  .btn.icon {
    width: 32px;
    padding: 0;
    justify-content: center;
  }
  section {
    position: relative;
    margin-top: 26px;
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 0 10px;
    font-size: 13px;
    font-weight: 600;
  }
  .count {
    color: var(--text-3);
    font-weight: 500;
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
  .table {
    display: flex;
    flex-direction: column;
    border-radius: var(--radius-lg);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke);
    overflow: hidden;
  }
  .table:empty {
    display: none;
  }
  .srow,
  .drow {
    display: grid;
    align-items: center;
    gap: 12px;
    min-height: 52px;
    padding: 6px 10px;
  }
  .srow {
    grid-template-columns: minmax(180px, 1.6fr) auto minmax(110px, 1fr) minmax(0, 1fr) auto;
  }
  .drow {
    grid-template-columns: 32px minmax(140px, 1.4fr) auto minmax(120px, 1fr) auto;
  }
  .srow + .srow,
  .drow + .drow {
    border-top: 1px solid var(--stroke);
  }
  .srow:hover,
  .drow:hover {
    background: var(--hover);
  }
  .sname {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    text-align: left;
    border-radius: var(--radius);
  }
  .sname:hover .title {
    color: var(--accent);
    text-decoration: underline;
  }
  .ficon {
    position: relative;
    flex: none;
  }
  .badge {
    position: absolute;
    right: -3px;
    bottom: -2px;
    display: grid;
    place-items: center;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--accent);
    color: var(--accent-text);
    box-shadow: 0 0 0 2px var(--layer-2);
  }
  .stext {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .title {
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sub {
    font-size: 11.5px;
    color: var(--text-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 22px;
    padding: 0 9px;
    border-radius: 11px;
    font-size: 11.5px;
    background: var(--hover);
    color: var(--text-2);
    white-space: nowrap;
  }
  .pill.edit {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .aud {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    font-size: 12.5px;
    color: var(--text-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .who {
    min-width: 0;
    overflow: hidden;
  }
  .live {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 100%;
    font-size: 11.5px;
    color: var(--live);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--live);
    flex: none;
    animation: pulse 2s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.4;
    }
  }
  .acts {
    display: flex;
    gap: 2px;
    opacity: 0.55;
    transition: opacity 0.12s;
  }
  .srow:hover .acts,
  .acts:focus-within {
    opacity: 1;
  }
  .ibtn {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border-radius: var(--radius);
    color: var(--text-2);
    flex: none;
  }
  .ibtn:hover {
    background: var(--pressed);
    color: var(--text);
  }
  .ibtn.danger:hover {
    color: var(--danger);
  }
  .ibtn-space {
    width: 28px;
  }
  .hint {
    margin: 8px 2px 0;
    font-size: 12px;
    color: var(--text-3);
  }
  .muted {
    color: var(--text-3);
    font-size: 12px;
  }
  .drop.dropping .table,
  .drop.dropping .empty-drop {
    box-shadow: 0 0 0 2px var(--accent);
  }
  .drop-hint {
    position: absolute;
    inset: 28px 0 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    border-radius: var(--radius-lg);
    background: color-mix(in srgb, var(--accent-soft) 85%, transparent);
    color: var(--accent);
    font-weight: 600;
    pointer-events: none;
  }
  .empty-drop {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 28px 24px;
    border: 1.5px dashed var(--stroke-strong);
    border-radius: 14px;
    color: var(--accent);
    text-align: center;
  }
  .empty-drop:hover {
    background: var(--accent-soft);
    border-color: var(--accent);
  }
  .etitle {
    color: var(--text);
    font-weight: 600;
  }
  .esub {
    max-width: 460px;
    color: var(--text-3);
    font-size: 12px;
    line-height: 1.5;
  }
  .devices {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: 10px;
  }
  .dcard {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px;
    border-radius: var(--radius-lg);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke);
  }
  .dhead {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .dtext {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
  }
  .dicon {
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    border-radius: 10px;
    background: var(--accent-soft);
    color: var(--accent);
    flex: none;
  }
  .dicon.small {
    width: 28px;
    height: 28px;
    border-radius: 8px;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 10px 0 8px;
    border-radius: 14px;
    background: var(--hover);
    font-size: 12.5px;
    color: var(--text);
  }
  .chip:hover {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .chip.skel {
    width: 92px;
    animation: pulse 1.2s ease-in-out infinite;
  }
  .chip.skel.short {
    width: 64px;
  }
  .derr,
  .dnone {
    margin: 0;
    font-size: 12px;
    line-height: 1.45;
    color: var(--text-3);
  }
  .empty-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 14px 16px;
    border-radius: var(--radius-lg);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke);
    color: var(--text-2);
  }
  .empty-row span {
    flex: 1;
  }
  .nearby {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 10px;
  }
  .state {
    font-size: 11.5px;
    padding: 2px 9px;
    border-radius: 10px;
    background: var(--hover);
    color: var(--text-2);
    white-space: nowrap;
  }
  .state.live {
    background: color-mix(in srgb, var(--live) 18%, transparent);
    color: var(--live);
  }
  .state.off {
    color: var(--text-3);
  }
  .access {
    font-size: 12px;
  }
  .opt {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    margin-top: 12px;
    font-size: 13px;
  }
  .opt > span {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .code-box {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 12px 16px;
    margin-bottom: 10px;
    border-radius: var(--radius-lg);
    background: var(--accent-soft);
  }
  .code {
    font-size: 26px;
    font-weight: 600;
    letter-spacing: 0.12em;
    font-variant-numeric: tabular-nums;
    color: var(--accent);
  }
  .code-box .muted {
    flex: 1;
  }
  .activity {
    display: flex;
    flex-direction: column;
  }
  .arow {
    display: grid;
    grid-template-columns: 28px 1fr auto 28px;
    align-items: center;
    gap: 10px;
    min-height: 36px;
    padding: 2px 6px;
    border-radius: var(--radius);
  }
  .arow:hover {
    background: var(--hover);
  }
  .atext {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-2);
  }
  .atext b {
    color: var(--text);
    font-weight: 600;
  }
  .path {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 12px;
    color: var(--text);
  }
  .times {
    margin-left: 4px;
    font-size: 11px;
    color: var(--text-3);
  }
  .arow.failed .atext {
    color: var(--danger);
  }
  .aerr {
    font-size: 12px;
  }
  .when {
    font-size: 11.5px;
    color: var(--text-3);
    white-space: nowrap;
  }
</style>
