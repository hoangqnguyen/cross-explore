// SSH for SFTP folders. The local account name is not assumed: the first
// login asks, a successful one is remembered, and a password login can
// install a public key so the next session doesn't ask.
import { errorText, inTauri, sshCopyId, sshSavedUser } from "./api";
import { dialogs } from "./stores/dialogs.svelte";
import { toasts } from "./toasts.svelte";

export interface SshTarget {
  host: string;
  port: number;
  user: string | null;
}

/** `sftp://` (and `ssh://`, which the address bar accepts as SFTP) or null. */
export function sftpTarget(uri: string): SshTarget | null {
  if (!/^sftp:\/\//i.test(uri) && !/^ssh:\/\//i.test(uri)) return null;
  let url: URL;
  try {
    url = new URL(uri);
  } catch {
    return null;
  }
  if (!url.hostname) return null;
  return {
    host: url.hostname.replace(/^\[|\]$/g, ""),
    port: url.port ? Number(url.port) : 22,
    user: url.username ? decodeURIComponent(url.username) : null,
  };
}

export function withSftpUser(uri: string, user: string): string {
  const url = new URL(uri);
  url.username = user;
  return url.toString();
}

/**
 * URI to open a terminal in. Local folders come back unchanged. SFTP folders
 * get an explicit user: the one that signed in before, or whatever the person
 * types now (prefilled with the user from the folder URI, never the local
 * account). `null` means they cancelled.
 */
export async function resolveSshUri(uri: string): Promise<string | null> {
  const target = sftpTarget(uri);
  if (!target) return uri;
  const saved = inTauri ? await sshSavedUser(target.host, target.port) : null;
  if (saved) return withSftpUser(uri, saved);
  const typed = await dialogs.prompt(`SSH to ${target.host}`, "User name on this server", target.user ?? "", "Connect", false, true);
  if (!typed) return null;
  return withSftpUser(uri, typed);
}

/** After a password login, offer to copy an SSH key onto the server. */
export async function offerSshKey(sessionId: number, method: string, copyId: boolean, user: string, host: string) {
  if (method !== "password" && method !== "keyboard-interactive") return;
  const yes = await dialogs.confirm(
    `Copy your SSH key to ${host}?`,
    `You signed in as ${user} with a password. Copy your public key to this server so later SSH sessions sign in without asking?`,
    "Copy key",
  );
  if (!yes) return;
  let password: string | null = null;
  if (!copyId) {
    password = await dialogs.prompt("Server password", `Password for ${user}@${host}. It is used once to install the key and is not saved.`, "", "Install key", true);
    if (!password) return;
  }
  try {
    toasts.show(await sshCopyId(sessionId, password));
  } catch (e) {
    toasts.show(errorText(e), "error");
  }
}
