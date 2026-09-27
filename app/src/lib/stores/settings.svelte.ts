// User preferences, persisted in localStorage (they're per-machine UI
// conveniences; nothing here must survive a cleared profile).
import type { SortSpec } from "../sort";

export type ViewMode = "details" | "icons" | "columns" | "gallery";
export type Keymap = "explorer" | "commander";
export type Theme = "system" | "light" | "dark";

export interface Bookmark {
  name: string;
  uri: string;
}

export interface SavedServer {
  name: string;
  uri: string;
}

export interface SavedSearch {
  name: string;
  root: string;
  text: string;
}

export interface SavedWorkspace {
  name: string;
  dual: boolean;
  panes: { tabs: { uri: string; view: ViewMode }[]; active: number }[];
}

export interface SettingsData {
  sort: SortSpec;
  showHidden: boolean;
  compact: boolean;
  sidebarWidth: number;
  previewWidth: number;
  defaultView: ViewMode;
  iconSize: number;
  keymap: Keymap;
  theme: Theme;
  previewPane: boolean;
  dual: boolean;
  confirmTrash: boolean;
  confirmPermanentDelete: boolean;
  onboarded: boolean;
  bookmarks: Bookmark[];
  servers: SavedServer[];
  savedSearches: SavedSearch[];
  workspaces: SavedWorkspace[];
  recent: string[];
  session: SavedWorkspace | null;
  groupBy: "none" | "date" | "kind";
  /** The Full Disk Access banner was dismissed (macOS). */
  fdaDismissed: boolean;
}

const KEY = "cx.settings";

const defaults: SettingsData = {
  sort: { key: "name", desc: false },
  showHidden: false,
  compact: false,
  sidebarWidth: 232,
  previewWidth: 300,
  defaultView: "details",
  iconSize: 96,
  keymap: "explorer",
  theme: "system",
  previewPane: false,
  dual: false,
  confirmTrash: false,
  confirmPermanentDelete: true,
  onboarded: false,
  bookmarks: [],
  servers: [],
  savedSearches: [],
  workspaces: [],
  recent: [],
  session: null,
  groupBy: "none",
  fdaDismissed: false,
};

function load(): SettingsData {
  try {
    return { ...defaults, ...JSON.parse(localStorage.getItem(KEY) ?? "{}") };
  } catch {
    return { ...defaults };
  }
}

class Settings {
  data = $state<SettingsData>(load());
  #timer: ReturnType<typeof setTimeout> | undefined;

  constructor() {
    // Save shortly after any change, batching bursts (sidebar resizing etc).
    $effect.root(() => {
      $effect(() => {
        const snapshot = JSON.stringify(this.data);
        clearTimeout(this.#timer);
        this.#timer = setTimeout(() => {
          try {
            localStorage.setItem(KEY, snapshot);
          } catch {
            /* storage unavailable: settings last for this session only */
          }
        }, 250);
      });
    });
  }

  addRecent(uri: string) {
    if (uri.startsWith("cx:")) return;
    this.data.recent = [uri, ...this.data.recent.filter((u) => u !== uri)].slice(0, 30);
  }

  toggleBookmark(name: string, uri: string) {
    const has = this.data.bookmarks.some((b) => b.uri === uri);
    this.data.bookmarks = has ? this.data.bookmarks.filter((b) => b.uri !== uri) : [...this.data.bookmarks, { name, uri }];
    return !has;
  }

  reset() {
    this.data = { ...defaults, onboarded: true };
  }
}

export const settings = new Settings();
