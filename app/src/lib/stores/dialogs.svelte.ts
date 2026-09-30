// Modal dialogs, stacked: `ask()` opens one on top of whatever is showing
// (Settings → "Pair with a device…" opens over Settings) and resolves with
// its result (or null when dismissed), so flows read top to bottom.
export type DialogKind =
  | "connect"
  | "signIn"
  | "hostKey"
  | "conflict"
  | "confirm"
  | "prompt"
  | "settings"
  | "onboarding"
  | "multiRename"
  | "selectPattern"
  | "sendTo"
  | "offer"
  | "diff"
  | "pair"
  | "tags"
  | "about"
  | "destination"
  | "cloud";

interface Open {
  kind: DialogKind;
  props: Record<string, unknown>;
  resolve: (v: unknown) => void;
}

class Dialogs {
  /** Open dialogs, bottom first; only the top one is interactive. */
  stack = $state.raw<Open[]>([]);

  /** The dialog on top (the one taking input), if any. */
  get current(): Open | null {
    return this.stack.at(-1) ?? null;
  }

  ask<T = unknown>(kind: DialogKind, props: Record<string, unknown> = {}): Promise<T | null> {
    return new Promise((resolve) => {
      this.stack = [...this.stack, { kind, props, resolve: resolve as (v: unknown) => void }];
    });
  }

  /** Close the top dialog with `result`. */
  close(result: unknown = null) {
    const d = this.stack.at(-1);
    if (!d) return;
    this.stack = this.stack.slice(0, -1);
    d.resolve(result);
  }

  async confirm(title: string, message: string, ok = "OK", danger = false): Promise<boolean> {
    return (await this.ask<boolean>("confirm", { title, message, ok, danger })) === true;
  }

  prompt(title: string, label: string, value = "", ok = "OK", secret = false, selectAll = false): Promise<string | null> {
    return this.ask<string>("prompt", { title, label, value, ok, secret, selectAll });
  }
}

export const dialogs = new Dialogs();
