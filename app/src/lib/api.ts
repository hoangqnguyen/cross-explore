// Typed bindings for the Rust commands in src-tauri/src/commands.rs.
// Outside Tauri (plain `npm run dev` in a browser) an in-memory mock backend
// answers instead, which is handy for UI work and screenshots.
import { Channel, invoke as tauriInvoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

export const inTauri = "__TAURI_INTERNALS__" in window;
const mock = inTauri ? null : await import("./mock");

function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return mock ? mock.invoke<T>(cmd, args ?? {}) : tauriInvoke<T>(cmd, args);
}

/** Window controls; no-ops in the browser preview. */
export const appWindow = {
  show: () => inTauri && getCurrentWindow().show(),
  setTitle: (t: string) => (inTauri ? getCurrentWindow().setTitle(t) : void (document.title = t)),
  minimize: () => inTauri && getCurrentWindow().minimize(),
  toggleMaximize: () => inTauri && getCurrentWindow().toggleMaximize(),
  close: () => inTauri && getCurrentWindow().close(),
};

export type EntryKind = "file" | "dir" | "symlink" | "other";

export interface Entry {
  name: string;
  kind: EntryKind;
  isDir: boolean;
  size: number;
  modified: number | null;
  created: number | null;
  hidden: boolean;
  readonly: boolean;
}

/** A row: a directory entry, plus where it lives for search results. */
export type Item = Entry & { uri?: string; parent?: string; relPath?: string; snippet?: string | null; line?: number | null; depth?: number };

export interface Crumb {
  label: string;
  uri: string;
  icon: "home" | "drive" | "folder" | "server" | "share" | "archive" | "search" | "tag";
}

export interface LocationInfo {
  uri: string;
  scheme: string;
  display: string;
  name: string;
  parent: string | null;
  crumbs: Crumb[];
  local: boolean;
}

export interface Capabilities {
  liveWatch: boolean;
  polling: boolean;
  serverCopy: boolean;
  trash: boolean;
  posix: boolean;
  writable: boolean;
}

export type ListEvent =
  | { type: "meta"; info: LocationInfo; capabilities: Capabilities }
  | { type: "batch"; entries: Entry[] }
  | { type: "done"; total: number; elapsedMs: number };

export type Change = { type: "upsert"; entry: Entry } | { type: "remove"; name: string } | { type: "reset" };

export interface Place {
  name: string;
  uri: string;
  icon: string;
}

export interface Volume {
  name: string;
  uri: string;
  total: number;
  free: number;
  removable: boolean;
}

export interface CloudPlace {
  name: string;
  uri: string;
  provider: "google" | "dropbox" | "onedrive" | "icloud" | "box" | "other";
  account: string | null;
}

export interface Places {
  platform: string;
  translucent: boolean;
  home: Place;
  favorites: Place[];
  volumes: Volume[];
  /** Folders synced by cloud apps (Google Drive, Dropbox, OneDrive, iCloud…). */
  cloud: CloudPlace[];
}

export type CxError =
  | { kind: "notFound" | "permissionDenied" | "alreadyExists" | "invalidLocation" | "invalidName" | "unsupported" | "connection" | "io"; message: string }
  | { kind: "authRequired"; message: { uri: string; user: string | null; reason: string } }
  | { kind: "hostKeyUnknown"; message: { uri: string; host: string; keyType: string; fingerprint: string; changed: boolean } }
  | { kind: "cancelled"; message?: undefined };

export function asCxError(e: unknown): CxError | null {
  return e && typeof e === "object" && "kind" in e ? (e as CxError) : null;
}

export function errorText(e: unknown): string {
  const err = asCxError(e);
  if (!err) return String(e);
  switch (err.kind) {
    case "notFound":
      return `Can't find "${err.message}". It may have been moved or deleted.`;
    case "permissionDenied":
      return `You don't have permission to access "${err.message}".`;
    case "alreadyExists":
      return `"${err.message}" already exists.`;
    case "invalidName":
      return `"${err.message}" isn't a valid name.`;
    case "authRequired":
      return err.message.reason || `Sign in to ${err.message.uri}`;
    case "hostKeyUnknown":
      return `The identity of ${err.message.host} couldn't be verified.`;
    case "connection":
      return `Couldn't connect: ${err.message}`;
    case "cancelled":
      return "Cancelled";
    default:
      return err.message;
  }
}

function channel<T>(onMessage: (m: T) => void) {
  if (mock) return onMessage;
  const ch = new Channel<T>();
  ch.onmessage = onMessage;
  return ch;
}

export async function listDir(uri: string, onEvent: (ev: ListEvent) => void): Promise<void> {
  await invoke("list_dir", { uri, onEvent: channel(onEvent) });
}

export interface WatchInfo {
  id: number;
  /** "live" when changes are pushed; "polling" when the folder is re-listed periodically. */
  mode: "live" | "polling";
}

export async function watchDir(uri: string, onChange: (changes: Change[]) => void): Promise<WatchInfo> {
  return invoke<WatchInfo>("watch_dir", { uri, onChange: channel(onChange) });
}

export const unwatchDir = (id: number) => invoke<void>("unwatch_dir", { id });
export const createFolder = (uri: string, name?: string) => invoke<Entry>("create_folder", { uri, name });
export const renameEntry = (uri: string, from: string, to: string) => invoke<Entry>("rename_entry", { uri, from, to });
export const trashEntries = (uri: string, names: string[]) => invoke<TrashedItem[]>("trash_entries", { uri, names });
export const openEntry = (uri: string) => invoke<void>("open_entry", { uri });
export const revealEntry = (uri: string) => invoke<void>("reveal_entry", { uri });
export const openTerminal = (uri: string) => invoke<void>("open_terminal", { uri });
export const osClipboardSet = (uris: string[]) => invoke<void>("os_clipboard_set", { uris });
export const osClipboardGet = () => invoke<string[]>("os_clipboard_get");
export const fullDiskAccess = () => invoke<boolean>("full_disk_access");
export const openFullDiskAccessSettings = () => invoke<void>("open_full_disk_access_settings");
export const statEntry = (uri: string) => invoke<Entry>("stat_entry", { uri });
export const places = () => invoke<Places>("places");

export interface CloudService {
  service: "gdrive" | "dropbox" | "onedrive";
  label: string;
  configured: boolean;
  clientId: string | null;
  hasSecret: boolean;
  env: [string, string | null];
}
export const CLOUD_SCHEMES = ["gdrive", "dropbox", "onedrive"];
export const isCloudUri = (uri: string) => CLOUD_SCHEMES.some((s) => uri.startsWith(s + "://"));
export const cloudServices = () => invoke<CloudService[]>("cloud_services");
export const cloudSetClient = (service: string, clientId: string, clientSecret: string | null) => invoke<void>("cloud_set_client", { service, clientId, clientSecret });
/** Browser sign-in; resolves with the account's root URI. */
export const cloudSignIn = (service: string) => invoke<string>("cloud_sign_in", { service });
export type OfficeView = { kind: "html"; html: string; title: string | null; pages: number | null } | { kind: "pdf"; uri: string };
/** Word, Excel, PowerPoint, OpenDocument, RTF and CSV previews (HTML, or a PDF made by LibreOffice). */
export const previewOffice = (uri: string, preferPdf = false) => invoke<OfficeView>("preview_office", { uri, preferPdf });
export const openWebPage = (url: string) => invoke<void>("open_web_page", { url });
export const freeSpace = (uri: string) => invoke<{ free: number; total: number } | null>("free_space", { uri });

export interface TrashedItem {
  original: string;
  trashed: string | null;
}

/** URI of `name` inside the folder at `dir`. */
export function childUri(dir: string, name: string): string {
  const bang = dir.startsWith("archive://") && !dir.includes("!/") ? "!" : "";
  return dir.replace(/\/+$/, "") + bang + "/" + encodeURIComponent(name);
}

/** Last path segment of a URI, decoded. */
export function uriName(uri: string): string {
  const trimmed = uri.replace(/\/+$/, "");
  return decodeURIComponent(trimmed.slice(trimmed.lastIndexOf("/") + 1));
}

/** URI of the folder containing `uri`. */
export function parentUri(uri: string): string {
  const trimmed = uri.replace(/\/+$/, "");
  const i = trimmed.lastIndexOf("/");
  const p = trimmed.slice(0, i);
  return p.endsWith("!") ? p.slice(0, -1) + "!/" : p.endsWith(":/") || p.endsWith(":") ? p + "/" : p;
}

export const isRemote = (uri: string) => !uri.startsWith("file:") && !uri.startsWith("cx:");

// ---------- folder sizes ----------

export interface SizeProgress {
  bytes: number;
  files: number;
  dirs: number;
  done: boolean;
}

export async function dirSize(uri: string, onProgress: (p: SizeProgress) => void): Promise<number> {
  return invoke<number>("dir_size", { uri, onProgress: channel(onProgress) });
}
export const cancelTask = (id: number) => invoke<void>("cancel_task", { id });

// ---------- previews ----------

function schemeUrl(scheme: string, path: string) {
  // Custom URI schemes are served as http://<scheme>.localhost on Windows and Android.
  const winLike = /Windows|Android/.test(navigator.userAgent);
  return winLike ? `http://${scheme}.localhost/${path}` : `${scheme}://localhost/${path}`;
}

/** URL the web view can load a file's bytes from (supports Range for media). */
export function fileUrl(uri: string): string {
  if (mock) return mock.fileUrl(uri);
  return schemeUrl("cxfile", encodeURIComponent(uri));
}

/** URL of a cached thumbnail; `version` (mtime) busts the browser cache. */
export function thumbUrl(uri: string, size: number, version: number | null): string {
  if (mock) return mock.thumbUrl(uri, size);
  return schemeUrl("cxthumb", `${size}/${version ?? 0}/${encodeURIComponent(uri)}`);
}

export interface TextPreview {
  text: string;
  truncated: boolean;
  encoding: string;
  language: string | null;
}
export const previewText = (uri: string, maxBytes = 512 * 1024) => invoke<TextPreview>("preview_text", { uri, maxBytes });

// ---------- transfers ----------

export type JobKind = "copy" | "move" | "delete" | "trash" | "extract" | "compress" | "send" | "receive";
export type ConflictPolicy = "ask" | "replace" | "skip" | "keepBoth" | "replaceIfNewer";
export type Resolution = "replace" | "skip" | "keepBoth" | "replaceIfNewer";
export type JobState = "queued" | "scanning" | "running" | "paused" | "waitingForConflict" | "done" | "failed" | "cancelled";

export interface JobRequest {
  kind: JobKind;
  sources: string[];
  dest?: string | null;
  conflict?: ConflictPolicy;
  verify?: boolean;
}

export interface JobSnapshot {
  id: number;
  kind: JobKind;
  state: JobState;
  sources: string[];
  dest: string | null;
  bytesDone: number;
  bytesTotal: number;
  filesDone: number;
  filesTotal: number;
  current: string | null;
  speed: number;
  eta: number | null;
  errors: { uri: string; message: string }[];
  conflict: Conflict | null;
  undo: UndoOp | null;
  startedAt: number;
}

export interface Conflict {
  id: number;
  source: Entry;
  sourceUri: string;
  dest: Entry;
  destUri: string;
}

/** Opaque description of how to reverse an operation (produced by the backend). */
export type UndoOp = { type: string; [k: string]: unknown };

export const submitJob = (req: JobRequest) => invoke<number>("transfer_submit", { req });
export const pauseJob = (id: number) => invoke<void>("transfer_pause", { id });
export const resumeJob = (id: number) => invoke<void>("transfer_resume", { id });
export const cancelJob = (id: number) => invoke<void>("transfer_cancel", { id });
export const resolveConflict = (id: number, conflictId: number, resolution: Resolution, applyToAll: boolean) =>
  invoke<void>("transfer_resolve", { id, conflictId, resolution, applyToAll });
export const listJobs = () => invoke<JobSnapshot[]>("transfer_list");
export const clearJobs = () => invoke<void>("transfer_clear");
export const undoOp = (op: UndoOp) => invoke<void>("undo", { op });

// ---------- search ----------

export interface SearchQuery {
  text: string;
  mode?: "substring" | "glob" | "regex";
  content?: string | null;
  kind?: "file" | "dir" | null;
  includeHidden?: boolean;
  maxResults?: number;
}

export interface SearchHit {
  uri: string;
  parent: string;
  relPath: string;
  entry: Entry;
  line?: number | null;
  snippet?: string | null;
}

export type SearchEvent = { type: "hits"; hits: SearchHit[] } | { type: "done"; scanned: number; elapsedMs: number; truncated: boolean };

export async function search(root: string, query: SearchQuery, onEvent: (e: SearchEvent) => void): Promise<number> {
  return invoke<number>("search_start", { root, query, onEvent: channel(onEvent) });
}

export interface FuzzyHit {
  index: number;
  score: number;
  positions: number[];
}

// ---------- remote servers ----------

export type Secret = { type: "none" } | { type: "password"; password: string } | { type: "key"; path: string; passphrase: string | null };
export interface Credentials {
  user: string;
  secret: Secret;
}
export const connectServer = (uri: string, credentials: Credentials | null, remember: boolean) => invoke<void>("connect_server", { uri, credentials, remember });
export const disconnectServer = (uri: string) => invoke<void>("disconnect_server", { uri });
export const trustHostKey = (uri: string, keyType: string, fingerprint: string) => invoke<void>("trust_host_key", { uri, keyType, fingerprint });
export const connections = () => invoke<string[]>("connections");

// ---------- discovery & peers ----------

export type DeviceKind = "mac" | "pc" | "linux" | "nas" | "phone" | "tablet" | "router" | "printer" | "unknown";

export interface Service {
  scheme: string;
  port: number;
  uri: string;
  label: string;
  source: string;
}

export interface Device {
  id: string;
  name: string;
  kind: DeviceKind;
  addresses: string[];
  hostname: string | null;
  sources: string[];
  tailnet: { online: boolean; os: string; owner: string; isSelf: boolean; dnsName: string; lastSeen: number | null } | null;
  services: Service[];
  shares: { name: string; uri: string }[];
  lastSeen: number;
}

export const devices = () => invoke<Device[]>("discovery_devices");
export const refreshDiscovery = () => invoke<void>("discovery_refresh");

export interface PeerShare {
  name: string;
  path: string;
  readOnly: boolean;
}

export interface PeerStatus {
  enabled: boolean;
  deviceId: string;
  name: string;
  port: number;
  shares: PeerShare[];
  trusted: { id: string; name: string; addedAt: number }[];
  tailnetAutoTrust: boolean;
}

export interface IncomingOffer {
  id: string;
  from: { id: string; name: string };
  files: { name: string; size: number }[];
  total: number;
}

export const peerStatus = () => invoke<PeerStatus>("peer_status");
export const peerSetEnabled = (enabled: boolean) => invoke<PeerStatus>("peer_set_enabled", { enabled });
export const peerSetShares = (shares: PeerShare[]) => invoke<PeerStatus>("peer_set_shares", { shares });
export const peerSetAutoTrust = (on: boolean) => invoke<PeerStatus>("peer_set_auto_trust", { on });
export const peerPairCode = () => invoke<string>("peer_pair_code");
export const peerPair = (address: string, code: string) => invoke<{ id: string; name: string }>("peer_pair", { address, code });
export const peerForget = (id: string) => invoke<PeerStatus>("peer_forget", { id });
export const peerSend = (device: string, uris: string[]) => invoke<string>("peer_send", { device, uris });
export const peerRespond = (offerId: string, accept: boolean, dest: string | null) => invoke<void>("peer_respond", { offerId, accept, dest });

// ---------- tags ----------

export const getTags = (uris: string[]) => invoke<Record<string, string[]>>("tags_get", { uris });
export const setTags = (uri: string, tags: string[]) => invoke<void>("tags_set", { uri, tags });
export const findTagged = (tag: string) => invoke<SearchHit[]>("tags_find", { tag });

// ---------- compare ----------

export type DiffKind = "leftOnly" | "rightOnly" | "newerLeft" | "newerRight" | "different" | "same";
export interface DiffItem {
  relPath: string;
  kind: DiffKind;
  left: Entry | null;
  right: Entry | null;
}
export const compareDirs = (left: string, right: string, byContent: boolean) => invoke<DiffItem[]>("compare_dirs", { left, right, byContent });

// ---------- app-wide events ----------

export type AppEvent =
  | { type: "job"; job: JobSnapshot }
  | { type: "devices"; devices: Device[] }
  | { type: "offer"; offer: IncomingOffer }
  | { type: "peer"; status: PeerStatus }
  | { type: "nav"; dir: "back" | "forward" }
  | { type: "command"; id: string }
  | { type: "task"; id: number; label: string; progress: number; done: boolean; error: string | null };

export async function subscribe(onEvent: (e: AppEvent) => void): Promise<void> {
  await invoke<void>("subscribe", { onEvent: channel(onEvent) });
}

// ---------- terminal ----------

export type TermEvent = { kind: "output"; data: string } | { kind: "exit"; code: number | null };
export async function termOpen(uri: string, cols: number, rows: number, onEvent: (e: TermEvent) => void): Promise<number> {
  return invoke<number>("term_open", { uri, cols, rows, onEvent: channel(onEvent) });
}
export const termWrite = (id: number, data: string) => invoke<void>("term_write", { id, data });
export const termResize = (id: number, cols: number, rows: number) => invoke<void>("term_resize", { id, cols, rows });
export const termClose = (id: number) => invoke<void>("term_close", { id });
export const termCwd = (id: number) => invoke<string | null>("term_cwd", { id });
