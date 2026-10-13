<script lang="ts">
  // Share a local folder with my other devices: the name they see, view or
  // edit, which devices, and the link to open it from another device. Also
  // used to change or stop an existing share.
  import { untrack } from "svelte";
  import { fileUriToPath, uriName, type PeerShare } from "../api";
  import DeviceIcon from "../components/DeviceIcon.svelte";
  import FileIcon from "../components/FileIcon.svelte";
  import Icon, { type IconName } from "../components/Icon.svelte";
  import { copyText, peerKind, saveShares, setSharingOn, shareLink, shareOf, stopSharing, uniqueName } from "../sharing.svelte";
  import { devices } from "../stores/devices.svelte";
  import { dialogs } from "../stores/dialogs.svelte";
  import { settings } from "../stores/settings.svelte";
  import { toasts } from "../toasts.svelte";
  import { ws } from "../workspace.svelte";
  import Modal from "./Modal.svelte";

  let { uri: given }: { uri?: string } = $props();

  // Places to pick from when the dialog was opened without a folder.
  let candidates = $derived.by(() => {
    const out: { uri: string; name: string }[] = [];
    const add = (uri: string | undefined, name: string) => uri?.startsWith("file://") && !out.some((c) => c.uri === uri) && out.push({ uri, name });
    const cur = ws.activeTab?.folder.info?.local ? ws.activeTab.dirUri : undefined;
    add(cur, `Current folder — ${uriName(cur ?? "")}`);
    for (const p of ws.places?.favorites ?? []) add(p.uri, p.name);
    if (ws.places) add(ws.places.home.uri, ws.places.home.name);
    for (const b of settings.data.bookmarks) add(b.uri, b.name);
    return out.filter((c) => !shareOf(c.uri));
  });

  let uri = $state(untrack(() => given ?? ""));
  let existing = $derived(uri ? shareOf(uri) : null);
  // svelte-ignore state_referenced_locally
  const original = existing;
  let name = $state(untrack(() => original?.name ?? (uri ? uniqueName(uriName(uri)) : "")));
  let canEdit = $state(untrack(() => (original ? !original.readOnly : false)));
  let everyone = $state(untrack(() => !original?.devices));
  let chosen = $state<string[]>(untrack(() => original?.devices ?? []));
  let busy = $state(false);

  let peer = $derived(devices.peer);
  let trusted = $derived(peer?.trusted ?? []);
  let nameError = $derived.by(() => {
    const n = name.trim();
    if (!n) return "Give the share a name";
    if (/[\\/:*?"<>|]/.test(n) || n === "." || n === "..") return "Names can't contain \\ / : * ? \" < > |";
    if (peer?.shares.some((s) => s.name.toLowerCase() === n.toLowerCase() && s.name !== original?.name)) return `“${n}” is already the name of another share`;
    return null;
  });
  let draft = $derived<PeerShare>({ name: name.trim(), path: uri ? fileUriToPath(uri) : "", readOnly: !canEdit, devices: everyone ? null : chosen });
  let link = $derived(peer && !nameError ? shareLink(draft, peer) : "");
  let valid = $derived(!!uri && !nameError && (everyone || chosen.length > 0));

  function pick(u: string) {
    uri = u;
    name = uniqueName(uriName(u));
  }

  function toggle(id: string) {
    chosen = chosen.includes(id) ? chosen.filter((x) => x !== id) : [...chosen, id];
  }

  async function save() {
    if (!valid || !peer || busy) return;
    busy = true;
    if (!peer.enabled && !(await setSharingOn(true))) return void (busy = false);
    const rest = (devices.peer ?? peer).shares.filter((s) => s.name !== original?.name);
    const ok = await saveShares(original ? (devices.peer ?? peer).shares.map((s) => (s.name === original.name ? draft : s)) : [...rest, draft]);
    busy = false;
    if (!ok) return;
    const who = everyone ? "all your devices" : chosen.length === 1 ? (trusted.find((t) => t.id === chosen[0])?.name ?? "1 device") : `${chosen.length} devices`;
    toasts.show(original ? `Updated “${draft.name}”` : `Sharing “${draft.name}” with ${who}`);
    dialogs.close(true);
  }

  async function stop() {
    if (original && (await stopSharing(original))) dialogs.close(false);
  }

  const access: { edit: boolean; icon: IconName; title: string; hint: string }[] = [
    { edit: false, icon: "eye", title: "Can view", hint: "Open, preview and copy files" },
    { edit: true, icon: "rename", title: "Can edit", hint: "Also add, change and delete" },
  ];
</script>

<Modal title={original ? `Sharing “${original.name}”` : uri ? `Share “${uriName(uri)}”` : "Share a folder"} width={520} onsubmit={save}>
  {#if !peer}
    <p>Sharing with your devices isn't available in this build.</p>
  {:else}
    {#if uri}
      <div class="folder">
        <FileIcon name="" isDir size={36} />
        <div class="ftext">
          <span class="fname">{uriName(uri)}</span>
          <span class="fpath" title={fileUriToPath(uri)}>{fileUriToPath(uri)}</span>
        </div>
        {#if !given}<button type="button" class="link-btn" onclick={() => (uri = "")}>Change</button>{/if}
      </div>
    {:else}
      <div class="section-label">Choose a folder</div>
      <div class="picks">
        {#each candidates as c (c.uri)}
          <button type="button" class="pick" onclick={() => pick(c.uri)}>
            <FileIcon name="" isDir size={20} />
            <span class="pname">{c.name}</span>
            <span class="ppath">{fileUriToPath(c.uri)}</span>
          </button>
        {:else}
          <p class="muted">Open the folder you want to share and choose Share… from its menu.</p>
        {/each}
      </div>
      <p class="muted tip">Tip: right-click any folder and choose <b>Share…</b>, or drop folders on the Sharing page.</p>
    {/if}

    {#if uri}
      {#if !peer.enabled}
        <div class="banner">
          <Icon name="info" size={16} />
          <span>Sharing is off on this device. It will be turned on so your devices can connect.</span>
        </div>
      {/if}

      <label class="field">Shared as
        <input type="text" bind:value={name} spellcheck="false" aria-invalid={!!nameError} />
      </label>
      {#if nameError && name}<div class="error">{nameError}</div>{/if}

      <div class="section-label">Access</div>
      <div class="cards" role="radiogroup" aria-label="Access">
        {#each access as a (a.title)}
          <button type="button" class="card" class:on={canEdit === a.edit} role="radio" aria-checked={canEdit === a.edit} onclick={() => (canEdit = a.edit)}>
            <span class="cicon"><Icon name={a.icon} size={16} /></span>
            <span class="ctitle">{a.title}</span>
            <span class="chint">{a.hint}</span>
          </button>
        {/each}
      </div>

      <div class="section-label">Who can open it</div>
      <div class="who">
        <label class="opt">
          <input type="radio" name="who" checked={everyone} onchange={() => (everyone = true)} />
          <span>
            All my devices
            <span class="muted">{trusted.length ? `${trusted.length} paired${peer.tailnetAutoTrust ? ", plus your devices on Tailscale" : ""}` : peer.tailnetAutoTrust ? "Your devices on Tailscale, and any you pair later" : "Any device you pair"}</span>
          </span>
        </label>
        <label class="opt">
          <input type="radio" name="who" checked={!everyone} onchange={() => (everyone = false)} disabled={!trusted.length} />
          <span>
            Only the devices I choose
            {#if !trusted.length}<span class="muted">Pair a device first</span>{/if}
          </span>
        </label>
        {#if !everyone}
          <div class="devlist">
            {#each trusted as t (t.id)}
              <label class="dev">
                <input type="checkbox" checked={chosen.includes(t.id)} onchange={() => toggle(t.id)} />
                <span class="dicon"><DeviceIcon kind={peerKind(t.id, t.os)} size={16} /></span>
                <span class="dname">{t.name}</span>
                {#if t.connected}<span class="live" title="Connected now">Connected</span>{/if}
              </label>
            {/each}
            {#if !chosen.length}<div class="error">Choose at least one device</div>{/if}
          </div>
        {/if}
        <button type="button" class="link-btn pair" onclick={() => dialogs.ask("pair")}><Icon name="plus" size={13} /> Pair another device…</button>
      </div>

      {#if link}
        <div class="section-label">Link for your devices</div>
        <div class="linkrow">
          <input type="text" readonly value={link} aria-label="Share link" onfocus={(e) => (e.currentTarget as HTMLInputElement).select()} />
          <button type="button" class="btn" onclick={() => copyText(link)}><Icon name="copy" size={14} /> Copy</button>
        </div>
        <p class="muted">On a paired device, paste it in the address bar — or find <b>{peer.name}</b> under Network.</p>
      {/if}
    {/if}
  {/if}
  {#snippet footer()}
    {#if original}<button type="button" class="btn stop" onclick={stop}>Stop sharing</button>{/if}
    <button type="button" class="btn" onclick={() => dialogs.close(null)}>Cancel</button>
    {#if peer && uri}
      <button type="submit" class="btn primary" disabled={!valid || busy}>{busy ? "Saving…" : original ? "Save" : "Share"}</button>
    {/if}
  {/snippet}
</Modal>

<style>
  .folder {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px;
    margin-bottom: 14px;
    border-radius: var(--radius-lg);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke);
  }
  .ftext {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1;
  }
  .fname {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .fpath,
  .ppath {
    font-size: 11.5px;
    color: var(--text-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .section-label {
    margin: 14px 0 6px;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-2);
  }
  .field input {
    width: 100%;
  }
  .picks {
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-height: 260px;
    overflow-y: auto;
  }
  .pick {
    display: grid;
    grid-template-columns: 24px minmax(90px, auto) 1fr;
    align-items: center;
    gap: 10px;
    height: 36px;
    padding: 0 10px;
    border-radius: var(--radius);
    text-align: left;
  }
  .pick:hover {
    background: var(--hover);
  }
  .pname {
    font-weight: 500;
  }
  .tip {
    margin-top: 10px;
  }
  .banner {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    margin-bottom: 12px;
    border-radius: var(--radius-lg);
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 12.5px;
  }
  .banner span {
    color: var(--text);
  }
  .cards {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
  }
  .card {
    display: grid;
    grid-template-columns: 28px 1fr;
    grid-template-rows: auto auto;
    column-gap: 8px;
    align-items: center;
    padding: 10px 12px;
    border-radius: var(--radius-lg);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke-strong);
    text-align: left;
    transition: box-shadow 0.12s, background 0.12s;
  }
  .card:hover {
    background: var(--hover);
  }
  .card.on {
    box-shadow: 0 0 0 2px var(--accent);
    background: var(--accent-soft);
  }
  .cicon {
    grid-row: span 2;
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border-radius: 8px;
    background: var(--hover);
    color: var(--accent);
  }
  .card.on .cicon {
    background: var(--accent);
    color: var(--accent-text);
  }
  .ctitle {
    font-weight: 600;
  }
  .chint {
    font-size: 11.5px;
    color: var(--text-3);
  }
  .who {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .opt {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 6px 4px;
    font-size: 13px;
  }
  .opt > span {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .opt input {
    margin-top: 2px;
    accent-color: var(--accent);
  }
  .devlist {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin: 0 0 4px 26px;
    padding: 4px;
    border-radius: var(--radius-lg);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke);
  }
  .dev {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 32px;
    padding: 0 8px;
    border-radius: var(--radius);
  }
  .dev:hover {
    background: var(--hover);
  }
  .dev input {
    accent-color: var(--accent);
  }
  .dicon {
    color: var(--accent);
    display: grid;
  }
  .dname {
    flex: 1;
  }
  .live {
    font-size: 11px;
    padding: 1px 7px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--live) 18%, transparent);
    color: var(--live);
  }
  .link-btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 26px;
    padding: 0 8px;
    border-radius: var(--radius);
    color: var(--accent);
    font-size: 12.5px;
  }
  .link-btn:hover {
    background: var(--hover);
  }
  .link-btn.pair {
    align-self: flex-start;
    margin-top: 2px;
  }
  .linkrow {
    display: flex;
    gap: 8px;
    margin-bottom: 6px;
  }
  .linkrow input {
    flex: 1;
    min-width: 0;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 12px;
  }
  .linkrow .btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    min-width: 0;
  }
  .stop {
    margin-right: auto;
    color: var(--danger);
  }
</style>
