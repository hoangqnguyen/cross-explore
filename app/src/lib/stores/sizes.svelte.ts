// Folder sizes computed on demand (Total Commander's Space / Alt+Shift+Enter).
import { dirSize, errorText } from "../api";
import { toasts } from "../toasts.svelte";

export interface SizeInfo {
  bytes: number;
  files: number;
  done: boolean;
}

class Sizes {
  map = $state.raw<ReadonlyMap<string, SizeInfo>>(new Map());

  get(uri: string) {
    return this.map.get(uri);
  }

  #set(uri: string, v: SizeInfo) {
    const m = new Map(this.map);
    m.set(uri, v);
    this.map = m;
  }

  compute(uri: string) {
    if (this.map.get(uri)?.done === false) return;
    this.#set(uri, { bytes: 0, files: 0, done: false });
    dirSize(uri, (p) => this.#set(uri, { bytes: p.bytes, files: p.files, done: p.done })).catch((e) => {
      const m = new Map(this.map);
      m.delete(uri);
      this.map = m;
      toasts.show(errorText(e), "error");
    });
  }
}

export const sizes = new Sizes();
