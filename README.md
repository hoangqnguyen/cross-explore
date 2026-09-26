# Cross Explore

A fast, live, cross-platform file explorer: the Windows 11 Explorer layout, Finder-style
live updates, and (in later phases) Total Commander power tools, network protocols and
peer-to-peer transfers across your LAN and tailnet. See the full design and roadmap in
[`docs/PLAN.md`](docs/PLAN.md).

**Status: Phase 0 (foundations).** A single-window local browser with tabs, live updates,
keyboard navigation, type-to-filter, and new folder / rename / move to trash.

## Layout

| Path | What |
|---|---|
| `crates/cx-core` | Locations (URIs), entries, the `Provider` trait and the local provider (streamed listings) |
| `crates/cx-watch` | Live folder watching: OS events → coalesced → re-stat'ed `Upsert`/`Remove` patches |
| `crates/cx-bench` | Performance budget checks (listing speed, change-to-patch latency) |
| `app/src-tauri` | Tauri 2 shell: IPC commands, window materials (vibrancy / Mica) |
| `app/src` | Svelte 5 UI: virtualized Details view, tabs, address bar, sidebar |
| `app/tests` | UI smoke test driven over the Chrome DevTools protocol |

## Develop

```sh
cd app
npm install
npx tauri dev            # the app
npm run dev              # UI only, in a browser, against an in-memory mock backend
                         #   http://localhost:1420/?path=~/Downloads
```

## Test

```sh
cargo test --workspace                       # core + watcher
cargo run -p cx-bench --release -- --check   # performance budgets (exit 1 on a miss)
cd app && npm run check                      # type-check the UI
cd app && npm run dev & node tests/ui-smoke.mjs   # UI smoke test (needs Chrome)
```

## Keys

| | macOS | Windows / Linux |
|---|---|---|
| Open | ⌘O / ⌘↓ | Enter |
| Rename | Enter | F2 |
| Enclosing folder | ⌘↑ | Alt+↑ |
| Back / forward | ⌘[ / ⌘] | Alt+← / Alt+→ |
| Move to Trash | ⌘⌫ | Del |
| New folder | ⌘⇧N | Ctrl+Shift+N |
| New / close tab | ⌘T / ⌘W | Ctrl+T / Ctrl+W |
| Edit path / filter | ⌘L / ⌘F | Ctrl+L / Ctrl+F |
| Hidden items | ⌘⇧. | Ctrl+H |
| Filter | just type | just type |
