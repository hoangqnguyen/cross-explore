//! Browsing and simple file operations. Listings and watch patches stream
//! over Tauri channels so the UI renders before a folder is fully read.

use super::AppState;
use cx_core::poll::{poll_watch_from, PollConfig};
use cx_core::{Capabilities, Change, CxError, Entry, Location, LocationInfo, Result, Space, TrashedItem, WatchGuard};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
use tauri::State;
use tokio::sync::mpsc;

#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ListEvent {
    Meta { info: LocationInfo, capabilities: Capabilities },
    Batch { entries: Vec<Entry> },
    Done { total: usize, elapsed_ms: f64 },
}

/// The last full listing of each polled (non-live) folder: the baseline its
/// poller diffs against, so changes right after the UI's listing are caught.
#[derive(Default)]
pub struct RecentListings(Mutex<HashMap<String, (Instant, Vec<Entry>)>>);

impl RecentListings {
    fn take(&self, uri: &str) -> Option<Vec<Entry>> {
        let mut map = self.0.lock().unwrap();
        map.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(30));
        map.remove(uri).map(|(_, v)| v)
    }
}

#[tauri::command]
pub async fn list_dir(uri: String, on_event: Channel<ListEvent>, app: AppState<'_>, recent: State<'_, RecentListings>) -> Result<()> {
    let started = Instant::now();
    let loc = Location::parse(&uri)?;
    let provider = app.vfs.provider(&loc).await?;
    let caps = provider.capabilities();
    let _ = on_event.send(ListEvent::Meta { info: loc.info(), capabilities: caps });

    let (tx, mut rx) = mpsc::channel(8);
    let key = loc.uri();
    let listing = tokio::spawn(async move { provider.list(&loc, tx).await });
    let mut kept = (!caps.live_watch).then(Vec::new);
    while let Some(entries) = rx.recv().await {
        if let Some(k) = kept.as_mut() {
            k.extend(entries.iter().cloned());
        }
        if on_event.send(ListEvent::Batch { entries }).is_err() {
            break; // UI went away; dropping rx cancels the listing
        }
    }
    drop(rx);
    let total = listing.await.map_err(|e| CxError::Io(e.to_string()))??;
    if let Some(k) = kept {
        recent.0.lock().unwrap().insert(key, (Instant::now(), k));
    }
    let _ = on_event.send(ListEvent::Done { total, elapsed_ms: started.elapsed().as_secs_f64() * 1e3 });
    Ok(())
}

#[derive(Default)]
pub struct Watches {
    next: AtomicU64,
    active: Mutex<HashMap<u64, WatchGuard>>,
}

#[derive(Serialize)]
pub struct WatchInfo {
    id: u64,
    /// "live" when changes are pushed, "polling" when we re-list periodically.
    mode: &'static str,
}

#[tauri::command]
pub async fn watch_dir(uri: String, on_change: Channel<Vec<Change>>, watches: State<'_, Watches>, app: AppState<'_>, recent: State<'_, RecentListings>) -> Result<WatchInfo> {
    let loc = Location::parse(&uri)?;
    let provider = app.vfs.provider(&loc).await?;
    let sink: cx_core::WatchSink = Arc::new(move |changes| {
        let _ = on_change.send(changes);
    });
    let (guard, mode) = match provider.watch(&loc, sink.clone()).await? {
        Some(g) => (g, "live"),
        None => {
            let baseline = recent.take(&loc.uri());
            (poll_watch_from(provider, loc, sink, PollConfig::default(), baseline), "polling")
        }
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
pub async fn stat_entry(uri: String, app: AppState<'_>) -> Result<Entry> {
    let loc = Location::parse(&uri)?;
    app.vfs.provider(&loc).await?.stat(&loc).await
}

#[tauri::command]
pub async fn create_folder(uri: String, name: Option<String>, app: AppState<'_>) -> Result<Entry> {
    let loc = Location::parse(&uri)?;
    app.vfs.provider(&loc).await?.create_dir(&loc, name.as_deref()).await
}

#[tauri::command]
pub async fn rename_entry(uri: String, from: String, to: String, app: AppState<'_>) -> Result<Entry> {
    let loc = Location::parse(&uri)?;
    app.vfs.provider(&loc).await?.rename(&loc, &from, &to).await
}

#[tauri::command]
pub async fn trash_entries(uri: String, names: Vec<String>, app: AppState<'_>) -> Result<Vec<TrashedItem>> {
    let loc = Location::parse(&uri)?;
    app.vfs.provider(&loc).await?.trash(&loc, &names).await
}

#[tauri::command]
pub async fn free_space(uri: String, app: AppState<'_>) -> Result<Option<Space>> {
    let loc = Location::parse(&uri)?;
    app.vfs.provider(&loc).await?.free_space(&loc).await
}

#[derive(Serialize, Clone, Copy)]
pub struct SizeProgress {
    bytes: u64,
    files: u64,
    dirs: u64,
    done: bool,
}

/// Total size of a folder (recursively, through any provider), reported
/// every 100 ms while counting.
#[tauri::command]
pub async fn dir_size(uri: String, on_progress: Channel<SizeProgress>, app: AppState<'_>) -> Result<u64> {
    let root = Location::parse(&uri)?;
    let provider = app.vfs.provider(&root).await?;
    let mut p = SizeProgress { bytes: 0, files: 0, dirs: 0, done: false };
    let mut queue = VecDeque::from([root]);
    let mut last = Instant::now();
    while let Some(dir) = queue.pop_front() {
        let Ok(entries) = cx_core::provider::list_all(provider.as_ref(), &dir).await else { continue };
        for e in entries {
            if e.kind == cx_core::EntryKind::Dir {
                p.dirs += 1;
                queue.push_back(dir.join(&e.name));
            } else {
                p.files += 1;
                p.bytes += e.size;
            }
        }
        if last.elapsed() > Duration::from_millis(100) {
            last = Instant::now();
            if on_progress.send(p).is_err() {
                return Err(CxError::Cancelled);
            }
        }
    }
    p.done = true;
    let _ = on_progress.send(p);
    Ok(p.bytes)
}

#[derive(Serialize)]
pub struct TextPreview {
    text: String,
    truncated: bool,
    encoding: String,
    language: Option<String>,
}

#[tauri::command]
pub async fn preview_text(uri: String, max_bytes: Option<usize>, app: AppState<'_>) -> Result<TextPreview> {
    let loc = Location::parse(&uri)?;
    let t = cx_thumbs::preview_text(&app.vfs, &loc, max_bytes.unwrap_or(512 * 1024)).await?;
    Ok(TextPreview { text: t.text, truncated: t.truncated, encoding: t.encoding.to_string(), language: t.language_guess.map(|l| l.to_string()) })
}

/// Surface UI-side errors in the terminal during development.
#[tauri::command]
pub fn ui_log(level: String, message: String) {
    eprintln!("[ui:{level}] {message}");
}

#[tauri::command]
pub fn subscribe(on_event: Channel<serde_json::Value>, app: AppState<'_>) {
    app.events.set(on_event);
}
