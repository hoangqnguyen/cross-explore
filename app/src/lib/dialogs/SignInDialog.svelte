<script lang="ts">
  import { connect } from "../connect";
  import { dialogs } from "../stores/dialogs.svelte";
  import { ws } from "../workspace.svelte";
  import Modal from "./Modal.svelte";

  let { uri, user: initialUser = null, reason = "" }: { uri: string; user?: string | null; reason?: string } = $props();
  // svelte-ignore state_referenced_locally
  let user = $state(initialUser ?? "");
  let password = $state("");
  let remember = $state(true);
  let busy = $state(false);
  // svelte-ignore state_referenced_locally
  let error = $state<string | null>(reason && reason !== "Sign in required" ? reason : null);
  let host = $derived(uri.replace(/^\w+:\/\/([^@/]*@)?/, "").replace(/\/.*$/, ""));

  async function submit() {
    busy = true;
    error = null;
    const r = await connect(uri, { user, secret: password ? { type: "password", password } : { type: "none" } }, remember);
    busy = false;
    if (r.ok) dialogs.close(true);
    else error = r.error;
  }
</script>

<Modal title="Sign in to “{host}”" onsubmit={submit}>
  <p>Enter your name and password for this server.</p>
  <label class="field">User<input type="text" bind:value={user} autocomplete="username" spellcheck="false" /></label>
  <label class="field">Password<input type="password" bind:value={password} autocomplete="current-password" /></label>
  <label class="check"><input type="checkbox" bind:checked={remember} /> Remember in the {ws.platform === "macos" ? "Keychain" : "system keychain"}</label>
  {#if error}<div class="error">{error}</div>{/if}
  {#snippet footer()}
    <button type="button" class="btn" onclick={() => dialogs.close(false)}>Cancel</button>
    <button type="submit" class="btn primary" disabled={busy || !user}>{busy ? "Signing in…" : "Sign in"}</button>
  {/snippet}
</Modal>
