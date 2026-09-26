<script lang="ts">
  // Commander One-style terminal under the file panes. Opens in the current
  // folder (an SSH session for SFTP folders).
  import { FitAddon } from "@xterm/addon-fit";
  import { Terminal } from "@xterm/xterm";
  import "@xterm/xterm/css/xterm.css";
  import { errorText, termClose, termCwd, termOpen, termResize, termWrite } from "../api";
  import { ui } from "../stores/ui.svelte";
  import { ws } from "../workspace.svelte";
  import Icon from "./Icon.svelte";

  let host: HTMLDivElement | undefined = $state();
  let id: number | null = null;
  let status = $state<string | null>(null);
  let startedIn = $state("");

  function theme() {
    const css = getComputedStyle(document.documentElement);
    const v = (n: string) => css.getPropertyValue(n).trim();
    return { background: v("--layer"), foreground: v("--text"), cursor: v("--accent"), selectionBackground: v("--sel") };
  }

  function decode(b64: string) {
    const bin = atob(b64);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return bytes;
  }

  $effect(() => {
    if (!host) return;
    const term = new Terminal({ fontFamily: 'ui-monospace, "SF Mono", Menlo, Consolas, "Cascadia Code", monospace', fontSize: 12.5, cursorBlink: true, theme: theme(), allowProposedApi: true, scrollback: 5000 });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    fit.fit();
    let disposed = false;
    const dir = ws.activeTab.folder.kind === "folder" ? ws.activeTab.dirUri : (ws.places?.home.uri ?? "~");
    startedIn = ws.activeTab.title;
    termOpen(dir, term.cols, term.rows, (e) => {
      if (e.kind === "output") term.write(decode(e.data));
      else {
        status = `Process exited${e.code != null ? ` (${e.code})` : ""}`;
        id = null;
      }
    })
      .then((sid) => {
        if (disposed) void termClose(sid);
        else id = sid;
      })
      .catch((e) => (status = errorText(e)));
    const input = term.onData((d) => id != null && void termWrite(id, d));
    const ro = new ResizeObserver(() => {
      fit.fit();
      if (id != null) void termResize(id, term.cols, term.rows);
    });
    ro.observe(host);
    const mq = matchMedia("(prefers-color-scheme: dark)");
    const retheme = () => (term.options.theme = theme());
    mq.addEventListener("change", retheme);
    term.focus();
    return () => {
      disposed = true;
      ro.disconnect();
      input.dispose();
      mq.removeEventListener("change", retheme);
      if (id != null) void termClose(id);
      term.dispose();
    };
  });

  function cdHere() {
    const t = ws.activeTab;
    if (id == null || !t.folder.info?.local || t.folder.kind !== "folder") return;
    const path = decodeURIComponent(t.dirUri.replace(/^file:\/\//, ""));
    void termWrite(id, ` cd '${path.replaceAll("'", `'\\''`)}'\r`);
  }

  async function followShell() {
    if (id == null) return;
    const uri = await termCwd(id);
    if (uri) ws.activeTab.navigate(uri);
  }

  let dragging = false;
  function resize(e: PointerEvent) {
    if (dragging) ui.terminalHeight = Math.round(Math.min(window.innerHeight * 0.7, Math.max(120, window.innerHeight - e.clientY - 28)));
  }
</script>

<section class="terminal" style:height="{ui.terminalHeight}px">
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="grip"
    onpointerdown={(e) => {
      dragging = true;
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    }}
    onpointermove={resize}
    onpointerup={() => (dragging = false)}
  ></div>
  <header>
    <Icon name="terminal" size={14} />
    <span class="title">Terminal — {startedIn}</span>
    {#if status}<span class="status">{status}</span>{/if}
    <span class="spacer"></span>
    <button title="cd to the folder shown above" onclick={cdHere}><Icon name="forward" size={13} /> Go to current folder</button>
    <button title="Show the shell's folder above" onclick={followShell}><Icon name="up" size={13} /> Show shell's folder</button>
    <button class="icon" aria-label="Close terminal" onclick={() => (ui.terminalOpen = false)}><Icon name="close" size={12} /></button>
  </header>
  <div class="host" bind:this={host}></div>
</section>

<style>
  .terminal {
    position: relative;
    display: flex;
    flex-direction: column;
    flex: none;
    border-top: 1px solid var(--stroke-strong);
    background: var(--layer);
  }
  .grip {
    position: absolute;
    top: -3px;
    left: 0;
    right: 0;
    height: 6px;
    cursor: row-resize;
    z-index: 2;
  }
  header {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 30px;
    padding: 0 8px 0 12px;
    font-size: 12px;
    color: var(--text-2);
    border-bottom: 1px solid var(--stroke);
    flex: none;
  }
  .title {
    white-space: nowrap;
  }
  .status {
    color: var(--text-3);
  }
  .spacer {
    flex: 1;
  }
  header button {
    display: flex;
    align-items: center;
    gap: 5px;
    height: 24px;
    padding: 0 8px;
    border-radius: 4px;
    color: var(--text-2);
    white-space: nowrap;
  }
  header button:hover {
    background: var(--hover);
    color: var(--text);
  }
  header .icon {
    padding: 0 6px;
  }
  .host {
    flex: 1;
    min-height: 0;
    padding: 4px 0 0 10px;
  }
  .host :global(.xterm-viewport) {
    background: transparent !important;
  }
</style>
