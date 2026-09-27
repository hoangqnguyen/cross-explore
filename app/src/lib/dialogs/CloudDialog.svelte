<script lang="ts">
  // Add a Google Drive, Dropbox or OneDrive account: paste the app's client ID
  // once (Settings keeps it), then sign in through the system browser.
  import { cloudServices, cloudSetClient, cloudSignIn, errorText, openWebPage, type CloudService } from "../api";
  import Icon from "../components/Icon.svelte";
  import { dialogs } from "../stores/dialogs.svelte";
  import { settings } from "../stores/settings.svelte";
  import { ws } from "../workspace.svelte";
  import Modal from "./Modal.svelte";

  let { service: initial = "gdrive", navigate = true }: { service?: string; navigate?: boolean } = $props();

  // svelte-ignore state_referenced_locally
  let service = $state(initial);
  let services = $state<CloudService[]>([]);
  let clientId = $state("");
  let secret = $state("");
  let editing = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);

  let current = $derived(services.find((s) => s.service === service));
  let needsId = $derived(!current?.configured || editing);

  const HELP: Record<string, { url: string; steps: string }> = {
    gdrive: {
      url: "https://console.cloud.google.com/apis/credentials",
      steps: "Google Cloud Console → enable the Google Drive API → OAuth consent screen (add yourself as a test user) → Credentials → Create OAuth client ID → Desktop app. Paste the client ID and secret.",
    },
    dropbox: {
      url: "https://www.dropbox.com/developers/apps",
      steps: "Dropbox App Console → Create app (Scoped access, Full Dropbox) → Permissions: files.content.read/write, files.metadata.read/write, account_info.read → Settings: add redirect URI http://127.0.0.1:47480/callback. Paste the App key.",
    },
    onedrive: {
      url: "https://entra.microsoft.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade",
      steps: "Microsoft Entra → App registrations → New (any org directory + personal accounts) → Authentication: Mobile and desktop, redirect http://127.0.0.1/callback, allow public client flows → API permissions: Files.ReadWrite.All, User.Read, offline_access. Paste the Application (client) ID.",
    },
  };

  async function refresh() {
    try {
      services = await cloudServices();
    } catch (e) {
      error = errorText(e);
    }
  }
  void refresh();

  async function submit() {
    if (busy || !current) return;
    error = null;
    busy = true;
    try {
      if (needsId) {
        if (!clientId.trim()) {
          error = current.service === "dropbox" ? "Paste the App key first." : "Paste the client ID first.";
          return;
        }
        await cloudSetClient(service, clientId.trim(), secret.trim() || null);
        editing = false;
        await refresh();
      }
      const uri = await cloudSignIn(service);
      const account = decodeURIComponent(uri.replace(/^\w+:\/\//, "").replace(/\/.*$/, ""));
      if (!settings.data.servers.some((s) => s.uri === uri)) settings.data.servers = [...settings.data.servers, { name: `${current.label} (${account})`, uri }];
      dialogs.close(uri);
      if (navigate) ws.activeTab.navigate(uri);
    } catch (e) {
      const text = errorText(e);
      error = /cancel/i.test(text) ? "Sign-in was cancelled or timed out." : text;
    } finally {
      busy = false;
    }
  }
</script>

<Modal title="Add cloud account" width={500} onsubmit={submit}>
  <div class="services" role="radiogroup" aria-label="Service">
    {#each services as s (s.service)}
      <button type="button" role="radio" aria-checked={service === s.service} class:active={service === s.service} onclick={() => ((service = s.service), (error = null), (editing = false))} disabled={busy}>
        <Icon name="cloud" size={20} />
        <span>{s.label}</span>
        <small class="muted">{s.configured ? "Ready" : "Needs client ID"}</small>
      </button>
    {/each}
  </div>

  {#if current}
    {#if needsId}
      <p class="muted help">
        {current.label} only lets registered apps in, so Cross Explore uses your own (free) app registration.
        {HELP[service]?.steps}
        <button type="button" class="link" onclick={() => openWebPage(HELP[service]?.url ?? "")}>Open console</button>
      </p>
      <label class="field">{service === "dropbox" ? "App key" : "Client ID"}<input type="text" bind:value={clientId} spellcheck="false" autocomplete="off" /></label>
      {#if service === "gdrive"}
        <label class="field">Client secret<input type="password" bind:value={secret} autocomplete="off" /></label>
      {/if}
      <p class="muted small">Or set {current.env[0]}{current.env[1] ? ` / ${current.env[1]}` : ""} in the environment.</p>
    {:else}
      <p class="muted">
        Your browser opens to sign in to {current.label}. Come back here when it says you're done.
        <button type="button" class="link" onclick={() => ((editing = true), (clientId = current?.clientId ?? ""))}>Change client ID</button>
      </p>
    {/if}
  {/if}
  {#if busy && !needsId}<p class="wait"><span class="spinner"></span> Waiting for the browser…</p>{/if}
  {#if error}<div class="error">{error}</div>{/if}

  {#snippet footer()}
    <button type="button" class="btn" onclick={() => dialogs.close(null)}>Cancel</button>
    <button type="submit" class="btn primary" disabled={busy || !current}>{busy ? "Signing in…" : "Sign in with browser"}</button>
  {/snippet}
</Modal>

<style>
  .services {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 8px;
    margin-bottom: 12px;
  }
  .services button {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    padding: 12px 6px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    color: inherit;
  }
  .services button.active {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  .help {
    line-height: 1.45;
  }
  .small {
    font-size: 12px;
  }
  .link {
    border: 0;
    background: none;
    color: var(--accent);
    padding: 0;
    cursor: pointer;
  }
  .wait {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .spinner {
    width: 14px;
    height: 14px;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
