<script lang="ts">
  // Settings → Previews: how previews behave, and the ways remembered for
  // file types Cross Explore doesn't know (set from the preview pane).
  import Icon from "../components/Icon.svelte";
  import Toggle from "../components/Toggle.svelte";
  import { PREVIEW_AS } from "../previewKinds";
  import { settings, type PreviewAs } from "../stores/settings.svelte";

  let s = settings.data;
  let types = $derived(Object.entries(settings.data.previewAs).sort(([a], [b]) => a.localeCompare(b)));
  let newExt = $state("");
  let newAs = $state<PreviewAs>("text");

  function setAs(ext: string, as: PreviewAs) {
    settings.data.previewAs = { ...settings.data.previewAs, [ext]: as };
  }

  function remove(ext: string) {
    const next = { ...settings.data.previewAs };
    delete next[ext];
    settings.data.previewAs = next;
  }

  function add(e: SubmitEvent) {
    e.preventDefault();
    const ext = newExt.trim().replace(/^\.+/, "").toLowerCase();
    if (!ext || /[\s/\\]/.test(ext)) return;
    setAs(ext, newAs);
    newExt = "";
  }
</script>

<div class="group-title">Behavior</div>
<div class="card">
  <div class="srow">
    <div class="row-text"><span class="row-label">Wait before previewing</span><span class="row-hint">In the preview pane and gallery, while the selection moves</span></div>
    <select bind:value={s.previewDelayMs}>
      <option value={0}>Don't wait</option>
      <option value={120}>A moment (0.1 s)</option>
      <option value={300}>Until I stop (0.3 s)</option>
    </select>
  </div>
  <div class="srow">
    <div class="row-text"><span class="row-label">Text read for the preview pane</span><span class="row-hint">Quick Look always reads up to 1 MB</span></div>
    <select bind:value={s.previewTextKB}>
      <option value={64}>64 KB</option>
      <option value={256}>256 KB</option>
      <option value={1024}>1 MB</option>
    </select>
  </div>
  <div class="srow">
    <div class="row-text"><span class="row-label">HTML files show as</span></div>
    <select bind:value={s.previewHtml}>
      <option value="page">The page</option>
      <option value="code">Code</option>
    </select>
  </div>
  <div class="srow">
    <div class="row-text"><span class="row-label">Show Markdown formatted</span><span class="row-hint">Off shows the source</span></div>
    <Toggle bind:checked={s.previewMarkdown} />
  </div>
  <div class="srow">
    <div class="row-text"><span class="row-label">Play video and audio in Quick Look right away</span></div>
    <Toggle bind:checked={s.previewAutoplay} />
  </div>
  <div class="srow">
    <div class="row-text"><span class="row-label">Show what's inside folders</span><span class="row-hint">Folders and archives preview as a list</span></div>
    <Toggle bind:checked={s.previewFolders} />
  </div>
</div>

<div class="group-title">File types</div>
<div class="card">
  {#each types as [ext, as] (ext)}
    <div class="srow">
      <div class="row-text"><span class="row-label">.{ext}</span></div>
      <select value={as} onchange={(e) => setAs(ext, (e.currentTarget as HTMLSelectElement).value as PreviewAs)} aria-label="Preview .{ext} files as">
        {#each PREVIEW_AS as p (p.kind)}<option value={p.kind}>{p.label}</option>{/each}
      </select>
      <button type="button" class="remove" aria-label="Forget .{ext}" onclick={() => remove(ext)}><Icon name="close" size={12} /></button>
    </div>
  {:else}
    <p class="empty">None yet. For a file type Cross Explore doesn't know, pick “Preview as” in the preview pane and Remember it, or add one here.</p>
  {/each}
  <form class="srow add" onsubmit={add}>
    <input type="text" placeholder="Extension, e.g. log2" bind:value={newExt} spellcheck="false" aria-label="Extension" />
    <select bind:value={newAs} aria-label="Preview as">
      {#each PREVIEW_AS as p (p.kind)}<option value={p.kind}>{p.label}</option>{/each}
    </select>
    <button type="submit" class="add-btn" disabled={!newExt.trim()}>Add</button>
  </form>
</div>

<style>
  .group-title {
    margin: 0 2px 6px;
    font-size: 11.5px;
    font-weight: 600;
    letter-spacing: 0.02em;
    text-transform: uppercase;
    color: var(--text-3);
  }
  .card {
    margin-bottom: 20px;
    border-radius: var(--radius-lg);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke);
    overflow: hidden;
  }
  .srow {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 42px;
    padding: 8px 14px;
  }
  .srow + .srow,
  .empty + .srow {
    border-top: 1px solid var(--stroke);
  }
  .row-text {
    display: flex;
    flex-direction: column;
    gap: 1px;
    flex: 1;
    min-width: 0;
  }
  .row-label {
    font-size: 13px;
    color: var(--text);
  }
  .row-hint {
    font-size: 11.5px;
    color: var(--text-3);
    line-height: 1.4;
  }
  select,
  input {
    flex: none;
    height: 28px;
    padding: 0 8px;
    border: 0;
    border-radius: var(--radius);
    background: var(--layer);
    box-shadow: inset 0 0 0 1px var(--stroke-strong);
    color: var(--text);
    font: inherit;
    font-size: 12.5px;
  }
  input {
    flex: 1;
    min-width: 0;
    outline: none;
  }
  .remove {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    border-radius: var(--radius);
    color: var(--text-3);
  }
  .remove:hover {
    background: var(--hover);
    color: var(--text);
  }
  .add-btn {
    height: 28px;
    padding: 0 12px;
    border-radius: var(--radius);
    background: var(--accent);
    color: var(--accent-text);
    font-size: 12.5px;
  }
  .add-btn:disabled {
    opacity: 0.5;
  }
  .empty {
    margin: 0;
    padding: 12px 14px;
    font-size: 12.5px;
    color: var(--text-3);
  }
</style>
