<script lang="ts">
  import { buildUri, connect, PROTOCOLS } from "../connect";
  import { dialogs } from "../stores/dialogs.svelte";
  import { settings } from "../stores/settings.svelte";
  import { ws } from "../workspace.svelte";
  import Modal from "./Modal.svelte";

  let { host: initialHost = "", scheme: initialScheme = "smb" }: { host?: string; scheme?: string } = $props();

  // svelte-ignore state_referenced_locally
  let scheme = $state(initialScheme);
  // svelte-ignore state_referenced_locally
  let host = $state(initialHost);
  let port = $state<string>("");
  let path = $state("");
  let user = $state("");
  let password = $state("");
  let keyPath = $state("");
  let anonymous = $state(false);
  let remember = $state(true);
  let save = $state(true);
  let busy = $state(false);
  let error = $state<string | null>(null);

  // Pasting a full URI fills in everything.
  $effect(() => {
    const m = /^(\w+):\/\/(?:([^@/]+)@)?([^/:]+|\[[^\]]+\])(?::(\d+))?(\/.*)?$/.exec(host.trim());
    if (!m) return;
    const s = m[1].toLowerCase().replace("webdavs", "davs").replace("webdav", "dav").replace("https", "davs").replace("http", "dav").replace("ssh", "sftp");
    if (PROTOCOLS.some((p) => p.scheme === s)) scheme = s;
    if (m[2]) user = decodeURIComponent(m[2]);
    if (m[4]) port = m[4];
    if (m[5]) path = decodeURIComponent(m[5]);
    host = m[3];
  });

  let defaultPort = $derived(PROTOCOLS.find((p) => p.scheme === scheme)?.port ?? 0);
  let uri = $derived(host.trim() ? buildUri(scheme, host, port || null, anonymous ? "" : user, path) : "");

  async function submit() {
    if (!uri) return;
    busy = true;
    error = null;
    const creds = anonymous || (!user && !password && !keyPath)
      ? null
      : { user, secret: keyPath ? { type: "key" as const, path: keyPath, passphrase: password || null } : password ? { type: "password" as const, password } : { type: "none" as const } };
    const r = await connect(uri, creds, remember);
    busy = false;
    if (!r.ok) {
      error = r.error;
      return;
    }
    if (save && !settings.data.servers.some((s) => s.uri === uri)) settings.data.servers = [...settings.data.servers, { name: host.trim() + (path.trim() ? `/${path.trim().replace(/^\//, "")}` : ""), uri }];
    dialogs.close(uri);
    ws.activeTab.navigate(uri);
  }
</script>

<Modal title="Connect to server" width={480} onsubmit={submit}>
  <label class="field">
    Protocol
    <select bind:value={scheme}>
      {#each PROTOCOLS as p}<option value={p.scheme}>{p.label}</option>{/each}
    </select>
  </label>
  <div class="row">
    <label class="field" style:flex="3">Server<input type="text" bind:value={host} placeholder={scheme === "s3" ? "s3.eu-west-1.amazonaws.com, <account>.r2.cloudflarestorage.com" : "nas.local, 192.168.1.20, host.tailnet.ts.net or a full URI"} spellcheck="false" /></label>
    <label class="field" style:flex="1">Port<input type="text" bind:value={port} placeholder={String(defaultPort)} inputmode="numeric" /></label>
  </div>
  <label class="field">{scheme === "s3" ? "Bucket (optional)" : "Folder (optional)"}<input type="text" bind:value={path} placeholder={scheme === "smb" ? "Share/folder" : scheme === "s3" ? "my-bucket/photos" : "/home/me"} spellcheck="false" /></label>
  {#if scheme !== "peer"}
    <label class="check"><input type="checkbox" bind:checked={anonymous} /> Connect as guest</label>
    {#if !anonymous}
      <div class="row">
        <label class="field">{scheme === "s3" ? "Access key ID" : "User"}<input type="text" bind:value={user} autocomplete="username" spellcheck="false" /></label>
        <label class="field">{scheme === "s3" ? "Secret access key" : keyPath ? "Key passphrase" : "Password"}<input type="password" bind:value={password} autocomplete="current-password" /></label>
      </div>
      {#if scheme === "sftp"}
        <label class="field">Private key file (optional — ssh-agent and ~/.ssh keys are tried automatically)<input type="text" bind:value={keyPath} placeholder="~/.ssh/id_ed25519" spellcheck="false" /></label>
      {/if}
      <label class="check"><input type="checkbox" bind:checked={remember} /> Remember password in the {ws.platform === "macos" ? "Keychain" : "system keychain"}</label>
    {/if}
  {/if}
  <label class="check"><input type="checkbox" bind:checked={save} /> Add to the sidebar</label>
  {#if error}<div class="error">{error}</div>{/if}
  {#if uri}<div class="muted">{uri}</div>{/if}
  {#snippet footer()}
    <button type="button" class="btn" onclick={() => dialogs.close(null)}>Cancel</button>
    <button type="submit" class="btn primary" disabled={!uri || busy}>{busy ? "Connecting…" : "Connect"}</button>
  {/snippet}
</Modal>
