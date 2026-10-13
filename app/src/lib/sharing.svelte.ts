// Sharing folders with my other devices (peer mode): which local folders are
// shared, saving changes, the links other devices open shares by, and what
// those devices did here lately.
import { errorText, fileUriToPath, peerActivity, peerSetEnabled, peerSetShares, uriName, type DeviceKind, type PeerAccess, type PeerShare, type PeerStatus } from "./api";
import { devices } from "./stores/devices.svelte";
import { dialogs } from "./stores/dialogs.svelte";
import { toasts } from "./toasts.svelte";

export const SHARING_URI = "cx:sharing";

/** Compare OS paths: no trailing separator, `\` as `/`, Windows paths case-insensitive. */
function normPath(p: string) {
  const s = p.replace(/[\\/]+$/, "").replaceAll("\\", "/");
  return /^[A-Za-z]:/.test(s) || s.startsWith("//") ? s.toLowerCase() : s;
}

/** The share for the local folder at `uri`, if it is shared. */
export function shareOf(uri: string, status: PeerStatus | null = devices.peer): PeerShare | null {
  if (!status || !uri.startsWith("file://")) return null;
  const p = normPath(fileUriToPath(uri));
  return status.shares.find((s) => normPath(s.path) === p) ?? null;
}

/** Shared folders, by normalized path, for marking rows cheaply. */
class SharedPaths {
  set = $derived(new Set((devices.peer?.shares ?? []).map((s) => normPath(s.path))));
}
const shared = new SharedPaths();

export function isShared(uri: string): boolean {
  return uri.startsWith("file://") && shared.set.size > 0 && shared.set.has(normPath(fileUriToPath(uri)));
}

/** What another device types (or pastes) to open a share of this one. */
export function shareLink(share: PeerShare, status: PeerStatus | null = devices.peer): string {
  return status ? `peer://${status.deviceId}/${encodeURIComponent(share.name)}/` : "";
}

/** A share name not taken yet: "Photos", "Photos 2", … */
export function uniqueName(base: string, except?: string): string {
  const taken = new Set((devices.peer?.shares ?? []).filter((s) => s.name !== except).map((s) => s.name.toLowerCase()));
  const clean = base.replace(/[\\/:*?"<>|]/g, "").trim() || "Shared";
  if (!taken.has(clean.toLowerCase())) return clean;
  for (let i = 2; ; i++) if (!taken.has(`${clean} ${i}`.toLowerCase())) return `${clean} ${i}`;
}

/** "All my devices", "hpc", "hpc and Joy's laptop", "hpc and 2 others", "No devices". */
export function audienceLabel(share: PeerShare, status: PeerStatus | null = devices.peer): string {
  if (!share.devices) return "All my devices";
  const names = share.devices.map((id) => status?.trusted.find((t) => t.id === id)?.name ?? "a removed device");
  if (!names.length) return "No devices";
  if (names.length === 1) return names[0];
  if (names.length === 2) return `${names[0]} and ${names[1]}`;
  return `${names[0]} and ${names.length - 1} others`;
}

/** Save the whole share list. Returns false (after telling the user) on failure. */
export async function saveShares(shares: PeerShare[]): Promise<boolean> {
  try {
    devices.peer = await peerSetShares(shares);
    return true;
  } catch (e) {
    toasts.show(errorText(e), "error");
    return false;
  }
}

/** Turn sharing (listening for other devices) on or off. */
export async function setSharingOn(on: boolean): Promise<boolean> {
  try {
    devices.peer = await peerSetEnabled(on);
    return true;
  } catch (e) {
    toasts.show(errorText(e), "error");
    return false;
  }
}

/** Stop sharing one folder, with a confirmation when devices are using it. */
export async function stopSharing(share: PeerShare): Promise<boolean> {
  const peer = devices.peer;
  if (!peer) return false;
  const busy = peer.trusted.filter((t) => t.connected && (!share.devices || share.devices.includes(t.id)));
  if (busy.length && !(await dialogs.confirm("Stop sharing?", `${busy.map((t) => t.name).join(", ")} ${busy.length === 1 ? "is" : "are"} connected right now and will lose access to “${share.name}”.`, "Stop sharing", true))) return false;
  const ok = await saveShares(peer.shares.filter((s) => s.name !== share.name));
  if (ok) toasts.show(`Stopped sharing “${share.name}”`);
  return ok;
}

/** Open the share dialog for a local folder (or to pick one). */
export function openShareDialog(uri?: string) {
  return dialogs.ask("share", uri ? { uri } : {});
}

export async function copyText(text: string, what = "Link") {
  try {
    await navigator.clipboard.writeText(text);
    toasts.show(`${what} copied`);
  } catch {
    toasts.show(`Couldn't copy the ${what.toLowerCase()}`, "error");
  }
}

// ---------------- activity ----------------

const NOISE = new Set(["stat", "watch", "freeSpace", "hash", "listShares"]);

export interface ActivityItem {
  key: string;
  time: number;
  deviceId: string;
  device: string;
  verb: string;
  /** Peer path ("/Share/a/b"), when the action had one. */
  path: string | null;
  ok: boolean;
  error: string | null;
  /** How many raw records this line stands for (repeated browsing). */
  count: number;
}

/** Past tense for what happened, and the plain verb for "couldn't …". */
const VERBS: Record<string, [string, string]> = {
  list: ["Browsed", "browse"],
  read: ["Opened", "open"],
  write: ["Saved", "save"],
  createDir: ["Created folder", "create a folder in"],
  move: ["Moved", "move"],
  remove: ["Deleted", "delete"],
  trash: ["Moved to trash", "move to trash"],
  copy: ["Copied", "copy"],
  setModified: ["Updated", "update"],
  offer: ["Sent you files", "send you files"],
  pair: ["Paired", "pair"],
};

/**
 * Turn raw audit records (newest first) into readable lines: lookups that
 * only feed a listing are dropped, and repeats of the same action on the
 * same path by the same device within a few minutes collapse into one.
 */
export function summarize(records: PeerAccess[], limit = 50): ActivityItem[] {
  const out: ActivityItem[] = [];
  for (const r of records) {
    if (NOISE.has(r.op) && r.ok) continue;
    const [past, base] = VERBS[r.op] ?? [r.op, r.op];
    const verb = r.ok ? past : `Couldn't ${base}`;
    const prev = out.at(-1);
    if (prev && prev.deviceId === r.deviceId && prev.verb === verb && prev.path === r.path && prev.ok === r.ok && prev.time - r.time < 5 * 60_000) {
      prev.count++;
      continue;
    }
    out.push({ key: `${r.time}-${r.deviceId}-${r.op}-${out.length}`, time: r.time, deviceId: r.deviceId, device: r.name, verb, path: r.path, ok: r.ok, error: r.error, count: 1 });
    if (out.length >= limit) break;
  }
  return out;
}

class Activity {
  records = $state.raw<PeerAccess[]>([]);
  loaded = false;

  async load() {
    try {
      this.records = await peerActivity(300);
      this.loaded = true;
    } catch {
      /* peer mode unavailable */
    }
  }

  add(r: PeerAccess) {
    this.records = [r, ...this.records].slice(0, 300);
  }
}

export const activity = new Activity();

/** "just now", "5 min ago", "3 h ago", "Yesterday", or a date. */
export function ago(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 45) return "just now";
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  if (s < 2 * 86400) return "Yesterday";
  return new Date(ms).toLocaleDateString();
}

/** The last segment of a peer path, for display. */
export const peerPathName = (p: string) => uriName(p) || p;

/** Icon kind for a paired device: from discovery when it's around, else its OS. */
export function peerKind(id: string, os: string): DeviceKind {
  const seen = devices.list.find((d) => d.services.some((s) => s.scheme === "peer" && s.uri.replace(/\/+$/, "") === `peer://${id}`));
  if (seen) return seen.kind;
  const o = os.toLowerCase();
  return o.includes("mac") ? "mac" : o.includes("win") ? "pc" : o.includes("android") || o.includes("ios") ? "phone" : o.includes("linux") ? "linux" : "unknown";
}

/** Whether a paired device is reachable: connected, or online per discovery. */
export function peerOnline(id: string): boolean | null {
  if (devices.peer?.trusted.some((t) => t.id === id && t.connected)) return true;
  const seen = devices.list.find((d) => d.services.some((s) => s.scheme === "peer" && s.uri.replace(/\/+$/, "") === `peer://${id}`));
  if (!seen) return null;
  return seen.tailnet ? seen.tailnet.online : true;
}

/** `file://` URI of an OS path (the inverse of `fileUriToPath`). */
export function pathToFileUri(p: string): string {
  const s = p.replaceAll("\\", "/");
  const enc = (parts: string[]) => parts.map((x) => encodeURIComponent(x)).join("/");
  if (/^[A-Za-z]:/.test(s)) {
    const [drive, ...rest] = s.split("/");
    return `file:///${drive}/${enc(rest)}`.replace(/\/$/, "");
  }
  if (s.startsWith("//")) return "file:" + "//" + enc(s.slice(2).split("/"));
  return "file://" + enc(s.split("/"));
}

/** The local folder and item a peer path ("/Share/a/b.txt") points at. */
export function localTarget(path: string | null, status: PeerStatus | null = devices.peer): { dir: string; name: string } | null {
  if (!path || !status) return null;
  const segs = path.split("/").filter(Boolean);
  const share = status.shares.find((s) => s.name === segs[0]);
  if (!share) return null;
  const base = pathToFileUri(share.path);
  const rest = segs.slice(1);
  if (!rest.length) return { dir: base, name: "" };
  let dir = base;
  for (const seg of rest.slice(0, -1)) dir = dir.replace(/\/+$/, "") + "/" + encodeURIComponent(seg);
  return { dir, name: rest.at(-1)! };
}

/** "Windows", "macOS", "Linux"… as people write them. */
export function osLabel(os: string): string {
  const o = os.toLowerCase();
  return o.startsWith("mac") ? "macOS" : o.startsWith("win") ? "Windows" : o === "ios" ? "iOS" : o ? o.charAt(0).toUpperCase() + o.slice(1) : "";
}
