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

export interface Crumb {
  label: string;
  uri: string;
  icon: "home" | "drive" | "folder";
}

export interface LocationInfo {
  uri: string;
  scheme: string;
  display: string;
  name: string;
  parent: string | null;
  crumbs: Crumb[];
}

export interface Capabilities {
  liveWatch: boolean;
  polling: boolean;
  serverCopy: boolean;
  trash: boolean;
  posix: boolean;
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

export interface Places {
  platform: string;
  translucent: boolean;
  home: Place;
  favorites: Place[];
  volumes: Volume[];
}

export interface CxError {
  kind: string;
  message: string;
}

export function errorText(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) {
    const err = e as CxError;
    switch (err.kind) {
      case "notFound":
        return `Can't find "${err.message}". It may have been moved or deleted.`;
      case "permissionDenied":
        return `You don't have permission to access "${err.message}".`;
      case "alreadyExists":
        return `"${err.message}" already exists.`;
      case "invalidName":
        return `"${err.message}" isn't a valid name.`;
      default:
        return err.message;
    }
  }
  return String(e);
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

export async function watchDir(uri: string, onChange: (changes: Change[]) => void): Promise<number> {
  return invoke<number>("watch_dir", { uri, onChange: channel(onChange) });
}

export const unwatchDir = (id: number) => invoke<void>("unwatch_dir", { id });
export const createFolder = (uri: string, name?: string) => invoke<Entry>("create_folder", { uri, name });
export const renameEntry = (uri: string, from: string, to: string) => invoke<Entry>("rename_entry", { uri, from, to });
export const trashEntries = (uri: string, names: string[]) => invoke<void>("trash_entries", { uri, names });
export const openEntry = (uri: string) => invoke<void>("open_entry", { uri });
export const places = () => invoke<Places>("places");
export const freeSpace = (uri: string) => invoke<{ free: number; total: number } | null>("free_space", { uri });

/** URI of `name` inside the folder at `dir`. */
export function childUri(dir: string, name: string): string {
  return dir.replace(/\/+$/, "") + "/" + encodeURIComponent(name);
}
