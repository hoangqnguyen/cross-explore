//! Turns raw OS file-system events into small patches the UI can apply
//! without re-reading the folder.
//!
//! Events are gathered until the folder has been quiet for [`QUIET`] (capped
//! at [`MAX_WAIT`]), so a burst of writes to one file becomes one update.
//! Then every affected name is re-stat'ed: a name that still exists becomes a
//! [`Change::Upsert`] with fresh metadata, one that is gone becomes a
//! [`Change::Remove`]. Re-stat'ing instead of trusting event kinds keeps this
//! correct across backends (FSEvents, ReadDirectoryChangesW, inotify), which
//! all report renames and saves differently. When the OS says it dropped
//! events we send [`Change::Reset`] and the UI re-lists the folder.

use cx_core::Entry;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A batch is flushed once no new event arrived for this long...
pub const QUIET: Duration = Duration::from_millis(25);
/// ...or at the latest this long after its first event, so a folder that is
/// constantly being written to still updates on screen.
pub const MAX_WAIT: Duration = Duration::from_millis(100);

/// Past this many changed names in one batch, a full re-list is cheaper
/// than patching row by row.
const RESET_THRESHOLD: usize = 5_000;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Change {
    Upsert { entry: Entry },
    Remove { name: String },
    Reset,
}

/// Watches one directory (not its subfolders) until dropped.
pub struct DirWatch {
    // Dropping the watcher closes the event channel, which ends the thread.
    _watcher: RecommendedWatcher,
}

pub fn watch_dir<F>(dir: impl Into<PathBuf>, on_changes: F) -> notify::Result<DirWatch>
where
    F: Fn(Vec<Change>) + Send + 'static,
{
    let dir: PathBuf = dir.into();
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = notify::recommended_watcher(tx)?;
    watcher.watch(&dir, RecursiveMode::NonRecursive)?;
    std::thread::Builder::new()
        .name("cx-watch".into())
        .spawn(move || coalesce_loop(&dir, &rx, &on_changes))
        .map_err(notify::Error::io)?;
    Ok(DirWatch { _watcher: watcher })
}

fn coalesce_loop(dir: &Path, rx: &mpsc::Receiver<notify::Result<Event>>, on_changes: &dyn Fn(Vec<Change>)) {
    while let Ok(first) = rx.recv() {
        let started = Instant::now();
        let mut names = BTreeSet::new();
        let mut rescan = false;
        let mut ev = Some(first);
        while let Some(e) = ev.take() {
            match e {
                Ok(e) => {
                    rescan |= e.need_rescan();
                    for p in &e.paths {
                        collect_name(dir, p, &mut names, &mut rescan);
                    }
                }
                Err(_) => rescan = true,
            }
            let left = MAX_WAIT.saturating_sub(started.elapsed());
            if left.is_zero() {
                break;
            }
            match rx.recv_timeout(QUIET.min(left)) {
                Ok(next) => ev = Some(next),
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        let changes: Vec<Change> = if rescan || names.len() > RESET_THRESHOLD {
            vec![Change::Reset]
        } else {
            names.into_iter().filter_map(|n| restat(dir, n)).collect()
        };
        if !changes.is_empty() {
            on_changes(changes);
        }
    }
}

fn collect_name(dir: &Path, path: &Path, names: &mut BTreeSet<String>, rescan: &mut bool) {
    if path == dir {
        // Events on the folder itself are mostly noise (metadata, or replayed
        // history right after the watch starts). Only its disappearance
        // matters: re-listing then surfaces a "folder is gone" error.
        if !dir.exists() {
            *rescan = true;
        }
    } else if path.parent() == Some(dir) {
        if let Some(name) = path.file_name() {
            names.insert(name.to_string_lossy().into_owned());
        }
    }
    // Events deeper down (some backends report them even when watching
    // non-recursively) do not change this folder's rows.
}

fn restat(dir: &Path, name: String) -> Option<Change> {
    match Entry::from_path(&dir.join(&name)) {
        Ok(entry) => Some(Change::Upsert { entry }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Some(Change::Remove { name }),
        // Exists but can't be stat'ed right now: leave the row as it is.
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::sync::mpsc;
    use std::time::Instant;

    /// Apply changes until `done` holds for the folder state, or time out.
    fn wait_for(rx: &mpsc::Receiver<Vec<Change>>, state: &mut HashMap<String, Entry>, done: impl Fn(&HashMap<String, Entry>) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(state) {
            let left = deadline.saturating_duration_since(Instant::now());
            let changes = rx.recv_timeout(left).expect("timed out waiting for watcher");
            for c in changes {
                match c {
                    Change::Upsert { entry } => {
                        state.insert(entry.name.clone(), entry);
                    }
                    Change::Remove { name } => {
                        state.remove(&name);
                    }
                    Change::Reset => {}
                }
            }
        }
    }

    #[test]
    fn create_modify_rename_remove_converge() {
        let tmp = tempfile::tempdir().unwrap();
        // Canonicalize: on macOS the temp dir is behind the /var -> /private/var symlink
        // and FSEvents reports canonical paths.
        let dir = tmp.path().canonicalize().unwrap();
        let (tx, rx) = mpsc::channel();
        let _w = watch_dir(&dir, move |c| {
            let _ = tx.send(c);
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let mut state = HashMap::new();

        fs::write(dir.join("a.txt"), b"1").unwrap();
        fs::create_dir(dir.join("folder")).unwrap();
        wait_for(&rx, &mut state, |s| s.contains_key("a.txt") && s.contains_key("folder"));
        assert!(state["folder"].is_dir);

        fs::write(dir.join("a.txt"), b"12345").unwrap();
        wait_for(&rx, &mut state, |s| s.get("a.txt").map(|e| e.size) == Some(5));

        fs::rename(dir.join("a.txt"), dir.join("b.txt")).unwrap();
        wait_for(&rx, &mut state, |s| !s.contains_key("a.txt") && s.contains_key("b.txt"));

        fs::remove_file(dir.join("b.txt")).unwrap();
        wait_for(&rx, &mut state, |s| !s.contains_key("b.txt"));

        // Changes inside a subfolder must not surface as rows here.
        fs::write(dir.join("folder").join("inner.txt"), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        while let Ok(changes) = rx.try_recv() {
            for c in changes {
                if let Change::Upsert { entry } = c {
                    assert_ne!(entry.name, "inner.txt");
                }
            }
        }
    }
}
