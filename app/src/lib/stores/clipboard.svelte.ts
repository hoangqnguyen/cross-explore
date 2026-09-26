// The app's file clipboard (cut/copy between tabs, panes and servers).
class FileClipboard {
  uris = $state.raw<string[]>([]);
  mode = $state<"copy" | "cut">("copy");

  set(uris: string[], mode: "copy" | "cut") {
    this.uris = uris;
    this.mode = mode;
  }

  clear() {
    this.uris = [];
  }

  isCut(uri: string) {
    return this.mode === "cut" && this.uris.includes(uri);
  }
}

export const clipboard = new FileClipboard();
