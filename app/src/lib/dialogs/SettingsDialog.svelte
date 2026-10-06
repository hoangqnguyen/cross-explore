<script lang="ts">
  import { untrack, type Snippet } from "svelte";
  import { appVersion, errorText, fileUriToPath, peerForget, peerPairCode, peerSetAutoTrust, peerSetEnabled, peerSetShares, type PeerShare } from "../api";
  import Icon, { type IconName } from "../components/Icon.svelte";
  import Toggle from "../components/Toggle.svelte";
  import { devices } from "../stores/devices.svelte";
  import { dialogs } from "../stores/dialogs.svelte";
  import { settings } from "../stores/settings.svelte";
  import { toasts } from "../toasts.svelte";
  import { ws } from "../workspace.svelte";
  import Modal from "./Modal.svelte";
  import PreviewSettings from "./PreviewSettings.svelte";
  import ShortcutsSettings from "./ShortcutsSettings.svelte";

  const SECTIONS: { id: "general" | "shortcuts" | "previews" | "sharing" | "network" | "about"; label: string; icon: IconName }[] = [
    { id: "general", label: "General", icon: "settings" },
    { id: "shortcuts", label: "Shortcuts", icon: "command" },
    { id: "previews", label: "Previews", icon: "eye" },
    { id: "sharing", label: "Sharing & devices", icon: "laptop" },
    { id: "network", label: "Servers", icon: "server" },
    { id: "about", label: "About", icon: "info" },
  ];

  type Section = (typeof SECTIONS)[number]["id"];
  let { section: initial = "general" }: { section?: Section } = $props();
  let section = $state<Section>(untrack(() => initial));
  let s = settings.data;
  let code = $state<string | null>(null);
  let version = $state<string | null>(null);
  void appVersion().then((v) => (version = v));

  async function peer<T>(p: Promise<T>) {
    try {
      const r = await p;
      if (r && typeof r === "object" && "deviceId" in r) devices.peer = r as never;
      return r;
    } catch (e) {
      toasts.show(errorText(e), "error");
      return null;
    }
  }

  async function addShare() {
    const path = ws.activeTab.folder.info?.local ? ws.activeTab.dirUri : ws.places?.home.uri;
    if (!path?.startsWith("file://")) return;
    const name = await dialogs.prompt("Share a folder", "Name other devices will see", decodeURIComponent(path.split("/").pop() ?? "Share"), "Share");
    if (!name || !devices.peer) return;
    const shares: PeerShare[] = [...devices.peer.shares, { name, path: fileUriToPath(path), readOnly: false }];
    await peer(peerSetShares(shares));
  }

  async function updateShare(i: number, patch: Partial<PeerShare> | null) {
    if (!devices.peer) return;
    const shares = devices.peer.shares.map((x, j) => (j === i && patch ? { ...x, ...patch } : x)).filter((_, j) => j !== i || patch);
    await peer(peerSetShares(shares));
  }
</script>

{#snippet group(title: string, children: Snippet)}
  <div class="group">
    <div class="group-title">{title}</div>
    <div class="card">{@render children()}</div>
  </div>
{/snippet}

{#snippet row(label: string, hint: string | undefined, children: Snippet)}
  <div class="srow">
    <div class="row-text">
      <span class="row-label">{label}</span>
      {#if hint}<span class="row-hint">{hint}</span>{/if}
    </div>
    {@render children()}
  </div>
{/snippet}

{#snippet entry(icon: IconName, title: string, subtitle: string, onremove?: () => void)}
  <div class="srow entry">
    <span class="entry-icon"><Icon name={icon} size={15} /></span>
    <div class="row-text">
      <span class="row-label">{title}</span>
      <span class="row-hint">{subtitle}</span>
    </div>
    {#if onremove}
      <button type="button" class="remove" aria-label="Remove" onclick={onremove}><Icon name="close" size={12} /></button>
    {/if}
  </div>
{/snippet}

<Modal title="Settings" width={700}>
  <div class="layout">
    <nav>
      {#each SECTIONS as sec (sec.id)}
        <button type="button" class:active={section === sec.id} onclick={() => (section = sec.id)}>
          <Icon name={sec.icon} size={16} />
          {sec.label}
        </button>
      {/each}
    </nav>
    <div class="panel">
      {#if section === "general"}
        {@render group("Keyboard", keyboardBody)}
        {#snippet keyboardBody()}
          <div class="choices">
            <label class="choice" class:on={s.keymap === "finder"}>
              <input type="radio" bind:group={s.keymap} value="finder" />
              {#if s.keymap === "finder"}<span class="badge"><Icon name="check" size={10} stroke={2.4} /></span>{/if}
              <strong>Finder</strong>
              <span>↩ renames, ⌘O / ⌘↓ opens, ⌘⌫ to Trash, ⌘[ ⌘] back/forward, ⌘1–4 views, ⌘I Get Info.</span>
            </label>
            <label class="choice" class:on={s.keymap === "explorer"}>
              <input type="radio" bind:group={s.keymap} value="explorer" />
              {#if s.keymap === "explorer"}<span class="badge"><Icon name="check" size={10} stroke={2.4} /></span>{/if}
              <strong>Explorer</strong>
              <span>Enter opens, F2 renames, Delete / Shift+Delete, Alt+←/→/↑, Backspace back, F5 refresh, Alt+Enter Properties.</span>
            </label>
            <label class="choice" class:on={s.keymap === "commander"}>
              <input type="radio" bind:group={s.keymap} value="commander" />
              {#if s.keymap === "commander"}<span class="badge"><Icon name="check" size={10} stroke={2.4} /></span>{/if}
              <strong>Commander</strong>
              <span>Explorer keys plus F3 view, F5 copy, F6 move, F7 new folder, F8 delete, Tab switch pane, Insert select.</span>
            </label>
          </div>
          {@render row("Your own shortcuts", "Change the key for any command", shortcutsCtl)}
          {#snippet shortcutsCtl()}
            <button type="button" class="open-shortcuts" onclick={() => (section = "shortcuts")}>Customize…</button>
          {/snippet}
          {@render row("Typing in a file list", undefined, typingCtl)}
          {#snippet typingCtl()}
            <select value={s.typeAction ?? "auto"} onchange={(e) => { const v = (e.currentTarget as HTMLSelectElement).value; s.typeAction = v === "auto" ? undefined : (v as "select" | "filter"); }}>
              <option value="auto">Follow the keys above ({s.keymap === "commander" ? "filters" : "jumps to the item"})</option>
              <option value="select">Jumps to the matching item</option>
              <option value="filter">Filters the list</option>
            </select>
          {/snippet}
        {/snippet}

        {@render group("Appearance", appearanceBody)}
        {#snippet appearanceBody()}
          {@render row("Theme", undefined, themeCtl)}
          {#snippet themeCtl()}
            <select bind:value={s.theme}>
              <option value="system">Match the system</option>
              <option value="light">Light</option>
              <option value="dark">Dark</option>
            </select>
          {/snippet}
          {@render row("Default view", undefined, viewCtl)}
          {#snippet viewCtl()}
            <select bind:value={s.defaultView}>
              <option value="details">Details</option>
              <option value="icons">Icons</option>
              <option value="columns">Columns</option>
              <option value="gallery">Gallery</option>
            </select>
          {/snippet}
          {@render row("Compact spacing", "Tighter rows in every list", compactCtl)}
          {#snippet compactCtl()}
            <Toggle bind:checked={s.compact} />
          {/snippet}
          {@render row("Show hidden items", undefined, hiddenCtl)}
          {#snippet hiddenCtl()}
            <Toggle bind:checked={s.showHidden} />
          {/snippet}
        {/snippet}

        {@render group("Safety", safetyBody)}
        {#snippet safetyBody()}
          {@render row("Ask before moving to the " + (ws.platform === "windows" ? "Recycle Bin" : "Trash"), undefined, trashCtl)}
          {#snippet trashCtl()}
            <Toggle bind:checked={s.confirmTrash} />
          {/snippet}
          {@render row("Ask before deleting permanently", undefined, permCtl)}
          {#snippet permCtl()}
            <Toggle bind:checked={s.confirmPermanentDelete} />
          {/snippet}
        {/snippet}
      {:else if section === "shortcuts"}
        <ShortcutsSettings />
      {:else if section === "previews"}
        <PreviewSettings />
      {:else if section === "sharing"}
        {#if devices.peer}
          {@render group("This device", deviceBody)}
          {#snippet deviceBody()}
            {@render row("Share with my other devices", "Your devices running Cross Explore can browse the folders below, live, and send you files.", shareCtl)}
            {#snippet shareCtl()}
              <Toggle checked={devices.peer!.enabled} onchange={(v) => peer(peerSetEnabled(v))} />
            {/snippet}
            <div class="srow">
              <div class="row-text">
                <span class="row-label">{devices.peer!.name}</span>
                <span class="row-hint">ID {devices.peer!.deviceId} · port {devices.peer!.port}</span>
              </div>
            </div>
          {/snippet}

          {@render group("Shared folders", sharedBody)}
          {#snippet sharedBody()}
            {#each devices.peer!.shares as sh, i (sh.name)}
              <div class="srow entry">
                <span class="entry-icon"><Icon name="folder" size={15} /></span>
                <div class="row-text">
                  <span class="row-label">{sh.name}</span>
                  <span class="row-hint">{sh.path}</span>
                </div>
                <label class="inline-check"><Toggle checked={sh.readOnly} onchange={(v) => updateShare(i, { readOnly: v })} /> Read only</label>
                <button type="button" class="remove" aria-label="Stop sharing" onclick={() => updateShare(i, null)}><Icon name="close" size={12} /></button>
              </div>
            {:else}
              <p class="empty">No folders shared yet.</p>
            {/each}
            <button type="button" class="add" onclick={addShare}><Icon name="plus" size={14} /> Share current folder…</button>
          {/snippet}

          {@render group("Trusted devices", trustedBody)}
          {#snippet trustedBody()}
            {@render row("Trust my own devices on my Tailscale tailnet automatically", undefined, autoTrustCtl)}
            {#snippet autoTrustCtl()}
              <Toggle checked={devices.peer!.tailnetAutoTrust} onchange={(v) => peer(peerSetAutoTrust(v))} />
            {/snippet}
            {#each devices.peer!.trusted as t (t.id)}
              {@render entry("laptop", t.name, `paired ${new Date(t.addedAt).toLocaleDateString()}`, () => peer(peerForget(t.id)))}
            {:else}
              <p class="empty">No paired devices yet.</p>
            {/each}
            <div class="pair-row">
              <button type="button" class="add" onclick={async () => (code = await peer(peerPairCode()))}><Icon name="command" size={14} /> Show pairing code</button>
              <button type="button" class="add" onclick={() => dialogs.ask("pair")}><Icon name="plus" size={14} /> Pair with a device…</button>
            </div>
            {#if code}
              <div class="code-box">
                <div class="code">{code}</div>
                <p class="row-hint">Enter this code on the other device within 2 minutes.</p>
              </div>
            {/if}
          {/snippet}
        {:else}
          <p class="empty">Peer mode isn't available in this build.</p>
        {/if}
      {:else if section === "network"}
        {@render group("Saved servers", serversBody)}
        {#snippet serversBody()}
          {#each s.servers as srv, i (srv.uri)}
            {@render entry(srv.uri.startsWith("gdrive://") || srv.uri.startsWith("dropbox://") || srv.uri.startsWith("onedrive://") ? "cloud" : "server", srv.name, srv.uri, () => (s.servers = s.servers.filter((_, j) => j !== i)))}
          {:else}
            <p class="empty">No saved servers.</p>
          {/each}
        {/snippet}
        <div class="actions-row">
          <button type="button" class="add" onclick={() => dialogs.ask("connect")}><Icon name="server" size={14} /> Connect to server…</button>
          <button type="button" class="add" onclick={() => dialogs.ask("cloud")}><Icon name="cloud" size={14} /> Add cloud account…</button>
        </div>
      {:else}
        <div class="about">
          <img src="/app-icon.svg" alt="" width="64" height="64" />
          <h3>Cross Explore</h3>
          <p class="row-hint">Version {version ?? "…"}</p>
          <p class="tagline">A fast, live file explorer for your devices, network and tailnet.</p>
          <button type="button" class="reset" onclick={() => { settings.reset(); toasts.show("Settings reset"); }}>Reset all settings</button>
        </div>
      {/if}
    </div>
  </div>
  {#snippet footer()}
    <button type="button" class="btn primary" onclick={() => dialogs.close(null)}>Done</button>
  {/snippet}
</Modal>

<style>
  .layout {
    display: flex;
    gap: 22px;
    min-height: 420px;
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 172px;
    flex: none;
  }
  nav button {
    display: flex;
    align-items: center;
    gap: 9px;
    height: 32px;
    padding: 0 10px;
    text-align: left;
    border-radius: var(--radius);
    color: var(--text-2);
    font-size: 13px;
  }
  nav button :global(svg) {
    flex: none;
    color: var(--text-3);
  }
  nav button:hover {
    background: var(--hover);
  }
  nav button.active {
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 600;
  }
  nav button.active :global(svg) {
    color: var(--accent);
  }
  .panel {
    flex: 1;
    min-width: 0;
    padding-bottom: 4px;
  }

  /* ---- grouped cards (macOS/iOS "Settings" style) ---- */
  .group {
    margin-bottom: 20px;
  }
  .group:last-child {
    margin-bottom: 4px;
  }
  .group-title {
    margin: 0 2px 6px;
    font-size: 11.5px;
    font-weight: 600;
    letter-spacing: 0.02em;
    text-transform: uppercase;
    color: var(--text-3);
  }
  .card {
    border-radius: var(--radius-lg);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke);
    overflow: hidden;
  }
  .srow {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 42px;
    padding: 8px 14px;
  }
  .srow + .srow {
    border-top: 1px solid var(--stroke);
  }
  .row-text {
    display: flex;
    flex-direction: column;
    gap: 1px;
    flex: 1;
    min-width: 0;
  }
  .row-label {
    font-size: 13px;
    color: var(--text);
  }
  /* A setting's description reads better wrapped; an entry's path or date
     (below) stays on one line and is clipped instead. */
  .row-hint {
    font-size: 11.5px;
    color: var(--text-3);
    line-height: 1.4;
  }
  .entry .row-hint {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .srow select {
    flex: none;
    height: 28px;
    padding: 0 8px;
    border: 0;
    border-radius: var(--radius);
    background: var(--layer);
    box-shadow: inset 0 0 0 1px var(--stroke-strong);
    color: var(--text);
    font: inherit;
    font-size: 12.5px;
  }
  .open-shortcuts {
    flex: none;
    height: 28px;
    padding: 0 12px;
    border-radius: var(--radius);
    background: var(--layer);
    box-shadow: inset 0 0 0 1px var(--stroke-strong);
    font-size: 12.5px;
  }
  .open-shortcuts:hover {
    background: var(--hover);
  }
  .empty {
    margin: 0;
    padding: 12px 14px;
    font-size: 12.5px;
    color: var(--text-3);
  }

  /* ---- keyboard preset cards ---- */
  .choices {
    display: grid;
    grid-template-columns: 1fr 1fr 1fr;
    gap: 1px;
    background: var(--stroke);
  }
  .choice {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 12px 13px;
    background: var(--layer-2);
    font-size: 11.5px;
    line-height: 1.4;
    color: var(--text-3);
  }
  .choice strong {
    color: var(--text);
    font-size: 13px;
  }
  .choice.on {
    background: var(--accent-soft);
  }
  .choice.on strong {
    color: var(--accent);
  }
  .choice input {
    position: absolute;
    width: 1px;
    height: 1px;
    opacity: 0;
  }
  .badge {
    position: absolute;
    top: 10px;
    right: 10px;
    display: grid;
    place-items: center;
    width: 15px;
    height: 15px;
    border-radius: 50%;
    background: var(--accent);
    color: var(--accent-text);
  }

  /* ---- entries (shares, devices, servers) ---- */
  .entry-icon {
    display: grid;
    place-items: center;
    flex: none;
    width: 26px;
    height: 26px;
    border-radius: var(--radius);
    background: var(--hover);
    color: var(--text-2);
  }
  .remove {
    display: grid;
    place-items: center;
    flex: none;
    width: 24px;
    height: 24px;
    border-radius: 50%;
    color: var(--text-3);
  }
  .remove:hover {
    background: var(--hover);
    color: var(--text);
  }
  .inline-check {
    display: flex;
    flex: none;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-2);
  }
  .add {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    height: 38px;
    padding: 0 14px;
    color: var(--accent);
    font-size: 12.5px;
    font-weight: 500;
  }
  .add:hover {
    background: var(--hover);
  }
  .add + .add {
    border-top: 1px solid var(--stroke);
  }
  .actions-row {
    display: flex;
    flex-direction: column;
    margin-top: 8px;
    border-radius: var(--radius-lg);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke);
    overflow: hidden;
  }
  .pair-row {
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--stroke);
  }
  .code-box {
    padding: 14px;
    text-align: center;
    border-top: 1px solid var(--stroke);
  }
  .code {
    font-size: 28px;
    font-weight: 600;
    letter-spacing: 0.15em;
    color: var(--text);
    font-variant-numeric: tabular-nums;
  }

  /* ---- about ---- */
  .about {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    padding-top: 36px;
  }
  .about img {
    border-radius: 14px;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.18);
    margin-bottom: 12px;
  }
  .about h3 {
    margin: 0 0 2px;
    font-size: 16px;
    font-weight: 600;
  }
  .tagline {
    max-width: 280px;
    margin: 10px 0 22px;
    font-size: 12.5px;
    color: var(--text-2);
    line-height: 1.5;
  }
  .reset {
    height: 32px;
    padding: 0 16px;
    border-radius: var(--radius);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke-strong);
    color: var(--danger);
    font-size: 12.5px;
  }
  .reset:hover {
    background: var(--hover);
  }
</style>
