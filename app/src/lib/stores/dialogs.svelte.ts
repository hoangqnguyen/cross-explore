// A single modal slot. `ask()` opens a dialog and resolves with its result
// (or null when dismissed), so flows read top to bottom.
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
  | "about";

interface Open {
  kind: DialogKind;
  props: Record<string, unknown>;
  resolve: (v: unknown) => void;
}

class Dialogs {
  current = $state.raw<Open | null>(null);
  #queue: Open[] = [];

  ask<T = unknown>(kind: DialogKind, props: Record<string, unknown> = {}): Promise<T | null> {
    return new Promise((resolve) => {
      const d: Open = { kind, props, resolve: resolve as (v: unknown) => void };
      if (this.current) this.#queue.push(d);
      else this.current = d;
    });
  }

  close(result: unknown = null) {
    const d = this.current;
    this.current = this.#queue.shift() ?? null;
    d?.resolve(result);
  }

  async confirm(title: string, message: string, ok = "OK", danger = false): Promise<boolean> {
    return (await this.ask<boolean>("confirm", { title, message, ok, danger })) === true;
  }

  prompt(title: string, label: string, value = "", ok = "OK"): Promise<string | null> {
    return this.ask<string>("prompt", { title, label, value, ok });
  }
}

export const dialogs = new Dialogs();
