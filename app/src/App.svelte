<script lang="ts">
  import { onMount } from "svelte";
  import { appWindow } from "./lib/api";
  import AddressBar from "./lib/components/AddressBar.svelte";
  import CommandBar from "./lib/components/CommandBar.svelte";
  import DetailsView from "./lib/components/DetailsView.svelte";
  import Menu from "./lib/components/Menu.svelte";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import StatusBar from "./lib/components/StatusBar.svelte";
  import TitleBar from "./lib/components/TitleBar.svelte";
  import Toasts from "./lib/components/Toasts.svelte";
  import { isMac, isTextInput, primary } from "./lib/keys";
  import { menu } from "./lib/menu.svelte";
  import { ws } from "./lib/workspace.svelte";

  let address: AddressBar | undefined = $state();
  let ready = $state(false);

  onMount(async () => {
    await ws.init();
    const root = document.documentElement;
    root.classList.add(`platform-${ws.platform}`);
    if (ws.places?.translucent) root.classList.add("translucent");
    ready = true;
    // Show the window once the first frame is painted: no white flash.
    requestAnimationFrame(() => requestAnimationFrame(() => appWindow.show()));
  });

  // Drives come and go (USB sticks, network mounts).
  $effect(() => {
    const t = setInterval(() => ws.refreshPlaces(), 5000);
    return () => clearInterval(t);
  });

  $effect(() => {
    document.documentElement.classList.toggle("compact", ws.settings.compact);
  });

  $effect(() => {
    const title = ws.active?.folder.info?.name;
    if (title) void appWindow.setTitle(`${title} — Cross Explore`);
  });

  function focusList() {
    (document.querySelector(".details") as HTMLElement | null)?.focus();
  }

  // Window-wide shortcuts. List-specific keys live in DetailsView.
  function onkeydown(e: KeyboardEvent) {
    if (menu.open) return;
    const tab = ws.active;
    if (!tab) return;
    const k = e.key.toLowerCase();
    const mod = primary(e);
    const inText = isTextInput(e.target);

    if (mod && k === "t") ws.newTab(tab.folder.uri);
    else if (mod && k === "w") ws.closeTab(tab.id);
    else if (e.ctrlKey && k === "tab") ws.cycleTab(e.shiftKey ? -1 : 1);
    else if (mod && /^[1-9]$/.test(k)) ws.activate(ws.tabs[Math.min(+k, ws.tabs.length) - 1].id);
    else if (mod && k === "l") address?.editPath();
    else if (mod && k === "f") address?.focusSearch();
    else if (mod && e.shiftKey && k === "n") tab.newFolder();
    else if ((isMac && e.metaKey && e.shiftKey && k === ".") || (!isMac && e.ctrlKey && k === "h")) ws.toggle("showHidden");
    else if (k === "f5" || (mod && k === "r")) tab.folder.load();
    else if (inText) return;
    else if ((isMac && e.metaKey && k === "[") || (!isMac && e.altKey && k === "arrowleft") || k === "browserback") tab.back();
    else if ((isMac && e.metaKey && k === "]") || (!isMac && e.altKey && k === "arrowright") || k === "browserforward") tab.forward();
    else if ((isMac && e.metaKey && k === "arrowup") || (!isMac && e.altKey && k === "arrowup")) tab.up();
    else return;
    e.preventDefault();
    if (!inText) queueMicrotask(focusList);
  }

  // Mouse back/forward buttons.
  function onmouseup(e: MouseEvent) {
    if (e.button === 3) ws.active?.back();
    if (e.button === 4) ws.active?.forward();
  }
</script>

<svelte:window {onkeydown} {onmouseup} oncontextmenu={(e) => !isTextInput(e.target) && e.preventDefault()} />

{#if ready && ws.active}
  <div class="window">
    <TitleBar />
    <AddressBar bind:this={address} />
    <CommandBar />
    <div class="body">
      <Sidebar />
      <main class="main">
        {#key ws.active.id}
          <DetailsView tab={ws.active} />
        {/key}
      </main>
    </div>
    <StatusBar />
  </div>
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
  }
  .body {
    background: var(--sidebar-bg);
  }
  .main {
    background: var(--layer);
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
    border-left: 1px solid var(--stroke);
  }
</style>
