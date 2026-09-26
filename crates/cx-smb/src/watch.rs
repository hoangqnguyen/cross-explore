//! Live folder updates through SMB2 CHANGE_NOTIFY.
//!
//! The server holds a CHANGE_NOTIFY request until something in the folder
//! changes (smb2 keeps one pre-issued so nothing falls into the gap between
//! answers). Like the local watcher, we don't trust the event kinds: every
//! reported name is re-stat'ed, so a name that exists becomes an
//! [`Change::Upsert`] with fresh metadata and one that is gone becomes a
//! [`Change::Remove`]. `STATUS_NOTIFY_ENUM_DIR` (the server's buffer
//! overflowed) and a lost connection become [`Change::Reset`] so the UI
//! re-lists; the watch then re-arms itself on the (reconnected) session.

use crate::provider::to_entry;
use crate::session::Session;
use crate::wire;
use cx_core::{Change, WatchSink};
use smb2::types::status::NtStatus;
use smb2::ErrorKind;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

/// Events arriving within this window of each other are handled as one batch.
const QUIET: Duration = Duration::from_millis(30);
/// Past this many names, a re-list is cheaper than re-stat'ing each.
const RESET_THRESHOLD: usize = 500;

/// Stops the watch when dropped.
pub(crate) struct Stop(#[allow(dead_code)] oneshot::Sender<()>);

/// Arm a watch on `path` (share-relative) and start delivering changes.
/// Fails if the first CHANGE_NOTIFY can't be set up, so the caller can fall
/// back to polling.
pub(crate) async fn start(session: Arc<Session>, share: String, path: String, sink: WatchSink) -> smb2::Result<Stop> {
    let mut watcher = arm(&session, &share, &path).await?;
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(1);
        loop {
            // Only a stop may cancel `next_events` (see below).
            let first = tokio::select! {
                _ = &mut stop_rx => break,
                r = watcher.next_events() => r,
            };
            match first {
                Ok(events) => {
                    backoff = Duration::from_secs(1);
                    let mut names = BTreeSet::new();
                    collect(&events, &mut names);
                    // Let the rest of a burst land before re-stat'ing. No
                    // timeout around `next_events` for this: cancelling it
                    // would drop an answered request and lose its events,
                    // whereas anything arriving now waits in the pre-issued
                    // request for the next turn of the loop.
                    tokio::time::sleep(QUIET).await;
                    let changes = if names.len() > RESET_THRESHOLD { vec![Change::Reset] } else { restat(&session, &share, &path, names).await };
                    if !changes.is_empty() {
                        sink(changes);
                    }
                }
                Err(e) if matches!(&e, smb2::Error::Protocol { status, .. } if *status == NtStatus::NOTIFY_ENUM_DIR) => {
                    sink(vec![Change::Reset]);
                }
                Err(e) => {
                    // The folder may be gone, or the connection dropped.
                    // Tell the UI to re-list (which surfaces "not found"),
                    // then try to re-arm on a revived session.
                    sink(vec![Change::Reset]);
                    if e.kind() == ErrorKind::NotFound {
                        break;
                    }
                    loop {
                        tokio::select! {
                            _ = &mut stop_rx => return,
                            _ = tokio::time::sleep(backoff) => {}
                        }
                        backoff = (backoff * 2).min(Duration::from_secs(30));
                        if let Ok(w) = arm(&session, &share, &path).await {
                            watcher = w;
                            sink(vec![Change::Reset]);
                            break;
                        }
                    }
                }
            }
        }
        let _ = watcher.close().await;
    });
    Ok(Stop(stop_tx))
}

async fn arm(session: &Session, share: &str, path: &str) -> smb2::Result<smb2::Watcher> {
    session.run(share, true, |mut conn, tree| async move { tree.watch(&mut conn, path, false).await }).await
}

fn collect(events: &[smb2::FileNotifyEvent], names: &mut BTreeSet<String>) {
    for e in events {
        // Non-recursive watch: names are direct children. Guard anyway.
        if !e.filename.is_empty() && !e.filename.contains('/') {
            names.insert(e.filename.clone());
        }
    }
}

async fn restat(session: &Session, share: &str, dir: &str, names: BTreeSet<String>) -> Vec<Change> {
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let path = if dir.is_empty() { name.clone() } else { format!("{dir}/{name}") };
        let r = session.run(share, true, |conn, tree| {
            let path = path.clone();
            async move { wire::stat(&conn, &tree, &path).await }
        });
        match r.await {
            Ok(raw) => out.push(Change::Upsert { entry: to_entry(raw) }),
            Err(e) if e.kind() == ErrorKind::NotFound => out.push(Change::Remove { name }),
            // Exists but can't be stat'ed right now (in use, ...): keep the row.
            Err(_) => {}
        }
    }
    out
}
