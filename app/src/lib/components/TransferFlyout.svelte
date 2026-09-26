<script lang="ts">
  // All transfer jobs with live progress. Speed history drives a sparkline.
  import { untrack } from "svelte";
  import { uriName, type JobSnapshot } from "../api";
  import { formatSize } from "../format";
  import { jobTitle, transfers } from "../stores/transfers.svelte";
  import { ws } from "../workspace.svelte";
  import Icon from "./Icon.svelte";

  const history = new Map<number, number[]>();
  let tick = $state(0);

  $effect(() => {
    const jobs = transfers.jobs;
    untrack(() => {
      for (const j of jobs) {
        if (j.state !== "running") continue;
        const h = history.get(j.id) ?? [];
        h.push(j.speed);
        if (h.length > 40) h.shift();
        history.set(j.id, h);
      }
      tick++;
    });
  });

  function spark(id: number) {
    void tick;
    const h = history.get(id) ?? [];
    if (h.length < 2) return "";
    const max = Math.max(...h, 1);
    return h.map((v, i) => `${(i / (h.length - 1)) * 100},${20 - (v / max) * 18}`).join(" ");
  }

  function eta(s: number | null) {
    if (s == null || !isFinite(s)) return "";
    if (s < 60) return `${Math.ceil(s)} s left`;
    if (s < 3600) return `${Math.ceil(s / 60)} min left`;
    return `${(s / 3600).toFixed(1)} h left`;
  }

  function status(j: JobSnapshot) {
    switch (j.state) {
      case "queued":
        return "Waiting…";
      case "scanning":
        return `Counting files… ${j.filesTotal.toLocaleString()}`;
      case "running":
        return `${formatSize(j.bytesDone)} of ${formatSize(j.bytesTotal)} · ${formatSize(j.speed)}/s · ${eta(j.eta)}`;
      case "paused":
        return `Paused · ${formatSize(j.bytesDone)} of ${formatSize(j.bytesTotal)}`;
      case "waitingForConflict":
        return "Waiting for your decision";
      case "done":
        return `${j.filesDone.toLocaleString()} ${j.filesDone === 1 ? "item" : "items"}${j.bytesTotal ? ` · ${formatSize(j.bytesTotal)}` : ""}${j.errors.length ? ` · ${j.errors.length} failed` : ""}`;
      case "failed":
        return j.errors[0]?.message ?? "Failed";
      case "cancelled":
        return "Cancelled";
    }
  }
</script>

{#if transfers.flyoutOpen}
  <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
  <div class="scrim" onclick={() => (transfers.flyoutOpen = false)}></div>
  <div class="flyout" role="dialog" aria-label="Transfers">
    <header>
      <strong>Transfers</strong>
      <span class="spacer"></span>
      {#if transfers.jobs.some((j) => ["done", "failed", "cancelled"].includes(j.state))}
        <button class="text" onclick={() => transfers.clearFinished()}>Clear finished</button>
      {/if}
    </header>
    <div class="list">
      {#each transfers.jobs as j (j.id)}
        {@const pct = j.bytesTotal ? (j.bytesDone / j.bytesTotal) * 100 : j.filesTotal ? (j.filesDone / j.filesTotal) * 100 : 0}
        <div class="job" class:done={j.state === "done"} class:failed={j.state === "failed"}>
          <div class="top">
            <div class="title">
              <strong>{jobTitle(j)}</strong>
              {#if j.dest}<button class="dest" onclick={() => ws.activeTab.navigate(j.dest!)}>to {uriName(j.dest) || j.dest}</button>{/if}
            </div>
            {#if j.state === "running" || j.state === "scanning"}
              <button class="icon" title="Pause" onclick={() => transfers.pause(j.id)}><Icon name="pause" size={14} /></button>
            {:else if j.state === "paused"}
              <button class="icon" title="Resume" onclick={() => transfers.resume(j.id)}><Icon name="play" size={14} /></button>
            {/if}
            {#if !["done", "failed", "cancelled"].includes(j.state)}
              <button class="icon" title="Cancel" onclick={() => transfers.cancel(j.id)}><Icon name="close" size={13} /></button>
            {/if}
          </div>
          {#if j.state !== "done" && j.state !== "cancelled"}
            <div class="bar"><span style:width="{pct}%" class:paused={j.state === "paused"}></span></div>
          {/if}
          <div class="meta">
            <span>{status(j)}</span>
            {#if j.state === "running"}
              <svg class="spark" viewBox="0 0 100 20" preserveAspectRatio="none" aria-hidden="true"><polyline points={spark(j.id)} fill="none" stroke="var(--accent)" stroke-width="1.5" vector-effect="non-scaling-stroke" /></svg>
            {/if}
          </div>
          {#if j.current && j.state === "running"}<div class="current">{j.current}</div>{/if}
        </div>
      {:else}
        <div class="empty">No transfers yet. Copy, move or drop files to see them here.</div>
      {/each}
    </div>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 60;
  }
  .flyout {
    position: fixed;
    z-index: 61;
    top: calc(var(--titlebar-h) + 4px);
    right: 12px;
    width: 380px;
    max-height: min(520px, calc(100vh - 80px));
    display: flex;
    flex-direction: column;
    border-radius: 10px;
    background: var(--flyout);
    backdrop-filter: blur(30px) saturate(1.4);
    -webkit-backdrop-filter: blur(30px) saturate(1.4);
    box-shadow: var(--shadow-flyout);
    animation: pop 0.14s var(--ease);
  }
  @keyframes pop {
    from {
      opacity: 0;
      transform: translateY(-6px);
    }
  }
  header {
    display: flex;
    align-items: center;
    padding: 12px 14px 8px;
  }
  .spacer {
    flex: 1;
  }
  .text {
    color: var(--accent);
    font-size: 12px;
  }
  .list {
    overflow-y: auto;
    padding: 0 8px 8px;
  }
  .job {
    padding: 10px 8px;
    border-radius: var(--radius);
  }
  .job + .job {
    border-top: 1px solid var(--stroke);
  }
  .top {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .title {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .title strong {
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dest {
    align-self: flex-start;
    font-size: 11.5px;
    color: var(--text-3);
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dest:hover {
    color: var(--accent);
  }
  .icon {
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border-radius: 4px;
    color: var(--text-2);
  }
  .icon:hover {
    background: var(--hover);
  }
  .bar {
    height: 4px;
    margin: 8px 0 6px;
    border-radius: 2px;
    background: var(--stroke-strong);
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 0.2s linear;
  }
  .bar span.paused {
    background: var(--text-3);
  }
  .meta {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 11.5px;
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }
  .meta span {
    flex: 1;
  }
  .spark {
    width: 70px;
    height: 16px;
  }
  .current {
    margin-top: 2px;
    font-size: 11px;
    color: var(--text-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .job.failed strong {
    color: var(--danger);
  }
  .empty {
    padding: 24px 12px;
    text-align: center;
    color: var(--text-3);
    font-size: 12px;
  }
</style>
