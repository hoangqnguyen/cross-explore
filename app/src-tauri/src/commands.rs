//! IPC surface for the UI. Listings and watch patches stream over Tauri
//! channels so the UI can render before a whole folder has been read.

use cx_core::{Capabilities, CxError, Entry, LocalProvider, Location, LocationInfo, Provider, Result};
use cx_watch::{Change, DirWatch};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;
use tokio::sync::mpsc;

#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ListEvent {
    Meta { info: LocationInfo, capabilities: Capabilities },
    Batch { entries: Vec<Entry> },
    Done { total: usize, elapsed_ms: f64 },
}

fn provider_for(loc: &Location) -> &'static dyn Provider {
    static LOCAL: LocalProvider = LocalProvider;
    match loc {
        Location::Local(_) => &LOCAL,
    }
}

#[tauri::command]
pub async fn list_dir(uri: String, on_event: Channel<ListEvent>) -> Result<()> {
    let started = Instant::now();
    let loc = Location::parse(&uri)?;
    let provider = provider_for(&loc);
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
    active: Mutex<HashMap<u64, DirWatch>>,
}

#[tauri::command]
pub fn watch_dir(uri: String, on_change: Channel<Vec<Change>>, watches: State<'_, Watches>) -> Result<u64> {
    let loc = Location::parse(&uri)?;
    let path = loc.local_path().ok_or_else(|| CxError::Unsupported("watching remote folders".into()))?;
    let watch = cx_watch::watch_dir(path, move |changes| {
        let _ = on_change.send(changes);
    })
    .map_err(|e| CxError::Io(format!("cannot watch {}: {e}", path.display())))?;
    let id = watches.next.fetch_add(1, Ordering::Relaxed);
    watches.active.lock().unwrap().insert(id, watch);
    Ok(id)
}

#[tauri::command]
pub fn unwatch_dir(id: u64, watches: State<'_, Watches>) {
    watches.active.lock().unwrap().remove(&id);
}

#[tauri::command]
pub async fn create_folder(uri: String, name: Option<String>) -> Result<Entry> {
    let loc = Location::parse(&uri)?;
    provider_for(&loc).create_dir(&loc, name.as_deref()).await
}

#[tauri::command]
pub async fn rename_entry(uri: String, from: String, to: String) -> Result<Entry> {
    let loc = Location::parse(&uri)?;
    provider_for(&loc).rename(&loc, &from, &to).await
}

#[tauri::command]
pub async fn trash_entries(uri: String, names: Vec<String>) -> Result<()> {
    let loc = Location::parse(&uri)?;
    provider_for(&loc).trash(&loc, &names).await
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
