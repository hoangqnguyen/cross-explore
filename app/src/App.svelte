<script lang="ts">
  import { onMount } from "svelte";
  import { appWindow, inTauri, setMenuKeys } from "./lib/api";
  import { handleKey, menuAccelerators } from "./lib/commands.svelte";
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
  import MobileBar from "./lib/components/MobileBar.svelte";
  import AccessBanner from "./lib/components/AccessBanner.svelte";
  import { ui } from "./lib/stores/ui.svelte";
  import { isMac, isTextInput } from "./lib/keys";
  import { dropDestAt, dropElementAt, dropIsMove, nativeDrag } from "./lib/listing";
  import { dialogs } from "./lib/stores/dialogs.svelte";
  import { settings } from "./lib/stores/settings.svelte";
  import { ws } from "./lib/workspace.svelte";

  // The macOS menu bar takes its keys before the page sees them: keep its
  // shortcuts in line with the user's own (it starts with the standard ones).
  let menuSynced = false;
  $effect(() => {
    const keys = menuAccelerators();
    if (!inTauri || !isMac || (!menuSynced && !Object.keys(settings.data.keyBindings).length)) return;
    menuSynced = true;
    void setMenuKeys(keys).catch(() => {});
  });

  onMount(async () => {
    await ws.init();
    const root = document.documentElement;
    root.classList.add(`platform-${ws.platform}`);
    if (ws.places?.translucent) root.classList.add("translucent");
    // Show the window once the first frame is painted: no white flash.
    requestAnimationFrame(() => requestAnimationFrame(() => appWindow.show()));
    const testing = !inTauri && new URLSearchParams(location.search).has("path");
    const selftest = inTauri && (await import("@tauri-apps/api/core").then((c) => c.invoke("selftest_config")));
    // The layout choice is a desktop thing; phones start right away.
    if (ui.phone) settings.data.onboarded = true;
    if (!settings.data.onboarded && !testing && !selftest) void dialogs.ask("onboarding");
    if (inTauri) {
      void listenForOsDrops();
      if (selftest) void import("./lib/selftest").then((m) => m.selftest());
    }
  });

  // Files dropped from Finder / Explorer (or our own native drags): copy them
  // where they landed; our own drags keep move-within-a-volume semantics.
  async function listenForOsDrops() {
    const { getCurrentWebview } = await import("@tauri-apps/api/webview");
    let hovered: HTMLElement | null = null;
    const unhover = () => {
      hovered?.classList.remove("drop-hover");
      hovered = null;
    };
    await getCurrentWebview().onDragDropEvent((ev) => {
      const p = ev.payload;
      if (p.type === "leave") return unhover();
      const scale = window.devicePixelRatio || 1;
      const x = p.position.x / scale;
      const y = p.position.y / scale;
      if (p.type === "over" || p.type === "enter") {
        const el = dropElementAt(x, y);
        if (el !== hovered) {
          unhover();
          hovered = el;
          el?.classList.add("drop-hover");
        }
        return;
      }
      unhover();
      if (p.type !== "drop" || !p.paths.length) return;
      const dest = dropDestAt(x, y) ?? ws.activeTab.dirUri;
      const uris = p.paths.map((path) => {
        const norm = path.replaceAll("\\", "/");
        const withRoot = norm.startsWith("/") ? norm : "/" + norm;
        return "file://" + withRoot.split("/").map((seg, i) => (i === 1 && /^[A-Za-z]:$/.test(seg) ? seg : encodeURIComponent(seg))).join("/");
      });
      const own = nativeDrag;
      const internal = !!own && own.uris.length === uris.length;
      const sources = internal ? own!.uris : uris;
      // Don't drop things onto the folder they're already in.
      const moving = sources.filter((u) => u !== dest && u.replace(/\/[^/]*$/, "") !== dest.replace(/\/$/, ""));
      if (moving.length) void ws.transfer(moving, dest, internal && dropIsMove(null, moving, dest));
    });
  }

  $effect(() => {
    const root = document.documentElement;
    root.classList.toggle("compact", settings.data.compact);
    root.classList.toggle("mobile", ui.mobile);
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

  // Mouse back/forward buttons (Windows / Linux web views deliver them as
  // buttons 3 and 4; macOS is handled natively, see mousenav.rs). Acting on
  // press also stops the web view's own history navigation.
  function onpointerdown(e: PointerEvent) {
    if (e.button !== 3 && e.button !== 4) return;
    e.preventDefault();
    if (e.button === 3) ws.activeTab?.back();
    else ws.activeTab?.forward();
  }
  function onmouseup(e: MouseEvent) {
    if (e.button === 3 || e.button === 4) e.preventDefault();
  }
</script>

<svelte:window {onkeydown} {onmouseup} {onpointerdown} oncontextmenu={(e) => !isTextInput(e.target) && e.preventDefault()} />

{#if ws.ready && ws.activeTab && ui.mobile}
  <div class="window phone">
    <header class="mtop"><AddressBar tab={ws.activeTab} compact /></header>
    <div class="body">
      <div class="panes"><PaneView pane={ws.panes[0]} /></div>
    </div>
    <MobileBar />
  </div>
  {#if ui.drawerOpen}
    <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
    <div class="scrim" onclick={() => (ui.drawerOpen = false)}></div>
    <aside class="drawer"><Sidebar /></aside>
  {/if}
  <QuickLook />
  <TransferFlyout />
  <ConflictDialog />
  <CommandPalette />
  <Dialogs />
  <Menu />
  <Toasts />
{:else if ws.ready && ws.activeTab}
  <div class="window">
    <TitleBar />
    {#if !ws.dual}<AddressBar tab={ws.activeTab} />{/if}
    <CommandBar />
    <AccessBanner />
    <div class="body">
      <Sidebar />
      <div class="panes" bind:clientWidth={ws.panesWidth}>
        <PaneView pane={ws.panes[0]} />
        {#if ws.dual}<PaneView pane={ws.panes[1]} />{/if}
      </div>
      {#if settings.data.previewPane}<PreviewPane />{/if}
    </div>
    <!-- xterm is big and most sessions never open the terminal: load it on first use. -->
    {#if ui.terminalOpen}{#await import("./lib/components/TerminalPanel.svelte") then { default: TerminalPanel }}<TerminalPanel />{/await}{/if}
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
    height: 100dvh;
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
  .phone .panes {
    border-left: 0;
  }
  .mtop {
    padding-top: env(safe-area-inset-top);
    background: var(--layer);
    border-bottom: 1px solid var(--stroke);
    flex: none;
  }
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 50;
    background: rgba(0, 0, 0, 0.35);
    animation: fade 0.15s;
  }
  .drawer {
    position: fixed;
    z-index: 51;
    top: 0;
    bottom: 0;
    left: 0;
    width: min(300px, 82vw);
    padding-top: env(safe-area-inset-top);
    background: var(--layer);
    box-shadow: 8px 0 32px rgba(0, 0, 0, 0.3);
    display: flex;
    animation: slidein 0.2s var(--ease);
  }
  .drawer :global(.sidebar) {
    width: 100% !important;
  }
  @keyframes slidein {
    from {
      transform: translateX(-100%);
    }
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
</style>
