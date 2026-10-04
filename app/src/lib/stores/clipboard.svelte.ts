// The app's file clipboard (cut/copy between tabs, panes and servers).
class FileClipboard {
  uris = $state.raw<string[]>([]);
  mode = $state<"copy" | "cut">("copy");
  /** Every visible row asks `isCut`; a set keeps that O(1) after cutting thousands. */
  #cut = $derived(this.mode === "cut" ? new Set(this.uris) : null);

  set(uris: string[], mode: "copy" | "cut") {
    this.uris = uris;
    this.mode = mode;
  }

  clear() {
    this.uris = [];
  }

  isCut(uri: string) {
    return this.#cut?.has(uri) ?? false;
  }
}

export const clipboard = new FileClipboard();
