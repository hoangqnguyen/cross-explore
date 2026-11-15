// Editors and terminals installed on this machine, for "Open in ….".
import { devApps as loadDevApps, errorText, openInApp, type DevApp } from "./api";
import { toasts } from "./toasts.svelte";

/** Every app we know how to launch, installed or not (the palette lists the installed ones). */
export const KNOWN_DEV_APPS: { id: string; name: string; kind: "editor" | "terminal" }[] = [
  { id: "vscode", name: "VS Code", kind: "editor" },
  { id: "cursor", name: "Cursor", kind: "editor" },
  { id: "windsurf", name: "Windsurf", kind: "editor" },
  { id: "zed", name: "Zed", kind: "editor" },
  { id: "sublime", name: "Sublime Text", kind: "editor" },
  { id: "nova", name: "Nova", kind: "editor" },
  { id: "bbedit", name: "BBEdit", kind: "editor" },
  { id: "textmate", name: "TextMate", kind: "editor" },
  { id: "terminal", name: "Terminal", kind: "terminal" },
  { id: "iterm", name: "iTerm", kind: "terminal" },
  { id: "kitty", name: "kitty", kind: "terminal" },
  { id: "warp", name: "Warp", kind: "terminal" },
  { id: "ghostty", name: "Ghostty", kind: "terminal" },
];

class DevApps {
  list = $state.raw<DevApp[]>([]);

  async init() {
    try {
      this.list = await loadDevApps();
    } catch {
      /* not available here */
    }
  }

  has(id: string) {
    return this.list.some((a) => a.id === id);
  }

  open(uri: string, app: string) {
    openInApp(uri, app).catch((e) => toasts.show(errorText(e), "error"));
  }
}

export const devApps = new DevApps();
