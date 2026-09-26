// Live view of transfer jobs (fed by backend events) and the undo stack.
import { cancelJob, clearJobs, errorText, pauseJob, resolveConflict, resumeJob, submitJob, undoOp, uriName, type JobRequest, type JobSnapshot, type Resolution, type UndoOp } from "../api";
import { toasts } from "../toasts.svelte";

export interface UndoEntry {
  label: string;
  op: UndoOp;
}

const verbs: Record<string, [string, string]> = {
  copy: ["Copying", "Copied"],
  move: ["Moving", "Moved"],
  trash: ["Moving to Trash", "Moved to Trash"],
  delete: ["Deleting", "Deleted"],
  send: ["Sending", "Sent"],
  receive: ["Receiving", "Received"],
  extract: ["Extracting", "Extracted"],
  compress: ["Compressing", "Compressed"],
};

export function jobTitle(j: Pick<JobSnapshot, "kind" | "sources" | "state">) {
  const [ing, ed] = verbs[j.kind] ?? ["Working", "Done"];
  const what = j.sources.length === 1 ? `“${uriName(j.sources[0])}”` : `${j.sources.length} items`;
  return `${j.state === "done" ? ed : ing} ${what}`;
}

class Transfers {
  jobs = $state.raw<JobSnapshot[]>([]);
  undoStack = $state.raw<UndoEntry[]>([]);
  flyoutOpen = $state(false);

  active = $derived(this.jobs.filter((j) => !["done", "failed", "cancelled"].includes(j.state)));
  conflicts = $derived(this.jobs.filter((j) => j.state === "waitingForConflict" && j.conflict));
  totalSpeed = $derived(this.active.reduce((a, j) => a + (j.state === "running" ? j.speed : 0), 0));
  progress = $derived.by(() => {
    const a = this.active;
    const total = a.reduce((s, j) => s + j.bytesTotal, 0);
    return total ? a.reduce((s, j) => s + j.bytesDone, 0) / total : 0;
  });

  #announced = new Set<number>();

  onJob(job: JobSnapshot) {
    const i = this.jobs.findIndex((j) => j.id === job.id);
    this.jobs = i < 0 ? [job, ...this.jobs] : this.jobs.map((j) => (j.id === job.id ? job : j));
    if (["done", "failed", "cancelled"].includes(job.state) && !this.#announced.has(job.id)) {
      this.#announced.add(job.id);
      if (job.state === "done" && job.undo) this.pushUndo(jobTitle(job), job.undo);
      if (job.errors.length) toasts.show(`${job.errors.length} ${job.errors.length === 1 ? "item" : "items"} couldn't be processed: ${job.errors[0].message}`, "error", 6000);
      else if (job.state === "failed") toasts.show(`${jobTitle(job)} failed`, "error");
    }
  }

  async submit(req: JobRequest) {
    try {
      return await submitJob({ conflict: "ask", ...req });
    } catch (e) {
      toasts.show(errorText(e), "error");
      return null;
    }
  }

  pause = (id: number) => pauseJob(id);
  resume = (id: number) => resumeJob(id);
  cancel = (id: number) => cancelJob(id);
  resolve = (id: number, conflictId: number, r: Resolution, all: boolean) => resolveConflict(id, conflictId, r, all);

  clearFinished() {
    void clearJobs().catch(() => {});
    this.jobs = this.jobs.filter((j) => !["done", "failed", "cancelled"].includes(j.state));
  }

  pushUndo(label: string, op: UndoOp) {
    this.undoStack = [...this.undoStack.slice(-49), { label, op }];
  }

  async undo() {
    const last = this.undoStack.at(-1);
    if (!last) {
      toasts.show("Nothing to undo");
      return;
    }
    this.undoStack = this.undoStack.slice(0, -1);
    try {
      // Batches (multi-rename) undo in reverse order.
      const ops = last.op.type === "batch" ? [...(last.op.ops as UndoOp[])].reverse() : [last.op];
      for (const op of ops) await undoOp(op);
      toasts.show(`Undid: ${last.label}`);
    } catch (e) {
      toasts.show(`Couldn't undo: ${errorText(e)}`, "error");
    }
  }
}

export const transfers = new Transfers();
