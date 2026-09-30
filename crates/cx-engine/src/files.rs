//! Browsing and simple file operations. Listings and watch patches stream
//! through callbacks so a front end renders before a folder is fully read.

use crate::Engine;
use cx_core::poll::poll_watch_from;
use cx_core::{
    Capabilities, Change, CxError, Entry, Location, LocationInfo, Result, Space, TrashedItem,
    WatchGuard, WatchSink,
};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// How long a polled folder's listing stays usable as its watch baseline.
const BASELINE_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Serialize, Clone)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ListEvent {
    /// Sent twice: before connecting (default capabilities, so the tab and
    /// breadcrumb are named even when the server is slow or fails) and once
    /// the provider is known.
    Meta {
        info: LocationInfo,
        capabilities: Capabilities,
    },
    Batch {
        entries: Vec<Entry>,
    },
    Done {
        total: usize,
        elapsed_ms: f64,
    },
}

/// The last full listing of each polled (non-live) folder: the baseline its
/// poller diffs against, so changes right after the UI's listing are caught.
#[derive(Default)]
pub struct RecentListings(Mutex<HashMap<String, (Instant, Vec<Entry>)>>);

impl RecentListings {
    pub fn put(&self, uri: String, entries: Vec<Entry>) {
        self.0
            .lock()
            .unwrap()
            .insert(uri, (Instant::now(), entries));
    }

    pub fn take(&self, uri: &str) -> Option<Vec<Entry>> {
        let mut map = self.0.lock().unwrap();
        map.retain(|_, (t, _)| t.elapsed() < BASELINE_TTL);
        map.remove(uri).map(|(_, v)| v)
    }
}

/// "live" when changes are pushed, "polling" when we re-list periodically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WatchMode {
    Live,
    Polling,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct WatchInfo {
    pub id: u64,
    pub mode: WatchMode,
}

#[derive(Default)]
pub struct Watches {
    next: AtomicU64,
    active: Mutex<HashMap<u64, WatchGuard>>,
}

impl Watches {
    pub fn len(&self) -> usize {
        self.active.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Serialize, Clone, Copy, Default, PartialEq, Eq)]
pub struct SizeProgress {
    pub bytes: u64,
    pub files: u64,
    pub dirs: u64,
    pub done: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct TextPreview {
    pub text: String,
    pub truncated: bool,
    pub encoding: String,
    pub language: Option<String>,
}

impl Engine {
    /// List a folder, streaming [`ListEvent`]s. `on_event` returns false
    /// when nobody is listening any more, which cancels the listing.
    /// Returns the number of entries. Polled folders keep the listing as the
    /// baseline for a following [`Engine::watch_dir`].
    pub async fn list_dir(
        &self,
        uri: &str,
        mut on_event: impl FnMut(ListEvent) -> bool + Send,
    ) -> Result<usize> {
        let started = Instant::now();
        let loc = Location::parse(uri)?;
        on_event(ListEvent::Meta {
            info: loc.info(),
            capabilities: Capabilities::default(),
        });
        let provider = self.vfs.provider(&loc).await?;
        let caps = provider.capabilities();
        on_event(ListEvent::Meta {
            info: loc.info(),
            capabilities: caps,
        });

        let (tx, mut rx) = mpsc::channel(8);
        let key = loc.uri();
        let listing = tokio::spawn(async move { provider.list(&loc, tx).await });
        let mut kept = (!caps.live_watch).then(Vec::new);
        while let Some(entries) = rx.recv().await {
            if let Some(k) = kept.as_mut() {
                k.extend(entries.iter().cloned());
            }
            if !on_event(ListEvent::Batch { entries }) {
                break; // UI went away; dropping rx cancels the listing
            }
        }
        drop(rx);
        let total = listing.await.map_err(|e| CxError::Io(e.to_string()))??;
        if let Some(k) = kept {
            self.recent.put(key, k);
        }
        on_event(ListEvent::Done {
            total,
            elapsed_ms: started.elapsed().as_secs_f64() * 1e3,
        });
        Ok(total)
    }

    /// Watch a folder: live when the provider pushes changes, otherwise a
    /// poller diffing against the last [`Engine::list_dir`] of it. Stop with
    /// [`Engine::unwatch_dir`].
    pub async fn watch_dir(&self, uri: &str, sink: WatchSink) -> Result<WatchInfo> {
        let loc = Location::parse(uri)?;
        let provider = self.vfs.provider(&loc).await?;
        // OS watchers report canonical paths (FSEvents: /private/var for
        // /var, /private/tmp for /tmp), and the local watcher matches event
        // paths against the folder it was given: watch the resolved folder.
        // Changes carry names only, so the caller never sees the difference.
        let watch_loc = match &loc {
            Location::Local(p) => std::fs::canonicalize(p)
                .map(Location::Local)
                .unwrap_or_else(|_| loc.clone()),
            _ => loc.clone(),
        };
        let (guard, mode) = match provider.watch(&watch_loc, sink.clone()).await? {
            Some(g) => (g, WatchMode::Live),
            None => {
                let baseline = self.recent.take(&loc.uri());
                (
                    poll_watch_from(provider, loc, sink, self.config.poll, baseline),
                    WatchMode::Polling,
                )
            }
        };
        let id = self.watches.next.fetch_add(1, Ordering::Relaxed);
        self.watches.active.lock().unwrap().insert(id, guard);
        Ok(WatchInfo { id, mode })
    }

    /// Like [`Engine::watch_dir`] with a closure sink.
    pub async fn watch_dir_with(
        &self,
        uri: &str,
        sink: impl Fn(Vec<Change>) + Send + Sync + 'static,
    ) -> Result<WatchInfo> {
        self.watch_dir(uri, Arc::new(sink)).await
    }

    pub fn unwatch_dir(&self, id: u64) {
        self.watches.active.lock().unwrap().remove(&id);
    }

    pub async fn stat(&self, uri: &str) -> Result<Entry> {
        let loc = Location::parse(uri)?;
        self.vfs.provider(&loc).await?.stat(&loc).await
    }

    /// New folder in `dir`; `None` picks a free "New folder" name.
    pub async fn create_folder(&self, dir: &str, name: Option<&str>) -> Result<Entry> {
        let loc = Location::parse(dir)?;
        self.vfs.provider(&loc).await?.create_dir(&loc, name).await
    }

    pub async fn rename(&self, dir: &str, from: &str, to: &str) -> Result<Entry> {
        let loc = Location::parse(dir)?;
        self.vfs.provider(&loc).await?.rename(&loc, from, to).await
    }

    /// Move `names` in `dir` to the trash (fast path; big or remote batches
    /// can go through a transfer job instead).
    pub async fn trash(&self, dir: &str, names: &[String]) -> Result<Vec<TrashedItem>> {
        let loc = Location::parse(dir)?;
        self.vfs.provider(&loc).await?.trash(&loc, names).await
    }

    pub async fn free_space(&self, uri: &str) -> Result<Option<Space>> {
        let loc = Location::parse(uri)?;
        self.vfs.provider(&loc).await?.free_space(&loc).await
    }

    /// Total size of a folder (recursively, through any provider), reported
    /// every 100 ms while counting. `on_progress` returning false cancels.
    pub async fn dir_size(
        &self,
        uri: &str,
        mut on_progress: impl FnMut(SizeProgress) -> bool + Send,
    ) -> Result<u64> {
        let root = Location::parse(uri)?;
        let provider = self.vfs.provider(&root).await?;
        let mut p = SizeProgress::default();
        let mut queue = VecDeque::from([root]);
        let mut last = Instant::now();
        while let Some(dir) = queue.pop_front() {
            let Ok(entries) = cx_core::provider::list_all(provider.as_ref(), &dir).await else {
                continue;
            };
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
                if !on_progress(p) {
                    return Err(CxError::Cancelled);
                }
            }
        }
        p.done = true;
        on_progress(p);
        Ok(p.bytes)
    }

    /// The start of a text file, decoded, with a language guess.
    pub async fn preview_text(&self, uri: &str, max_bytes: usize) -> Result<TextPreview> {
        let loc = Location::parse(uri)?;
        let t = cx_thumbs::preview_text(&self.vfs, &loc, max_bytes).await?;
        Ok(TextPreview {
            text: t.text,
            truncated: t.truncated,
            encoding: t.encoding.to_string(),
            language: t.language_guess.map(|l| l.to_string()),
        })
    }

    /// Image dimensions and EXIF basics.
    pub async fn media_info(&self, uri: &str) -> Result<cx_thumbs::MediaInfo> {
        let loc = Location::parse(uri)?;
        cx_thumbs::media_info(&self.vfs, &loc).await
    }

    pub async fn thumbnail(&self, uri: &str, size_px: u32) -> Result<cx_thumbs::Thumb> {
        let loc = Location::parse(uri)?;
        self.thumbs.thumbnail(&self.vfs, &loc, size_px).await
    }

    /// Stat every URI tagged `tag` (skipping ones that are gone).
    pub async fn tags_find(&self, tag: &str) -> Vec<crate::tags::TaggedHit> {
        let mut out = Vec::new();
        for uri in self.tags.find(tag) {
            let Ok(loc) = Location::parse(&uri) else {
                continue;
            };
            let Ok(provider) = self.vfs.provider(&loc).await else {
                continue;
            };
            let Ok(entry) = provider.stat(&loc).await else {
                continue;
            };
            let parent = loc.parent().map(|p| p.uri()).unwrap_or_default();
            out.push(crate::tags::TaggedHit {
                rel_path: entry.name.clone(),
                uri,
                parent,
                entry,
            });
        }
        out
    }
}
