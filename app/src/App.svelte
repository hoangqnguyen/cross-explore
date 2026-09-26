<script lang="ts">
  import { onMount } from "svelte";
  import { appWindow, inTauri } from "./lib/api";
  import { handleKey } from "./lib/commands.svelte";
  import AddressBar from "./lib/components/AddressBar.svelte";
  import CommandBar from "./lib/components/CommandBar.svelte";
  import CommandPalette from "./lib/components/CommandPalette.svelte";
  import ConflictDialog from "./lib/components/ConflictDialog.svelte";
  import Menu from "./lib/components/Menu.svelte";
  import PaneView from "./lib/components/PaneView.svelte";
  import PreviewPane from "./lib/components/PreviewPane.svelte";
  import QuickLook from "./lib/components/QuickLook.svelte";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import StatusBar from "./lib/components/StatusBar.svelte";
  import TitleBar from "./lib/components/TitleBar.svelte";
  import Toasts from "./lib/components/Toasts.svelte";
  import TransferFlyout from "./lib/components/TransferFlyout.svelte";
  import Dialogs from "./lib/dialogs/Dialogs.svelte";
  import { isTextInput } from "./lib/keys";
  import { dropDestAt } from "./lib/listing";
  import { dialogs } from "./lib/stores/dialogs.svelte";
  import { settings } from "./lib/stores/settings.svelte";
  import { ws } from "./lib/workspace.svelte";

  onMount(async () => {
    await ws.init();
    const root = document.documentElement;
    root.classList.add(`platform-${ws.platform}`);
    if (ws.places?.translucent) root.classList.add("translucent");
    // Show the window once the first frame is painted: no white flash.
    requestAnimationFrame(() => requestAnimationFrame(() => appWindow.show()));
    const testing = !inTauri && new URLSearchParams(location.search).has("path");
    const selftest = inTauri && (await import("@tauri-apps/api/core").then((c) => c.invoke("selftest_config")));
    if (!settings.data.onboarded && !testing && !selftest) void dialogs.ask("onboarding");
    if (inTauri) {
      void listenForOsDrops();
      void import("./lib/selftest").then((m) => m.selftest());
    }
  });

  // Files dropped in from Finder / Explorer: copy them where they landed.
  async function listenForOsDrops() {
    const { getCurrentWebview } = await import("@tauri-apps/api/webview");
    await getCurrentWebview().onDragDropEvent((ev) => {
      if (ev.payload.type !== "drop" || !ev.payload.paths.length) return;
      const scale = window.devicePixelRatio || 1;
      const dest = dropDestAt(ev.payload.position.x / scale, ev.payload.position.y / scale) ?? ws.activeTab.dirUri;
      const uris = ev.payload.paths.map((p) => "file://" + p.replaceAll("\\", "/").replace(/^\/?/, "/").split("/").map(encodeURIComponent).join("/").replace(/^\/%3A/, "/"));
      void ws.transfer(
        uris.map((u) => u.replace(/^file:\/\/\/([A-Za-z])%3A/, "file:///$1:")),
        dest,
        false,
      );
    });
  }

  $effect(() => {
    const root = document.documentElement;
    root.classList.toggle("compact", settings.data.compact);
    root.classList.toggle("light", settings.data.theme === "light");
    root.classList.toggle("dark", settings.data.theme === "dark");
  });

  $effect(() => {
    const title = ws.ready ? ws.activeTab?.folder.info?.name : null;
    if (title) void appWindow.setTitle(`${title} — Cross Explore`);
  });

  function onkeydown(e: KeyboardEvent) {
    if (dialogs.current) {
      // Escape always dismisses, even if focus drifted out of the dialog.
      if (e.key === "Escape") dialogs.close(null);
      return;
    }
    handleKey(e);
  }

  // Mouse back/forward buttons.
  function onmouseup(e: MouseEvent) {
    if (e.button === 3) ws.activeTab?.back();
    if (e.button === 4) ws.activeTab?.forward();
  }
</script>

<svelte:window {onkeydown} {onmouseup} oncontextmenu={(e) => !isTextInput(e.target) && e.preventDefault()} />

{#if ws.ready && ws.activeTab}
  <div class="window">
    <TitleBar />
    {#if !ws.dual}<AddressBar tab={ws.activeTab} />{/if}
    <CommandBar />
    <div class="body">
      <Sidebar />
      <div class="panes">
        <PaneView pane={ws.panes[0]} />
        {#if ws.dual}<PaneView pane={ws.panes[1]} />{/if}
      </div>
      {#if settings.data.previewPane}<PreviewPane />{/if}
    </div>
    <StatusBar />
  </div>
  <QuickLook />
  <TransferFlyout />
  <ConflictDialog />
  <CommandPalette />
  <Dialogs />
  <Menu />
  <Toasts />
{/if}

<style>
  .window {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  .body {
    display: flex;
    flex: 1;
    min-height: 0;
    background: var(--sidebar-bg);
  }
  .panes {
    display: flex;
    flex: 1;
    min-width: 0;
    border-left: 1px solid var(--stroke);
  }
</style>
