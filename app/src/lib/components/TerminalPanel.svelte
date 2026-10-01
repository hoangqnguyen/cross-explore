<script lang="ts">
  // Commander One-style terminal under the file panes. Opens in the current
  // folder (an SSH session for SFTP folders). Each tab keeps its own PTY and
  // xterm instance for as long as this panel stays open: switching tabs only
  // shows/hides the right one, it never reconnects — important for SSH,
  // where tearing a session down meant losing the shell and signing in again.
  import { untrack } from "svelte";
  import { FitAddon } from "@xterm/addon-fit";
  import { Terminal } from "@xterm/xterm";
  import "@xterm/xterm/css/xterm.css";
  import { errorText, termClose, termCwd, termOpen, termResize, termWrite, type TermEvent } from "../api";
  import { offerSshKey, resolveSshUri, sftpTarget } from "../ssh";
  import { ui } from "../stores/ui.svelte";
  import { ws } from "../workspace.svelte";
  import Icon from "./Icon.svelte";

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

  class Session {
    readonly container: HTMLDivElement;
    readonly term: Terminal;
    readonly fit: FitAddon;
    id: number | null = null;
    status = $state<string | null>(null);
    startedIn = $state("");
    #disposed = false;
    #offered = false;
    #queuedAuth: TermEvent | null = null;
    #input: { dispose: () => void };
    #mq: MediaQueryList;
    #retheme: () => void;

    constructor(parent: HTMLDivElement, dir: string, title: string) {
      this.startedIn = title;
      this.container = document.createElement("div");
      this.container.className = "host";
      parent.appendChild(this.container);

      this.term = new Terminal({ fontFamily: 'ui-monospace, "SF Mono", Menlo, Consolas, "Cascadia Code", monospace', fontSize: 12.5, cursorBlink: true, theme: theme(), allowProposedApi: true, scrollback: 5000 });
      this.fit = new FitAddon();
      this.term.loadAddon(this.fit);
      this.term.open(this.container);
      this.fit.fit();
      this.#input = this.term.onData((d) => this.id != null && void termWrite(this.id, d));
      this.#mq = matchMedia("(prefers-color-scheme: dark)");
      this.#retheme = () => (this.term.options.theme = theme());
      this.#mq.addEventListener("change", this.#retheme);

      const ssh = sftpTarget(dir);
      const term = this.term;
      void (async () => {
        const target = await resolveSshUri(dir);
        if (this.#disposed) return;
        if (!target) {
          this.status = "Cancelled";
          return;
        }
        if (ssh) {
          const user = sftpTarget(target)?.user;
          if (user) this.startedIn = `${title} (${user})`;
        }
        const onAuth = (sid: number, e: TermEvent) => {
          if (this.#offered || e.kind !== "authenticated" || !ssh) return;
          this.#offered = true;
          const user = sftpTarget(target)?.user ?? ssh.user ?? "";
          void offerSshKey(sid, e.method, e.copyId, user, ssh.host);
        };
        termOpen(target, term.cols, term.rows, (e) => {
          if (this.#disposed) return;
          if (e.kind === "output") term.write(decode(e.data));
          else if (e.kind === "authenticated") {
            if (this.id != null) onAuth(this.id, e);
            else this.#queuedAuth = e;
          } else {
            this.status = `Process exited${e.code != null ? ` (${e.code})` : ""}`;
            this.id = null;
          }
        })
          .then((sid) => {
            if (this.#disposed) void termClose(sid);
            else {
              this.id = sid;
              this.container.dataset.termId = String(sid);
              if (this.#queuedAuth) onAuth(sid, this.#queuedAuth);
            }
          })
          .catch((e) => (this.status = errorText(e)));
      })();
    }

    /** Make this the one visible terminal and give it focus. */
    show() {
      this.container.classList.add("active");
      this.term.focus();
    }

    hide() {
      this.container.classList.remove("active");
    }

    /** Re-measure after the panel (or window) resizes. */
    refit() {
      this.fit.fit();
      if (this.id != null) void termResize(this.id, this.term.cols, this.term.rows);
    }

    cdHere(path: string) {
      if (this.id == null) return;
      void termWrite(this.id, ` cd '${path.replaceAll("'", `'\\''`)}'\r`);
    }

    cwd() {
      return this.id != null ? termCwd(this.id) : Promise.resolve(null);
    }

    dispose() {
      this.#disposed = true;
      this.#input.dispose();
      this.#mq.removeEventListener("change", this.#retheme);
      if (this.id != null) void termClose(this.id);
      this.id = null;
      this.term.dispose();
      this.container.remove();
    }
  }

  let hostsEl: HTMLDivElement | undefined = $state();
  const sessions = new Map<number, Session>();
  let activeSession = $state<Session | null>(null);

  // Tear every session down only when the panel itself closes (it unmounts
  // on close — see App.svelte), not on every tab switch.
  $effect(() => {
    return () => {
      for (const s of sessions.values()) s.dispose();
      sessions.clear();
    };
  });

  $effect(() => {
    if (!hostsEl) return;
    const el = hostsEl;
    const ro = new ResizeObserver(() => activeSession?.refit());
    ro.observe(el);
    return () => ro.disconnect();
  });

  // A closed tab's terminal (and, for SSH, its connection) shouldn't linger
  // until the whole panel closes.
  $effect(() => {
    const live = new Set(ws.allTabs.map((t) => t.id));
    for (const [id, s] of sessions) {
      if (live.has(id)) continue;
      s.dispose();
      sessions.delete(id);
      if (activeSession === s) activeSession = null;
    }
  });

  $effect(() => {
    if (!hostsEl) return;
    const tab = ws.activeTab;
    if (!tab) return;
    let s = sessions.get(tab.id);
    if (!s) {
      // Read the folder/title snapshot untracked: this effect should only
      // react to *which tab* is active, never to that tab's own navigation,
      // or switching folders inside a tab would tear its terminal down too.
      const dir = untrack(() => (tab.folder.kind === "folder" ? tab.dirUri : (ws.places?.home.uri ?? "~")));
      const title = untrack(() => tab.title);
      s = new Session(hostsEl, dir, title);
      sessions.set(tab.id, s);
    }
    if (s !== activeSession) {
      activeSession?.hide();
      activeSession = s;
      s.show();
    }
  });

  function cdHere() {
    const t = ws.activeTab;
    if (!activeSession || !t.folder.info?.local || t.folder.kind !== "folder") return;
    const path = decodeURIComponent(t.dirUri.replace(/^file:\/\//, ""));
    activeSession.cdHere(path);
  }

  async function followShell() {
    if (!activeSession) return;
    const uri = await activeSession.cwd();
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
    <span class="title">Terminal — {activeSession?.startedIn ?? ""}</span>
    {#if activeSession?.status}<span class="status">{activeSession.status}</span>{/if}
    <span class="spacer"></span>
    <button title="cd to the folder shown above" onclick={cdHere}><Icon name="forward" size={13} /> Go to current folder</button>
    <button title="Show the shell's folder above" onclick={followShell}><Icon name="up" size={13} /> Show shell's folder</button>
    <button class="icon" aria-label="Close terminal" onclick={() => (ui.terminalOpen = false)}><Icon name="close" size={12} /></button>
  </header>
  <div class="hosts" bind:this={hostsEl}></div>
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
  .hosts {
    position: relative;
    flex: 1;
    min-height: 0;
  }
  /* Every tab's terminal stays mounted (so its PTY and scrollback survive a
     tab switch); only the active one is shown. Absolute + inset keeps every
     one correctly sized even while hidden, so there's no reflow/measure lag
     when it's switched to. */
  .hosts :global(.host) {
    position: absolute;
    inset: 0;
    padding: 4px 0 0 10px;
    visibility: hidden;
  }
  .hosts :global(.host.active) {
    visibility: visible;
  }
  .hosts :global(.xterm-viewport) {
    background: transparent !important;
  }
</style>
