//! Live updates for providers that can't push changes: re-list periodically
//! and diff. The interval adapts: quick while things change, slower when the
//! folder is idle.

use crate::{Change, Entry, Location, Provider, WatchGuard, WatchSink};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct PollConfig {
    pub min: Duration,
    pub max: Duration,
}

impl Default for PollConfig {
    fn default() -> Self {
        PollConfig { min: Duration::from_secs(2), max: Duration::from_secs(30) }
    }
}

/// Diff two listings into patches.
pub fn diff(old: &HashMap<String, Entry>, new: &HashMap<String, Entry>) -> Vec<Change> {
    let mut out = Vec::new();
    for (name, e) in new {
        if old.get(name) != Some(e) {
            out.push(Change::Upsert { entry: e.clone() });
        }
    }
    for name in old.keys() {
        if !new.contains_key(name) {
            out.push(Change::Remove { name: name.clone() });
        }
    }
    out
}

/// Start polling `dir`. Stops when the returned guard is dropped.
pub fn poll_watch(provider: Arc<dyn Provider>, dir: Location, sink: WatchSink, cfg: PollConfig) -> WatchGuard {
    poll_watch_from(provider, dir, sink, cfg, None)
}

/// Like [`poll_watch`], diffing against `baseline` (the listing the caller
/// is showing) instead of a fresh one. Without it, a change landing between
/// the caller's listing and the poller's first listing would never be
/// reported.
pub fn poll_watch_from(provider: Arc<dyn Provider>, dir: Location, sink: WatchSink, cfg: PollConfig, baseline: Option<Vec<Entry>>) -> WatchGuard {
    let task = tokio::spawn(async move {
        let snapshot = |v: Vec<Entry>| v.into_iter().map(|e| (e.name.clone(), e)).collect::<HashMap<_, _>>();
        let mut last = match baseline {
            Some(v) => snapshot(v),
            None => match crate::provider::list_all(provider.as_ref(), &dir).await {
                Ok(v) => snapshot(v),
                Err(_) => HashMap::new(),
            },
        };
        let mut interval = cfg.min;
        loop {
            tokio::time::sleep(interval).await;
            let Ok(now) = crate::provider::list_all(provider.as_ref(), &dir).await else {
                interval = (interval * 2).min(cfg.max);
                continue;
            };
            let now = snapshot(now);
            let changes = diff(&last, &now);
            if changes.is_empty() {
                interval = (interval + interval / 2).min(cfg.max);
            } else {
                interval = cfg.min;
                sink(changes);
            }
            last = now;
        }
    });
    WatchGuard::new(AbortOnDrop(task))
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EntryKind;

    fn e(name: &str, size: u64) -> Entry {
        Entry { name: name.into(), kind: EntryKind::File, is_dir: false, size, modified: None, created: None, hidden: false, readonly: false }
    }

    #[test]
    fn diff_reports_adds_changes_and_removes() {
        let old: HashMap<_, _> = [("a", 1), ("b", 1)].iter().map(|(n, s)| (n.to_string(), e(n, *s))).collect();
        let new: HashMap<_, _> = [("a", 2), ("c", 1)].iter().map(|(n, s)| (n.to_string(), e(n, *s))).collect();
        let mut d = diff(&old, &new);
        d.sort_by_key(|c| format!("{c:?}"));
        assert_eq!(d.len(), 3);
        assert!(d.contains(&Change::Remove { name: "b".into() }));
        assert!(d.contains(&Change::Upsert { entry: e("a", 2) }));
        assert!(d.contains(&Change::Upsert { entry: e("c", 1) }));
    }
}
