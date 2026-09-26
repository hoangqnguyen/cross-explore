# Cross Explore: Plan & Design

## Context
You want one file explorer for macOS, Windows, Linux and later iOS/Android. It should browse and move files across your own machine, your LAN and your Tailscale tailnet. The goals:
- **Windows 11 Explorer's look**: tabs, command bar, breadcrumb, Mica.
- **Total Commander / Commander One's power**: dual pane, keyboard-first, multi-rename, folder sync, FTP.
- **Finder's liveness and niceties**: instant live updates, Quick Look, column view, tags.
- **Built-in protocols**: FTP, SFTP, SMB and WebDAV, plus automatic discovery of shares nearby.

The project folder is empty, so this is a greenfield plan.

**Decisions so far:** Tauri 2 + Rust, all four platforms (mobile comes last), and a **peer-to-peer mode** where app instances talk to each other directly.

Working name: **Cross Explore** (short form "CX").

---

## 1. Product principles
1. **Live by default.** You never press refresh. Every view is a subscription, not a snapshot. Remote folders are live too: through a peer agent, SMB change-notify, or smart polling as a last resort.
2. **Instant feel.** The first rows paint in under 50 ms. Everything slow (sizes, thumbnails, remote listing) streams in without blocking input.
3. **Keyboard and mouse are equal.** The mouse gets Explorer-level friendliness. The keyboard gets Total Commander-level speed (F5 copy, F6 move, Tab switches pane).
4. **Network is a first-class location.** A NAS, a tailnet laptop or an SFTP box feels like a local folder, with the same views, previews and drag-and-drop.
5. **Operations are safe and reversible.** File operations can be undone, deletes go to the Trash, and conflicts get a real side-by-side resolver instead of a scary dialog.
6. **Light.** About a 15 MB installer, idle memory under 80 MB, and no background CPU when idle.

---

## 2. Architecture

```
┌──────────────────────── UI (Svelte 5 + TS, system webview) ────────────────────────┐
│ Panes/Tabs · Virtualized List/Grid/Columns · Quick Look · Palette · Transfer shelf │
│ State: per-location stores, patched by events (never full re-fetch)               │
└──────────▲──────────── Tauri IPC: commands + Channels (streamed batches) ──────────┘
           │            custom URI scheme  cx-thumb://  (binary thumbnails, cached)
┌──────────┴──────────────────────── Rust core (tokio) ──────────────────────────────┐
│ cx-core      VFS trait, Location URIs, Entry model, capability flags, sort/filter  │
│ cx-watch     local watchers (notify: FSEvents / ReadDirectoryChangesW / inotify)   │
│              + remote strategies; debounce → diff → patch events                   │
│ cx-providers local · sftp · smb · ftp/ftps · webdav · archive · peer               │
│ cx-transfer  job queue, per-host concurrency, resume, verify, conflicts, undo log  │
│ cx-discovery mDNS/DNS-SD · Tailscale LocalAPI · WS-Discovery · SSDP · port probes  │
│ cx-peer      QUIC agent (quinn), pairing, remote list/watch/read/write/thumbs      │
│ cx-thumbs    OS thumbnailers + image crate fallback, disk LRU cache                │
│ cx-search    in-folder instant filter, recursive search, content grep, index       │
│ cx-secrets   OS keychain (keyring crate), SSH known_hosts                          │
└────────────────────────────────────────────────────────────────────────────────────┘
```

### Location URIs (one address model for everything)
Examples: `file:///Users/me`, `sftp://nas.local/home`, `smb://nas/Media/Movies`, `ftp://host/pub`, `webdav://…`, `peer://my-laptop/Users/me/Downloads`, `archive://file:///x.zip!/inner/dir`.
The breadcrumb, tabs, favorites, history and drag-and-drop all use these URIs.

### VFS trait (cx-core)
`list(loc) -> Stream<Batch<Entry>>`, `stat`, `watch(loc) -> Stream<Change>`, `read_range`, `open_write`, `mkdir`, `rename`, `remove`, `copy_within` (server-side copy when the provider supports it), `capabilities()`.
Capabilities are flags such as `LiveWatch | Polling | ServerCopy | Thumbs | Posix | Trash`. The UI reads them to adapt, for example by showing a "live" dot or a "polling every 5s" dot in the status bar.

### Recommended crates
| Area | Crate |
|---|---|
| App shell | `tauri` 2, `tauri-plugin-*` (dialog, os, shell, updater), `window-vibrancy` (Mica / NSVisualEffect) |
| Watch | `notify` + `notify-debouncer-full` |
| SFTP/SSH | `russh` + `russh-sftp` (pure Rust, async) |
| SMB | `smb` (pure-Rust SMB2/3 client, supports CHANGE_NOTIFY). Fallback: shares the OS has already mounted |
| FTP/FTPS | `suppaftp` (async, TLS, MLSD) |
| WebDAV | `reqwest` + `reqwest_dav`. Consider `opendal` later for S3, GDrive and similar |
| Discovery | `mdns-sd`, `ssdp-client`, custom WS-Discovery (UDP 3702), Tailscale LocalAPI |
| Peer | `quinn` (QUIC), `rustls`, `blake3`, `postcard`/`serde` |
| Copy fast path | `reflink-copy` (APFS clonefile, ReFS, btrfs), `trash` crate |
| Archives | `zip`, `tar`, `flate2`, `sevenz-rust` |
| Search | `ignore` + `grep-searcher` (ripgrep internals), `nucleo` (fuzzy matching for the palette) |
| Secrets | `keyring` |

---

## 3. "Feels live": the liveness engine
- **Local folders.** The `notify` backend on each OS feeds a 50 ms coalescing window. We re-`stat` the affected entries and send **patches** (`add / update / remove / rename`) to the UI. The UI animates them: new files fade in with a short accent glow, and removed files collapse out.
- **Only visible locations are watched.** That covers open tabs, panes and the Quick Look target. Hidden tabs are downgraded to a cheap "dirty flag" and revalidate when you open them.
- **Remote strategy ladder**, shown in the UI as a status dot:
  1. **Peer agent** pushes native watcher events. Real-time.
  2. **SMB2 CHANGE_NOTIFY** on the open directory. Real-time.
  3. **SFTP.** If the server allows `exec` and has `inotifywait` or `fswatch`, we use it (opt-in). Otherwise adaptive polling.
  4. **FTP / WebDAV adaptive polling.** 2 s while the window is focused and the folder was recently active, backing off to 30 s. Uses ETag/mtime short-circuits.
- **Sizes and folder sizes** (the TC "Space" key) are computed in the background with cancellable walkers, and each row fills in as its result arrives.
- **Optimistic operations.** A rename or new folder appears instantly. The watcher confirms it later, or the change is rolled back with a toast.

---

## 4. Discovery & suggestions ("Nearby" in the sidebar)
Running continuously in the background, and throttled:
- **mDNS / DNS-SD** browses `_smb._tcp`, `_sftp-ssh._tcp`, `_ssh._tcp`, `_ftp._tcp`, `_webdav._tcp`, `_webdavs._tcp`, `_afpovertcp._tcp`, `_adisk._tcp`, `_device-info._tcp` and our own `_crossx._tcp`.
- **WS-Discovery** finds Windows PCs, which advertise this way. **SSDP/UPnP** finds NAS boxes and DLNA servers.
- **Tailscale.** We query the LocalAPI (`/localapi/v0/status`, falling back to `tailscale status --json`) to list online peers with their OS and MagicDNS names. Then we run a concurrent port probe on each peer (22, 445, 21, 80/443, and the CX port) with a 300 ms timeout.
- **Share enumeration.** SMB `NetShareEnum` (srvsvc) lists shares. SFTP suggests `~` and common roots. Peers list the folders they have chosen to publish.
- **Suggestions UI.** The "Nearby" section groups results by device (icon by device type: Mac, PC, NAS, phone, tailnet). Each device expands to show its shares. A banner offers things like *"Found 3 shares on `synology.local`, pin them?"* One click connects, credentials go to the keychain, and the share can be pinned to the sidebar.
- **Nothing connects on its own** except peers you have already paired. We only suggest.

---

## 5. Peer mode (the killer feature)
- **Agent.** A small service, either inside the app or run headless with `cx serve`. It advertises `_crossx._tcp` and listens on QUIC.
- **Trust:**
  - *Tailnet:* zero-config. We verify the caller with Tailscale `WhoIs` and allow devices owned by the same user automatically.
  - *LAN:* pair with a 6-digit code or a QR code. The two devices then pin each other's keys (ed25519, TOFU).
- **Shared folders.** Publish folders read-only or read-write, per device.
- **Protocol:** `List`, `Stat`, `Watch` (streamed events), `ReadRange`, `Write`, `Thumb`, `Hash`, `Search`. QUIC multiplexes many streams, so thumbnails, listing and a transfer can run at the same time without head-of-line blocking.
- **Transfers.** Chunked, parallel, resumable, and verified with BLAKE3. "Delta sync" (rsync-style) comes in a later phase.
- **Cross-device magic:**
  - **Send to…** in the context menu, AirDrop-style, with an accept prompt on the receiver.
  - **Universal clipboard for files.** Copy on machine A, then paste in a pane browsing machine B.
  - **Drag between windows** on different machines, via the transfer shelf.
  - **Open remote file.** It streams to a temp file and opens locally. On save it offers to write back.

---

## 6. Transfer engine
- It is a job graph: a job expands into many file tasks. Each host gets its own concurrency limit (local 4, SFTP 4, SMB 8, peer 16 streams).
- **Pause, resume and reorder.** Jobs survive app restarts, because job state is kept in SQLite (`rusqlite`).
- **Fast paths first:**
  - Same-volume moves become a rename.
  - Local copies use reflink/clonefile.
  - SMB copies use server-side copy (FSCTL_SRV_COPYCHUNK).
  - SFTP copies use `copy-data` when the server supports it.
  - Peer↔peer copies go device-to-device, not through this machine.
- **Conflicts.** A side-by-side card shows both files: thumbnail, size, date, and hash when it's cheap. The choices are Replace, Skip, Keep both, Newer wins, and "apply to all".
- Optional verification after copy. Timestamps and permissions are preserved. Bandwidth can be throttled.
- **Undo log** covers copy, move, rename and delete-to-trash (Ctrl/Cmd+Z), including batch renames.
- **UI.** A slim progress capsule sits in the status bar. Clicking it opens the Transfers flyout with a live throughput sparkline, ETA and per-file rows.

---

## 7. UI / UX design

### Layout (Windows 11 Explorer skeleton, polished Finder feel)
```
┌───────────────────────────────────────────────────────────────────────────────┐
│ ◀ ▶ ↑ ⟳* │  Home  │ ▸ Downloads × │ ▸ nas/Media × │ +          ─ □ ×   (tabs)│
├───────────────────────────────────────────────────────────────────────────────┤
│ ＋New ▾  ✂ ⧉ 📋 ✎ 🗑 │ ⇅ Sort ▾  ▦ View ▾  ⧉ Dual  │ ⋯        🔍 Search Media   │ command bar
├───────────────────────────────────────────────────────────────────────────────┤
│ 🖥 This Mac › 🌐 nas › Media › Movies ›             (click = edit path)  ● live │ breadcrumb
├──────────────┬────────────────────────────────────┬───────────────────────────┤
│ ★ Favorites  │ Name            Date       Size  ▾│                           │
│   Desktop    │ 📁 2024         Today 10:02   —   │   Preview / Info pane     │
│   Downloads  │ 🎬 dune.mkv     Yesterday  4.2 GB │   (Quick Look inline,     │
│ 🏷 Tags       │ 🖼 poster.jpg   ✨ just now 1.1 MB │    metadata, tags,        │
│ 💻 Devices   │ ...                                │    permissions, hash)     │
│   This Mac   │                                    │                           │
│   mbp-work ● │                                    │                           │
│ 📡 Nearby    │                                    │                           │
│   nas (SMB)  │                                    │                           │
│   pi (SFTP)  │                                    │                           │
├──────────────┴────────────────────────────────────┴───────────────────────────┤
│ 1,204 items · 3 selected (5.3 GB) · 212 GB free      ⇅ 2 transfers 64 MB/s ▂▄▆│ status bar
└───────────────────────────────────────────────────────────────────────────────┘
```
- **Dual-pane mode** (Ctrl/Cmd+D, or F9 like Commander One) splits the content area into two panes, each with its own tabs and breadcrumb. The active pane gets a subtle accent border. F5 and F6 copy or move to the other pane. The sidebar and preview pane stay shared.
- **Views:** Details (the default, with Explorer's grouping headers "Today / Earlier this week"), Icons (S/M/L/XL with thumbnails), **Columns** (Finder's Miller columns), **Gallery** (large preview + filmstrip) and **Flat/Branch** (TC Ctrl+B: every file in the subtree).
- **Home page** (Explorer's Home plus Finder's Recents): pinned folders, recent files with thumbnails, devices and their status, the Nearby suggestions card, and recent transfers.

### Visual language
- **Fluent-inspired, platform-adaptive.** Mica/acrylic on Windows, vibrancy on macOS, a solid surface on Linux. Accent color comes from the OS, and light/dark follows the system.
- **Type:** system font stack (Segoe UI Variable / SF Pro / Inter on Linux). Body 13 px; row height 32 px comfortable, 24 px compact.
- **Icons:** Fluent UI System Icons (MIT) for chrome. File-type icons come from one consistent line set plus real thumbnails for media.
- **Spacing and shape:** 4/8 px spacing grid, 8 px corner radius, soft elevation for flyouts only.
- **Motion:** 120–180 ms ease-out. Rows animate only on live changes, and reduced-motion is respected. There are no animations on navigation; it has to feel instant.
- **Visible states:** "live", "polling" and "offline" dots per location. Skeleton rows while remote listings stream in. Offline devices stay visible, greyed out, with "last seen".

### Signature interactions
| Feature | Source | Behavior |
|---|---|---|
| Quick Look | Finder | Space opens a floating preview (image, video, audio, PDF, code with highlighting, markdown, archive contents, fonts). Arrow keys move through files while it stays open. |
| Command palette | modern | Ctrl/Cmd+K: fuzzy search over actions, favorites, devices and recent folders. "Go to…" accepts paths and URIs. |
| Type-to-filter | TC quick filter | Typing in a list filters instantly. Esc clears. Ctrl+F opens the full search. |
| Select by pattern | TC `+` / `-` / `*` | Select or deselect by glob, and invert the selection. |
| Multi-rename | TC Ctrl+M | Live preview table with tokens `[N] [C] [E] [YMD]`, regex, case changes, and EXIF/ID3 fields. Undoable. |
| Compare / sync folders | TC | A diff tree across any two locations: local↔SMB, peer↔SFTP. Choose one-way or two-way, and preview before applying. |
| File diff | TC | Text diff side by side. Binary files get a hash comparison. |
| Spring-loaded folders | Finder | Hover while dragging, and the folder opens. |
| Transfer shelf | Yoink-like | A drop area at the window edge. Collect files from different places, then drop them all somewhere. |
| Tags & smart folders | Finder | Colored tags. They map to Finder tags on macOS and are stored in our own DB on other platforms. Saved searches show up as folders. |
| Archives as folders | TC / Commander One | Open a zip, tar or 7z like a directory and copy files in or out. |
| Terminal here | Commander One | Open the OS terminal here, locally or as SSH to a remote location. An embedded terminal comes later. |
| Hotlist / workspaces | TC | Ctrl+D hotlist. Saved workspaces restore panes, tabs and view settings. |
| Get Info inspector | Finder / Explorer | Size, dates, permissions (chmod on POSIX), hashes and "where is this used". |

### Keyboard map
- **Explorer/Finder defaults are always on.** Enter opens, F2 renames, Alt/Cmd+↑ goes to the parent, Ctrl/Cmd+T opens a new tab, Ctrl/Cmd+L edits the path.
- **"Commander keys" preset** (toggle in onboarding). F3 view, F4 edit, F5 copy, F6 move, F7 mkdir, F8 delete, Tab switches pane, Insert selects, Ctrl+U swaps panes, Alt+F1/F2 picks a drive.
- **Every shortcut can be remapped** in Settings.

### Onboarding (60 seconds)
1. Pick a style: **Explorer-like** or **Commander-like**, with a live preview.
2. Your devices: "We found 2 devices on your tailnet and 1 NAS. Pin them?"
3. Optional: pair your other machines (QR code).

---

## 8. Performance budgets (enforced by benchmarks in CI)
| Metric | Target |
|---|---|
| Cold start to interactive | < 400 ms (desktop) |
| 10k-item folder, first rows painted | < 50 ms; full listing < 300 ms |
| 100k items | Smooth scrolling at 60–120 fps (virtualized, fixed row height, sorting in Rust) |
| Local change to on screen | < 150 ms |
| Peer change to on screen (tailnet) | < 300 ms |
| Idle RAM / CPU | < 80 MB / ~0% |
| LAN transfer | ≥ 90% of link speed (SMB/peer); SFTP bound by cipher (AES-GCM preferred) |

**Techniques:**
- Streamed batches over Tauri Channels. The first 200 entries go first, the rest follows.
- One shared icon atlas per file type.
- Thumbnails load in viewport-priority order, and requests are cancelled when rows scroll away.
- No work on the webview main thread except rendering.

---

## 9. Security & privacy
- Credentials live only in the OS keychain. SSH verifies host keys against `known_hosts` and shows the fingerprint on first connect.
- Peer mode is off until enabled. It can be bound to the Tailscale interface only, shares are explicit and per-device, and there is an audit log of remote access.
- No telemetry by default.

---

## 10. Repo layout
```
cross-explore/
  Cargo.toml                 (workspace)
  crates/
    cx-core/ cx-watch/ cx-transfer/ cx-discovery/ cx-peer/ cx-thumbs/ cx-search/ cx-secrets/
    cx-providers/{local,sftp,smb,ftp,webdav,archive,peer}/
    cx-cli/                  (`cx serve`, headless agent for NAS / servers)
  app/
    src-tauri/               (commands, channels, window/vibrancy, menus)
    ui/                      (Svelte 5 + Vite + TS)
      lib/components/ (Pane, TabStrip, Breadcrumb, CommandBar, VirtualList, Grid,
                       ColumnView, QuickLook, Palette, TransferFlyout, Sidebar)
      lib/stores/     (locations, selection, transfers, devices, settings)
      lib/design/     (tokens.css, themes, icons)
  bench/                     (fixture generators, latency harness)
  docker/                    (test servers: sftp, samba, ftp, webdav)
```

---

## 11. Roadmap
| Phase | Scope | Result |
|---|---|---|
| **0. Foundations** | Workspace, Tauri shell with Mica/vibrancy, design tokens, VFS trait, local provider, virtual list, `notify` watcher with patch pipeline | A single-pane local browser that updates live |
| **1. Great local explorer (MVP)** | Tabs, dual pane, Details/Icons/Columns/Gallery views, breadcrumb and address editing, sidebar, Home, Quick Look, type-to-filter, palette, copy/move/delete/rename with undo, transfer queue, conflict resolver, keyboard presets | A daily-driver replacement on all 3 desktop OSes |
| **2. Network** | SFTP, SMB (with change-notify), FTP/FTPS, WebDAV; keychain; Connect dialog; discovery (mDNS, Tailscale, WSD, SSDP) and the Nearby panel with suggestions | Browse and transfer files across LAN and tailnet |
| **3. Peer mode** | QUIC agent, pairing and Tailscale auto-trust, live remote watch, remote thumbnails, Send to…, cross-device clipboard, resumable parallel transfers, `cx serve` | Remote folders feel local |
| **4. Power tools** | Multi-rename, folder compare/sync, file diff, archives, recursive and content search, tags, smart folders, terminal here, hotlist and workspaces | Parity with Total Commander |
| **5. Mobile** | Tauri 2 iOS/Android. A single pane with bottom navigation. Mainly a remote browser and transfer client (peer, SFTP, SMB). Local access through the platform document providers because of iOS sandboxing; watchers limited to the app container | Your phone joins the network |
| **6. Polish & extensibility** | Delta sync, embedded terminal, cloud backends through OpenDAL, WASM plugin API for previewers and column providers | — |

---

## 12. Risks & mitigations
- **Webview list performance.** Mitigation: a custom virtualizer, fixed row heights, CSS containment, and no framework reactivity per cell. We benchmark early, in Phase 0.
- **Maturity of the pure-Rust SMB crate.** Mitigation: fall back to OS-mounted shares (on macOS and Windows these behave like local paths with native watching). Keep libsmbclient behind a feature flag, but note it is GPLv3.
- **WebKitGTK quirks on Linux.** Mitigation: test on it early and avoid bleeding-edge CSS.
- **Mobile sandboxing.** Mitigation: scope mobile as a remote client first.
- **Scope creep.** Mitigation: ship Phase 1 as a genuinely good local explorer before adding network features.

---

## 13. Verification
- **Unit and integration tests per provider** against docker test servers (`atmoz/sftp`, `dperson/samba`, `stilliard/pure-ftpd`, `rclone serve webdav`). They cover list, watch, CRUD, large and resumed transfers, and unicode or odd filenames.
- **Watcher tests:** a script creates, renames and deletes files at high rate. We assert the UI patch stream converges to the true directory state, and that latency stays under budget.
- **Benchmarks** (`bench/`): generate folders of 10k and 100k files, measure time to first paint and scroll frame times, and fail CI when a budget regresses.
- **End-to-end UI tests** with `tauri-driver` (WebDriver) for navigation, dual-pane F5 copy, undo and Quick Look.
- **Manual matrix:** macOS, Windows 11 and Ubuntu/Fedora. Discovery on a real LAN with a NAS, plus two tailnet devices paired with each other.

## First implementation step (after approval)
Phase 0:
1. Scaffold the Cargo workspace and the Tauri 2 + Svelte 5 app.
2. Implement `cx-core` with the local provider and the `notify` patch pipeline.
3. Build the virtual Details list with the Explorer-style chrome (tabs, command bar, breadcrumb, sidebar).
4. Add the benchmark harness, so the "fast and live" claims are measured from day one.
