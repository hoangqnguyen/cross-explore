//! IPC surface for the UI. Listings and watch patches stream over Tauri
//! channels so the UI can render before a whole folder has been read.

use cx_core::poll::{poll_watch, PollConfig};
use cx_core::{Capabilities, Change, CxError, Entry, Location, LocationInfo, Result, Space, Vfs, WatchGuard};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;
use tokio::sync::mpsc;

pub type VfsState = Arc<Vfs>;

#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ListEvent {
    Meta { info: LocationInfo, capabilities: Capabilities },
    Batch { entries: Vec<Entry> },
    Done { total: usize, elapsed_ms: f64 },
}

#[tauri::command]
pub async fn list_dir(uri: String, on_event: Channel<ListEvent>, vfs: State<'_, VfsState>) -> Result<()> {
    let started = Instant::now();
    let loc = Location::parse(&uri)?;
    let provider = vfs.provider(&loc).await?;
    let _ = on_event.send(ListEvent::Meta { info: loc.info(), capabilities: provider.capabilities() });

    let (tx, mut rx) = mpsc::channel(8);
    let listing = tokio::spawn(async move { provider.list(&loc, tx).await });
    while let Some(entries) = rx.recv().await {
        if on_event.send(ListEvent::Batch { entries }).is_err() {
            break; // UI went away; dropping rx cancels the listing
        }
    }
    drop(rx);
    let total = listing.await.map_err(|e| CxError::Io(e.to_string()))??;
    let _ = on_event.send(ListEvent::Done { total, elapsed_ms: started.elapsed().as_secs_f64() * 1e3 });
    Ok(())
}

#[derive(Default)]
pub struct Watches {
    next: AtomicU64,
    active: Mutex<HashMap<u64, WatchGuard>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchInfo {
    id: u64,
    /// "live" when changes are pushed, "polling" when we re-list periodically.
    mode: &'static str,
}

#[tauri::command]
pub async fn watch_dir(uri: String, on_change: Channel<Vec<Change>>, watches: State<'_, Watches>, vfs: State<'_, VfsState>) -> Result<WatchInfo> {
    let loc = Location::parse(&uri)?;
    let provider = vfs.provider(&loc).await?;
    let sink: cx_core::WatchSink = Arc::new(move |changes| {
        let _ = on_change.send(changes);
    });
    let (guard, mode) = match provider.watch(&loc, sink.clone()).await? {
        Some(g) => (g, "live"),
        None => (poll_watch(provider, loc, sink, PollConfig::default()), "polling"),
    };
    let id = watches.next.fetch_add(1, Ordering::Relaxed);
    watches.active.lock().unwrap().insert(id, guard);
    Ok(WatchInfo { id, mode })
}

#[tauri::command]
pub fn unwatch_dir(id: u64, watches: State<'_, Watches>) {
    watches.active.lock().unwrap().remove(&id);
}

#[tauri::command]
pub async fn create_folder(uri: String, name: Option<String>, vfs: State<'_, VfsState>) -> Result<Entry> {
    let loc = Location::parse(&uri)?;
    vfs.provider(&loc).await?.create_dir(&loc, name.as_deref()).await
}

#[tauri::command]
pub async fn rename_entry(uri: String, from: String, to: String, vfs: State<'_, VfsState>) -> Result<Entry> {
    let loc = Location::parse(&uri)?;
    vfs.provider(&loc).await?.rename(&loc, &from, &to).await
}

#[tauri::command]
pub async fn trash_entries(uri: String, names: Vec<String>, vfs: State<'_, VfsState>) -> Result<Vec<cx_core::TrashedItem>> {
    let loc = Location::parse(&uri)?;
    vfs.provider(&loc).await?.trash(&loc, &names).await
}

#[tauri::command]
pub async fn free_space(uri: String, vfs: State<'_, VfsState>) -> Result<Option<Space>> {
    let loc = Location::parse(&uri)?;
    vfs.provider(&loc).await?.free_space(&loc).await
}

/// Open a file with its default application.
#[tauri::command]
pub fn open_entry(app: AppHandle, uri: String) -> Result<()> {
    let loc = Location::parse(&uri)?;
    let path = loc.local_path().ok_or_else(|| CxError::Unsupported("opening remote files".into()))?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| CxError::Io(format!("cannot open {}: {e}", path.display())))
}

/// Surface UI-side errors in the terminal during development.
#[tauri::command]
pub fn ui_log(level: String, message: String) {
    eprintln!("[ui:{level}] {message}");
}
