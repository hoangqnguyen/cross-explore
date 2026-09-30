// User preferences, persisted in localStorage (they're per-machine UI
// conveniences; nothing here must survive a cleared profile).
import type { SortSpec } from "../sort";

export type ViewMode = "details" | "icons" | "columns" | "gallery";
export type Keymap = "finder" | "explorer" | "commander";
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
  /** Column view (Miller columns): width shared by every folder column. */
  columnWidth: number;
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
  /** Destinations recently used with Copy to… / Move to…. */
  recentDestinations: string[];
  keymapV2?: boolean;
  /** Alternating row colors in the Details view. */
  stripes: boolean;
  /** Path of the selected item in the status bar (Finder's path bar). */
  pathBar: boolean;
  /** The Full Disk Access banner was dismissed (macOS). */
  fdaDismissed: boolean;
  /**
   * Typing letters in a file list: jump to the matching item (Finder,
   * Explorer) or filter the list (Total Commander). Unset follows the keymap.
   */
  typeAction?: "select" | "filter";
}

/** What typing in a file list does, given the settings. */
export function typeAction(d: SettingsData): "select" | "filter" {
  return d.typeAction ?? (d.keymap === "commander" ? "filter" : "select");
}

const KEY = "cx.settings";

const defaults: SettingsData = {
  sort: { key: "name", desc: false },
  showHidden: false,
  compact: false,
  sidebarWidth: 232,
  previewWidth: 300,
  columnWidth: 240,
  defaultView: "details",
  iconSize: 96,
  keymap: /Mac|iPhone|iPad/.test(typeof navigator === "undefined" ? "" : navigator.platform) ? "finder" : "explorer",
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
  stripes: true,
  recentDestinations: [],
  pathBar: true,
};

function load(): SettingsData {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    // Before Finder got its own keymap, "explorer" meant Finder keys on a Mac.
    if (saved.keymap === "explorer" && !saved.keymapV2 && defaults.keymap === "finder") saved.keymap = "finder";
    saved.keymapV2 = true;
    return { ...defaults, ...saved };
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
