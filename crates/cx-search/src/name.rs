//! Recursive name search through any provider.

use crate::batch::Batcher;
use crate::filter::{is_noise, Filter};
use crate::{Cancel, SearchQuery, SearchStats};
use cx_core::provider::list_all;
use cx_core::{Entry, Location, Provider, Result, Vfs};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::Instant;

/// Folders listed at once. Hides per-request latency on remote servers
/// without flooding them; locally it keeps a few disks queues busy.
const PARALLEL_LISTINGS: usize = 8;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub uri: String,
    /// The folder holding the hit ("reveal in folder").
    pub parent_uri: String,
    pub entry: Entry,
    /// POSIX path relative to the search root (`sub/dir/name.txt`).
    pub rel_path: String,
}

struct Pending {
    loc: Location,
    rel: String,
    depth: u32,
}

/// Search `root` recursively for names matching `query`, sending hits to
/// `sink` in batches as they are found.
///
/// The walk is breadth-first (a FIFO of folders, a few listed concurrently),
/// so matches near the root arrive before deep ones. Unreadable subfolders
/// are counted in [`SearchStats::errors`] and skipped; only a failure to
/// list `root` itself is an error. Dropping the receiver cancels the search.
pub async fn search(vfs: Arc<Vfs>, root: Location, query: SearchQuery, sink: mpsc::Sender<Vec<SearchHit>>, cancel: Cancel) -> Result<SearchStats> {
    let filter = Filter::new(&query)?;
    let provider = vfs.provider(&root).await?;
    walk(provider, root, &filter, query.max_depth, query.max_results, sink, cancel).await
}

pub(crate) async fn walk(
    provider: Arc<dyn Provider>,
    root: Location,
    filter: &Filter,
    max_depth: Option<u32>,
    max_results: Option<usize>,
    sink: mpsc::Sender<Vec<SearchHit>>,
    cancel: Cancel,
) -> Result<SearchStats> {
    let started = Instant::now();
    let mut stats = SearchStats::default();
    let mut batch = Batcher::new(sink);
    let mut queue = VecDeque::from([Pending { loc: root, rel: String::new(), depth: 0 }]);
    let mut tasks: JoinSet<(Pending, Result<Vec<Entry>>)> = JoinSet::new();

    'walk: loop {
        while tasks.len() < PARALLEL_LISTINGS {
            let Some(p) = queue.pop_front() else { break };
            let provider = provider.clone();
            tasks.spawn(async move {
                let res = list_all(provider.as_ref(), &p.loc).await;
                (p, res)
            });
        }
        if tasks.is_empty() {
            break;
        }
        let deadline = batch.deadline();
        let joined = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                stats.cancelled = true;
                break;
            }
            _ = tokio::time::sleep_until(deadline), if batch.has_pending() => {
                if batch.flush().await.is_err() {
                    stats.cancelled = true;
                    break;
                }
                continue;
            }
            j = tasks.join_next() => j,
        };
        let Some(joined) = joined else { break };
        let Ok((dir, listed)) = joined else {
            stats.errors += 1;
            continue;
        };
        let entries = match listed {
            Ok(entries) => entries,
            Err(e) if dir.depth == 0 => return Err(e),
            Err(_) => {
                stats.errors += 1;
                continue;
            }
        };
        stats.dirs_scanned += 1;
        let can_descend = max_depth.is_none_or(|m| dir.depth < m);
        for entry in entries {
            stats.entries_scanned += 1;
            let wanted = filter.matches(&entry);
            let descend = can_descend && filter.descend(&entry);
            if !wanted && !descend {
                continue;
            }
            let loc = dir.loc.join(&entry.name);
            let rel = if dir.rel.is_empty() { entry.name.clone() } else { format!("{}/{}", dir.rel, entry.name) };
            if descend && !is_noise(&loc) {
                queue.push_back(Pending { loc: loc.clone(), rel: rel.clone(), depth: dir.depth + 1 });
            }
            if wanted {
                stats.hits += 1;
                batch.push(SearchHit { uri: loc.uri(), parent_uri: dir.loc.uri(), entry, rel_path: rel });
                if max_results.is_some_and(|m| stats.hits as usize >= m) {
                    stats.truncated = true;
                    break 'walk;
                }
            }
        }
        if batch.flush_if_due().await.is_err() || batch.is_closed() {
            stats.cancelled = true;
            break;
        }
    }
    // Dropping `tasks` aborts listings still in flight.
    drop(tasks);
    if !stats.cancelled && batch.flush().await.is_err() {
        stats.cancelled = true;
    }
    stats.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(stats)
}
