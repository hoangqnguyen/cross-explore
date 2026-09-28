<script lang="ts">
  import { errorText, fileUriToPath, peerForget, peerPairCode, peerSetAutoTrust, peerSetEnabled, peerSetShares, type PeerShare } from "../api";
  import Icon from "../components/Icon.svelte";
  import { devices } from "../stores/devices.svelte";
  import { dialogs } from "../stores/dialogs.svelte";
  import { settings } from "../stores/settings.svelte";
  import { toasts } from "../toasts.svelte";
  import { ws } from "../workspace.svelte";
  import Modal from "./Modal.svelte";

  let section = $state<"general" | "sharing" | "network" | "about">("general");
  let s = settings.data;
  let code = $state<string | null>(null);

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

<Modal title="Settings" width={640}>
  <div class="layout">
    <nav>
      {#each [["general", "General"], ["sharing", "Sharing & devices"], ["network", "Servers"], ["about", "About"]] as [id, label]}
        <button type="button" class:active={section === id} onclick={() => (section = id as typeof section)}>{label}</button>
      {/each}
    </nav>
    <div class="panel">
      {#if section === "general"}
        <h3>Keyboard</h3>
        <div class="choices">
          <label class="choice" class:on={s.keymap === "finder"}>
            <input type="radio" bind:group={s.keymap} value="finder" />
            <strong>Finder</strong>
            <span>↩ renames, ⌘O / ⌘↓ opens, ⌘⌫ to Trash, ⌘[ ⌘] back/forward, ⌘1–4 views, ⌘I Get Info.</span>
          </label>
          <label class="choice" class:on={s.keymap === "explorer"}>
            <input type="radio" bind:group={s.keymap} value="explorer" />
            <strong>Explorer</strong>
            <span>Enter opens, F2 renames, Delete / Shift+Delete, Alt+←/→/↑, Backspace back, F5 refresh, Alt+Enter Properties.</span>
          </label>
          <label class="choice" class:on={s.keymap === "commander"}>
            <input type="radio" bind:group={s.keymap} value="commander" />
            <strong>Commander</strong>
            <span>Explorer keys plus F3 view, F5 copy, F6 move, F7 new folder, F8 delete, Tab switch pane, Insert select.</span>
          </label>
        </div>
        <h3>Appearance</h3>
        <label class="field">Theme
          <select bind:value={s.theme}>
            <option value="system">Match the system</option>
            <option value="light">Light</option>
            <option value="dark">Dark</option>
          </select>
        </label>
        <label class="field">Default view
          <select bind:value={s.defaultView}>
            <option value="details">Details</option>
            <option value="icons">Icons</option>
            <option value="columns">Columns</option>
            <option value="gallery">Gallery</option>
          </select>
        </label>
        <label class="check"><input type="checkbox" bind:checked={s.compact} /> Compact spacing</label>
        <label class="check"><input type="checkbox" bind:checked={s.showHidden} /> Show hidden items</label>
        <h3>Safety</h3>
        <label class="check"><input type="checkbox" bind:checked={s.confirmTrash} /> Ask before moving to the {ws.platform === "windows" ? "Recycle Bin" : "Trash"}</label>
        <label class="check"><input type="checkbox" bind:checked={s.confirmPermanentDelete} /> Ask before deleting permanently</label>
      {:else if section === "sharing"}
        {#if devices.peer}
          <label class="check big">
            <input type="checkbox" checked={devices.peer.enabled} onchange={(e) => peer(peerSetEnabled((e.currentTarget as HTMLInputElement).checked))} />
            <span><strong>Share with my other devices</strong><br /><span class="muted">Your devices running Cross Explore can browse the folders below, live, and send you files.</span></span>
          </label>
          <p class="muted">This device: <strong>{devices.peer.name}</strong> · ID {devices.peer.deviceId} · port {devices.peer.port}</p>
          <h3>Shared folders</h3>
          {#each devices.peer.shares as sh, i (sh.name)}
            <div class="share">
              <Icon name="folder" size={16} />
              <span class="sname">{sh.name}</span>
              <span class="spath muted">{sh.path}</span>
              <label class="check small"><input type="checkbox" checked={sh.readOnly} onchange={(e) => updateShare(i, { readOnly: (e.currentTarget as HTMLInputElement).checked })} /> Read only</label>
              <button type="button" class="icon" aria-label="Stop sharing" onclick={() => updateShare(i, null)}><Icon name="close" size={12} /></button>
            </div>
          {/each}
          <button type="button" class="btn" onclick={addShare}>Share current folder…</button>
          <h3>Trusted devices</h3>
          <label class="check"><input type="checkbox" checked={devices.peer.tailnetAutoTrust} onchange={(e) => peer(peerSetAutoTrust((e.currentTarget as HTMLInputElement).checked))} /> Trust my own devices on my Tailscale tailnet automatically</label>
          {#each devices.peer.trusted as t (t.id)}
            <div class="share">
              <Icon name="laptop" size={16} />
              <span class="sname">{t.name}</span>
              <span class="spath muted">paired {new Date(t.addedAt).toLocaleDateString()}</span>
              <button type="button" class="icon" aria-label="Forget device" onclick={() => peer(peerForget(t.id))}><Icon name="close" size={12} /></button>
            </div>
          {:else}
            <p class="muted">No paired devices yet.</p>
          {/each}
          <div class="row" style:margin-top="8px">
            <button type="button" class="btn" onclick={async () => (code = await peer(peerPairCode()))}>Show pairing code</button>
            <button type="button" class="btn" onclick={() => dialogs.ask("pair")}>Pair with a device…</button>
          </div>
          {#if code}<p class="code">{code}</p><p class="muted">Enter this code on the other device within 2 minutes.</p>{/if}
        {:else}
          <p class="muted">Peer mode isn't available in this build.</p>
        {/if}
      {:else if section === "network"}
        <h3>Saved servers</h3>
        {#each s.servers as srv, i (srv.uri)}
          <div class="share">
            <Icon name="server" size={16} />
            <span class="sname">{srv.name}</span>
            <span class="spath muted">{srv.uri}</span>
            <button type="button" class="icon" aria-label="Remove" onclick={() => (s.servers = s.servers.filter((_, j) => j !== i))}><Icon name="close" size={12} /></button>
          </div>
        {:else}
          <p class="muted">No saved servers.</p>
        {/each}
        <button type="button" class="btn" onclick={() => dialogs.ask("connect")}>Connect to server…</button>
        <button type="button" class="btn" onclick={() => dialogs.ask("cloud")}>Add cloud account…</button>
      {:else}
        <div class="about">
          <img src="/app-icon.svg" alt="" width="72" height="72" />
          <h3>Cross Explore</h3>
          <p class="muted">Version 0.1.0 · A fast, live file explorer for your devices, network and tailnet.</p>
          <button type="button" class="btn" onclick={() => { settings.reset(); toasts.show("Settings reset"); }}>Reset all settings</button>
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
    gap: 18px;
    min-height: 380px;
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 150px;
    flex: none;
  }
  nav button {
    height: 30px;
    padding: 0 10px;
    text-align: left;
    border-radius: var(--radius);
  }
  nav button:hover {
    background: var(--hover);
  }
  nav button.active {
    background: var(--pressed);
    font-weight: 500;
  }
  .panel {
    flex: 1;
    min-width: 0;
  }
  h3 {
    margin: 14px 0 8px;
    font-size: 13px;
    font-weight: 600;
  }
  h3:first-child {
    margin-top: 2px;
  }
  .choices {
    display: grid;
    grid-template-columns: 1fr 1fr 1fr;
    gap: 8px;
  }
  .choice {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 12px;
    border-radius: var(--radius-lg);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke-strong);
    font-size: 12px;
    color: var(--text-2);
  }
  .choice strong {
    color: var(--text);
    font-size: 13px;
  }
  .choice.on {
    box-shadow: 0 0 0 2px var(--accent);
  }
  .choice input {
    display: none;
  }
  .big {
    align-items: flex-start;
    margin-bottom: 10px;
  }
  .share {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 34px;
    padding: 0 8px;
    border-radius: var(--radius);
  }
  .share:hover {
    background: var(--hover);
  }
  .sname {
    font-weight: 500;
  }
  .spath {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .small {
    font-size: 12px;
    margin: 0;
  }
  .icon {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    border-radius: 4px;
  }
  .icon:hover {
    background: var(--pressed);
  }
  .code {
    font-size: 28px;
    font-weight: 600;
    letter-spacing: 0.15em;
    color: var(--text);
    margin: 12px 0 2px;
    font-variant-numeric: tabular-nums;
  }
  .about {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    padding-top: 30px;
  }
</style>
