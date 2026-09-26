export interface Toast {
  id: number;
  text: string;
  tone: "info" | "error";
}

let nextId = 1;

class Toasts {
  list = $state<Toast[]>([]);

  show(text: string, tone: Toast["tone"] = "info", ms = 4000) {
    const id = nextId++;
    this.list = [...this.list, { id, text, tone }];
    setTimeout(() => this.dismiss(id), ms);
  }

  dismiss(id: number) {
    this.list = this.list.filter((t) => t.id !== id);
  }
}

export const toasts = new Toasts();
