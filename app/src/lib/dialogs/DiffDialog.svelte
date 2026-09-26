<script lang="ts">
  import { errorText, previewText, uriName } from "../api";
  import { diffLines, type DiffLine } from "../diff";
  import { dialogs } from "../stores/dialogs.svelte";
  import Modal from "./Modal.svelte";

  let { left, right }: { left: string; right: string } = $props();
  let lines = $state<DiffLine[] | null>(null);
  let error = $state<string | null>(null);

  $effect(() => {
    Promise.all([previewText(left, 4 << 20), previewText(right, 4 << 20)])
      .then(([a, b]) => (lines = diffLines(a.text.split("\n"), b.text.split("\n"))))
      .catch((e) => (error = `${errorText(e)} — only text files can be compared line by line.`));
  });

  let stats = $derived(lines ? { add: lines.filter((l) => l.kind === "add").length, del: lines.filter((l) => l.kind === "del").length } : null);
</script>

<Modal title="Compare files" width={980}>
  <div class="heads">
    <span>{uriName(left)}</span>
    <span>{uriName(right)}</span>
  </div>
  {#if error}
    <p class="error">{error}</p>
  {:else if !lines}
    <p class="muted">Comparing…</p>
  {:else}
    <p class="muted">{stats!.del} removed · {stats!.add} added{stats!.add + stats!.del === 0 ? " — the files are identical" : ""}</p>
    <div class="diff">
      {#each lines as l, i (i)}
        <div class="line {l.kind}">
          <span class="no">{l.a != null ? l.a + 1 : ""}</span>
          <span class="txt left">{l.kind !== "add" ? l.text : ""}</span>
          <span class="no">{l.b != null ? l.b + 1 : ""}</span>
          <span class="txt right">{l.kind !== "del" ? l.text : ""}</span>
        </div>
      {/each}
    </div>
  {/if}
  {#snippet footer()}
    <button type="button" class="btn primary" onclick={() => dialogs.close(null)}>Close</button>
  {/snippet}
</Modal>

<style>
  .heads {
    display: grid;
    grid-template-columns: 1fr 1fr;
    font-weight: 600;
    margin-bottom: 6px;
  }
  .diff {
    height: min(60vh, 560px);
    overflow: auto;
    border-radius: var(--radius);
    box-shadow: inset 0 0 0 1px var(--stroke);
    font-family: ui-monospace, Menlo, Consolas, monospace;
    font-size: 12px;
    user-select: text;
    -webkit-user-select: text;
  }
  .line {
    display: grid;
    grid-template-columns: 44px 1fr 44px 1fr;
    min-height: 19px;
  }
  .no {
    padding: 0 8px;
    text-align: right;
    color: var(--text-3);
    background: var(--hover);
  }
  .txt {
    padding: 0 8px;
    white-space: pre-wrap;
    word-break: break-all;
  }
  .del .left {
    background: color-mix(in srgb, #ff3b30 16%, transparent);
  }
  .add .right {
    background: color-mix(in srgb, #34c759 18%, transparent);
  }
</style>
