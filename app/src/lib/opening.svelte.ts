// Opening a file on a server downloads it first, which can take a while:
// show how far it's got (and a way to stop it) instead of seeming to hang.
import { cancelOpen, errorText, openEntry, openEntryWith, uriName, type OpenProgress } from "./api";
import { toasts } from "./toasts.svelte";

export interface Opening {
  id: number;
  name: string;
  /** Bytes so far and in all; total is 0 until the server has said. */
  done: number;
  total: number;
  /** Bytes per second, smoothed. */
  speed: number;
}

/** Quick opens (small files, already downloaded) show nothing at all. */
const SHOW_AFTER_MS = 300;

let nextId = 1;

class Openings {
  list = $state<Opening[]>([]);

  cancel(id: number) {
    void cancelOpen(id);
    this.list = this.list.filter((o) => o.id !== id);
  }

  /** Open `uri` (with `withApp`, or the default app), showing download progress. */
  async open(uri: string, withApp?: string) {
    const id = nextId++;
    const name = uriName(uri);
    let state: Opening = { id, name, done: 0, total: 0, speed: 0 };
    let shown = false;
    let last = { t: performance.now(), done: 0 };
    const timer = setTimeout(() => {
      shown = true;
      this.list = [...this.list, state];
    }, SHOW_AFTER_MS);
    const onProgress = (p: OpenProgress) => {
      const now = performance.now();
      const dt = (now - last.t) / 1000;
      const speed = dt > 0.05 ? (p.done - last.done) / dt : state.speed;
      if (dt > 0.05) last = { t: now, done: p.done };
      state = { ...state, done: p.done, total: p.total, speed: state.speed ? state.speed * 0.7 + speed * 0.3 : speed };
      if (shown) this.list = this.list.map((o) => (o.id === id ? state : o));
    };
    try {
      await (withApp ? openEntryWith(uri, withApp, id, onProgress) : openEntry(uri, id, onProgress));
    } catch (e) {
      // Cancel was pressed: nothing to report.
      if ((e as { kind?: string })?.kind !== "cancelled") toasts.show(errorText(e), "error");
    } finally {
      clearTimeout(timer);
      this.list = this.list.filter((o) => o.id !== id);
    }
  }
}

export const openings = new Openings();
