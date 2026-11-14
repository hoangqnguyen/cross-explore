// Several logins on one remote machine: list the ones we know about, switch
// between them in one click, or open the Connect dialog to add another.
import { dialogs } from "./stores/dialogs.svelte";
import { devices } from "./stores/devices.svelte";
import { settings } from "./stores/settings.svelte";
import { ws } from "./workspace.svelte";

export interface Remote {
  scheme: string;
  user: string;
  host: string;
  port: string;
}

const URI = /^(\w+):\/\/(?:([^@/]+)@)?(\[[^\]]+\]|[^/:]+)(?::(\d+))?(?:\/|$)/;
/** Protocols that have users. */
const WITH_USERS = new Set(["sftp", "ftp", "ftps", "smb", "dav", "davs", "s3"]);

export function parseRemote(uri: string): Remote | null {
  const m = URI.exec(uri);
  if (!m || !WITH_USERS.has(m[1])) return null;
  let user = m[2] ?? "";
  try {
    user = decodeURIComponent(user);
  } catch {
    /* keep as is */
  }
  return { scheme: m[1], user, host: m[3].toLowerCase(), port: m[4] ?? "" };
}

const same = (a: Remote, b: Remote) => a.scheme === b.scheme && a.host === b.host && a.port === b.port;

export interface Account {
  user: string;
  /** Where switching to this login goes. */
  uri: string;
  connected: boolean;
}

/** Every login we know for the machine `uri` is on: saved servers and live connections. */
export function accountsFor(uri: string): Account[] {
  const r = parseRemote(uri);
  if (!r) return [];
  const out = new Map<string, Account>();
  const add = (u: string, connected: boolean) => {
    const p = parseRemote(u);
    if (!p || !same(p, r)) return;
    const prev = out.get(p.user);
    out.set(p.user, { user: p.user, uri: prev?.uri ?? u, connected: connected || !!prev?.connected });
  };
  for (const s of settings.data.servers) add(s.uri, false);
  for (const c of devices.connected) add(c, true);
  add(uri, devices.connected.some((c) => c.replace(/\/+$/, "") === uri.replace(/\/+$/, "")));
  return [...out.values()].sort((a, b) => a.user.localeCompare(b.user));
}

/** Open the Connect dialog for the same machine, ready for another user name. */
export function signInAsAnother(uri: string) {
  const r = parseRemote(uri);
  if (!r) return;
  const host = r.host.includes(":") && !r.host.startsWith("[") ? `[${r.host}]` : r.host;
  void dialogs.ask("connect", { host: `${r.scheme}://${host}${r.port ? `:${r.port}` : ""}/`, scheme: r.scheme, focusUser: true });
}

/** Menu entries: "Switch to <user>" for each other known login, then "Sign in as another user…". */
export function accountMenuItems(uri: string, open: (uri: string) => void) {
  const r = parseRemote(uri);
  if (!r) return [];
  const mine = r.user;
  return [
    ...accountsFor(uri)
      .filter((a) => a.user !== mine)
      .map((a) => ({ label: `Switch to ${a.user || "guest"}`, icon: "user" as const, action: () => open(a.uri) })),
    { label: "Sign in as another user…", icon: "user" as const, action: () => signInAsAnother(uri) },
  ];
}

/** The remote machine the active tab is showing, if any. */
export function activeRemoteUri(): string | null {
  const t = ws.activeTab;
  const uri = t?.folder.info?.uri ?? t?.folder.uri ?? "";
  return parseRemote(uri) ? uri : null;
}
