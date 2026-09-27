<script lang="ts">
  import { byId, enabled, run, shortcut } from "../commands.svelte";
  import { menu, type MenuItem } from "../menu.svelte";
  import type { SortKey } from "../sort";
  import { settings } from "../stores/settings.svelte";
  import { ws } from "../workspace.svelte";
  import Icon, { type IconName } from "./Icon.svelte";

  let tab = $derived(ws.activeTab);

  const sortNames: Record<SortKey, string> = { name: "Name", modified: "Date modified", type: "Type", size: "Size" };

  function item(id: string, label?: string): MenuItem {
    const c = byId.get(id)!;
    return { label: label ?? c.label, icon: c.icon, shortcut: shortcut(id), disabled: !enabled(id), action: () => run(id) };
  }

  function sortMenu(e: MouseEvent) {
    const s = settings.data.sort;
    const items: MenuItem[] = (Object.keys(sortNames) as SortKey[]).map((key) => ({ label: sortNames[key], checked: s.key === key, action: () => ws.setSort({ key, desc: s.desc }) }));
    items.push({ separator: true });
    items.push({ label: "Ascending", checked: !s.desc, action: () => ws.setSort({ ...s, desc: false }) });
    items.push({ label: "Descending", checked: s.desc, action: () => ws.setSort({ ...s, desc: true }) });
    menu.showBelow(items, e.currentTarget as HTMLElement);
  }

  function viewMenu(e: MouseEvent) {
    const s = settings.data;
    const view = (id: string, v: string): MenuItem => ({ ...item(id), checked: tab.view === v, icon: undefined });
    menu.showBelow(
      [
        view("view.details", "details"),
        view("view.icons", "icons"),
        view("view.columns", "columns"),
        view("view.gallery", "gallery"),
        { separator: true },
        { ...item("view.preview", "Preview pane"), checked: s.previewPane, icon: undefined },
        { ...item("pane.dual", "Dual pane"), checked: s.dual, icon: undefined },
        { separator: true },
        { ...item("view.hidden", "Hidden items"), checked: s.showHidden, icon: undefined },
        { label: "Compact spacing", checked: s.compact, action: () => (s.compact = !s.compact) },
        { label: "Alternating row colors", checked: s.stripes, action: () => (s.stripes = !s.stripes) },
        { ...item("view.pathBar", "Path bar"), checked: s.pathBar, icon: undefined },
        ...(tab.expanded.size ? [{ separator: true } as MenuItem, item("view.collapseAll")] : []),
      ],
      e.currentTarget as HTMLElement,
    );
  }

  function moreMenu(e: MouseEvent) {
    menu.showBelow(
      [
        item("sel.all"),
        item("sel.none"),
        item("sel.invert"),
        item("sel.pattern"),
        { separator: true },
        item("file.copyTo"),
        item("file.moveTo"),
        { separator: true },
        item("file.multiRename"),
        item("file.compress"),
        item("file.extract"),
        item("file.calcSize"),
        item("file.compareDirs"),
        item("file.diff"),
        { separator: true },
        item("view.search"),
        item("view.searchContent"),
        { separator: true },
        item("file.copyPath"),
        item("file.reveal"),
        item("file.terminal"),
        item("bookmark.toggle", settings.data.bookmarks.some((b) => b.uri === tab.dirUri) ? "Remove from Favorites" : "Add to Favorites"),
        { separator: true },
        item("net.connect"),
        item("file.sendTo"),
        item("app.saveWorkspace"),
      ],
      e.currentTarget as HTMLElement,
    );
  }

  const actions: { id: string; icon: IconName }[] = [
    { id: "edit.cut", icon: "cut" },
    { id: "edit.copy", icon: "copy" },
    { id: "edit.paste", icon: "paste" },
    { id: "file.rename", icon: "rename" },
    { id: "file.sendTo", icon: "send" },
    { id: "file.trash", icon: "trash" },
  ];
</script>

<div class="commands" role="toolbar">
  <button class="labeled" disabled={!enabled("file.newFolder")} title="New folder ({shortcut('file.newFolder')})" onclick={() => run("file.newFolder")}>
    <Icon name="newFolder" size={18} /> New folder
  </button>
  <div class="divider"></div>
  {#each actions as a (a.id)}
    <button disabled={!enabled(a.id)} title="{byId.get(a.id)!.label}{shortcut(a.id) ? ` (${shortcut(a.id)})` : ''}" aria-label={byId.get(a.id)!.label} onclick={() => run(a.id)}>
      <Icon name={a.icon} size={18} />
    </button>
  {/each}
  <div class="divider"></div>
  <button class="labeled" onclick={sortMenu}><Icon name="sort" size={18} /> Sort <Icon name="chevronDown" size={12} /></button>
  <button class="labeled" onclick={viewMenu}><Icon name={tab.view === "icons" ? "grid" : tab.view === "columns" ? "columns" : tab.view === "gallery" ? "gallery" : "rows"} size={18} /> View <Icon name="chevronDown" size={12} /></button>
  <button title="More" aria-label="More" onclick={moreMenu}><Icon name="more" size={18} stroke={2.4} /></button>
  <span class="spacer"></span>
  <button class:on={settings.data.dual} title="Dual pane ({shortcut('pane.dual')})" aria-label="Dual pane" onclick={() => run("pane.dual")}><Icon name="columns" size={18} /></button>
  <button class:on={settings.data.previewPane} title="Preview pane ({shortcut('view.preview')})" aria-label="Preview pane" onclick={() => run("view.preview")}><Icon name="sidebarRight" size={18} /></button>
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
    overflow: hidden;
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
    flex: none;
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
  button.on {
    background: var(--accent-soft);
  }
  button.on :global(svg) {
    color: var(--accent);
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
    flex: none;
  }
  .spacer {
    flex: 1;
  }
</style>
