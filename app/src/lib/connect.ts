// Connecting to a server, with the sign-in and host-key prompts in between.
import { asCxError, connectServer, errorText, trustHostKey, type Credentials } from "./api";
import { devices } from "./stores/devices.svelte";
import { dialogs } from "./stores/dialogs.svelte";

export type ConnectResult = { ok: true } | { ok: false; error: string; auth?: boolean };

/**
 * Try to connect. Unknown SSH host keys are shown for review and, once
 * trusted, the connection is retried. Auth failures are returned so the
 * caller (which owns the password field) can show them.
 */
export async function connect(uri: string, credentials: Credentials | null, remember: boolean): Promise<ConnectResult> {
  for (let attempt = 0; attempt < 3; attempt++) {
    try {
      await connectServer(uri, credentials, remember);
      void devices.refreshConnections();
      return { ok: true };
    } catch (e) {
      const err = asCxError(e);
      if (err?.kind === "hostKeyUnknown") {
        const trusted = await dialogs.ask<boolean>("hostKey", { ...err.message });
        if (!trusted) return { ok: false, error: "Connection cancelled" };
        try {
          await trustHostKey(err.message.uri, err.message.keyType, err.message.fingerprint);
        } catch (e2) {
          return { ok: false, error: errorText(e2) };
        }
        continue;
      }
      if (err?.kind === "authRequired") return { ok: false, error: err.message.reason || (credentials ? "Wrong user name or password" : "Sign in required"), auth: true };
      return { ok: false, error: errorText(e) };
    }
  }
  return { ok: false, error: "Couldn't verify the server" };
}

export const PROTOCOLS = [
  { scheme: "smb", label: "SMB (Windows / NAS share)", port: 445 },
  { scheme: "sftp", label: "SFTP (SSH)", port: 22 },
  { scheme: "ftp", label: "FTP", port: 21 },
  { scheme: "ftps", label: "FTPS (FTP over TLS)", port: 21 },
  { scheme: "davs", label: "WebDAV (HTTPS)", port: 443 },
  { scheme: "dav", label: "WebDAV (HTTP)", port: 80 },
  { scheme: "s3", label: "S3 / object storage (AWS, R2, B2, MinIO…)", port: 443 },
  { scheme: "peer", label: "Cross Explore device", port: 47470 },
] as const;

export function buildUri(scheme: string, host: string, port: string | number | null, user: string, path: string) {
  const h = host.trim().replace(/^\w+:\/\//, "").replace(/\/.*$/, "");
  const u = user.trim() ? `${encodeURIComponent(user.trim())}@` : "";
  const def = PROTOCOLS.find((p) => p.scheme === scheme)?.port;
  const pt = port && Number(port) !== def ? `:${port}` : "";
  const p = path.trim() ? "/" + path.trim().replace(/^\/+/, "").split("/").map(encodeURIComponent).join("/") : "/";
  return `${scheme}://${u}${h.includes(":") && !h.startsWith("[") ? `[${h}]` : h}${pt}${p}`;
}
