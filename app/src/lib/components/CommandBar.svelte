<script lang="ts">
  import { ws } from "../workspace.svelte";
  import { menu, type MenuItem } from "../menu.svelte";
  import { isMac, mod } from "../keys";
  import type { SortKey } from "../sort";
  import Icon from "./Icon.svelte";

  let tab = $derived(ws.active);
  let selCount = $derived(tab?.selectedEntries.length ?? 0);
  let writable = $derived(tab?.folder.status === "ready");

  const sortNames: Record<SortKey, string> = { name: "Name", modified: "Date modified", type: "Type", size: "Size" };

  function sortMenu(e: MouseEvent) {
    const s = ws.settings.sort;
    const items: MenuItem[] = (Object.keys(sortNames) as SortKey[]).map((key) => ({
      label: sortNames[key],
      checked: s.key === key,
      action: () => ws.setSort({ key, desc: s.desc }),
    }));
    items.push({ separator: true });
    items.push({ label: "Ascending", checked: !s.desc, action: () => ws.setSort({ ...s, desc: false }) });
    items.push({ label: "Descending", checked: s.desc, action: () => ws.setSort({ ...s, desc: true }) });
    menu.showBelow(items, e.currentTarget as HTMLElement);
  }

  function viewMenu(e: MouseEvent) {
    menu.showBelow(
      [
        { label: "Details", icon: "rows", checked: true, action: () => {} },
        { separator: true },
        { label: "Compact view", checked: ws.settings.compact, action: () => ws.toggle("compact") },
        { label: "Hidden items", checked: ws.settings.showHidden, shortcut: isMac ? "⌘⇧." : "Ctrl+H", action: () => ws.toggle("showHidden") },
      ],
      e.currentTarget as HTMLElement,
    );
  }
</script>

<div class="commands" role="toolbar">
  <button class="labeled" disabled={!writable} title="New folder ({mod}⇧N)" onclick={() => tab.newFolder()}>
    <Icon name="newFolder" size={18} /> New folder
  </button>
  <div class="divider"></div>
  <button disabled={selCount !== 1} title="Rename (F2)" aria-label="Rename" onclick={() => (tab.renaming = tab.selectedEntries[0].name)}>
    <Icon name="rename" size={18} />
  </button>
  <button disabled={!writable} title="Copy path" aria-label="Copy path" onclick={() => tab.copyPath()}>
    <Icon name="copy" size={18} />
  </button>
  <button disabled={selCount === 0} title={isMac ? "Move to Trash (⌘⌫)" : "Delete (Del)"} aria-label="Delete" onclick={() => tab.trashSelection()}>
    <Icon name="trash" size={18} />
  </button>
  <div class="divider"></div>
  <button class="labeled" onclick={sortMenu}>
    <Icon name="sort" size={18} /> Sort <Icon name="chevronDown" size={12} />
  </button>
  <button class="labeled" onclick={viewMenu}>
    <Icon name="view" size={18} /> View <Icon name="chevronDown" size={12} />
  </button>
</div>

<style>
  .commands {
    display: flex;
    align-items: center;
    gap: 2px;
    height: 44px;
    padding: 0 10px;
    background: var(--layer);
    border-bottom: 1px solid var(--stroke);
    flex: none;
  }
  button {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    min-width: 34px;
    height: 32px;
    padding: 0 8px;
    border-radius: var(--radius);
    color: var(--text);
    white-space: nowrap;
  }
  button :global(svg) {
    color: var(--text-2);
  }
  button:hover:not(:disabled) {
    background: var(--hover);
  }
  button:active:not(:disabled) {
    background: var(--pressed);
  }
  button:disabled {
    opacity: 0.4;
  }
  .labeled {
    padding: 0 10px;
  }
  .divider {
    width: 1px;
    height: 22px;
    margin: 0 6px;
    background: var(--stroke-strong);
  }
</style>
