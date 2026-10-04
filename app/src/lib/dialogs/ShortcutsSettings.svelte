<script lang="ts">
  // Settings → Shortcuts: every command with the keys that run it in the
  // current keyboard style. Click + and press keys to add one; a key taken
  // from another command moves here. Changed commands keep their keys in
  // every style until reset.
  import { boundKeys, byId, commands, commandsOn, isCustomized, keyRecorder, resetKeys, setKeys, type Command } from "../commands.svelte";
  import Icon from "../components/Icon.svelte";
  import { comboOf, formatCombo, reachable } from "../keys";
  import { settings } from "../stores/settings.svelte";
  import { toasts } from "../toasts.svelte";

  let query = $state("");
  let onlyCustom = $state(false);
  let recording = $state<string | null>(null);

  let anyCustom = $derived(Object.keys(settings.data.keyBindings).length > 0);

  let groups = $derived.by(() => {
    const q = query.trim().toLowerCase();
    const out = new Map<string, { c: Command; keys: string[] }[]>();
    for (const c of commands) {
      if (onlyCustom && !isCustomized(c.id)) continue;
      const keys = boundKeys(c);
      if (q && !`${c.label} ${c.group}`.toLowerCase().includes(q) && !keys.some((k) => formatCombo(k).toLowerCase().includes(q))) continue;
      const list = out.get(c.group) ?? [];
      list.push({ c, keys });
      out.set(c.group, list);
    }
    return [...out];
  });

  const MODIFIERS = new Set(["Shift", "Control", "Alt", "Meta", "CapsLock", "Fn", "Dead"]);

  function stop() {
    recording = null;
    keyRecorder.active = false;
  }

  function add(id: string, combo: string) {
    const c = byId.get(id)!;
    const before = boundKeys(c);
    if (before.includes(combo)) return;
    const others = commandsOn(combo, id);
    setKeys(id, [...before, combo]);
    if (others.length) toasts.show(`${formatCombo(combo)} now runs “${c.label}” instead of “${others.map((o) => o.label).join("”, “")}”`);
    if (!reachable(combo)) toasts.show(`macOS keeps ${formatCombo(combo)} for itself, so it may never reach Cross Explore`, "error", 6000);
  }

  function remove(id: string, combo: string) {
    setKeys(
      id,
      boundKeys(byId.get(id)!).filter((k) => k !== combo),
    );
  }

  // While recording, every key press is the new shortcut (Escape cancels):
  // nothing else in the app sees it, the menu bar included.
  $effect(() => {
    const id = recording;
    if (!id) return;
    keyRecorder.active = true;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopImmediatePropagation();
      if (MODIFIERS.has(e.key) || e.isComposing) return;
      if (e.key === "Escape" && !e.shiftKey && !e.metaKey && !e.ctrlKey && !e.altKey) return stop();
      add(id, comboOf(e));
      stop();
    };
    const onDown = () => stop();
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("pointerdown", onDown, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("pointerdown", onDown, true);
      keyRecorder.active = false;
    };
  });
</script>

<div class="bar">
  <div class="search">
    <Icon name="search" size={13} />
    <input type="search" placeholder="Search commands or keys" bind:value={query} spellcheck="false" />
  </div>
  <label class="only"><input type="checkbox" bind:checked={onlyCustom} /> Changed only</label>
  <button type="button" class="link" disabled={!anyCustom} onclick={() => resetKeys()}>Reset all</button>
</div>
<p class="note">Shortcuts you change stay the same in every keyboard style. Keys in grey are taken by macOS before they reach the app.</p>

{#each groups as [group, rows] (group)}
  <div class="group-title">{group}</div>
  <div class="card">
    {#each rows as { c, keys } (c.id)}
      <div class="krow" class:custom={isCustomized(c.id)}>
        <span class="label">{c.label}</span>
        <div class="keys">
          {#each keys as k (k)}
            <span class="key" class:unreachable={!reachable(k)} title={reachable(k) ? undefined : "macOS takes this key before Cross Explore sees it"}>
              {formatCombo(k)}
              <button type="button" aria-label="Remove {formatCombo(k)}" onclick={() => remove(c.id, k)}><Icon name="close" size={9} /></button>
            </span>
          {/each}
          {#if recording === c.id}
            <span class="key recording">Press keys… <small>Esc cancels</small></span>
          {:else}
            <button type="button" class="add" title="Add a shortcut" aria-label="Add a shortcut for {c.label}" onclick={() => (recording = c.id)}><Icon name="plus" size={11} /></button>
          {/if}
          {#if isCustomized(c.id)}
            <button type="button" class="link reset" title="Back to the keyboard style's keys" onclick={() => resetKeys(c.id)}>Reset</button>
          {/if}
        </div>
      </div>
    {/each}
  </div>
{:else}
  <p class="note">No commands match.</p>
{/each}

<style>
  .bar {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 6px;
  }
  .search {
    display: flex;
    flex: 1;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 8px;
    border-radius: var(--radius);
    background: var(--layer);
    box-shadow: inset 0 0 0 1px var(--stroke-strong);
    color: var(--text-3);
  }
  .search input {
    flex: 1;
    min-width: 0;
    border: 0;
    outline: none;
    background: none;
    color: var(--text);
    font: inherit;
    font-size: 12.5px;
  }
  .only {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 12px;
    color: var(--text-2);
    white-space: nowrap;
  }
  .link {
    color: var(--accent);
    font-size: 12px;
    white-space: nowrap;
  }
  .link:disabled {
    color: var(--text-3);
  }
  .note {
    margin: 0 2px 14px;
    font-size: 11.5px;
    line-height: 1.4;
    color: var(--text-3);
  }
  .group-title {
    margin: 0 2px 6px;
    font-size: 11.5px;
    font-weight: 600;
    letter-spacing: 0.02em;
    text-transform: uppercase;
    color: var(--text-3);
  }
  .card {
    margin-bottom: 16px;
    border-radius: var(--radius-lg);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke);
    overflow: hidden;
  }
  .krow {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 36px;
    padding: 5px 10px 5px 14px;
  }
  .krow + .krow {
    border-top: 1px solid var(--stroke);
  }
  .label {
    flex: 1;
    min-width: 0;
    font-size: 13px;
    color: var(--text);
  }
  .krow.custom .label::after {
    content: "•";
    margin-left: 6px;
    color: var(--accent);
  }
  .keys {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    align-items: center;
    gap: 4px;
  }
  .key {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    height: 22px;
    padding: 0 3px 0 7px;
    border-radius: 5px;
    background: var(--layer);
    box-shadow: inset 0 0 0 1px var(--stroke-strong);
    font-size: 12px;
    color: var(--text);
    white-space: nowrap;
  }
  .key.unreachable {
    color: var(--text-3);
  }
  .key button {
    display: grid;
    place-items: center;
    width: 15px;
    height: 15px;
    border-radius: 3px;
    color: var(--text-3);
  }
  .key button:hover {
    background: var(--hover);
    color: var(--text);
  }
  .key.recording {
    padding: 0 8px;
    background: var(--accent-soft);
    box-shadow: inset 0 0 0 1px var(--accent);
    color: var(--accent);
  }
  .key.recording small {
    color: var(--text-3);
  }
  .add {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border-radius: 5px;
    color: var(--text-3);
  }
  .add:hover {
    background: var(--hover);
    color: var(--text);
  }
  .reset {
    margin-left: 4px;
  }
</style>
